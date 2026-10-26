"""Export the decision model Mortal-S42 to ONNX and check it (#22).

The pinned weights and Mortal's model code (see [mortal] in ml/vlm_eval/config.toml)
are downloaded into `ml/models/mortal-s42/` and checked against their SHA-256. The
two parts used for play, the encoder ("brain") and the Q head ("dqn"), are exported
as one graph to `ml/models/mortal-s42/model.onnx`:

    obs (batch, 1012, 34) f32, mask (batch, 46) bool -> q (batch, 46) f32

with illegal actions at -inf, as the PyTorch engine gives them. Then:

1. ONNX Runtime and PyTorch must agree on real observations: the decision points of
   game logs, written by `cargo run -p mahjong-core --release --example
   dump_observations -- ml/models/mortal-s42/observations.bin <logs...>`. Same best
   action everywhere, legal-action Q within a small tolerance.
2. The first points' Q go to `ml/models/mortal-s42/reference.bin` for the Rust test.
3. `--install DIR` copies the model to `DIR/mortal-s42.onnx`. The app downloads its own
   copy (see models/src/lib.rs), so this is only for trying a new export by hand.

Nothing here is committed: weights, model, observations and reference stay local. The
model is published from our own Hugging Face repository; prepare_upload.py assembles it.

Run from the repository root:
    ml/.venv/bin/python -I ml/convert/export_mortal.py
    ml/.venv/bin/python -I ml/convert/export_mortal.py --install /tmp/models
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import pathlib
import shutil
import sys
import time
import tomllib
import types
import urllib.request

import numpy as np
import onnxruntime as ort
import torch

ML_DIR = pathlib.Path(__file__).resolve().parent.parent
CONFIG_PATH = ML_DIR / "vlm_eval" / "config.toml"
MODEL_DIR = ML_DIR / "models" / "mortal-s42"
OBS_SHAPE = (1012, 34)
ACTIONS = 46
OPSET = 18
BATCH = 32
REFERENCE_POINTS = 64
# Largest difference allowed between ONNX Runtime's and PyTorch's legal-action Q.
TOLERANCE = 1e-3


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def fetch(config: dict[str, str], name: str, checksum: str) -> pathlib.Path:
    """The pinned file, downloaded once and checked."""
    path = MODEL_DIR / pathlib.Path(name).name
    if not path.exists():
        url = f"https://huggingface.co/{config['repo']}/resolve/{config['revision']}/{name}"
        print(f"downloading {url}")
        MODEL_DIR.mkdir(parents=True, exist_ok=True)
        urllib.request.urlretrieve(url, path)
    got = sha256(path)
    if got != checksum:
        raise SystemExit(f"{path}: SHA-256 {got}, expected {checksum}")
    return path


def load_model_code(path: pathlib.Path) -> types.ModuleType:
    """Mortal's model.py; it imports a few constants from libriichi's Python module,
    which is not built here, so they are given to it."""
    consts = types.ModuleType("libriichi.consts")
    consts.ACTION_SPACE = ACTIONS
    consts.GRP_SIZE = 7
    consts.obs_shape = lambda version: {1: (938, 34), 2: (942, 34), 3: (934, 34), 4: OBS_SHAPE}[version]
    consts.oracle_obs_shape = lambda version: (211, 34) if version == 1 else (217, 34)
    package = types.ModuleType("libriichi")
    package.consts = consts
    sys.modules["libriichi"] = package
    sys.modules["libriichi.consts"] = consts
    spec = importlib.util.spec_from_file_location("mortal_model", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Mortal(torch.nn.Module):
    """Brain then DQN, as Mortal's engine runs them for v4."""

    def __init__(self, brain: torch.nn.Module, dqn: torch.nn.Module) -> None:
        super().__init__()
        self.brain = brain
        self.dqn = dqn

    def forward(self, obs: torch.Tensor, mask: torch.Tensor) -> torch.Tensor:
        return self.dqn(self.brain(obs), mask)


def observations(path: pathlib.Path) -> tuple[np.ndarray, np.ndarray]:
    """The decision points written by the dump_observations example."""
    raw = np.fromfile(path, dtype=np.uint8)
    point = OBS_SHAPE[0] * OBS_SHAPE[1] * 4 + ACTIONS
    count = len(raw) // point
    raw = raw[: count * point].reshape(count, point)
    obs = raw[:, : point - ACTIONS].copy().view("<f4").reshape(count, *OBS_SHAPE)
    mask = raw[:, point - ACTIONS :].astype(bool)
    return obs, mask


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--install", type=pathlib.Path, help="the app's models folder")
    args = parser.parse_args()
    config = tomllib.loads(CONFIG_PATH.read_text())["mortal"]
    weights = fetch(config, config["weights"], config["weights_sha256"])
    code = fetch(config, config["model_code"], config["model_code_sha256"])
    module = load_model_code(code)

    state = torch.load(weights, map_location="cpu", weights_only=True)
    brain = module.Brain(version=4, conv_channels=192, num_blocks=40).eval()
    dqn = module.DQN(version=4).eval()
    brain.load_state_dict(state["mortal"], strict=True)
    dqn.load_state_dict(state["current_dqn"], strict=True)
    model = Mortal(brain, dqn).eval()

    onnx_path = MODEL_DIR / "model.onnx"
    example = (torch.zeros(1, *OBS_SHAPE), torch.ones(1, ACTIONS, dtype=torch.bool))
    with torch.inference_mode():
        torch.onnx.export(
            model,
            example,
            onnx_path,
            input_names=["obs", "mask"],
            output_names=["q"],
            dynamic_axes={"obs": {0: "batch"}, "mask": {0: "batch"}, "q": {0: "batch"}},
            opset_version=OPSET,
            dynamo=False,
        )
    print(f"{onnx_path}: {onnx_path.stat().st_size / 1e6:.1f} MB, SHA-256 {sha256(onnx_path)}")

    obs, mask = observations(MODEL_DIR / "observations.bin")
    session = ort.InferenceSession(str(onnx_path), providers=["CPUExecutionProvider"])
    worst, same = 0.0, 0
    reference = []
    for start in range(0, len(obs), BATCH):
        o, m = obs[start : start + BATCH], mask[start : start + BATCH]
        with torch.inference_mode():
            want = model(torch.from_numpy(o), torch.from_numpy(m)).numpy()
        got = session.run(["q"], {"obs": o, "mask": m})[0]
        legal = m
        worst = max(worst, float(np.abs(got[legal] - want[legal]).max()))
        if not np.array_equal(np.isinf(got), ~legal):
            raise SystemExit("ONNX Runtime marks other actions illegal than the mask")
        same += int((got.argmax(-1) == want.argmax(-1)).sum())
        reference.extend(want)
    print(f"{len(obs)} decision points: same best action in {same}, largest Q difference {worst:.2e}")
    if same != len(obs) or worst > TOLERANCE:
        raise SystemExit("ONNX Runtime and PyTorch disagree")
    np.asarray(reference[:REFERENCE_POINTS], dtype="<f4").tofile(MODEL_DIR / "reference.bin")

    # Single-decision latency on the CPU, as the app runs it.
    one = {"obs": obs[:1], "mask": mask[:1]}
    for _ in range(5):
        session.run(["q"], one)
    times = []
    for i in range(100):
        start_time = time.perf_counter()
        session.run(["q"], {"obs": obs[i % len(obs) : i % len(obs) + 1], "mask": mask[i % len(obs) : i % len(obs) + 1]})
        times.append((time.perf_counter() - start_time) * 1e3)
    times.sort()
    print(f"ONNX Runtime CPU, one decision: median {times[50]:.1f} ms, p95 {times[95]:.1f} ms")

    if args.install:
        args.install.mkdir(parents=True, exist_ok=True)
        shutil.copy(onnx_path, args.install / "mortal-s42.onnx")
        print(f"installed to {args.install / 'mortal-s42.onnx'}")


if __name__ == "__main__":
    main()
