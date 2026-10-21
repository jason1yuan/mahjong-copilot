"""Generate simulated screenshots from one labeled Mahjong Soul screenshot (#24).

Each variant applies a random mix of window size, browser borders, colour shift,
JPEG compression, blur, a hovered (raised) tile and a reshuffled hand, and is
saved with ground truth: the mjai label and box of every hand tile in the new
image. This only tests robustness to those changes; it cannot add tiles or
detail that the source screenshot does not contain.

Run from the repository root:
    ml/.venv/bin/python -I ml/vlm_eval/simulate.py recordings/source/majsoul_01.json -n 40
"""

from __future__ import annotations

import argparse
import io
import json
import pathlib
import random
from typing import Any

from PIL import Image, ImageEnhance, ImageFilter, ImageOps

Box = tuple[int, int, int, int]


def tile_boxes(box: list[int], count: int) -> list[Box]:
    x0, y0, x1, y1 = box
    width = (x1 - x0) / count
    return [(round(x0 + i * width), y0, round(x0 + (i + 1) * width), y1) for i in range(count)]


def reshuffle(image: Image.Image, boxes: list[Box], labels: list[str], rng: random.Random) -> list[str]:
    """Pastes the hand tiles back in a new order, keeping each slot's size."""
    crops = [image.crop(b) for b in boxes]
    order = list(range(len(boxes)))
    rng.shuffle(order)
    for slot, src in zip(boxes, order):
        image.paste(crops[src].resize((slot[2] - slot[0], slot[3] - slot[1])), slot[:2])
    return [labels[i] for i in order]


def hover(image: Image.Image, boxes: list[Box], index: int, lift: int) -> list[Box]:
    """Raises one tile like a mouse hover; the strip it leaves behind keeps the old pixels."""
    x0, y0, x1, y1 = boxes[index]
    image.paste(image.crop((x0, y0, x1, y1)), (x0, y0 - lift))
    boxes = list(boxes)
    boxes[index] = (x0, y0 - lift, x1, y1 - lift)
    return boxes


def make_variant(source: Image.Image, ann: dict[str, Any], rng: random.Random) -> tuple[Image.Image, dict[str, Any]]:
    image = source.copy()
    labels = list(ann["hand"]["tiles"])
    boxes = tile_boxes(ann["hand"]["box"], len(labels))
    ops: list[str] = []

    if rng.random() < 0.5:
        labels = reshuffle(image, boxes, labels, rng)
        ops.append("reshuffle")
    if rng.random() < 0.4:
        index = rng.randrange(len(boxes))
        lift = max(2, round((boxes[0][3] - boxes[0][1]) * rng.uniform(0.08, 0.2)))
        boxes = hover(image, boxes, index, lift)
        ops.append(f"hover:{index}")

    # Window size: the game is rendered at a new width (keeping its aspect ratio).
    width = rng.choice([480, 600, 800, 960, 1280, 1600, 1920])
    scale = width / image.width
    image = image.resize((width, round(image.height * scale)), Image.Resampling.LANCZOS)
    boxes = [tuple(round(v * scale) for v in b) for b in boxes]  # type: ignore[misc]
    ops.append(f"width:{width}")

    if rng.random() < 0.5:
        top = rng.randint(30, 90)
        side = rng.randint(0, 40)
        bottom = rng.randint(0, 40)
        colour = rng.choice([(32, 33, 36), (222, 225, 230), (0, 0, 0)])
        image = ImageOps.expand(image, border=(side, top, side, bottom), fill=colour)
        boxes = [(b[0] + side, b[1] + top, b[2] + side, b[3] + top) for b in boxes]
        ops.append(f"border:{side},{top},{bottom}")
    if rng.random() < 0.6:
        image = ImageEnhance.Brightness(image).enhance(rng.uniform(0.85, 1.15))
        image = ImageEnhance.Contrast(image).enhance(rng.uniform(0.85, 1.15))
        ops.append("colour")
    if rng.random() < 0.3:
        image = image.filter(ImageFilter.GaussianBlur(rng.uniform(0.3, 1.0)))
        ops.append("blur")
    if rng.random() < 0.5:
        quality = rng.randint(50, 90)
        buf = io.BytesIO()
        image.save(buf, format="JPEG", quality=quality)
        image = Image.open(io.BytesIO(buf.getvalue())).convert("RGB")
        ops.append(f"jpeg:{quality}")

    truth = {
        "source": ann["image"],
        "size": list(image.size),
        "ops": ops,
        "hand": [{"tile": t, "box": list(b)} for t, b in zip(labels, boxes)],
    }
    return image, truth


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("annotation", type=pathlib.Path, help="JSON annotation next to the source image")
    parser.add_argument("-n", type=int, default=40, help="number of variants")
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--out", type=pathlib.Path, default=pathlib.Path("recordings/simulated"))
    args = parser.parse_args()

    ann = json.loads(args.annotation.read_text(encoding="utf-8"))
    source = Image.open(args.annotation.parent / ann["image"]).convert("RGB")
    rng = random.Random(args.seed)
    args.out.mkdir(parents=True, exist_ok=True)
    stem = pathlib.Path(ann["image"]).stem
    for i in range(args.n):
        image, truth = make_variant(source, ann, rng)
        image.save(args.out / f"{stem}_{i:03d}.png")
        (args.out / f"{stem}_{i:03d}.json").write_text(json.dumps(truth, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"wrote {args.n} variants to {args.out}")


if __name__ == "__main__":
    main()
