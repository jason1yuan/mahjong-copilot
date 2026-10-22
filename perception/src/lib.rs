//! Window capture and tile recognition. Platform-specific code goes in per-OS modules
//! behind `#[cfg(target_os = ...)]`; capture lands in #1.

pub mod classifier;
mod pillow;
pub mod red_five;

pub use classifier::{ClassifierError, Prediction, TileClassifier};
