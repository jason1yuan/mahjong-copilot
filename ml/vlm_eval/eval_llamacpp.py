"""Evaluate VLMs served by llama.cpp on single Mahjong Soul tile crops (#24).

Each tile image is sent on its own, and a grammar restricts the answer to the
37 mjai tile names. Per-image predictions and a summary go to
`<results_dir>/<model>.json`.

Run from the repository root:
    ml/.venv/bin/python -I ml/vlm_eval/eval_llamacpp.py qwen3-vl-2b
    ml/.venv/bin/python -I ml/vlm_eval/eval_llamacpp.py qwen3-vl-2b --delete-model
"""

from __future__ import annotations

import argparse
import base64
import collections
import io
import json
import pathlib
import random
import re
import shutil
import statistics
import subprocess
import time
import tomllib
from dataclasses import dataclass
from typing import Any

import requests
from huggingface_hub import hf_hub_download
from PIL import Image

CONFIG_PATH = pathlib.Path(__file__).with_name("config.toml")

MJAI_TILES = [
    *(f"{n}{s}" for s in "mps" for n in range(1, 10)),
    *"ESWNPFC",
    "5mr",
    "5pr",
    "5sr",
]

# pjura/mahjong_souls_tiles folder names: n = characters (man), p = circles,
# b = bamboo (sou); winds ew/sw/ww/nw; dragons wd (white), gd (green), rd (red).
DATASET_TO_MJAI = {
    **{f"{n}n": f"{n}m" for n in range(1, 10)},
    **{f"{n}p": f"{n}p" for n in range(1, 10)},
    **{f"{n}b": f"{n}s" for n in range(1, 10)},
    "ew": "E",
    "sw": "S",
    "ww": "W",
    "nw": "N",
    "wd": "P",
    "gd": "F",
    "rd": "C",
}

# Answers are constrained to plain English names, which small VLMs handle far
# better than codes like "3p", then mapped to mjai.
SUITS = {"characters": "m", "circles": "p", "bamboo": "s"}
HONORS = {
    "east wind": "E",
    "south wind": "S",
    "west wind": "W",
    "north wind": "N",
    "white dragon": "P",
    "green dragon": "F",
    "red dragon": "C",
}
GRAMMAR = "\n".join(
    [
        'root ::= numbered | "red five " suit | honor',
        'numbered ::= [1-9] " " suit',
        "suit ::= " + " | ".join(f'"{s}"' for s in SUITS),
        "honor ::= " + " | ".join(f'"{h}"' for h in HONORS),
    ]
)


ANSWER_RE = re.compile(
    r"(?P<red>red five )?(?P<num>[1-9])? ?(?P<suit>characters?|circles?|bamboos?)"
    r"|(?P<honor>(?:east|south|west|north) wind|(?:white|green|red) dragon)"
)


def to_mjai(answer: str) -> str:
    """Maps the first tile name in an answer to mjai; "?" when there is none."""
    match = ANSWER_RE.search(answer.lower())
    if not match:
        return "?"
    if match["honor"]:
        return HONORS[match["honor"]]
    suit = next(code for name, code in SUITS.items() if name[:4] == match["suit"][:4])
    if match["red"]:
        return f"5{suit}r"
    return f"{match['num']}{suit}" if match["num"] else "?"


@dataclass
class Sample:
    path: pathlib.Path
    split: str
    label: str


def red_fives(cfg: dict[str, Any]) -> set[str]:
    """Dataset images that show a red five although the dataset files them as plain fives."""
    return set(json.loads(pathlib.Path(cfg["red_fives"]).read_text(encoding="utf-8"))["red"])


def dataset_label(path: pathlib.PurePath, red: set[str]) -> str:
    label = DATASET_TO_MJAI[path.parent.name]
    return label + "r" if str(path) in red else label


def load_samples(cfg: dict[str, Any]) -> list[Sample]:
    root = pathlib.Path(cfg["dataset_dir"])
    red = red_fives(cfg)
    samples = []
    for split in cfg["splits"]:
        for path in sorted((root / split).glob("*/*")):
            if path.suffix.lower() in {".png", ".jpg", ".jpeg"} and "_aug_" not in path.name:
                samples.append(Sample(path, split, dataset_label(path, red)))
    return samples


def encode_image(path: pathlib.Path, min_long_side: int) -> str:
    image = Image.open(path).convert("RGB")
    scale = min_long_side / max(image.size)
    if scale > 1:
        image = image.resize(
            (round(image.width * scale), round(image.height * scale)), Image.Resampling.LANCZOS
        )
    buf = io.BytesIO()
    image.save(buf, format="PNG")
    return "data:image/png;base64," + base64.b64encode(buf.getvalue()).decode()


def fetch_model(cfg: dict[str, Any], name: str) -> tuple[pathlib.Path, pathlib.Path]:
    spec = cfg["models"][name]
    local_dir = pathlib.Path(cfg["models_dir"]) / name
    paths = [
        pathlib.Path(
            hf_hub_download(spec["repo"], file, revision=spec["revision"], local_dir=local_dir)
        )
        for file in (spec["model"], spec["mmproj"])
    ]
    return paths[0], paths[1]


def rss_mb(pid: int) -> float:
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True)
    return int(out.stdout.strip() or 0) / 1024


def start_server(
    cfg: dict[str, Any], model: pathlib.Path, mmproj: pathlib.Path, extra: list[str]
) -> tuple[subprocess.Popen[bytes], float]:
    server = cfg["server"]
    args = [
        server["binary"],
        "-m", str(model),
        "--mmproj", str(mmproj),
        "--port", str(server["port"]),
        "-c", str(server["ctx_size"]),
        "-ngl", str(server["gpu_layers"]),
        "--jinja",
        "--no-webui",
        *extra,
    ]
    start = time.perf_counter()
    proc = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    url = f"http://127.0.0.1:{server['port']}/health"
    while time.perf_counter() - start < server["startup_timeout_s"]:
        if proc.poll() is not None:
            raise RuntimeError(f"llama-server exited with code {proc.returncode}")
        try:
            if requests.get(url, timeout=1).status_code == 200:
                return proc, time.perf_counter() - start
        except requests.ConnectionError:
            pass
        time.sleep(0.5)
    proc.terminate()
    raise TimeoutError("llama-server did not become healthy in time")


def ask(cfg: dict[str, Any], image_url: str, constrained: bool) -> tuple[str, str, float, dict[str, Any]]:
    body = {
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "image_url", "image_url": {"url": image_url}},
                    {"type": "text", "text": cfg["prompt"]["text"]},
                ],
            }
        ],
        "temperature": 0,
        "max_tokens": 12 if constrained else 24,
    }
    if constrained:
        body["grammar"] = GRAMMAR
    url = f"http://127.0.0.1:{cfg['server']['port']}/v1/chat/completions"
    start = time.perf_counter()
    resp = requests.post(url, json=body, timeout=120)
    elapsed = time.perf_counter() - start
    resp.raise_for_status()
    data = resp.json()
    answer = data["choices"][0]["message"]["content"]
    return to_mjai(answer), answer, elapsed, data.get("timings", {})


def percentile(values: list[float], q: float) -> float:
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, round(q * (len(ordered) - 1)))]


def calibration(rows: list[dict[str, Any]]) -> dict[str, Any] | None:
    """How well stated confidences match accuracy: the expected calibration error over
    ten equal-width bins, and how often answers at or above a confidence are right.
    Mean confidence below accuracy means the model is underconfident."""
    scored = [(r["confidence"], r["pred"] == r["label"]) for r in rows if isinstance(r.get("confidence"), (int, float))]
    if not scored:
        return None
    bins: dict[int, list[tuple[float, bool]]] = collections.defaultdict(list)
    for confidence, right in scored:
        bins[min(int(confidence * 10), 9)].append((confidence, right))
    gaps = (abs(statistics.fmean(c for c, _ in b) - statistics.fmean(r for _, r in b)) * len(b) for b in bins.values())
    return {
        "n": len(scored),
        "accuracy": statistics.fmean(r for _, r in scored),
        "mean_confidence": statistics.fmean(c for c, _ in scored),
        "ece": sum(gaps) / len(scored),
        "at_least": {
            str(t): {"n": sum(c >= t for c, _ in scored), "right": sum(c >= t and r for c, r in scored)} for t in (0.5, 0.7)
        },
    }


def summarize(name: str, rows: list[dict[str, Any]], extra: dict[str, Any]) -> dict[str, Any]:
    def acc(subset: list[dict[str, Any]]) -> float | None:
        return sum(r["pred"] == r["label"] for r in subset) / len(subset) if subset else None

    latencies = [r["seconds"] for r in rows[1:]]  # the first request is reported separately
    per_label = {
        label: acc([r for r in rows if r["label"] == label]) for label in sorted({r["label"] for r in rows})
    }
    confusions = collections.Counter((r["label"], r["pred"]) for r in rows if r["pred"] != r["label"])
    return {
        "model": name,
        "n": len(rows),
        "accuracy": acc(rows),
        # Right tile kind, ignoring whether a five is red.
        "kind_accuracy": sum(r["pred"].removesuffix("r") == r["label"].removesuffix("r") for r in rows) / len(rows),
        "calibration": calibration(rows),
        "accuracy_by_split": {s: acc([r for r in rows if r["split"] == s]) for s in sorted({r["split"] for r in rows})},
        "accuracy_by_label": per_label,
        "top_confusions": [
            {"label": a, "pred": b, "count": c} for (a, b), c in confusions.most_common(15)
        ],
        "first_request_s": rows[0]["seconds"],
        "latency_s": {
            "mean": statistics.fmean(latencies),
            "median": statistics.median(latencies),
            "p95": percentile(latencies, 0.95),
        },
        **extra,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("model", help="model key from config.toml")
    parser.add_argument("--sample", type=int, help="evaluate a random subset of N samples")
    parser.add_argument("--per-label", type=int, help="evaluate N random samples of every tile")
    parser.add_argument("--seed", type=int, default=0, help="seed for --sample and --per-label")
    parser.add_argument(
        "--server-arg", action="append", default=[], help="extra llama-server argument (repeatable)"
    )
    parser.add_argument("--min-long-side", type=int, help="override min_long_side from the config")
    parser.add_argument(
        "--free", action="store_true", help="let the model answer freely and parse the answer, no grammar"
    )
    parser.add_argument("--tag", default="", help="suffix for the results file name")
    parser.add_argument("--delete-model", action="store_true", help="remove the downloaded files afterwards")
    args = parser.parse_args()

    cfg = tomllib.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    if args.min_long_side:
        cfg["min_long_side"] = args.min_long_side
    samples = load_samples(cfg)
    rng = random.Random(args.seed)
    if args.per_label:
        by_label = collections.defaultdict(list)
        for sample in samples:
            by_label[sample.label].append(sample)
        samples = [s for group in by_label.values() for s in rng.sample(group, min(args.per_label, len(group)))]
    if args.sample:
        samples = rng.sample(samples, args.sample)
    model, mmproj = fetch_model(cfg, args.model)
    size_gb = (model.stat().st_size + mmproj.stat().st_size) / 1e9
    server_args = [*cfg["models"][args.model].get("server_args", []), *args.server_arg]

    proc, load_s = start_server(cfg, model, mmproj, server_args)
    rows: list[dict[str, Any]] = []
    peak_rss = rss_mb(proc.pid)
    try:
        for i, sample in enumerate(samples):
            pred, answer, seconds, timings = ask(
                cfg, encode_image(sample.path, cfg["min_long_side"]), not args.free
            )
            rows.append(
                {
                    "path": str(sample.path),
                    "split": sample.split,
                    "label": sample.label,
                    "pred": pred,
                    "answer": answer,
                    "seconds": seconds,
                    "prompt_tokens": timings.get("prompt_n"),
                }
            )
            if i % cfg["rss_every"] == 0:
                peak_rss = max(peak_rss, rss_mb(proc.pid))
                done = sum(r["pred"] == r["label"] for r in rows)
                print(f"{args.model}: {i + 1}/{len(samples)} correct {done}", flush=True)
    finally:
        peak_rss = max(peak_rss, rss_mb(proc.pid))
        proc.terminate()
        proc.wait(timeout=30)

    summary = summarize(
        args.model,
        rows,
        {
            "load_s": load_s,
            "peak_rss_mb": peak_rss,
            "files_gb": size_gb,
            "repo": cfg["models"][args.model]["repo"],
            "files": [cfg["models"][args.model]["model"], cfg["models"][args.model]["mmproj"]],
            "license": cfg["models"][args.model]["license"],
            "min_long_side": cfg["min_long_side"],
            "server_args": server_args,
            "answers": "free" if args.free else "grammar",
            "sample": args.sample,
            "per_label": args.per_label,
            "seed": args.seed,
        },
    )
    out_dir = pathlib.Path(cfg["results_dir"])
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / f"{args.model}{args.tag}.json").write_text(
        json.dumps({"summary": summary, "rows": rows}, ensure_ascii=False, indent=1), encoding="utf-8"
    )
    print(json.dumps({k: v for k, v in summary.items() if k != "accuracy_by_label"}, ensure_ascii=False, indent=1))

    if args.delete_model:
        shutil.rmtree(pathlib.Path(cfg["models_dir"]) / args.model)


if __name__ == "__main__":
    main()
