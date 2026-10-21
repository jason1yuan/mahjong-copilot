"""Evaluate a VLM under MLX on single Mahjong Soul tile crops (#24).

Compares the MLX runtime with llama.cpp on the same samples and prompt. MLX has
no grammar support here, so answers are free text parsed into mjai. Results go
to `<results_dir>/mlx-<model>.json`.

Run from the repository root:
    ml/.venv/bin/python -I ml/vlm_eval/eval_mlx.py qwen3-vl-2b --per-label 5 --seed 7
"""

from __future__ import annotations

import argparse
import base64
import collections
import json
import pathlib
import random
import resource
import shutil
import sys
import tempfile
import time
import tomllib
from typing import Any

from huggingface_hub import snapshot_download

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from eval_llamacpp import encode_image, load_samples, summarize, to_mjai  # noqa: E402

CONFIG_PATH = pathlib.Path(__file__).with_name("config.toml")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("model", help="key under [mlx_models] in config.toml")
    parser.add_argument("--per-label", type=int, help="evaluate N random samples of every tile")
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--delete-model", action="store_true", help="remove the downloaded files afterwards")
    args = parser.parse_args()

    from mlx_vlm import generate, load
    from mlx_vlm.prompt_utils import apply_chat_template

    cfg = tomllib.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    spec = cfg["mlx_models"][args.model]
    samples = load_samples(cfg)
    if args.per_label:
        rng = random.Random(args.seed)
        by_label = collections.defaultdict(list)
        for sample in samples:
            by_label[sample.label].append(sample)
        samples = [s for group in by_label.values() for s in rng.sample(group, min(args.per_label, len(group)))]

    local_dir = pathlib.Path(cfg["models_dir"]) / f"mlx-{args.model}"
    snapshot_download(spec["repo"], revision=spec["revision"], local_dir=local_dir)
    size_gb = sum(p.stat().st_size for p in local_dir.rglob("*") if p.is_file()) / 1e9

    start = time.perf_counter()
    model, processor = load(str(local_dir))
    load_s = time.perf_counter() - start
    prompt = apply_chat_template(processor, model.config, cfg["prompt"]["text"], num_images=1)

    rows: list[dict[str, Any]] = []
    with tempfile.TemporaryDirectory() as tmp:
        for i, sample in enumerate(samples):
            image_path = pathlib.Path(tmp) / f"{i}.png"
            data_url = encode_image(sample.path, cfg["min_long_side"])
            image_path.write_bytes(base64.b64decode(data_url.split(",", 1)[1]))
            start = time.perf_counter()
            result = generate(
                model, processor, prompt, image=[str(image_path)], max_tokens=24, temperature=0.0, verbose=False
            )
            seconds = time.perf_counter() - start
            answer = result if isinstance(result, str) else result.text
            rows.append(
                {
                    "path": str(sample.path),
                    "split": sample.split,
                    "label": sample.label,
                    "pred": to_mjai(answer),
                    "answer": answer,
                    "seconds": seconds,
                    "prompt_tokens": getattr(result, "prompt_tokens", None),
                }
            )

    # ru_maxrss is in bytes on macOS.
    peak_rss_mb = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024 / 1024
    summary = summarize(
        f"mlx-{args.model}",
        rows,
        {
            "load_s": load_s,
            "peak_rss_mb": peak_rss_mb,
            "files_gb": size_gb,
            "repo": spec["repo"],
            "license": spec["license"],
            "min_long_side": cfg["min_long_side"],
            "server_args": ["mlx"],
            "answers": "free",
            "per_label": args.per_label,
            "seed": args.seed,
        },
    )
    out = pathlib.Path(cfg["results_dir"]) / f"mlx-{args.model}_pl{args.per_label}_free.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps({"summary": summary, "rows": rows}, ensure_ascii=False, indent=1), encoding="utf-8")
    print(json.dumps({k: v for k, v in summary.items() if k != "accuracy_by_label"}, ensure_ascii=False, indent=1))

    if args.delete_model:
        shutil.rmtree(local_dir)


if __name__ == "__main__":
    main()
