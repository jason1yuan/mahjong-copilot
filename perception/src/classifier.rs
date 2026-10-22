//! Tile classifier: pjura/mahjong_soul_vision, a ViT-base its author fine-tuned on Mahjong
//! Soul tiles, exported to ONNX by `ml/convert/export_classifier.py` and run with ONNX
//! Runtime. It only knows the 34 tile kinds; [`TileClassifier::recognize`] tells red fives
//! apart with the colour rule in [`crate::red_five`].

use std::path::Path;

use image::RgbImage;
use mahjong_core::{ParseTileError, Tile};
use ort::environment::Environment;
use ort::session::Session;
use ort::session::builder::{AutoDevicePolicy, GraphOptimizationLevel, SessionBuilder};
use ort::value::TensorRef;

use crate::{pillow, red_five};

/// Side of the square model input.
pub const INPUT_SIZE: u32 = 224;
const PIXELS: usize = (INPUT_SIZE * INPUT_SIZE) as usize;
/// Floats per image: 3 channels of 224x224, channel-first.
pub const INPUT_LEN: usize = 3 * PIXELS;

/// Number of model outputs: the 34 tile kinds.
pub const KINDS: usize = 34;

/// Model outputs in order (its `id2label`), in mjai notation.
pub const LABELS: [&str; KINDS] = [
    "1s", "1m", "1p", "2s", "2m", "2p", "3s", "3m", "3p", "4s", "4m", "4p", "5s", "5m", "5p", "6s",
    "6m", "6p", "7s", "7m", "7p", "8s", "8m", "8p", "9s", "9m", "9p", "E", "F", "N", "C", "S", "P",
    "W",
];

#[derive(Debug, thiserror::Error)]
pub enum ClassifierError {
    #[error("ONNX Runtime: {0}")]
    Ort(#[from] ort::Error),
    #[error(transparent)]
    Label(#[from] ParseTileError),
    #[error("model output has shape {0:?}, expected [{1}, 34]")]
    OutputShape(Vec<i64>, usize),
}

impl From<ort::Error<SessionBuilder>> for ClassifierError {
    fn from(err: ort::Error<SessionBuilder>) -> Self {
        Self::Ort(err.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prediction {
    pub tile: Tile,
    /// Softmax probability of the predicted kind.
    pub confidence: f32,
}

pub struct TileClassifier {
    session: Session,
    labels: Vec<Tile>,
    /// Reused input buffer, so a batch does not allocate it again.
    input: Vec<f32>,
}

impl TileClassifier {
    /// Loads the ONNX model to run on the CPU. `environment` is the process-wide ONNX
    /// Runtime environment (`ort::init().build()`), shared with the decision model.
    ///
    /// The CPU is asked for explicitly: `ort` defaults to a "max efficiency" device policy,
    /// which on a Mac sent the model elsewhere and made it 2x slower, 20x slower to load
    /// and 9 GB big. CoreML was tried and left out too: it rejects the dynamic batch axis,
    /// and with a fixed batch it takes 70-110 s to compile for a 2x faster whole hand.
    pub fn load(environment: &Environment, model: &Path) -> Result<Self, ClassifierError> {
        let session = Session::builder(environment)?
            .with_auto_device(AutoDevicePolicy::PreferCPU)?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .commit_from_file(model)?;
        let labels = LABELS.iter().map(|l| l.parse()).collect::<Result<_, _>>()?;
        Ok(Self {
            session,
            labels,
            input: Vec::new(),
        })
    }

    /// Classifies tile crops in one batch. Red fives come out as plain fives.
    pub fn classify(&mut self, crops: &[RgbImage]) -> Result<Vec<Prediction>, ClassifierError> {
        let logits = self.logits(crops)?;
        Ok(self.predictions(&logits))
    }

    /// The most likely tile of each row of 34 logits, with its softmax probability.
    pub fn predictions(&self, logits: &[f32]) -> Vec<Prediction> {
        let (rows, _) = logits.as_chunks::<KINDS>();
        rows.iter().map(|row| best(&self.labels, row)).collect()
    }

    /// Like [`classify`](Self::classify), then tells red fives apart by stroke colour.
    pub fn recognize(&mut self, crops: &[RgbImage]) -> Result<Vec<Prediction>, ClassifierError> {
        let mut predictions = self.classify(crops)?;
        for (prediction, crop) in predictions.iter_mut().zip(crops) {
            prediction.tile = red_five::refine(prediction.tile, crop);
        }
        Ok(predictions)
    }

    /// Raw logits of tile crops: `crops.len()` rows of 34, in [`LABELS`] order.
    pub fn logits(&mut self, crops: &[RgbImage]) -> Result<Vec<f32>, ClassifierError> {
        self.input.resize(crops.len() * INPUT_LEN, 0.0);
        let (images, _) = self.input.as_chunks_mut::<INPUT_LEN>();
        for (crop, out) in crops.iter().zip(images) {
            preprocess(crop, out);
        }
        run(&mut self.session, &self.input, crops.len())
    }

    /// Raw logits of already preprocessed input: `batch` images of [`INPUT_LEN`] floats.
    pub fn logits_of(&mut self, input: &[f32], batch: usize) -> Result<Vec<f32>, ClassifierError> {
        run(&mut self.session, input, batch)
    }
}

/// The model's preprocessing: Pillow's bilinear resize to 224x224, written channel-first
/// as floats in [-1, 1].
pub fn preprocess(crop: &RgbImage, out: &mut [f32]) {
    let resized = pillow::resize_bilinear(crop, INPUT_SIZE, INPUT_SIZE);
    for (i, pixel) in resized.pixels().enumerate() {
        for (c, value) in pixel.0.iter().enumerate() {
            out[c * PIXELS + i] = f32::from(*value) / 127.5 - 1.0;
        }
    }
}

fn run(session: &mut Session, input: &[f32], batch: usize) -> Result<Vec<f32>, ClassifierError> {
    if batch == 0 {
        return Ok(Vec::new());
    }
    let tensor = TensorRef::from_array_view((
        [batch, 3, INPUT_SIZE as usize, INPUT_SIZE as usize],
        &input[..batch * INPUT_LEN],
    ))?;
    let outputs = session.run(ort::inputs!["pixel_values" => tensor])?;
    let (shape, logits) = outputs["logits"].try_extract_tensor::<f32>()?;
    if **shape != [batch as i64, KINDS as i64] {
        return Err(ClassifierError::OutputShape(shape.to_vec(), batch));
    }
    Ok(logits.to_vec())
}

/// The most likely tile and its softmax probability.
fn best(labels: &[Tile], logits: &[f32]) -> Prediction {
    let (index, top) =
        logits
            .iter()
            .copied()
            .enumerate()
            .fold((0, f32::NEG_INFINITY), |best, (i, v)| {
                if v > best.1 { (i, v) } else { best }
            });
    let total: f32 = logits.iter().map(|v| (v - top).exp()).sum();
    Prediction {
        tile: labels[index],
        confidence: 1.0 / total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_the_34_kinds_once_each() {
        let mut kinds: Vec<u8> = LABELS
            .iter()
            .map(|l| l.parse::<Tile>().unwrap().kind())
            .collect();
        kinds.sort_unstable();
        assert_eq!(kinds, (0..34).collect::<Vec<u8>>());
    }

    #[test]
    fn best_picks_the_top_logit_with_its_softmax_probability() {
        let labels: Vec<Tile> = LABELS.iter().map(|l| l.parse().unwrap()).collect();
        let mut logits = [0.0f32; 34];
        logits[13] = 2.0_f32.ln() + 1.0; // e^(top - 0) = 2e against 33 ones
        let prediction = best(&labels, &logits);
        assert_eq!(prediction.tile.to_string(), "5m");
        let want = 2.0 * 1.0_f32.exp() / (2.0 * 1.0_f32.exp() + 33.0);
        assert!((prediction.confidence - want).abs() < 1e-6);
    }

    #[test]
    fn preprocess_scales_pixels_to_minus_one_to_one_channel_first() {
        let crop = RgbImage::from_pixel(10, 20, image::Rgb([0, 255, 51]));
        let mut out = vec![9.0; INPUT_LEN];
        preprocess(&crop, &mut out);
        assert!(out[..PIXELS].iter().all(|&v| v == -1.0));
        assert!(out[PIXELS..2 * PIXELS].iter().all(|&v| v == 1.0));
        assert!(out[2 * PIXELS..].iter().all(|&v| (v + 0.6).abs() < 1e-6));
    }
}
