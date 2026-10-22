"""Find the player's hand tiles at the bottom of a Mahjong Soul screenshot (#24).

Tile faces are light and low in saturation, so the hand shows up as a band of
bright rows near the bottom; inside that band, runs of bright columns are tile
groups (the concealed hand, the drawn tile after a gap, called melds), and dips
in column brightness between them are the borders of single tiles. Prints the
boxes it finds as JSON and can draw them for checking by eye. It is an
annotation aid, not part of the app.

Run from the repository root:
    ml/.venv/bin/python -I ml/vlm_eval/find_hand.py recordings/web/x.png --draw /tmp/x.png
"""

from __future__ import annotations

import argparse
import json
import pathlib

import numpy as np
from PIL import Image, ImageDraw

Box = tuple[int, int, int, int]


def face_mask(rgb: np.ndarray) -> np.ndarray:
    bright = rgb.mean(axis=2) > 175
    grey = (rgb.max(axis=2).astype(int) - rgb.min(axis=2)) < 45
    return bright & grey


def runs(flags: np.ndarray, min_len: int = 1, max_gap: int = 0) -> list[tuple[int, int]]:
    """[start, end) runs of True, merging runs separated by at most max_gap False values."""
    out: list[list[int]] = []
    for i, f in enumerate(flags):
        if not f:
            continue
        if out and i - out[-1][1] <= max_gap:
            out[-1][1] = i + 1
        else:
            out.append([i, i + 1])
    return [(a, b) for a, b in out if b - a >= min_len]


def find_band(mask: np.ndarray) -> tuple[int, int]:
    h, w = mask.shape
    start = int(h * 0.7)
    rows = mask[start:].mean(axis=1) > 0.22
    band = max(runs(rows, min_len=8, max_gap=3), key=lambda r: r[1] - r[0])
    return start + band[0], start + band[1]


def find_groups(image: Image.Image) -> dict[str, object]:
    """Finds the hand band and the horizontal extent of each group of touching tiles.

    Columns are judged on a thin strip near the top of the faces, above the
    symbols, so a tile's own red, green or black strokes do not split it.
    """
    rgb = np.asarray(image.convert("RGB"))
    mask = face_mask(rgb)
    y0, y1 = find_band(mask)
    h = y1 - y0
    strip = mask[y0 + h // 10 : y0 + h // 5]
    col = strip.mean(axis=0)
    groups = runs(col > 0.6, min_len=h // 3, max_gap=max(2, h // 30))
    return {"band": [y0, y1], "groups": [list(g) for g in groups]}


def tile_boxes(band: list[int], groups: list[list[int]], counts: list[int]) -> list[Box]:
    """Equal slots inside each group; counts[i] tiles in groups[i] (0 skips a group)."""
    y0, y1 = band
    boxes: list[Box] = []
    for (gx0, gx1), n in zip(groups, counts):
        width = (gx1 - gx0) / n if n else 0
        boxes += [(round(gx0 + k * width), y0, round(gx0 + (k + 1) * width), y1) for k in range(n)]
    return boxes


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("image", type=pathlib.Path)
    parser.add_argument("--counts", type=int, nargs="*", help="tiles per group, to draw tile boxes")
    parser.add_argument("--draw", type=pathlib.Path, help="write the bottom of the image with the boxes drawn")
    args = parser.parse_args()
    image = Image.open(args.image).convert("RGB")
    found = find_groups(image)
    band, groups = found["band"], found["groups"]
    boxes = tile_boxes(band, groups, args.counts) if args.counts else [(g[0], band[0], g[1], band[1]) for g in groups]  # type: ignore[arg-type,index]
    print(json.dumps({"image": str(args.image), **found, "boxes": [list(b) for b in boxes]}))
    if args.draw:
        y0, y1 = band  # type: ignore[misc]
        top = max(0, y0 - (y1 - y0) // 2)
        strip = image.crop((0, top, image.width, min(image.height, y1 + (y1 - y0) // 2)))
        draw = ImageDraw.Draw(strip)
        for i, (a, b, c, d) in enumerate(boxes):
            draw.rectangle((a, b - top, c, d - top), outline=(255, 0, 0), width=2)
            draw.text((a + 3, b - top + 3), str(i), fill=(255, 0, 0))
        strip.save(args.draw)


if __name__ == "__main__":
    main()
