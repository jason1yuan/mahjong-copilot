"""Evaluate the ready-made Mahjong Soul tile classifier pjura/mahjong_soul_vision (#24).

A ViT-base fine-tuned by its author on pjura/mahjong_souls_tiles; we do not train
it. It was trained on the dataset's train split, so only the test split is a
fair measure. Results go to `<results_dir>/vit-<split>.json`.

Run from the repository root:
    ml/.venv/bin/python -I ml/vlm_eval/eval_vit.py --split test
    ml/.venv/bin/python -I ml/vlm_eval/eval_vit.py --simulated recordings/simulated
"""

from __future__ import annotations

import argparse
import json
import pathlib
import resource
import sys
import time
import tomllib
from typing import Any

import torch
from huggingface_hub import snapshot_download
from PIL import Image
from transformers import AutoImageProcessor, AutoModelForImageClassification

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from eval_llamacpp import DATASET_TO_MJAI, load_samples, summarize  # noqa: E402
from red_five import refine  # noqa: E402

CONFIG_PATH = pathlib.Path(__file__).with_name("config.toml")

# The classifier has no red-five class, so a red five counts as right when the
# plain five is predicted only in the "kind" accuracy.
Item = tuple[Image.Image, str, str, str]  # image, mjai label, split, source


def dataset_items(cfg: dict[str, Any], split: str) -> list[Item]:
    cfg = {**cfg, "splits": [split]}
    return [(Image.open(s.path).convert("RGB"), s.label, s.split, str(s.path)) for s in load_samples(cfg)]


def simulated_items(directory: pathlib.Path) -> list[Item]:
    """Hand tiles of every annotation in a folder: simulate.py output (image next to the
    JSON) or labeled screenshots (image path from the repository root in "image")."""
    items = []
    for truth_path in sorted(directory.glob("*.json")):
        truth = json.loads(truth_path.read_text(encoding="utf-8"))
        if not isinstance(truth, dict) or not isinstance(truth.get("hand"), list):
            continue  # not an annotation (e.g. the spec file)
        image_path = pathlib.Path(truth["image"]) if "image" in truth else truth_path.with_suffix(".png")
        image = Image.open(image_path).convert("RGB")
        for k, tile in enumerate(truth["hand"]):
            items.append((image.crop(tuple(tile["box"])), tile["tile"], directory.name, f"{truth_path.stem}#{k}"))
    return items


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--split", choices=["test", "train"], default="test")
    parser.add_argument("--simulated", type=pathlib.Path, help="evaluate hand tiles of simulate.py output instead")
    parser.add_argument("--red-rule", action="store_true", help="tell red fives apart by stroke colour (red_five.py)")
    parser.add_argument("--device", default="mps" if torch.backends.mps.is_available() else "cpu")
    args = parser.parse_args()

    cfg = tomllib.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    spec = cfg["classifier"]
    local_dir = pathlib.Path(cfg["models_dir"]) / "mahjong_soul_vision"
    # training_args.bin is a pickle we have no use for, so it is never downloaded.
    snapshot_download(
        spec["repo"],
        revision=spec["revision"],
        local_dir=local_dir,
        allow_patterns=["config.json", "preprocessor_config.json", "pytorch_model.bin"],
    )
    size_gb = sum(p.stat().st_size for p in local_dir.glob("*") if p.is_file()) / 1e9

    start = time.perf_counter()
    processor = AutoImageProcessor.from_pretrained(local_dir)
    # Weights are a torch pickle; torch >= 2.6 loads them with weights_only=True.
    model = AutoModelForImageClassification.from_pretrained(local_dir, use_safetensors=False)
    model.to(args.device).eval()
    load_s = time.perf_counter() - start
    id2label = model.config.id2label

    items = simulated_items(args.simulated) if args.simulated else dataset_items(cfg, args.split)
    name = (args.simulated.name if args.simulated else args.split) + ("_redrule" if args.red_rule else "")
    rows: list[dict[str, Any]] = []
    with torch.inference_mode():
        for image, label, split, source in items:
            start = time.perf_counter()
            inputs = processor(images=image, return_tensors="pt").to(args.device)
            logits = model(**inputs).logits
            index = int(logits.argmax(-1).item())
            seconds = time.perf_counter() - start
            raw = id2label[index]
            rows.append(
                {
                    "path": source,
                    "split": split,
                    "label": label,
                    "pred": refine(DATASET_TO_MJAI.get(raw, "?"), image) if args.red_rule else DATASET_TO_MJAI.get(raw, "?"),
                    "answer": raw,
                    "seconds": seconds,
                    "prompt_tokens": None,
                }
            )

    peak_rss_mb = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024 / 1024
    summary = summarize(
        "mahjong_soul_vision",
        rows,
        {
            "load_s": load_s,
            "peak_rss_mb": peak_rss_mb,
            "files_gb": size_gb,
            "repo": spec["repo"],
            "license": spec["license"],
            "server_args": [f"vit on {args.device}"],
            "answers": "classifier",
            "split": name,
        },
    )
    out = pathlib.Path(cfg["results_dir"]) / f"vit-{name}.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps({"summary": summary, "rows": rows}, ensure_ascii=False, indent=1), encoding="utf-8")
    print(json.dumps({k: v for k, v in summary.items() if k != "accuracy_by_label"}, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
