//! Accuracy and latency of the Rust tile recognition (#4): Rust preprocessing, the ONNX
//! model and the red-five rule, from the original images.
//!
//! Every tile of `ml/results/classifier_reference.json` (written by
//! `ml/convert/export_classifier.py`) is recognized again and compared with its label and
//! with PyTorch's logits and red-five ink share. Tiles cut from screenshots are found
//! through the annotation folders given as arguments, matched by folder name. Images stay
//! local. Run from the repository root, in release mode:
//!     cargo run -p mahjong-perception --release --example eval_classifier -- recordings/source recordings/simulated_multi
//!     cargo run -p mahjong-perception --release --example eval_classifier -- --model ml/models/mahjong_soul_vision/model.int8.onnx recordings/source

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Instant;

use image::RgbImage;
use image::imageops;
use mahjong_core::Tile;
use mahjong_perception::TileClassifier;
use mahjong_perception::classifier::KINDS;
use mahjong_perception::red_five::{self, NON_RED_THRESHOLD};
use serde::Deserialize;
use serde_json::json;

const DEFAULT_MODEL: &str = "ml/models/mahjong_soul_vision/model.onnx";
const DEFAULT_REFERENCE: &str = "ml/results/classifier_reference.json";
const HAND: usize = 14;
const HAND_REPEATS: usize = 5;

#[derive(Deserialize)]
struct Reference {
    rows: Vec<Row>,
}

#[derive(Deserialize)]
struct Row {
    split: String,
    source: String,
    label: String,
    pred: String,
    logits: Vec<f32>,
    non_red_share: Option<f64>,
}

#[derive(Deserialize)]
struct Annotation {
    image: Option<PathBuf>,
    hand: Vec<HandTile>,
}

#[derive(Deserialize)]
struct HandTile {
    r#box: [u32; 4],
}

/// Cuts tiles out of dataset files and annotated screenshots, caching decoded images.
struct Crops {
    folders: HashMap<String, PathBuf>,
    annotations: HashMap<PathBuf, Annotation>,
    images: HashMap<PathBuf, RgbImage>,
}

impl Crops {
    fn image(&mut self, path: &Path) -> Result<&RgbImage, Box<dyn Error>> {
        if !self.images.contains_key(path) {
            let image = image::open(path)
                .map_err(|e| format!("{}: {e}", path.display()))?
                .to_rgb8();
            self.images.insert(path.to_owned(), image);
        }
        Ok(&self.images[path])
    }

    fn crop(&mut self, row: &Row) -> Result<RgbImage, Box<dyn Error>> {
        let file = Path::new(&row.source);
        if file.is_file() {
            return Ok(self.image(file)?.clone());
        }
        let (stem, index) = row
            .source
            .rsplit_once('#')
            .ok_or_else(|| format!("bad source {}", row.source))?;
        let folder = self
            .folders
            .get(&row.split)
            .ok_or_else(|| format!("no annotation folder named {}", row.split))?;
        let json = folder.join(format!("{stem}.json"));
        if !self.annotations.contains_key(&json) {
            let annotation: Annotation = serde_json::from_str(&std::fs::read_to_string(&json)?)?;
            self.annotations.insert(json.clone(), annotation);
        }
        let annotation = &self.annotations[&json];
        let [left, top, right, bottom] = annotation.hand[index.parse::<usize>()?].r#box;
        let path = annotation
            .image
            .clone()
            .unwrap_or_else(|| json.with_extension("png"));
        let image = self.image(&path)?;
        Ok(imageops::crop_imm(image, left, top, right - left, bottom - top).to_image())
    }
}

#[derive(Default)]
struct Split {
    n: usize,
    exact: usize,
    kind: usize,
    agrees_with_pytorch: usize,
    max_logit_diff: f32,
    red_labels: usize,
    red_right: usize,
    shares_compared: usize,
    max_share_diff: f64,
    share_decisions_differ: usize,
}

fn kind(name: &str) -> Result<u8, Box<dyn Error>> {
    Ok(name.parse::<Tile>()?.kind())
}

fn stats(seconds: &mut [f64]) -> serde_json::Value {
    seconds.sort_by(f64::total_cmp);
    let at = |q: f64| seconds[((seconds.len() - 1) as f64 * q).round() as usize] * 1000.0;
    json!({"n": seconds.len(), "median_ms": at(0.5), "p95_ms": at(0.95), "max_ms": at(1.0)})
}

fn peak_rss_mb() -> f64 {
    // SAFETY: getrusage only writes into the zeroed struct we pass.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    // macOS reports bytes.
    usage.ru_maxrss as f64 / 1024.0 / 1024.0
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut model = PathBuf::from(DEFAULT_MODEL);
    let mut reference = PathBuf::from(DEFAULT_REFERENCE);
    let mut folders = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--model" => model = args.next().ok_or("--model needs a path")?.into(),
            "--reference" => reference = args.next().ok_or("--reference needs a path")?.into(),
            _ => folders.push(PathBuf::from(arg)),
        }
    }
    if folders.is_empty() {
        folders.push("recordings/source".into());
    }
    let reference: Reference = serde_json::from_str(&std::fs::read_to_string(&reference)?)?;
    let mut crops = Crops {
        folders: folders
            .iter()
            .filter_map(|f| Some((f.file_name()?.to_string_lossy().into_owned(), f.clone())))
            .collect(),
        annotations: HashMap::new(),
        images: HashMap::new(),
    };
    let rows: Vec<&Row> = reference
        .rows
        .iter()
        .filter(|r| Path::new(&r.source).is_file() || crops.folders.contains_key(&r.split))
        .collect();

    let environment = ort::init().build()?;
    let start = Instant::now();
    let mut classifier = TileClassifier::load(&environment, &model)?;
    let load_ms = start.elapsed().as_secs_f64() * 1000.0;

    // Hands: the tiles of one screenshot, or 14 dataset tiles in a row.
    let mut hands: Vec<Vec<&Row>> = Vec::new();
    let mut last_key = None;
    for row in rows {
        let key = match row.source.rsplit_once('#') {
            Some((stem, _)) if !Path::new(&row.source).is_file() => {
                Some((row.split.clone(), stem.to_owned()))
            }
            _ => None,
        };
        let same_hand = key.is_some() && key == last_key;
        let dataset_room =
            key.is_none() && last_key.is_none() && hands.last().is_some_and(|h| h.len() < HAND);
        if same_hand || dataset_room {
            if let Some(hand) = hands.last_mut() {
                hand.push(row);
            }
        } else {
            hands.push(vec![row]);
        }
        last_key = key;
    }

    // Accuracy and agreement with PyTorch.
    let mut splits: BTreeMap<String, Split> = BTreeMap::new();
    let mut real_hands: Vec<Vec<RgbImage>> = Vec::new();
    for hand in &hands {
        let images = hand
            .iter()
            .map(|row| crops.crop(row))
            .collect::<Result<Vec<_>, _>>()?;
        let logits = classifier.logits(&images)?;
        let predictions = classifier.predictions(&logits);
        for (((row, image), prediction), row_logits) in hand
            .iter()
            .zip(&images)
            .zip(&predictions)
            .zip(logits.as_chunks::<KINDS>().0)
        {
            let tile = red_five::refine(prediction.tile, image);
            let s = splits.entry(row.split.clone()).or_default();
            s.n += 1;
            s.exact += usize::from(tile.to_string() == row.label);
            s.kind += usize::from(tile.kind() == kind(&row.label)?);
            s.agrees_with_pytorch += usize::from(prediction.tile.kind() == kind(&row.pred)?);
            let diff = row_logits
                .iter()
                .zip(&row.logits)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            s.max_logit_diff = s.max_logit_diff.max(diff);
            if row.label.ends_with('r') {
                s.red_labels += 1;
                s.red_right += usize::from(tile.to_string() == row.label);
            }
            if let Some(python) = row.non_red_share {
                let rust = red_five::non_red_share(image);
                s.shares_compared += 1;
                s.max_share_diff = s.max_share_diff.max((rust - python).abs());
                s.share_decisions_differ +=
                    usize::from((rust < NON_RED_THRESHOLD) != (python < NON_RED_THRESHOLD));
            }
        }
        if hand.first().is_some_and(|row| row.split == "source") {
            real_hands.push(images);
        }
    }

    // Latency on the real screenshots: single tiles, then whole hands.
    let mut single = Vec::new();
    let mut whole = Vec::new();
    if let Some(hand) = real_hands.first() {
        classifier.recognize(hand)?; // warm-up
    }
    for hand in &real_hands {
        for tile in hand {
            let start = Instant::now();
            classifier.recognize(std::slice::from_ref(tile))?;
            single.push(start.elapsed().as_secs_f64());
        }
        for _ in 0..HAND_REPEATS {
            let start = Instant::now();
            classifier.recognize(hand)?;
            whole.push(start.elapsed().as_secs_f64());
        }
    }

    let summary = json!({
        "model": model,
        "model_mb": std::fs::metadata(&model)?.len() as f64 / 1e6,
        "load_ms": load_ms,
        "peak_rss_mb": peak_rss_mb(),
        "single_tile": if single.is_empty() { json!(null) } else { stats(&mut single) },
        "whole_hand": if whole.is_empty() { json!(null) } else { stats(&mut whole) },
        "splits": splits.iter().map(|(name, s)| (name.clone(), json!({
            "n": s.n,
            "accuracy": s.exact as f64 / s.n as f64,
            "kind_accuracy": s.kind as f64 / s.n as f64,
            "agrees_with_pytorch": s.agrees_with_pytorch as f64 / s.n as f64,
            "max_logit_diff_vs_pytorch": s.max_logit_diff,
            "red_fives": format!("{}/{}", s.red_right, s.red_labels),
            "red_share_max_diff_vs_python": s.max_share_diff,
            "red_share_decisions_differ": format!("{}/{}", s.share_decisions_differ, s.shares_compared),
        }))).collect::<serde_json::Map<_, _>>(),
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
