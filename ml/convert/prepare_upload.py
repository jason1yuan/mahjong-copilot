"""Assemble the Hugging Face upload of the converted tile classifier (#4).

The original author's repository only has PyTorch weights, which the app cannot run,
so we host the ONNX export ourselves. This puts together, in
`<models_dir>/mahjong_soul_vision-onnx/` (never committed):

- `model.onnx` from export_classifier.py (fp32, weights unchanged);
- the original `config.json` and `preprocessor_config.json` (labels, preprocessing);
- `LICENSE`: the Apache License 2.0, the original model's license;
- `README.md`: a model card naming the source, the change and the training data.

It contains no images. Upload it yourself after `hf auth login`:
    ml/.venv/bin/python -I ml/convert/prepare_upload.py
    ml/.venv/bin/hf upload <user>/mahjong_soul_vision-onnx ml/models/mahjong_soul_vision-onnx . --no-private
"""

from __future__ import annotations

import hashlib
import pathlib
import shutil
import tomllib

import requests

ML_DIR = pathlib.Path(__file__).resolve().parent.parent
CONFIG_PATH = ML_DIR / "vlm_eval" / "config.toml"
LICENSE_URL = "https://www.apache.org/licenses/LICENSE-2.0.txt"
COPIED = ["model.onnx", "config.json", "preprocessor_config.json"]
# Reported by export_classifier.py for the exported model (3.9e-5 on 2989 crops).
MAX_LOGIT_DIFF = "4e-5"

CARD = """---
license: apache-2.0
base_model: {repo}
library_name: onnx
pipeline_tag: image-classification
tags:
- onnx
- vit
- mahjong
---

# mahjong_soul_vision (ONNX)

An ONNX export of [{repo}](https://huggingface.co/{repo}), a ViT-base image classifier its
author fine-tuned to tell the 34 Japanese mahjong tile kinds apart on Mahjong Soul screen
crops. Hosted here because the original repository only has PyTorch weights; it is used by
an offline mahjong study aid that reads the game window and never touches the game.

## Changes from the original

- Exported from revision `{revision}` with `torch.onnx.export` (opset 18, dynamic batch
  axis). The weights are not changed, retrained or quantized.
- On 2989 tile crops the ONNX model and the original PyTorch model give the same answer
  for every crop; the largest logit difference is {max_diff}.

## Input and output

- `pixel_values`: float32 `[batch, 3, 224, 224]`. Resize the RGB crop to 224x224 with
  Pillow's bilinear filter, then scale each value as `v / 127.5 - 1` (see
  `preprocessor_config.json`).
- `logits`: float32 `[batch, 34]`, in the order of `id2label` in `config.json`
  (`n` = characters, `p` = circles, `b` = bamboo; `ew sw ww nw` winds; `wd gd rd`
  dragons). There is no red-five class.

## Files

| File | SHA-256 |
|---|---|
{hashes}

## License and training data

Apache License 2.0, as the original model; see `LICENSE`. The model was trained by its
original author on screenshots of the game Mahjong Soul
([pjura/mahjong_souls_tiles](https://huggingface.co/datasets/pjura/mahjong_souls_tiles)).
The game art belongs to its owners. This repository contains no images. If you hold rights
to the game and want it removed, open a discussion and it will be taken down.
"""


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def main() -> None:
    cfg = tomllib.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    spec = cfg["classifier"]
    source = pathlib.Path(cfg["models_dir"]) / "mahjong_soul_vision"
    out = pathlib.Path(cfg["models_dir"]) / "mahjong_soul_vision-onnx"
    out.mkdir(parents=True, exist_ok=True)
    for name in COPIED:
        shutil.copy2(source / name, out / name)

    license_text = requests.get(LICENSE_URL, timeout=30)
    license_text.raise_for_status()
    (out / "LICENSE").write_text(license_text.text, encoding="utf-8")

    hashes = "\n".join(f"| `{name}` | `{sha256(out / name)}` |" for name in COPIED)
    (out / "README.md").write_text(
        CARD.format(repo=spec["repo"], revision=spec["revision"], max_diff=MAX_LOGIT_DIFF, hashes=hashes),
        encoding="utf-8",
    )
    print(f"ready to upload: {out}")
    for name in [*COPIED, "LICENSE", "README.md"]:
        print(f"  {name}: {(out / name).stat().st_size / 1e6:.2f} MB")


if __name__ == "__main__":
    main()
