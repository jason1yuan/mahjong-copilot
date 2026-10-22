"""Evaluate the OpenAI Decisions API on single Mahjong Soul tile crops (#24).

Each tile is sent as its own request. By default a "choice" question over the 37
mjai tile names makes the answer always one of them, with per-choice
probabilities and a confidence. Variants in config.toml change the request:
upscaling the crop, the image detail level, attaching a labeled legend of all
tiles, or splitting the question into suit / number / honor / red five.

This is an online reference: it needs network access and your own API key, and
sends the tile images to OpenAI. The key is read from OPENAI_API_KEY, or asked
for once without echo; it is never written anywhere. Run from the repository root:
    ml/.venv/bin/python -I ml/vlm_eval/eval_decisions.py --per-label 5 --seed 7
    ml/.venv/bin/python -I ml/vlm_eval/eval_decisions.py --simulated recordings/simulated \
        --variant up512 --variant up512-legend
"""

from __future__ import annotations

import argparse
import base64
import collections
import getpass
import io
import json
import os
import pathlib
import random
import sys
import time
import tomllib
from typing import Any

import requests
from PIL import Image, ImageDraw, ImageFont

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from eval_llamacpp import DATASET_TO_MJAI, MJAI_TILES, load_samples, summarize  # noqa: E402

CONFIG_PATH = pathlib.Path(__file__).with_name("config.toml")
NUMERALS = "一二三四伍六七八九"
HONOR_NAMES = {
    "E": "east wind (東)",
    "S": "south wind (南)",
    "W": "west wind (西)",
    "N": "north wind (北)",
    "P": "white dragon (blank face)",
    "F": "green dragon (發)",
    "C": "red dragon (中)",
}
SUIT_NAMES = {"m": "characters (萬)", "p": "circles (筒)", "s": "bamboo (索)"}
INSTRUCTIONS = (
    "The image shows one Japanese riichi mahjong tile cropped from the video game Mahjong Soul. "
    "It may be rotated or seen at an angle. Which tile is it? A five whose symbols are drawn "
    "entirely in red is a red five."
)
LEGEND_INSTRUCTIONS = (
    "The first image is a reference sheet of every Mahjong Soul tile with its code printed under "
    "it (red fives are not shown: they look like the five of the same suit drawn entirely in red). "
    "The second image is the tile to identify. Find the matching tile on the sheet."
)


def describe(tile: str) -> str:
    if tile in HONOR_NAMES:
        return HONOR_NAMES[tile]
    number, suit = int(tile[0]), tile[1]
    if tile.endswith("r"):
        return f"red five {SUIT_NAMES[suit]}, drawn entirely in red"
    detail = {
        "m": f"the numeral {NUMERALS[number - 1]} above 萬",
        "p": f"{number} round dots",
        "s": "a bird" if number == 1 else f"{number} sticks",
    }[suit]
    return f"{number} {SUIT_NAMES[suit]}: {detail}"


def one_question(instructions: str) -> list[dict[str, Any]]:
    return [
        {
            "type": "choice",
            "name": "tile",
            "instructions": instructions,
            "choices": [{"value": t, "description": describe(t)} for t in MJAI_TILES],
        }
    ]
TWO_STEP_QUESTIONS = [
    {
        "type": "choice",
        "name": "suit",
        "instructions": "Which kind of tile is it?",
        "choices": [
            {"value": "m", "description": "characters: a Chinese numeral above the red character 萬"},
            {"value": "p", "description": "circles: round dots"},
            {"value": "s", "description": "bamboo: sticks, or a bird for the one of bamboo"},
            {"value": "z", "description": "honor: a wind (東南西北) or a dragon (blank, 發, 中)"},
        ],
    },
    {
        "type": "choice",
        "name": "number",
        "instructions": "If it is a characters, circles or bamboo tile, what is its number? "
        "For circles and bamboo count the dots or sticks; a bird is 1. For characters read the numeral.",
        "choices": [{"value": str(n), "description": f"{n} ({NUMERALS[n - 1]})"} for n in range(1, 10)],
    },
    {
        "type": "choice",
        "name": "honor",
        "instructions": "If it is an honor tile, which one?",
        "choices": [{"value": h, "description": d} for h, d in HONOR_NAMES.items()],
    },
    {
        "type": "predicate",
        "name": "red_five",
        "instructions": "Is it a five whose symbols are all drawn in red (a red five)?",
    },
]

Item = tuple[Image.Image, str, str, str]  # image, mjai label, split, source


def data_url(image: Image.Image) -> str:
    buf = io.BytesIO()
    image.save(buf, format="PNG")
    return "data:image/png;base64," + base64.b64encode(buf.getvalue()).decode()


def upscaled(image: Image.Image, long_side: int | None) -> Image.Image:
    if not long_side or max(image.size) >= long_side:
        return image
    scale = long_side / max(image.size)
    return image.resize((round(image.width * scale), round(image.height * scale)), Image.Resampling.LANCZOS)


def build_legend(cfg: dict[str, Any]) -> Image.Image:
    """Grid of the dataset's clean example crops (one per tile), each labeled with its mjai code."""
    root = pathlib.Path(cfg["dataset_dir"]) / "test"
    by_mjai = {DATASET_TO_MJAI[d.name]: d / f"{d.name}.png" for d in root.iterdir() if (d / f"{d.name}.png").exists()}
    order = [t for t in MJAI_TILES if t in by_mjai]
    cell_w, cell_h, label_h, cols = 88, 142, 30, 9
    rows = (len(order) + cols - 1) // cols
    sheet = Image.new("RGB", (cols * (cell_w + 8) + 8, rows * (cell_h + label_h + 8) + 8), "white")
    draw = ImageDraw.Draw(sheet)
    font = ImageFont.load_default(size=24)
    for i, tile in enumerate(order):
        x = 8 + (i % cols) * (cell_w + 8)
        y = 8 + (i // cols) * (cell_h + label_h + 8)
        sheet.paste(Image.open(by_mjai[tile]).convert("RGB").resize((cell_w, cell_h)), (x, y))
        draw.text((x + cell_w // 2, y + cell_h + 2), tile, fill="black", font=font, anchor="ma")
    return sheet


def build_body(cfg: dict[str, Any], variant: dict[str, Any], image: Image.Image, legend_url: str | None) -> dict[str, Any]:
    tile_part: dict[str, Any] = {"type": "input_image", "image_url": data_url(upscaled(image, variant.get("upscale")))}
    if variant.get("detail"):
        tile_part["detail"] = variant["detail"]
    content: list[dict[str, Any]] = [{"type": "input_text", "text": INSTRUCTIONS}]
    if legend_url:
        content = [
            {"type": "input_text", "text": LEGEND_INSTRUCTIONS},
            {"type": "input_image", "image_url": legend_url},
        ]
    content.append(tile_part)
    if variant.get("two_step"):
        questions = TWO_STEP_QUESTIONS
    else:
        # The base question repeats INSTRUCTIONS, exactly as in the first runs.
        questions = one_question("Which tile on the reference sheet matches the second image?" if legend_url else INSTRUCTIONS)
    return {"model": cfg["model"], "input": [{"role": "user", "content": content}], "questions": questions}


def interpret(answers: list[dict[str, Any]], two_step: bool) -> tuple[str, float | None]:
    """Maps the answers to an mjai tile and a confidence; "?" when it refused."""
    by_name = {a.get("name"): a for a in answers}
    if not two_step:
        tile = by_name.get("tile", {})
        return (tile.get("choice") or "?", tile.get("confidence")) if tile.get("type") == "choice" else ("?", None)
    suit, number, honor = (by_name.get(n, {}) for n in ("suit", "number", "honor"))
    if suit.get("choice") is None:
        return "?", None
    if suit["choice"] == "z":
        return honor.get("choice") or "?", _product(suit.get("confidence"), honor.get("confidence"))
    tile = f"{number.get('choice', '?')}{suit['choice']}"
    red = (by_name.get("red_five", {}).get("probability") or 0) > 0.5
    if red and tile[0] == "5":
        tile += "r"
    return tile, _product(suit.get("confidence"), number.get("confidence"))


def _product(a: float | None, b: float | None) -> float | None:
    return a * b if isinstance(a, (int, float)) and isinstance(b, (int, float)) else None


def post(cfg: dict[str, Any], key: str, body: dict[str, Any]) -> tuple[dict[str, Any], float]:
    """Retries rate limits, server errors and network failures (timeouts, dropped connections)."""
    headers = {"Authorization": f"Bearer {key}", "Content-Type": "application/json"}
    for attempt in range(6):
        start = time.perf_counter()
        try:
            resp = requests.post(cfg["endpoint"], headers=headers, json=body, timeout=cfg["timeout_s"])
        except requests.RequestException as err:
            if attempt == 5:
                raise RuntimeError(f"network error: {err}") from err
            print(f"  network error, retrying: {type(err).__name__}", flush=True)
            time.sleep(2**attempt)
            continue
        elapsed = time.perf_counter() - start
        if resp.status_code in (429, 500, 502, 503) and attempt < 5:
            time.sleep(2**attempt)
            continue
        if resp.status_code != 200:
            raise RuntimeError(f"HTTP {resp.status_code}: {resp.text[:500]}")
        return resp.json(), elapsed
    raise RuntimeError("gave up after retries")


def load_items(cfg: dict[str, Any], args: argparse.Namespace) -> tuple[list[Item], str]:
    if args.simulated:
        items = []
        for truth_path in sorted(args.simulated.glob("*.json")):
            truth = json.loads(truth_path.read_text(encoding="utf-8"))
            image = Image.open(truth_path.with_suffix(".png")).convert("RGB")
            for k, tile in enumerate(truth["hand"]):
                items.append((image.crop(tuple(tile["box"])), tile["tile"], "simulated", f"{truth_path.stem}#{k}"))
        return items, "simulated"
    samples = load_samples(cfg)
    if args.per_label:
        rng = random.Random(args.seed)
        by_label = collections.defaultdict(list)
        for s in samples:
            by_label[s.label].append(s)
        samples = [s for g in by_label.values() for s in rng.sample(g, min(args.per_label, len(g)))]
    items = [(Image.open(s.path).convert("RGB"), s.label, s.split, str(s.path)) for s in samples]
    return items, f"pl{args.per_label}" if args.per_label else "full"


def evaluate(cfg: dict[str, Any], api: dict[str, Any], key: str, items: list[Item], name: str, variant_name: str) -> None:
    variant = api["variants"][variant_name]
    legend_url = data_url(build_legend(cfg)) if variant.get("legend") else None
    suffix = "" if variant_name == "base" else f"_{variant_name}"
    out = pathlib.Path(cfg["results_dir"]) / f"openai-decisions_{name}{suffix}.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    # Rows are appended here as they arrive, so an interrupted run resumes where it stopped.
    partial = out.with_name(out.stem + ".partial.jsonl")
    rows: list[dict[str, Any]] = []
    if partial.exists():
        rows = [json.loads(line) for line in partial.read_text(encoding="utf-8").splitlines() if line]
        print(f"{variant_name}: resuming after {len(rows)} done", flush=True)
    with partial.open("a", encoding="utf-8") as checkpoint:
        for i, (image, label, split, source) in enumerate(items):
            if i < len(rows):
                continue
            data, seconds = post(api, key, build_body(api, variant, image, legend_url))
            pred, confidence = interpret(data.get("answers", []), bool(variant.get("two_step")))
            row = {
                "path": source,
                "split": split,
                "label": label,
                "pred": pred if pred in MJAI_TILES else "?",
                "answer": data.get("answers"),
                "confidence": confidence,
                "seconds": seconds,
                "prompt_tokens": (data.get("usage") or {}).get("input_tokens"),
            }
            rows.append(row)
            checkpoint.write(json.dumps(row, ensure_ascii=False) + "\n")
            checkpoint.flush()
            if i % 50 == 0:
                done = sum(r["pred"] == r["label"] for r in rows)
                print(f"{variant_name}: {i + 1}/{len(items)} correct {done}", flush=True)
    input_tokens = sum(int(r["prompt_tokens"] or 0) for r in rows)

    summary = summarize(
        "openai-decisions",
        rows,
        {
            "load_s": 0.0,
            "peak_rss_mb": 0.0,
            "files_gb": 0.0,
            "repo": api["model"],
            "license": "OpenAI API (online)",
            "server_args": ["online", variant_name],
            "answers": "two-step" if variant.get("two_step") else "choice",
            "variant": {"name": variant_name, **variant},
            "input_tokens": input_tokens,
            "cost_usd": input_tokens / 1e6 * api["usd_per_million_input_tokens"],
        },
    )
    out.write_text(json.dumps({"summary": summary, "rows": rows}, ensure_ascii=False, indent=1), encoding="utf-8")
    partial.unlink()
    keep = ("n", "accuracy", "kind_accuracy", "latency_s", "input_tokens", "cost_usd", "calibration")
    print(json.dumps({"variant": variant_name, **{k: summary[k] for k in keep}}, ensure_ascii=False, indent=1))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--per-label", type=int, help="evaluate N random dataset samples of every tile")
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--simulated", type=pathlib.Path, help="evaluate hand tiles of simulate.py output instead")
    parser.add_argument("--variant", action="append", help="request variant from config.toml (repeatable)")
    parser.add_argument("--save-legend", type=pathlib.Path, help="only write the legend image to this path and exit")
    args = parser.parse_args()

    cfg = tomllib.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    api = cfg["openai_decisions"]
    if args.save_legend:
        build_legend(cfg).save(args.save_legend)
        return
    key = os.environ.get("OPENAI_API_KEY") or getpass.getpass("OpenAI API key (not shown, not saved): ")
    items, name = load_items(cfg, args)
    for variant_name in args.variant or ["base"]:
        try:
            evaluate(cfg, api, key, items, name, variant_name)
        except RuntimeError as err:
            print(f"{variant_name}: failed: {err}", flush=True)


if __name__ == "__main__":
    main()
