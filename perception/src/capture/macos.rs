//! Window capture with ScreenCaptureKit (macOS 14+).
//!
//! Each frame's info says where the window's content is in the buffer and how much it was
//! scaled to fit: the window's real size in pixels is `content_rect / content_scale *
//! scale_factor`. When the window grew past the capture size (content shrunk to fit) or
//! became much smaller (pixels wasted), the capture size is changed to match, from a
//! helper thread so the capture thread never waits. Until then the content is cropped
//! out of the buffer as it is.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use screencapturekit::SCFrameStatus;
use screencapturekit::cv::CVPixelBufferLockFlags;
use screencapturekit::prelude::*;

use super::{CaptureConfig, CaptureError, Counters, FrameView, Handler, WindowInfo};

/// Our own windows are never offered for capture.
const OWN_BUNDLE_ID: &str = "io.github.jason1yuan.mahjongcopilot";
/// Smallest window worth offering, in points.
const MIN_WINDOW: f64 = 100.0;
/// Largest capture side in pixels.
const MAX_SIDE: u32 = 8192;
/// `BGRA` as a Core Video pixel format code.
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");

fn platform(err: impl std::fmt::Display) -> CaptureError {
    CaptureError::Platform(err.to_string())
}

/// ScreenCaptureKit needs CoreGraphics set up. A GUI app has it already; a command-line
/// tool (tests, examples) does not.
fn init_core_graphics() {
    // SAFETY: the bridge function only calls CGMainDisplayID(), which has no preconditions.
    unsafe { screencapturekit::ffi::sc_initialize_core_graphics() }
}

pub(super) fn list_windows() -> Result<Vec<WindowInfo>, CaptureError> {
    init_core_graphics();
    let content = SCShareableContent::get().map_err(platform)?;
    let snapshot = content
        .snapshot()
        .ok_or_else(|| platform("no shareable content"))?;
    Ok(snapshot
        .windows
        .iter()
        .filter_map(|w| {
            let app = snapshot.applications.get(w.owning_app_index?)?;
            let normal = w.window_layer == 0
                && w.frame.size.width >= MIN_WINDOW
                && w.frame.size.height >= MIN_WINDOW
                && app.bundle_identifier != OWN_BUNDLE_ID;
            normal.then(|| WindowInfo {
                id: w.window_id,
                app: app.application_name.clone(),
                bundle_id: app.bundle_identifier.clone(),
                title: w.title.clone().unwrap_or_default(),
                width: w.frame.size.width,
                height: w.frame.size.height,
                on_screen: w.is_on_screen,
            })
        })
        .collect())
}

/// Messages to the resize thread. The capture callback keeps a sender for as long as the
/// stream lives, so the channel never closes on its own; `Stop` ends the thread.
enum Request {
    Resize(u32, u32),
    Stop,
}

fn pack(width: u32, height: u32) -> u64 {
    u64::from(width) << 32 | u64::from(height)
}

fn unpack(size: u64) -> (u32, u32) {
    ((size >> 32) as u32, size as u32)
}

fn configuration(width: u32, height: u32, fps: u32) -> Result<SCStreamConfiguration, CaptureError> {
    SCStreamConfiguration::new()
        .with_width(width.clamp(1, MAX_SIDE))
        .with_height(height.clamp(1, MAX_SIDE))
        .with_pixel_format(PixelFormat::BGRA)
        .with_shows_cursor(false)
        .with_fps(fps.max(1))
        .with_queue_depth(3)
        .with_ignores_shadows_single_window(true)
        .map_err(platform)
}

pub(super) struct Session {
    stream: SCStream,
    requests: SyncSender<Request>,
    resizer: Option<JoinHandle<()>>,
    stopped: bool,
}

impl Session {
    pub(super) fn stop(mut self) -> Result<(), CaptureError> {
        self.stopped = true;
        let result = self.stream.stop_capture().map_err(platform);
        self.join_resizer();
        result
    }

    fn join_resizer(&mut self) {
        if let Some(resizer) = self.resizer.take() {
            // The thread drains the queue, so a blocking send returns promptly.
            let _ = self.requests.send(Request::Stop);
            let _ = resizer.join();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if !self.stopped {
            let _ = self.stream.stop_capture();
        }
        self.join_resizer();
    }
}

pub(super) fn start(
    window_id: u32,
    config: CaptureConfig,
    handler: Handler,
    counters: Arc<Counters>,
) -> Result<Session, CaptureError> {
    init_core_graphics();
    let content = SCShareableContent::get().map_err(platform)?;
    let window = content
        .windows()
        .into_iter()
        .find(|w| w.window_id() == window_id)
        .ok_or(CaptureError::NoWindow(window_id))?;
    let filter = SCContentFilter::create()
        .with_window(&window)
        .build()
        .map_err(platform)?;
    let scale = f64::from(filter.point_pixel_scale()).max(1.0);
    let rect = filter.content_rect();
    let size = (
        (rect.size.width * scale).round() as u32,
        (rect.size.height * scale).round() as u32,
    );
    let configured = Arc::new(AtomicU64::new(pack(size.0, size.1)));
    let mut stream = SCStream::new_with_delegate(
        &filter,
        &configuration(size.0, size.1, config.fps)?,
        Delegate {
            counters: Arc::clone(&counters),
        },
    )
    .map_err(platform)?;

    let (requests, wanted) = mpsc::sync_channel(1);
    let resizer = {
        let stream = stream.clone();
        let configured = Arc::clone(&configured);
        let counters = Arc::clone(&counters);
        std::thread::Builder::new()
            .name("capture-resize".into())
            .spawn(move || resize(&stream, &wanted, &configured, &counters, config.fps))
            .map_err(platform)?
    };
    stream
        .add_output_handler(
            Output {
                handler: Mutex::new(handler),
                counters,
                configured,
                requested: AtomicU64::new(pack(size.0, size.1)),
                requests: requests.clone(),
            },
            SCStreamOutputType::Screen,
        )
        .map_err(platform)?;
    stream.start_capture().map_err(platform)?;
    tracing::info!(
        window_id,
        width = size.0,
        height = size.1,
        "window capture started"
    );
    Ok(Session {
        stream,
        requests,
        resizer: Some(resizer),
        stopped: false,
    })
}

/// Applies capture size requests until the session ends.
fn resize(
    stream: &SCStream,
    wanted: &Receiver<Request>,
    configured: &AtomicU64,
    counters: &Counters,
    fps: u32,
) {
    while let Ok(Request::Resize(width, height)) = wanted.recv() {
        let applied = configuration(width, height, fps)
            .and_then(|c| stream.update_configuration(&c).map_err(platform));
        match applied {
            Ok(()) => {
                configured.store(pack(width, height), Ordering::Relaxed);
                counters.resized.fetch_add(1, Ordering::Relaxed);
                tracing::info!(width, height, "capture size follows the window");
            }
            Err(err) => tracing::warn!("could not change the capture size: {err}"),
        }
    }
}

struct Output {
    handler: Mutex<Handler>,
    counters: Arc<Counters>,
    configured: Arc<AtomicU64>,
    /// The last size asked for, so one resize is not requested on every frame.
    requested: AtomicU64,
    requests: SyncSender<Request>,
}

impl Output {
    fn follow_window_size(&self, rect: CGRect, scale: f64, content_scale: f64) {
        let want = (
            ((rect.size.width / content_scale) * scale).round() as u32,
            ((rect.size.height / content_scale) * scale).round() as u32,
        );
        let (width, height) = unpack(self.configured.load(Ordering::Relaxed));
        let shrunk = content_scale < 0.999;
        let wasteful = want.0 * 10 < width * 9 || want.1 * 10 < height * 9;
        let packed = pack(want.0, want.1);
        if (shrunk || wasteful) && self.requested.swap(packed, Ordering::Relaxed) != packed {
            // A full queue means a resize is already on its way; the next frame asks again.
            if self
                .requests
                .try_send(Request::Resize(want.0, want.1))
                .is_err()
            {
                self.requested.store(0, Ordering::Relaxed);
            }
        }
    }
}

impl SCStreamOutputTrait for Output {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, kind: SCStreamOutputType) {
        if !matches!(kind, SCStreamOutputType::Screen) {
            return;
        }
        let Some(info) = sample.frame_info() else {
            return;
        };
        if info.frame_status != Some(SCFrameStatus::Complete) {
            self.counters.idle.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let (Some(rect), Some(scale)) = (info.content_rect, info.scale_factor) else {
            return;
        };
        let content_scale = info.content_scale.filter(|s| *s > 0.0).unwrap_or(1.0);
        self.follow_window_size(rect, scale, content_scale);

        let Some(buffer) = sample.pixel_buffer() else {
            return;
        };
        let Ok(guard) = buffer.lock(CVPixelBufferLockFlags::READ_ONLY) else {
            return;
        };
        if guard.pixel_format() != BGRA {
            return;
        }
        // SAFETY: the buffer stays locked read-only until `guard` drops at the end of this
        // call, and ScreenCaptureKit does not reuse a buffer while its sample is delivered.
        let Some(bytes) = (unsafe { guard.as_slice() }) else {
            return;
        };
        let Some(full) = FrameView::new(
            guard.width() as u32,
            guard.height() as u32,
            guard.bytes_per_row(),
            bytes,
            scale * content_scale,
        ) else {
            return;
        };
        let px = |v: f64| (v * scale).round().max(0.0) as u32;
        let view = full.sub(
            px(rect.origin.x),
            px(rect.origin.y),
            px(rect.size.width),
            px(rect.size.height),
        );
        self.counters.frames.fetch_add(1, Ordering::Relaxed);
        let mut handler = self.handler.lock().unwrap_or_else(PoisonError::into_inner);
        handler(&view);
    }
}

struct Delegate {
    counters: Arc<Counters>,
}

impl SCStreamDelegateTrait for Delegate {
    fn did_stop_with_error(&self, error: SCError) {
        self.counters.stopped.store(true, Ordering::Relaxed);
        tracing::warn!("window capture stopped: {error}");
    }
}
