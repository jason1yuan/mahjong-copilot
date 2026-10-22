"""Pre-label hand tiles of real screenshots for review (#24).

Reads a spec listing, per screenshot, the hand band and its tile groups (left,
right, tile count), cuts equal slots, labels each tile with the
pjura/mahjong_soul_vision classifier, and writes one annotation JSON per
screenshot next to the spec plus a review sheet showing every tile enlarged with
its predicted label. Labels must be checked by eye and corrected in the JSON;
the classifier has no red-five class, so red fives always need a manual fix.

Run from the repository root:
    ml/.venv/bin/python -I ml/vlm_eval/prelabel.py recordings/source/hands_spec.json --sheet /tmp/review.png
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
import tomllib

import torch
from PIL import Image, ImageDraw, ImageFont
from transformers import AutoImageProcessor, AutoModelForImageClassification

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from eval_llamacpp import DATASET_TO_MJAI  # noqa: E402
from find_hand import tile_boxes  # noqa: E402

CONFIG_PATH = pathlib.Path(__file__).with_name("config.toml")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("spec", type=pathlib.Path)
    parser.add_argument("--sheet", type=pathlib.Path, required=True)
    args = parser.parse_args()

    cfg = tomllib.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    model_dir = pathlib.Path(cfg["models_dir"]) / "mahjong_soul_vision"
    processor = AutoImageProcessor.from_pretrained(model_dir)
    model = AutoModelForImageClassification.from_pretrained(model_dir, use_safetensors=False).eval()

    font = ImageFont.load_default(size=18)
    rows = []
    for entry in json.loads(args.spec.read_text(encoding="utf-8")):
        image_path = (args.spec.parent / entry["image"]).resolve()
        image = Image.open(image_path).convert("RGB")
        boxes = tile_boxes(entry["band"], [g[:2] for g in entry["groups"]], [g[2] for g in entry["groups"]])
        crops = [image.crop(b) for b in boxes]
        with torch.inference_mode():
            logits = model(**processor(images=crops, return_tensors="pt")).logits
        probs = logits.softmax(-1)
        labels = [DATASET_TO_MJAI[model.config.id2label[int(i)]] for i in probs.argmax(-1)]
        confidence = [round(float(p), 3) for p in probs.max(-1).values]
        annotation = {
            "image": str(image_path.relative_to(args.spec.parent.resolve().parent.parent))
            if image_path.is_relative_to(args.spec.parent.resolve().parent.parent)
            else str(image_path),
            "id": entry["id"],
            "note": "Hand labels pre-filled by pjura/mahjong_soul_vision, then checked by eye.",
            "hand": [{"tile": t, "box": list(b), "prelabel": t, "prelabel_confidence": c} for t, b, c in zip(labels, boxes, confidence)],
        }
        (args.spec.parent / f"{entry['id']}.json").write_text(json.dumps(annotation, ensure_ascii=False, indent=1), encoding="utf-8")
        rows.append((entry["id"], crops, labels, confidence))

    cell_w, cell_h = 80, 110
    width = max(len(r[1]) for r in rows) * (cell_w + 6) + 10
    sheet = Image.new("RGB", (width, len(rows) * (cell_h + 52) + 10), "white")
    draw = ImageDraw.Draw(sheet)
    for r, (ident, crops, labels, confidence) in enumerate(rows):
        y = 10 + r * (cell_h + 52)
        draw.text((10, y), ident, fill="black", font=font)
        for k, (crop, label, conf) in enumerate(zip(crops, labels, confidence)):
            x = 10 + k * (cell_w + 6)
            sheet.paste(crop.resize((cell_w, cell_h)), (x, y + 22))
            colour = "black" if conf >= 0.9 else "red"
            draw.text((x + 2, y + cell_h + 24), f"{k}:{label}", fill=colour, font=font)
    sheet.save(args.sheet)
    print(f"wrote {len(rows)} annotations and {args.sheet}")


if __name__ == "__main__":
    main()
