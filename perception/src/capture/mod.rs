//! Window capture (#1): frames of the game window, behind an interface that does not
//! depend on the platform. macOS uses ScreenCaptureKit (`macos.rs`); Windows
//! (Windows.Graphics.Capture) comes in P2.
//!
//! Only the chosen window is captured, so our own overlay never shows up in it, and the
//! system only sends a frame when the window changed: an unchanged window costs nothing.
//! Each frame reaches a handler on the capture thread as a borrowed BGRA view of the
//! window's content in pixels. Positions are relative to that content (0..1 on both
//! axes), so they stay right when the window is resized, moved to a Retina display or
//! made full screen.

mod latest;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod unsupported;

#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(target_os = "macos"))]
use unsupported as platform;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use image::{Rgb, RgbImage};

pub use latest::{LatestFrame, OwnedFrame};

/// Called on the capture thread for every changed frame.
type Handler = Box<dyn FnMut(&FrameView<'_>) + Send>;

/// A window the user can pick.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowInfo {
    pub id: u32,
    pub app: String,
    pub bundle_id: String,
    pub title: String,
    /// Size in points.
    pub width: f64,
    pub height: f64,
    pub on_screen: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureConfig {
    /// Most frames per second; fewer arrive while the window does not change.
    pub fps: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self { fps: 30 }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("window capture is not supported on this platform yet")]
    Unsupported,
    #[error("no window with id {0}; it may have closed")]
    NoWindow(u32),
    #[error("screen capture failed: {0} (is screen recording allowed in System Settings?)")]
    Platform(String),
}

/// Frame counters of a running capture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureStats {
    /// Frames handed to the handler.
    pub frames: u64,
    /// Notifications that nothing changed; no pixels, nothing to do.
    pub idle: u64,
    /// Times the capture size followed a change of the window size.
    pub resized: u64,
    /// The system stopped the capture, e.g. because the window closed.
    pub stopped: bool,
}

#[derive(Default)]
struct Counters {
    frames: AtomicU64,
    idle: AtomicU64,
    resized: AtomicU64,
    stopped: AtomicBool,
}

impl Counters {
    fn snapshot(&self) -> CaptureStats {
        CaptureStats {
            frames: self.frames.load(Ordering::Relaxed),
            idle: self.idle.load(Ordering::Relaxed),
            resized: self.resized.load(Ordering::Relaxed),
            stopped: self.stopped.load(Ordering::Relaxed),
        }
    }
}

/// A running capture; dropping it stops the capture.
pub struct CaptureSession {
    inner: platform::Session,
    counters: Arc<Counters>,
}

impl CaptureSession {
    pub fn stats(&self) -> CaptureStats {
        self.counters.snapshot()
    }

    pub fn stop(self) -> Result<(), CaptureError> {
        self.inner.stop()
    }
}

/// The windows on screen that can be captured, largest first.
pub fn list_windows() -> Result<Vec<WindowInfo>, CaptureError> {
    let mut windows = platform::list_windows()?;
    windows.sort_by(|a, b| (b.width * b.height).total_cmp(&(a.width * a.height)));
    Ok(windows)
}

/// Starts capturing one window. `handler` runs on the capture thread for every changed
/// frame; keep it short (e.g. publish to a [`LatestFrame`]) so frames are not dropped.
pub fn start(
    window_id: u32,
    config: CaptureConfig,
    handler: impl FnMut(&FrameView<'_>) + Send + 'static,
) -> Result<CaptureSession, CaptureError> {
    let counters = Arc::new(Counters::default());
    let inner = platform::start(window_id, config, Box::new(handler), Arc::clone(&counters))?;
    Ok(CaptureSession { inner, counters })
}

/// A rectangle relative to the window's content: 0..1 on both axes, from the top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RelRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl RelRect {
    pub const FULL: Self = Self {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };

    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// One frame of the window's content: BGRA pixels, `stride` bytes per row.
#[derive(Clone, Copy, Debug)]
pub struct FrameView<'a> {
    width: u32,
    height: u32,
    stride: usize,
    bgra: &'a [u8],
    scale: f64,
}

impl<'a> FrameView<'a> {
    /// `None` when `bgra` is too short for the size and stride.
    pub fn new(width: u32, height: u32, stride: usize, bgra: &'a [u8], scale: f64) -> Option<Self> {
        let row = width as usize * 4;
        let needed = match height {
            0 => 0,
            h => stride * (h as usize - 1) + row,
        };
        (stride >= row && bgra.len() >= needed).then_some(Self {
            width,
            height,
            stride,
            bgra,
            scale,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    pub fn bgra(&self) -> &'a [u8] {
        self.bgra
    }

    /// Pixels per point: 2 on a Retina display.
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// The part of the frame from (`x`, `y`), `width` x `height` pixels, without copying;
    /// clamped to the frame.
    pub fn sub(&self, x: u32, y: u32, width: u32, height: u32) -> FrameView<'a> {
        let x = x.min(self.width);
        let y = y.min(self.height);
        let width = width.min(self.width - x);
        let height = height.min(self.height - y);
        let start = y as usize * self.stride + x as usize * 4;
        FrameView {
            width,
            height,
            stride: self.stride,
            bgra: &self.bgra[start.min(self.bgra.len())..],
            scale: self.scale,
        }
    }

    /// A relative rectangle in pixels: `[left, top, right, bottom]`, clamped to the frame.
    pub fn pixel_rect(&self, rect: RelRect) -> [u32; 4] {
        let at = |v: f32, size: u32| ((v * size as f32).round().max(0.0) as u32).min(size);
        let left = at(rect.x, self.width);
        let top = at(rect.y, self.height);
        let right = at(rect.x + rect.width, self.width).max(left);
        let bottom = at(rect.y + rect.height, self.height).max(top);
        [left, top, right, bottom]
    }

    /// RGB of the pixel at (`x`, `y`), which must be inside the frame.
    pub fn rgb(&self, x: u32, y: u32) -> [u8; 3] {
        let i = y as usize * self.stride + x as usize * 4;
        [self.bgra[i + 2], self.bgra[i + 1], self.bgra[i]]
    }

    /// Brightness of the pixel at (`x`, `y`), 0-255.
    pub fn luma(&self, x: u32, y: u32) -> u8 {
        let [r, g, b] = self.rgb(x, y);
        ((77 * u32::from(r) + 150 * u32::from(g) + 29 * u32::from(b)) >> 8) as u8
    }

    /// The whole view as an RGB image.
    pub fn to_rgb(&self) -> RgbImage {
        RgbImage::from_fn(self.width, self.height, |x, y| Rgb(self.rgb(x, y)))
    }

    /// The pixels of a relative rectangle as an RGB image, e.g. one tile for recognition.
    pub fn crop_rgb(&self, rect: RelRect) -> RgbImage {
        let [left, top, right, bottom] = self.pixel_rect(rect);
        RgbImage::from_fn(right - left, bottom - top, |x, y| {
            Rgb(self.rgb(left + x, top + y))
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tightly packed BGRA frame where `colour(x, y)` gives RGB.
    pub(crate) fn bgra(width: u32, height: u32, colour: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = colour(x, y);
                data.extend_from_slice(&[b, g, r, 255]);
            }
        }
        data
    }

    #[test]
    fn rejects_buffers_too_short_for_their_size() {
        let data = bgra(4, 3, |_, _| [0, 0, 0]);
        assert!(FrameView::new(4, 3, 16, &data, 1.0).is_some());
        assert!(FrameView::new(4, 4, 16, &data, 1.0).is_none());
        assert!(
            FrameView::new(5, 3, 16, &data, 1.0).is_none(),
            "stride shorter than a row"
        );
    }

    #[test]
    fn reads_pixels_in_rgb_order() {
        let data = bgra(4, 3, |x, y| [x as u8 * 10, y as u8 * 20, 7]);
        let frame = FrameView::new(4, 3, 16, &data, 2.0).unwrap();
        assert_eq!(frame.rgb(3, 2), [30, 40, 7]);
        assert_eq!(frame.scale(), 2.0);
        let sub = frame.sub(1, 1, 10, 10);
        assert_eq!((sub.width(), sub.height()), (3, 2), "clamped to the frame");
        assert_eq!(sub.rgb(0, 0), [10, 20, 7]);
    }

    #[test]
    fn relative_rectangles_follow_the_frame_size() {
        let small = bgra(100, 50, |_, _| [0, 0, 0]);
        let big = bgra(200, 100, |_, _| [0, 0, 0]);
        let rect = RelRect::new(0.25, 0.5, 0.5, 0.25);
        let small = FrameView::new(100, 50, 400, &small, 1.0).unwrap();
        let big = FrameView::new(200, 100, 800, &big, 2.0).unwrap();
        assert_eq!(small.pixel_rect(rect), [25, 25, 75, 38]);
        assert_eq!(big.pixel_rect(rect), [50, 50, 150, 75]);
        assert_eq!(
            small.pixel_rect(RelRect::new(0.9, 0.9, 0.5, 0.5)),
            [90, 45, 100, 50]
        );
    }

    #[test]
    fn crops_a_relative_rectangle_to_rgb() {
        let data = bgra(10, 10, |x, y| [x as u8, y as u8, 0]);
        let frame = FrameView::new(10, 10, 40, &data, 1.0).unwrap();
        let tile = frame.crop_rgb(RelRect::new(0.2, 0.3, 0.3, 0.2));
        assert_eq!(tile.dimensions(), (3, 2));
        assert_eq!(tile.get_pixel(0, 0).0, [2, 3, 0]);
        assert_eq!(tile.get_pixel(2, 1).0, [4, 4, 0]);
    }
}
