"""Rescore saved results with the current labels and metrics, without running any model (#24).

Every results file keeps the prediction for each tile, so a label fix or a new
metric only needs the summaries recomputed. Rows from the dataset get their
label again from `load_samples`' rules (red fives from `red_fives` in
config.toml), and `summarize` recomputes the summary. Fields only the evaluation
script knew (load time, memory, cost, ...) are kept.

Run from the repository root:
    ml/.venv/bin/python -I ml/vlm_eval/rescore.py
    ml/.venv/bin/python -I ml/vlm_eval/rescore.py ml/experiments/2026-10-21-tile-recognition/results
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
import tomllib

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from eval_llamacpp import dataset_label, red_fives, summarize  # noqa: E402

CONFIG_PATH = pathlib.Path(__file__).with_name("config.toml")
# Summary fields that summarize() now computes itself (inside "calibration").
OBSOLETE = {"mean_confidence"}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("folder", nargs="?", type=pathlib.Path, help="folder of results files (default: results_dir)")
    args = parser.parse_args()

    cfg = tomllib.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    folder = args.folder or pathlib.Path(cfg["results_dir"])
    dataset_dir = pathlib.PurePath(cfg["dataset_dir"])
    red = red_fives(cfg)

    for path in sorted(folder.glob("*.json")):
        data = json.loads(path.read_text(encoding="utf-8"))
        rows, old = data["rows"], data["summary"]
        relabeled = 0
        for row in rows:
            source = pathlib.PurePath(row["path"])
            if dataset_dir in source.parents:
                label = dataset_label(source, red)
                relabeled += label != row["label"]
                row["label"] = label
        computed = summarize(old["model"], rows, {})
        extra = {k: v for k, v in old.items() if k not in computed and k not in OBSOLETE}
        data["summary"] = summarize(old["model"], rows, extra)
        path.write_text(json.dumps(data, ensure_ascii=False, indent=1), encoding="utf-8")
        print(f"{path.stem}: {relabeled} relabeled, accuracy {old['accuracy']:.1%} -> {data['summary']['accuracy']:.1%}")


if __name__ == "__main__":
    main()
