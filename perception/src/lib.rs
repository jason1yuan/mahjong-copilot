//! Window capture and tile recognition. Platform-specific code goes in per-OS modules
//! behind `#[cfg(target_os = ...)]`.

pub mod cache;
pub mod capture;
pub mod change;
pub mod classifier;
pub mod hand;
mod pillow;
pub mod profile;
pub mod recognizer;
pub mod record;
pub mod red_five;
pub mod watch;

pub use classifier::{ClassifierError, Prediction, TileClassifier};
pub use recognizer::{Doubt, Recognition, Source, TileRecognizer};
pub use record::{RecordError, Recorder};
pub use watch::{FrameTap, HandReading, Timings, WatchError, WatchEvent, Watcher};
