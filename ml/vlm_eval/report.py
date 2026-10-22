"""Print a Markdown table of every results file in the results directory (#24).

Run from the repository root:
    ml/.venv/bin/python -I ml/vlm_eval/report.py
"""

from __future__ import annotations

import json
import pathlib
import tomllib

CONFIG_PATH = pathlib.Path(__file__).with_name("config.toml")


def main() -> None:
    cfg = tomllib.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    print("| 结果 | 样本 | 准确率 | 单张耗时中位数 | P95 | 首次请求 | 加载 | 峰值内存 | 文件大小 | 额外参数 |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    for path in sorted(pathlib.Path(cfg["results_dir"]).glob("*.json")):
        s = json.loads(path.read_text(encoding="utf-8"))["summary"]
        lat = s["latency_s"]
        print(
            f"| {path.stem} | {s['n']} | {s['accuracy']:.1%} | {lat['median'] * 1000:.0f} ms"
            f" | {lat['p95'] * 1000:.0f} ms | {s['first_request_s'] * 1000:.0f} ms | {s['load_s']:.1f} s"
            f" | {s['peak_rss_mb'] / 1024:.1f} GB | {s['files_gb']:.2f} GB"
            f" | {' '.join(s.get('server_args') or []) or '—'} |"
        )


if __name__ == "__main__":
    main()
