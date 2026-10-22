"""Write Pillow's colour conversion and resize results as Rust test fixtures (#4).

The Rust recognition reproduces two Pillow operations exactly: the 8-bit RGB -> HSV
conversion behind the red-five rule (ml/vlm_eval/red_five.py), and the bilinear resize
of the classifier's preprocessing. The fixtures hold synthetic inputs only, no game art:

- `perception/tests/fixtures/pil_hsv.json`: a 0-255 grid of colours in steps of 17 plus
  seeded random colours, as flat [r, g, b, h, s, v] rows;
- `perception/tests/fixtures/pil_resize.json`: patterned images of several sizes
  resized to 224x224, each with a 64-bit FNV-1a hash of the output bytes. It also
  checks that the model's preprocessor gives exactly Pillow's resize scaled to [-1, 1].

Run from the repository root:
    ml/.venv/bin/python -I ml/convert/pillow_fixtures.py
"""

from __future__ import annotations

import json
import pathlib

import numpy as np
import PIL
from PIL import Image
from transformers import AutoImageProcessor

FIXTURES = pathlib.Path("perception/tests/fixtures")
MODEL_DIR = pathlib.Path("ml/models/mahjong_soul_vision")
RANDOM_COLOURS = 4000
SEED = 7
OUT = 224
# (width, height) of the inputs: up- and downscaling, one axis only, and no change.
SIZES = [(31, 47), (13, 9), (1, 1), (300, 200), (600, 40), (5, 300), (224, 100), (100, 224), (224, 224), (64, 87)]


def pattern(width: int, height: int) -> Image.Image:
    """value(x, y, c) = (x * 31 + y * 17 + c * 73 + (x * y) % 13) % 256, from integers only."""
    y, x, c = np.meshgrid(np.arange(height), np.arange(width), np.arange(3), indexing="ij")
    return Image.fromarray(((x * 31 + y * 17 + c * 73 + (x * y) % 13) % 256).astype(np.uint8), "RGB")


def fnv1a64(data: bytes) -> int:
    h = 0xCBF29CE484222325
    for byte in data:
        h = ((h ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def hsv_fixture() -> None:
    levels = np.arange(0, 256, 17)
    grid = np.stack(np.meshgrid(levels, levels, levels, indexing="ij"), axis=-1).reshape(-1, 3)
    rng = np.random.default_rng(SEED)
    colours = np.concatenate([grid, rng.integers(0, 256, size=(RANDOM_COLOURS, 3))]).astype(np.uint8)
    image = Image.fromarray(colours[None, :, :], "RGB")
    hsv = np.asarray(image.convert("HSV"))[0]
    rows = np.concatenate([colours, hsv], axis=1).astype(int).tolist()
    (FIXTURES / "pil_hsv.json").write_text(
        json.dumps({"pillow": PIL.__version__, "columns": ["r", "g", "b", "h", "s", "v"], "rows": rows}, separators=(",", ":"))
        + "\n",
        encoding="utf-8",
    )
    print(f"wrote {len(rows)} colours")


def resize_fixture() -> None:
    processor = AutoImageProcessor.from_pretrained(MODEL_DIR)
    cases = []
    for width, height in SIZES:
        image = pattern(width, height)
        resized = image.resize((OUT, OUT), Image.Resampling.BILINEAR)
        pixels = np.asarray(resized)
        # The model's preprocessing must be exactly this resize, scaled to [-1, 1].
        want = (np.transpose(pixels, (2, 0, 1)).astype(np.float32) / np.float32(127.5) - np.float32(1.0))[None]
        got = processor(images=[image], return_tensors="np")["pixel_values"]
        assert np.abs(got - want).max() < 1e-6, (width, height, np.abs(got - want).max())
        cases.append({"width": width, "height": height, "fnv1a64": f"{fnv1a64(pixels.tobytes()):016x}"})
    (FIXTURES / "pil_resize.json").write_text(
        json.dumps(
            {
                "pillow": PIL.__version__,
                "pattern": "value(x, y, c) = (x * 31 + y * 17 + c * 73 + (x * y) % 13) % 256",
                "output": [OUT, OUT],
                "hash": "64-bit FNV-1a of the row-major RGB bytes",
                "cases": cases,
            },
            indent=1,
        )
        + "\n",
        encoding="utf-8",
    )
    print(f"wrote {len(cases)} resize cases")


def main() -> None:
    FIXTURES.mkdir(parents=True, exist_ok=True)
    hsv_fixture()
    resize_fixture()


if __name__ == "__main__":
    main()
