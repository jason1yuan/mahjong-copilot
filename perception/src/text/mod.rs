//! Reading text off the screen (#9) with the system's on-device text recognition, behind an
//! interface that does not depend on the platform. macOS uses Vision (`macos.rs`); Windows
//! (Windows.Media.Ocr) comes in P2. Both run locally: nothing is sent anywhere and there is
//! no model to download.
//!
//! Only small crops of fixed labels are read, and only when they change (see
//! `round::RoundReader`), so this is not on the per-frame path.

#[cfg(target_os = "macos")]
mod macos;

use image::RgbImage;

#[derive(Debug, thiserror::Error)]
pub enum TextError {
    #[error("could not encode the image: {0}")]
    Encode(#[from] image::ImageError),
    #[error("text recognition failed: {0}")]
    Recognize(String),
}

/// Reads the lines of text in a picture.
pub trait ReadText: Send {
    /// The lines found, each as its most likely text, top to bottom as the system lists
    /// them; empty when there is none.
    fn read_text(&mut self, image: &RgbImage) -> Result<Vec<String>, TextError>;
}

/// The system's text recognition, set up for Chinese and Japanese text; `None` on a
/// platform without one yet.
pub fn system() -> Option<Box<dyn ReadText>> {
    #[cfg(target_os = "macos")]
    {
        Some(Box::new(macos::VisionText))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Runs the system's text recognition once on a blank picture, so the first real reading
/// does not pay for loading it (about 150 ms; much longer the first time after an OS
/// update). Meant for a background thread at start-up.
pub fn warm_up() {
    if let Some(mut reader) = system()
        && let Err(err) = reader.read_text(&RgbImage::new(64, 32))
    {
        tracing::warn!("warming up text recognition: {err}");
    }
}
