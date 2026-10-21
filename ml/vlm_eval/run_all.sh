#!/bin/sh
# Runs the full llama.cpp evaluation for every candidate (#24), one model at a
# time, deleting each model afterwards to save disk space. Run from the repo root;
# pass model keys to run only those.
set -eu

PY=ml/.venv/bin/python
EVAL=ml/vlm_eval/eval_llamacpp.py
mkdir -p ml/results

if [ "$#" -eq 0 ]; then
    set -- qwen3-vl-2b qwen3-vl-4b smolvlm2-2.2b internvl3-2b
fi

for model in "$@"; do
    echo "== $model: all tiles, grammar"
    $PY -I $EVAL "$model" --tag=_full > "ml/results/run_${model}_full.log" 2>&1
    echo "== $model: 5 per tile, free answers"
    $PY -I $EVAL "$model" --per-label 5 --seed 7 --free --tag=_pl5_free > "ml/results/run_${model}_free.log" 2>&1
    case "$model" in
        qwen3-vl-*)
            echo "== $model: 5 per tile, at least 1024 image tokens"
            $PY -I $EVAL "$model" --per-label 5 --seed 7 \
                --server-arg=--image-min-tokens --server-arg=1024 \
                --tag=_pl5_min1024 > "ml/results/run_${model}_min1024.log" 2>&1
            ;;
    esac
    rm -rf "ml/models/$model"
done
echo "== done"
