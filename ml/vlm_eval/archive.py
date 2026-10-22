"""Archive evaluation results with everything needed to reproduce them (#24).

Copies every results file and run log from `results_dir` into
`ml/experiments/<run-id>/` (committed to git) and writes `manifest.json` with
the machine, tool versions, the exact config and prompt, pinned model and
dataset revisions with file hashes, and hashes of local inputs such as
screenshots. Images themselves are never copied: they are game art and stay local.
Running it again refreshes the archive.

Run from the repository root:
    ml/.venv/bin/python -I ml/vlm_eval/archive.py 2026-10-21-tile-recognition
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import importlib.metadata
import json
import pathlib
import platform
import shutil
import subprocess
import tomllib
from typing import Any

import requests

CONFIG_PATH = pathlib.Path(__file__).with_name("config.toml")
PACKAGES = ["huggingface_hub", "mlx", "mlx-vlm", "numpy", "pillow", "requests", "torch", "transformers"]


def run(*cmd: str) -> str:
    try:
        out = subprocess.run(cmd, capture_output=True, text=True, check=False)
        # Some tools (llama-server --version) print to stderr.
        return (out.stdout or out.stderr).strip()
    except FileNotFoundError:
        return ""


def sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def hub_files(kind: str, repo: str, revision: str, wanted: list[str] | None) -> dict[str, Any]:
    """File sizes and LFS sha256 of a pinned Hugging Face revision."""
    url = f"https://huggingface.co/api/{kind}/{repo}/revision/{revision}?blobs=true"
    siblings = requests.get(url, timeout=30).json().get("siblings", [])
    return {
        s["rfilename"]: {"size": s.get("size"), "sha256": (s.get("lfs") or {}).get("sha256")}
        for s in siblings
        if wanted is None or s["rfilename"] in wanted
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("run_id", help="folder name under ml/experiments")
    parser.add_argument("--inputs", nargs="*", default=["recordings/source", "recordings/simulated"])
    parser.add_argument(
        "--annotations",
        type=pathlib.Path,
        default=pathlib.Path("recordings/source"),
        help="folder of hand annotations (labels and boxes only) to copy into the archive",
    )
    args = parser.parse_args()

    cfg = tomllib.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    out = pathlib.Path("ml/experiments") / args.run_id
    (out / "results").mkdir(parents=True, exist_ok=True)
    (out / "logs").mkdir(parents=True, exist_ok=True)

    results_dir = pathlib.Path(cfg["results_dir"])
    copied = {}
    for path in sorted(results_dir.glob("*.json")):
        shutil.copy2(path, out / "results" / path.name)
        copied[f"results/{path.name}"] = sha256(path)
    for path in sorted(results_dir.glob("*.log")):
        shutil.copy2(path, out / "logs" / path.name)
    shutil.copy2(CONFIG_PATH, out / "config.toml")
    # Annotations are our own labels and boxes, not game art, so they are kept with the results.
    (out / "annotations").mkdir(exist_ok=True)
    for path in sorted(args.annotations.glob("*.json")):
        shutil.copy2(path, out / "annotations" / path.name)

    models = {
        name: {**spec, "files_on_hub": hub_files("models", spec["repo"], spec["revision"], [spec["model"], spec["mmproj"]])}
        for name, spec in cfg["models"].items()
    }
    mlx_models = {
        name: {**spec, "files_on_hub": hub_files("models", spec["repo"], spec["revision"], None)}
        for name, spec in cfg.get("mlx_models", {}).items()
    }
    classifier = cfg["classifier"]
    classifier = {
        **classifier,
        "files_on_hub": hub_files(
            "models", classifier["repo"], classifier["revision"], ["config.json", "preprocessor_config.json", "pytorch_model.bin"]
        ),
    }

    inputs = {}
    for folder in args.inputs:
        for path in sorted(pathlib.Path(folder).rglob("*")):
            if path.is_file():
                inputs[str(path)] = sha256(path)

    manifest = {
        "run_id": args.run_id,
        "archived_at": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "git": {
            "commit": run("git", "rev-parse", "HEAD"),
            "branch": run("git", "rev-parse", "--abbrev-ref", "HEAD"),
            "dirty": bool(run("git", "status", "--porcelain")),
        },
        "machine": {
            "cpu": run("sysctl", "-n", "machdep.cpu.brand_string"),
            "memory_bytes": int(run("sysctl", "-n", "hw.memsize") or 0),
            "os": f"macOS {platform.mac_ver()[0]}",
            "python": platform.python_version(),
        },
        "tools": {
            "llama.cpp": run(cfg["server"]["binary"], "--version"),
            **{p: importlib.metadata.version(p) for p in PACKAGES if _installed(p)},
        },
        "dataset": {
            "repo": cfg["dataset_repo"],
            "revision": cfg["dataset_revision"],
            "used": "original images only (no *_aug_* files)",
        },
        "models": models,
        "mlx_models": mlx_models,
        "classifier": classifier,
        "online": cfg.get("openai_decisions"),
        "prompt": cfg["prompt"]["text"],
        "results": copied,
        "local_inputs_sha256": inputs,
    }
    (out / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"archived {len(copied)} results, {len(inputs)} input hashes to {out}")


def _installed(package: str) -> bool:
    try:
        importlib.metadata.version(package)
        return True
    except importlib.metadata.PackageNotFoundError:
        return False


if __name__ == "__main__":
    main()
