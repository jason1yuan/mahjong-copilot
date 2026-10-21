"""Tell red fives from plain fives by the colour of their strokes (#24).

The classifier only knows the 34 tile kinds. A plain five has non-red strokes
(the black 伍 of 5m, the dark circles of 5p, the green sticks of 5s) while a red
five is drawn in red only, so the share of non-red ink among all ink is a
simple, art-independent signal. The top of the crop (the orange rim) and the
edges (tile borders) are ignored.
"""

from __future__ import annotations

import numpy as np
from PIL import Image

# Below this share of non-red ink a five is called red. Chosen on the dataset's
# 145 fives only (62 red at <= 0.20, 83 plain at >= 0.30, judged by eye), then
# checked on screenshots that played no part in choosing it; see NOTES.md.
NON_RED_THRESHOLD = 0.25


def ink_shares(crop: Image.Image) -> dict[str, float]:
    """Shares of red, green and dark pixels among ink pixels in the tile's face."""
    w, h = crop.size
    face = crop.convert("RGB").crop((round(w * 0.12), round(h * 0.18), round(w * 0.88), round(h * 0.92)))
    hsv = np.asarray(face.convert("HSV"), dtype=float) / 255.0
    hue, sat, val = hsv[..., 0] * 360, hsv[..., 1], hsv[..., 2]
    red = ((hue < 15) | (hue > 330)) & (sat > 0.35) & (val > 0.2)
    green = (hue > 75) & (hue < 175) & (sat > 0.25) & (val > 0.15)
    # Black ink is unsaturated; this keeps the blue table out of small river crops.
    dark = (val < 0.4) & (sat < 0.45) & ~red & ~green
    ink = red | green | dark
    total = max(int(ink.sum()), 1)
    return {
        "red": float(red.sum()) / total,
        "green": float(green.sum()) / total,
        "dark": float(dark.sum()) / total,
        "ink": float(ink.mean()),
    }


def non_red_share(crop: Image.Image) -> float:
    shares = ink_shares(crop)
    return shares["green"] + shares["dark"]


def refine(kind: str, crop: Image.Image) -> str:
    """Turns a predicted plain five into a red five when its ink is (almost) all red."""
    if kind in ("5m", "5p", "5s") and non_red_share(crop) < NON_RED_THRESHOLD:
        return kind + "r"
    return kind
