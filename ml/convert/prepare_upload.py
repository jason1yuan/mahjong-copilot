"""Assemble the Hugging Face uploads of our ONNX exports (#4, #22).

The app runs models in ONNX Runtime, but the original repositories only have PyTorch
weights, so we host the exports ourselves. Each upload is put together in
`<models_dir>/<name>/` (never committed) and contains no images:

- `classifier` -> `mahjong_soul_vision-onnx/`: the tile classifier from
  export_classifier.py (fp32, weights unchanged), its original `config.json` and
  `preprocessor_config.json`, the Apache License 2.0 and a model card.
- `mortal` -> `Mortal-S42-onnx/`: the decision model from export_mortal.py (weights
  unchanged), the upstream `NOTICE` and `model-identity.json` as they are, the Apache
  License 2.0 of the weights, the AGPL-3.0 of the network code it was traced from, and a
  model card.

Upload it yourself after `hf auth login`:
    ml/.venv/bin/python -I ml/convert/prepare_upload.py classifier
    ml/.venv/bin/hf upload <user>/mahjong_soul_vision-onnx ml/models/mahjong_soul_vision-onnx . --no-private
    ml/.venv/bin/python -I ml/convert/prepare_upload.py mortal
    ml/.venv/bin/hf upload <user>/Mortal-S42-onnx ml/models/Mortal-S42-onnx . --no-private
"""

from __future__ import annotations

import argparse
import hashlib
import pathlib
import shutil
import tomllib

import requests

ML_DIR = pathlib.Path(__file__).resolve().parent.parent
CONFIG_PATH = ML_DIR / "vlm_eval" / "config.toml"
AGPL_PATH = ML_DIR.parent / "LICENSE"
LICENSE_URL = "https://www.apache.org/licenses/LICENSE-2.0.txt"
COPIED = ["model.onnx", "config.json", "preprocessor_config.json"]
# Reported by export_classifier.py for the exported model (3.9e-5 on 2989 crops).
MAX_LOGIT_DIFF = "4e-5"
# Copied from the upstream Mortal-S42 download as they are.
MORTAL_COPIED = ["model.onnx", "NOTICE", "model-identity.json"]
# Reported by export_mortal.py for the exported model (PR #47).
MORTAL_POINTS = 605
MORTAL_MAX_Q_DIFF = "9.5e-6"
# Upstream Mortal commit the network code comes from, as named in the upstream NOTICE.
MORTAL_COMMIT = "0cff2b52982be5b1163aa9a62fb01f03ce91e0d2"

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

MORTAL_CARD = """---
license: apache-2.0
base_model: {repo}
library_name: onnx
tags:
- onnx
- mahjong
- riichi-mahjong
- mortal
---

# Mortal-S42 (ONNX)

An ONNX export of the decision model [{repo}](https://huggingface.co/{repo}), a
[Mortal](https://github.com/Equim-chan/Mortal) v4 model for four-player riichi mahjong.
Hosted here because the original repository only has PyTorch weights; it is used by
[mahjong-copilot](https://github.com/jason1yuan/mahjong-copilot), an offline mahjong study
aid that reads the game window and never touches the game.

## Changes from the original

- Exported from revision `{revision}`, file `{weights}`, with `torch.onnx.export`
  (opset 18, dynamic batch axis). The encoder ("brain", 40 residual blocks, 192
  channels) and the Q head ("dqn") are exported together as one graph. The weights are
  not changed, retrained or quantized.
- On {points} decision points taken from game logs, the ONNX model and the original
  PyTorch model pick the same best action every time; the largest difference in a legal
  action's Q value is {max_diff}.

## Input and output

- `obs`: float32 `[batch, 1012, 34]`, Mortal's v4 observation, as encoded by
  libriichi's `obs_repr`.
- `mask`: bool `[batch, 46]`, the legal actions.
- `q`: float32 `[batch, 46]`, the Q value of each action; illegal actions are `-inf`.
  Mortal plays the action with the highest Q.

## Files

| File | SHA-256 |
|---|---|
{hashes}

## License

- The weights are under the Apache License 2.0, as stated by the original repository;
  see `LICENSE` and the upstream `NOTICE`, kept as it is. `model-identity.json` is the
  upstream description of the weights, also kept as it is.
- The network structure in the graph was traced from Mortal's `mortal/model.py`
  (commit `{commit}`), which is under the GNU Affero General Public License v3.0 or
  later; see `LICENSE.AGPL`. Mortal's source code is at
  https://github.com/Equim-chan/Mortal.
"""


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def apache_license() -> str:
    response = requests.get(LICENSE_URL, timeout=30)
    response.raise_for_status()
    return response.text


def hash_table(out: pathlib.Path, names: list[str]) -> str:
    return "\n".join(f"| `{name}` | `{sha256(out / name)}` |" for name in names)


def classifier(cfg: dict) -> pathlib.Path:
    spec = cfg["classifier"]
    source = pathlib.Path(cfg["models_dir"]) / "mahjong_soul_vision"
    out = pathlib.Path(cfg["models_dir"]) / "mahjong_soul_vision-onnx"
    out.mkdir(parents=True, exist_ok=True)
    for name in COPIED:
        shutil.copy2(source / name, out / name)
    (out / "LICENSE").write_text(apache_license(), encoding="utf-8")
    (out / "README.md").write_text(
        CARD.format(
            repo=spec["repo"], revision=spec["revision"], max_diff=MAX_LOGIT_DIFF, hashes=hash_table(out, COPIED)
        ),
        encoding="utf-8",
    )
    return out


def mortal(cfg: dict) -> pathlib.Path:
    spec = cfg["mortal"]
    source = pathlib.Path(cfg["models_dir"]) / "mortal-s42"
    out = pathlib.Path(cfg["models_dir"]) / "Mortal-S42-onnx"
    out.mkdir(parents=True, exist_ok=True)
    for name in MORTAL_COPIED:
        shutil.copy2(source / name, out / name)
    (out / "LICENSE").write_text(apache_license(), encoding="utf-8")
    shutil.copy2(AGPL_PATH, out / "LICENSE.AGPL")
    (out / "README.md").write_text(
        MORTAL_CARD.format(
            repo=spec["repo"],
            revision=spec["revision"],
            weights=spec["weights"],
            points=MORTAL_POINTS,
            max_diff=MORTAL_MAX_Q_DIFF,
            commit=MORTAL_COMMIT,
            hashes=hash_table(out, MORTAL_COPIED),
        ),
        encoding="utf-8",
    )
    return out


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("model", choices=["classifier", "mortal"])
    args = parser.parse_args()
    cfg = tomllib.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    out = classifier(cfg) if args.model == "classifier" else mortal(cfg)
    print(f"ready to upload: {out}")
    for path in sorted(out.iterdir()):
        print(f"  {path.name}: {path.stat().st_size / 1e6:.2f} MB")


if __name__ == "__main__":
    main()
