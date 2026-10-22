//! Calibrates and checks the template cache (#4) on annotated screenshots.
//!
//! A real screenshot is a cache group of its own; simulated screenshots are grouped by
//! source and window width. Two parts:
//!
//! 1. Distances between hand tiles of a group, split into same tile and different tile,
//!    also for tiles raised like a mouse hover: cut from the fixed slot while the tile is
//!    drawn 8-20 % of its height higher.
//! 2. Sessions: the hands of each group go through `TileRecognizer` with the real
//!    classifier in order (a real screenshot as it is, then once per tile with that tile
//!    raised), counting cache hits, wrong hits, doubts and time.
//!
//! Run from the repository root, in release mode (images stay local):
//!     cargo run -p mahjong-perception --release --example eval_cache -- recordings/source recordings/simulated_multi

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Instant;

use image::{RgbImage, imageops};
use mahjong_perception::cache::{MAX_DISTANCE, MIN_MARGIN, crop_distance};
use mahjong_perception::{Source, TileClassifier, TileRecognizer};
use serde::Deserialize;
use serde_json::json;

const DEFAULT_MODEL: &str = "ml/models/mahjong_soul_vision/model.onnx";
const LIFTS: [f64; 3] = [0.08, 0.14, 0.2];

#[derive(Deserialize)]
struct Annotation {
    image: Option<PathBuf>,
    source: Option<String>,
    #[serde(default)]
    ops: Vec<String>,
    hand: Vec<HandTile>,
}

#[derive(Deserialize)]
struct HandTile {
    tile: String,
    r#box: [u32; 4],
}

struct Hand {
    /// "real" or "simulated", for reporting.
    kind: &'static str,
    group: String,
    image: RgbImage,
    tiles: Vec<HandTile>,
}

impl Hand {
    /// The tile's slot, with the tile drawn `lift` pixels higher.
    fn crop(&self, index: usize, lift: u32) -> RgbImage {
        let [left, top, right, bottom] = self.tiles[index].r#box;
        let top = (top + lift).min(self.image.height().saturating_sub(bottom - top));
        imageops::crop_imm(
            &self.image,
            left,
            top,
            right - left,
            bottom - self.tiles[index].r#box[1],
        )
        .to_image()
    }

    fn lift(&self, index: usize, share: f64) -> u32 {
        let [_, top, _, bottom] = self.tiles[index].r#box;
        (f64::from(bottom - top) * share).round() as u32
    }

    fn crops(&self) -> Vec<RgbImage> {
        (0..self.tiles.len()).map(|i| self.crop(i, 0)).collect()
    }
}

fn load(folder: &Path) -> Result<Vec<Hand>, Box<dyn Error>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(folder)?
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    let mut hands = Vec::new();
    for json in paths {
        let Ok(annotation) = serde_json::from_str::<Annotation>(&std::fs::read_to_string(&json)?)
        else {
            continue; // not an annotation, e.g. the spec file
        };
        let stem = json
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let width = annotation
            .ops
            .iter()
            .find_map(|op| op.strip_prefix("width:"));
        let (kind, group) = match (&annotation.source, width) {
            (Some(source), Some(width)) => ("simulated", format!("{source}@{width}")),
            _ => ("real", stem),
        };
        let path = annotation
            .image
            .clone()
            .unwrap_or_else(|| json.with_extension("png"));
        let image = image::open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .to_rgb8();
        hands.push(Hand {
            kind,
            group,
            image,
            tiles: annotation.hand,
        });
    }
    Ok(hands)
}

fn percentiles(values: &mut [f32]) -> serde_json::Value {
    if values.is_empty() {
        return json!(null);
    }
    values.sort_by(f32::total_cmp);
    let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
    json!({"n": values.len(), "min": at(0.0), "p01": at(0.01), "p05": at(0.05), "p50": at(0.5), "p95": at(0.95), "p99": at(0.99), "max": at(1.0)})
}

fn millis(values: &mut [f64]) -> serde_json::Value {
    if values.is_empty() {
        return json!(null);
    }
    values.sort_by(f64::total_cmp);
    let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize] * 1000.0;
    json!({"n": values.len(), "median_ms": at(0.5), "p95_ms": at(0.95), "max_ms": at(1.0)})
}

#[derive(Default)]
struct Distances {
    same: Vec<f32>,
    different: Vec<f32>,
    raised_same: Vec<f32>,
    raised_different: Vec<f32>,
}

#[derive(Default)]
struct Session {
    frames: usize,
    tiles: usize,
    hits: usize,
    wrong_hits: Vec<String>,
    near_misses: Vec<String>,
    disagreements: Vec<String>,
    classified: usize,
    in_doubt: usize,
    sure_right: usize,
    sure: usize,
    all_hit_frames: Vec<f64>,
    other_frames: Vec<f64>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut model = PathBuf::from(DEFAULT_MODEL);
    let mut folders = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--model" => model = args.next().ok_or("--model needs a path")?.into(),
            _ => folders.push(PathBuf::from(arg)),
        }
    }
    if folders.is_empty() {
        folders.push("recordings/source".into());
    }
    let mut groups: BTreeMap<String, Vec<Hand>> = BTreeMap::new();
    for folder in &folders {
        for hand in load(folder)? {
            groups.entry(hand.group.clone()).or_default().push(hand);
        }
    }

    // 1. Distances within each group.
    let mut distances: BTreeMap<&str, Distances> = BTreeMap::new();
    for hands in groups.values() {
        let kind = hands[0].kind;
        let d = distances.entry(kind).or_default();
        let tiles: Vec<(&str, RgbImage)> = hands
            .iter()
            .flat_map(|h| h.tiles.iter().map(|t| t.tile.as_str()).zip(h.crops()))
            .collect();
        for (i, (a, crop_a)) in tiles.iter().enumerate() {
            for (b, crop_b) in &tiles[i + 1..] {
                let distance = crop_distance(crop_a, crop_b);
                if a == b {
                    d.same.push(distance)
                } else {
                    d.different.push(distance)
                }
            }
        }
        if kind == "real" {
            for hand in hands {
                for i in 0..hand.tiles.len() {
                    for share in LIFTS {
                        let raised = hand.crop(i, hand.lift(i, share));
                        for (j, flat) in hand.crops().iter().enumerate() {
                            let distance = crop_distance(&raised, flat);
                            if hand.tiles[i].tile == hand.tiles[j].tile {
                                d.raised_same.push(distance);
                            } else {
                                d.raised_different.push(distance);
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Sessions through the recognizer.
    let environment = ort::init().build()?;
    let mut recognizer = TileRecognizer::new(TileClassifier::load(&environment, &model)?);
    // Answers what the classifier would have said for every cache hit.
    let mut checker = TileClassifier::load(&environment, &model)?;
    let mut sessions: BTreeMap<&str, Session> = BTreeMap::new();
    for (name, hands) in &groups {
        recognizer.clear_cache();
        let kind = hands[0].kind;
        let mut frames: Vec<(Vec<RgbImage>, Vec<&str>)> = Vec::new();
        for hand in hands {
            let labels: Vec<&str> = hand.tiles.iter().map(|t| t.tile.as_str()).collect();
            frames.push((hand.crops(), labels.clone()));
            if kind == "real" {
                for i in 0..hand.tiles.len() {
                    let mut crops = hand.crops();
                    crops[i] = hand.crop(i, hand.lift(i, LIFTS[1]));
                    frames.push((crops, labels.clone()));
                }
            }
        }
        let s = sessions.entry(kind).or_default();
        for (crops, labels) in &frames {
            let before: Vec<_> = crops
                .iter()
                .map(|c| recognizer.cache().closest_to(c))
                .collect();
            let start = Instant::now();
            let results = recognizer.recognize(crops)?;
            let seconds = start.elapsed().as_secs_f64();
            s.frames += 1;
            let mut all_hits = true;
            for ((result, label), closest) in results.iter().zip(labels).zip(&before) {
                s.tiles += 1;
                let right = result.tile.to_string() == *label;
                match result.source {
                    Source::Cache => {
                        s.hits += 1;
                        if !right {
                            let m = closest.ok_or("a hit without a template")?;
                            s.wrong_hits.push(format!(
                                "{name}: {label} read as {} (distance {:.1}, margin {:.1})",
                                result.tile, m.distance, m.margin
                            ));
                        }
                    }
                    Source::Classifier => {
                        s.classified += 1;
                        all_hits = false;
                    }
                }
                if let (Source::Classifier, Some(m)) = (result.source, closest) {
                    // A near miss: the closest template is the wrong tile but close.
                    if m.tile.to_string() != *label && m.distance <= MAX_DISTANCE + 4.0 {
                        s.near_misses.push(format!(
                            "{name}: {label} closest to {} (distance {:.1}, margin {:.1})",
                            m.tile, m.distance, m.margin
                        ));
                    }
                }
                if result.doubt.is_some() {
                    s.in_doubt += 1;
                } else {
                    s.sure += 1;
                    s.sure_right += usize::from(right);
                }
            }
            if all_hits {
                s.all_hit_frames.push(seconds)
            } else {
                s.other_frames.push(seconds)
            }
            let hits: Vec<usize> = (0..results.len())
                .filter(|&i| results[i].source == Source::Cache)
                .collect();
            let hit_crops: Vec<RgbImage> = hits.iter().map(|&i| crops[i].clone()).collect();
            for (&i, check) in hits.iter().zip(checker.recognize(&hit_crops)?) {
                if check.tile != results[i].tile {
                    s.disagreements.push(format!(
                        "{name}: cache {} vs classifier {} (label {})",
                        results[i].tile, check.tile, labels[i]
                    ));
                }
            }
        }
    }

    let summary = json!({
        "thresholds": {"max_distance": MAX_DISTANCE, "min_margin": MIN_MARGIN},
        "groups": groups.len(),
        "distances": distances.iter_mut().map(|(kind, d)| (kind.to_string(), json!({
            "same_tile": percentiles(&mut d.same),
            "different_tile": percentiles(&mut d.different),
            "raised_vs_same_tile": percentiles(&mut d.raised_same),
            "raised_vs_different_tile": percentiles(&mut d.raised_different),
        }))).collect::<serde_json::Map<_, _>>(),
        "sessions": sessions.iter_mut().map(|(kind, s)| (kind.to_string(), json!({
            "frames": s.frames,
            "tiles": s.tiles,
            "cache_hits": s.hits,
            "wrong_hits": s.wrong_hits,
            "near_misses": s.near_misses,
            "hits_unlike_classifier": s.disagreements,
            "classified": s.classified,
            "in_doubt": s.in_doubt,
            "sure_accuracy": s.sure_right as f64 / s.sure.max(1) as f64,
            "all_hit_frame": millis(&mut s.all_hit_frames),
            "other_frame": millis(&mut s.other_frames),
        }))).collect::<serde_json::Map<_, _>>(),
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
