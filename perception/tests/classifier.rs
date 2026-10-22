//! The ONNX classifier run from Rust must match the original PyTorch model.
//!
//! Needs the local model from `ml/convert/export_classifier.py` (never committed), so the
//! tests are ignored by default:
//!     cargo test -p mahjong-perception --test classifier -- --ignored

use std::path::PathBuf;

use mahjong_perception::TileClassifier;
use mahjong_perception::classifier::{INPUT_LEN, INPUT_SIZE, LABELS};

#[derive(serde::Deserialize)]
struct Fixture {
    labels: Vec<String>,
    cases: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    seed: u32,
    logits: Vec<f32>,
}

fn model() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ml/models/mahjong_soul_vision/model.onnx")
}

/// The synthetic input of `export_classifier.py`, built from integers so it is bit-exact.
fn synthetic_input(seed: u32) -> Vec<f32> {
    let side = INPUT_SIZE;
    let mut input = Vec::with_capacity(INPUT_LEN);
    for c in 0..3 {
        for y in 0..side {
            for x in 0..side {
                let level = (x * 7 + y * 13 + c * 29 + seed * 101) % 256;
                input.push(level as f32 / 127.5 - 1.0);
            }
        }
    }
    input
}

#[test]
#[ignore = "needs the local ONNX model"]
fn matches_pytorch_on_synthetic_inputs() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/classifier_synthetic.json")).unwrap();
    assert_eq!(
        fixture.labels, LABELS,
        "label order differs from the exported model"
    );
    let environment = ort::init().build().unwrap();
    let mut classifier = TileClassifier::load(&environment, &model()).unwrap();
    for case in fixture.cases {
        let got = classifier
            .logits_of(&synthetic_input(case.seed), 1)
            .unwrap();
        let diff = got
            .iter()
            .zip(&case.logits)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            diff < 1e-3,
            "seed {}: max logit difference {diff}",
            case.seed
        );
        let argmax = |v: &[f32]| (0..v.len()).max_by(|&a, &b| v[a].total_cmp(&v[b]));
        assert_eq!(argmax(&got), argmax(&case.logits), "seed {}", case.seed);
    }
}
