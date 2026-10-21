"""Evaluate the OpenAI Decisions API on single Mahjong Soul tile crops (#24).

Each tile is sent as one image with a "choice" question over the 37 mjai tile
names, so the answer is always one of them, with per-choice probabilities and a
confidence. This is an online reference: it needs network access and your own
API key, and sends the tile images to OpenAI.

The key is read from OPENAI_API_KEY, or asked for without echo; it is never
written anywhere. Run from the repository root:
    ml/.venv/bin/python -I ml/vlm_eval/eval_decisions.py --per-label 5 --seed 7
    ml/.venv/bin/python -I ml/vlm_eval/eval_decisions.py --simulated recordings/simulated
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
import statistics
import sys
import time
import tomllib
from typing import Any

import requests
from PIL import Image

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from eval_llamacpp import MJAI_TILES, load_samples, summarize  # noqa: E402

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


QUESTION = {
    "type": "choice",
    "name": "tile",
    "instructions": INSTRUCTIONS,
    "choices": [{"value": t, "description": describe(t)} for t in MJAI_TILES],
}

Item = tuple[Image.Image, str, str, str]  # image, mjai label, split, source


def data_url(image: Image.Image) -> str:
    buf = io.BytesIO()
    image.save(buf, format="PNG")
    return "data:image/png;base64," + base64.b64encode(buf.getvalue()).decode()


def decide(cfg: dict[str, Any], key: str, image: Image.Image) -> tuple[dict[str, Any], float, dict[str, Any]]:
    body = {
        "model": cfg["model"],
        "input": [
            {
                "role": "user",
                "content": [
                    {"type": "input_text", "text": INSTRUCTIONS},
                    {"type": "input_image", "image_url": data_url(image)},
                ],
            }
        ],
        "questions": [QUESTION],
    }
    headers = {"Authorization": f"Bearer {key}", "Content-Type": "application/json"}
    for attempt in range(5):
        start = time.perf_counter()
        resp = requests.post(cfg["endpoint"], headers=headers, json=body, timeout=cfg["timeout_s"])
        elapsed = time.perf_counter() - start
        if resp.status_code in (429, 500, 502, 503) and attempt < 4:
            time.sleep(2**attempt)
            continue
        if resp.status_code != 200:
            raise RuntimeError(f"HTTP {resp.status_code}: {resp.text[:500]}")
        data = resp.json()
        answer = next((a for a in data.get("answers", []) if a.get("name") == "tile"), {})
        return answer, elapsed, data.get("usage", {})
    raise RuntimeError("gave up after retries")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--per-label", type=int, help="evaluate N random dataset samples of every tile")
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--simulated", type=pathlib.Path, help="evaluate hand tiles of simulate.py output instead")
    args = parser.parse_args()

    cfg = tomllib.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    api = cfg["openai_decisions"]
    key = os.environ.get("OPENAI_API_KEY") or getpass.getpass("OpenAI API key (not shown, not saved): ")

    items: list[Item] = []
    if args.simulated:
        for truth_path in sorted(args.simulated.glob("*.json")):
            truth = json.loads(truth_path.read_text(encoding="utf-8"))
            image = Image.open(truth_path.with_suffix(".png")).convert("RGB")
            for k, tile in enumerate(truth["hand"]):
                items.append((image.crop(tuple(tile["box"])), tile["tile"], "simulated", f"{truth_path.stem}#{k}"))
        name = "simulated"
    else:
        samples = load_samples(cfg)
        if args.per_label:
            rng = random.Random(args.seed)
            by_label = collections.defaultdict(list)
            for s in samples:
                by_label[s.label].append(s)
            samples = [s for g in by_label.values() for s in rng.sample(g, min(args.per_label, len(g)))]
        items = [(Image.open(s.path).convert("RGB"), s.label, s.split, str(s.path)) for s in samples]
        name = f"pl{args.per_label}" if args.per_label else "full"

    rows: list[dict[str, Any]] = []
    input_tokens = 0
    for i, (image, label, split, source) in enumerate(items):
        answer, seconds, usage = decide(api, key, image)
        input_tokens += int(usage.get("input_tokens") or 0)
        pred = answer.get("choice") if answer.get("type") == "choice" else "?"
        rows.append(
            {
                "path": source,
                "split": split,
                "label": label,
                "pred": pred if pred in MJAI_TILES else "?",
                "answer": answer,
                "confidence": answer.get("confidence"),
                "seconds": seconds,
                "prompt_tokens": usage.get("input_tokens"),
            }
        )
        if i % 20 == 0:
            done = sum(r["pred"] == r["label"] for r in rows)
            print(f"{i + 1}/{len(items)} correct {done}", flush=True)

    confidences = [r["confidence"] for r in rows if isinstance(r["confidence"], (int, float))]
    summary = summarize(
        "openai-decisions",
        rows,
        {
            "load_s": 0.0,
            "peak_rss_mb": 0.0,
            "files_gb": 0.0,
            "repo": api["model"],
            "license": "OpenAI API (online)",
            "server_args": ["online"],
            "answers": "choice",
            "input_tokens": input_tokens,
            "cost_usd": input_tokens / 1e6 * api["usd_per_million_input_tokens"],
            "mean_confidence": statistics.fmean(confidences) if confidences else None,
            "kind_accuracy": sum(r["pred"].removesuffix("r") == r["label"].removesuffix("r") for r in rows) / len(rows),
        },
    )
    out = pathlib.Path(cfg["results_dir"]) / f"openai-decisions_{name}.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps({"summary": summary, "rows": rows}, ensure_ascii=False, indent=1), encoding="utf-8")
    keep = ("n", "accuracy", "kind_accuracy", "latency_s", "first_request_s", "input_tokens", "cost_usd", "mean_confidence")
    print(json.dumps({k: summary[k] for k in keep}, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
