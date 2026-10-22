"""Export the tile classifier pjura/mahjong_soul_vision to ONNX and check it (#4).

The pinned PyTorch weights (see [classifier] in ml/vlm_eval/config.toml) are
exported with a dynamic batch axis to `<models_dir>/mahjong_soul_vision/model.onnx`,
and also quantized to int8 weights (`model.int8.onnx`, a quarter of the size).
Then:

1. ONNX Runtime and PyTorch must agree on the same input tensors: the dataset's
   test split and the hand tiles of the real screenshots, plus two synthetic
   inputs that the Rust test rebuilds bit for bit.
2. The synthetic logits go to `perception/tests/fixtures/classifier_synthetic.json`
   (34 numbers each, no game art) so the Rust side can check its runtime.
3. Per-tile PyTorch logits, predictions and red-five ink shares go to
   `<results_dir>/classifier_reference.json` (local only: it points at game art)
   so the Rust evaluation can compare its whole pipeline, preprocessing included.

Run from the repository root:
    ml/.venv/bin/python -I ml/convert/export_classifier.py
    ml/.venv/bin/python -I ml/convert/export_classifier.py --simulated recordings/simulated_multi
"""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import sys
import time
import tomllib
from typing import Any

import numpy as np
import onnx
import onnxruntime as ort
import torch
from huggingface_hub import snapshot_download
from onnxruntime.quantization import QuantType, quantize_dynamic
from transformers import AutoImageProcessor, AutoModelForImageClassification

ML_DIR = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ML_DIR / "vlm_eval"))
from eval_llamacpp import DATASET_TO_MJAI  # noqa: E402
from eval_vit import dataset_items, simulated_items  # noqa: E402
from red_five import non_red_share  # noqa: E402

CONFIG_PATH = ML_DIR / "vlm_eval" / "config.toml"
FIXTURE_PATH = pathlib.Path("perception/tests/fixtures/classifier_synthetic.json")
OPSET = 18
BATCH = 32


class Logits(torch.nn.Module):
    """The classifier with plain tensors in and out, as ONNX needs."""

    def __init__(self, model: torch.nn.Module) -> None:
        super().__init__()
        self.model = model

    def forward(self, pixel_values: torch.Tensor) -> torch.Tensor:
        return self.model(pixel_values=pixel_values).logits


def synthetic_input(seed: int) -> np.ndarray:
    """A 1x3x224x224 input in [-1, 1] built from integers only, so Rust can rebuild it exactly."""
    c, y, x = np.meshgrid(np.arange(3), np.arange(224), np.arange(224), indexing="ij")
    levels = (x * 7 + y * 13 + c * 29 + seed * 101) % 256
    return (levels.astype(np.float32) / np.float32(127.5) - np.float32(1.0))[None]


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--simulated",
        type=pathlib.Path,
        action="append",
        default=[],
        help="also check hand tiles of these annotation folders (recordings/source is always included)",
    )
    args = parser.parse_args()

    cfg = tomllib.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    spec = cfg["classifier"]
    local_dir = pathlib.Path(cfg["models_dir"]) / "mahjong_soul_vision"
    snapshot_download(
        spec["repo"],
        revision=spec["revision"],
        local_dir=local_dir,
        allow_patterns=["config.json", "preprocessor_config.json", "pytorch_model.bin"],
    )
    processor = AutoImageProcessor.from_pretrained(local_dir)
    model = AutoModelForImageClassification.from_pretrained(local_dir, use_safetensors=False).eval()
    wrapped = Logits(model).eval()

    out = local_dir / "model.onnx"
    start = time.perf_counter()
    torch.onnx.export(
        wrapped,
        (torch.from_numpy(synthetic_input(0)),),
        out,
        input_names=["pixel_values"],
        output_names=["logits"],
        dynamic_shapes={"pixel_values": {0: torch.export.Dim("batch", min=1, max=64)}},
        opset_version=OPSET,
        external_data=False,
    )
    onnx.checker.check_model(str(out))
    print(f"exported {out} ({out.stat().st_size / 1e6:.0f} MB) in {time.perf_counter() - start:.1f} s")

    int8 = local_dir / "model.int8.onnx"
    stripped = onnx.load(str(out))
    # The exporter leaves value_info that the quantizer's shape inference rejects; ONNX
    # Runtime infers shapes again on load, so it can go.
    del stripped.graph.value_info[:]
    bare = local_dir / "model.bare.onnx"
    onnx.save(stripped, str(bare))
    quantize_dynamic(str(bare), str(int8), weight_type=QuantType.QInt8)
    bare.unlink()
    print(f"quantized {int8} ({int8.stat().st_size / 1e6:.0f} MB)")

    session = ort.InferenceSession(str(out), providers=["CPUExecutionProvider"])
    session_int8 = ort.InferenceSession(str(int8), providers=["CPUExecutionProvider"])
    labels = [model.config.id2label[i] for i in range(model.config.num_labels)]

    # 1. Synthetic inputs: the fixture for the Rust runtime check.
    synthetic = []
    for seed in (0, 1):
        x = synthetic_input(seed)
        with torch.inference_mode():
            want = wrapped(torch.from_numpy(x)).numpy()[0]
        got = session.run(["logits"], {"pixel_values": x})[0][0]
        synthetic.append({"seed": seed, "logits": [float(v) for v in want], "onnx_max_abs_diff": float(np.abs(want - got).max())})
    FIXTURE_PATH.parent.mkdir(parents=True, exist_ok=True)
    FIXTURE_PATH.write_text(
        json.dumps(
            {
                "model": {"repo": spec["repo"], "revision": spec["revision"], "onnx_sha256": sha256(out), "opset": OPSET},
                "input": "x[c][y][x] = ((x * 7 + y * 13 + c * 29 + seed * 101) % 256) / 127.5 - 1",
                "labels": [DATASET_TO_MJAI[label] for label in labels],
                "cases": synthetic,
            },
            indent=1,
        )
        + "\n",
        encoding="utf-8",
    )

    # 2. Real crops: PyTorch against ONNX Runtime on identical tensors.
    items = dataset_items(cfg, "test") + simulated_items(pathlib.Path("recordings/source"))
    for folder in args.simulated:
        items += simulated_items(folder)
    rows: list[dict[str, Any]] = []
    max_diff = 0.0
    disagree = 0
    disagree_int8 = 0
    for i in range(0, len(items), BATCH):
        batch = items[i : i + BATCH]
        x = processor(images=[image for image, *_ in batch], return_tensors="np")["pixel_values"].astype(np.float32)
        with torch.inference_mode():
            want = wrapped(torch.from_numpy(x)).numpy()
        got = session.run(["logits"], {"pixel_values": x})[0]
        max_diff = max(max_diff, float(np.abs(want - got).max()))
        disagree += int((want.argmax(-1) != got.argmax(-1)).sum())
        got_int8 = session_int8.run(["logits"], {"pixel_values": x})[0]
        disagree_int8 += int((want.argmax(-1) != got_int8.argmax(-1)).sum())
        for (image, label, split, source), logits in zip(batch, want):
            pred = DATASET_TO_MJAI[labels[int(logits.argmax())]]
            row = {"split": split, "source": source, "label": label, "pred": pred, "logits": [float(v) for v in logits]}
            if pred in ("5m", "5p", "5s") or label.startswith("5"):
                row["non_red_share"] = non_red_share(image)
            rows.append(row)

    reference = pathlib.Path(cfg["results_dir"]) / "classifier_reference.json"
    reference.parent.mkdir(parents=True, exist_ok=True)
    reference.write_text(
        json.dumps({"labels": [DATASET_TO_MJAI[label] for label in labels], "rows": rows}, ensure_ascii=False),
        encoding="utf-8",
    )
    print(
        json.dumps(
            {
                "synthetic_max_abs_diff": max(c["onnx_max_abs_diff"] for c in synthetic),
                "tiles": len(rows),
                "max_abs_logit_diff": max_diff,
                "argmax_disagreements": disagree,
                "int8_argmax_disagreements": disagree_int8,
                "fixture": str(FIXTURE_PATH),
                "reference": str(reference),
            },
            indent=1,
        )
    )


if __name__ == "__main__":
    main()
