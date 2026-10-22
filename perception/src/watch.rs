//! Watches the game window and reads the hand each time it settles (#7): capture, change
//! detection, hand finding and recognition on one background thread, with what each step
//! cost. What to do with the hand is up to the caller (the decision core).
//!
//! Until the hand is found the whole window is watched; after that only the rows of the
//! hand (a little above, for raised tiles), so animations elsewhere on the table cost
//! nothing.
//!
//! [`FrameTap`] gives the dev build's screenshot button and recording switch the frames
//! (#8); nothing is saved unless they are used.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use image::RgbImage;
use ort::environment::Environment;

use crate::capture::{
    self, CaptureConfig, CaptureError, FrameView, LatestFrame, RelRect, WindowInfo,
};
use crate::change::{ChangeDetector, FrameGate, Region};
use crate::classifier::{ClassifierError, TileClassifier};
use crate::hand::{HandTracker, TileBox};
use crate::profile::Profile;
use crate::recognizer::{Recognition, TileRecognizer};
use crate::record::Recorder;

/// The whole window, watched until the hand is found.
const WHOLE: Region = Region {
    rect: RelRect::FULL,
    cols: 32,
    rows: 18,
};
/// How often the loop wakes up to check for a stop or a closed window when nothing moves.
const IDLE_WAKE: Duration = Duration::from_millis(250);

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error(transparent)]
    Classifier(#[from] ClassifierError),
    #[error("could not start the watcher thread: {0}")]
    Thread(#[from] std::io::Error),
}

/// What each step of one reading cost.
#[derive(Clone, Copy, Debug)]
pub struct Timings {
    /// When the hand last changed: the screen has been still since. Latency counts from
    /// here.
    pub still_since: Instant,
    /// When the change detector judged it settled.
    pub settled: Instant,
    pub find: Duration,
    pub recognize: Duration,
}

/// The hand read from one settled frame.
#[derive(Clone, Debug)]
pub struct HandReading {
    /// Concealed tiles left to right, then the drawn tile if [`drawn`](Self::drawn).
    pub tiles: Vec<Recognition>,
    pub boxes: Vec<TileBox>,
    pub drawn: bool,
    /// Frame size in pixels.
    pub frame: (u32, u32),
    pub timings: Timings,
}

#[derive(Clone, Debug)]
pub enum WatchEvent {
    /// The hand changed.
    Hand(HandReading),
    /// The window settled without a hand on screen (lobby, results screen, ...).
    NoHand,
    /// Capture ended on its own, e.g. the window closed.
    Stopped,
}

/// A running watcher; dropping it stops the thread and the capture.
pub struct Watcher {
    stop: Arc<AtomicBool>,
    tap: FrameTap,
    thread: Option<JoinHandle<()>>,
}

/// Debug access to a running watcher's frames, for the dev build's screenshot button and
/// recording switch (#8). Cheap to clone; not used on the advice path.
#[derive(Clone)]
pub struct FrameTap {
    latest: Arc<LatestFrame>,
    recorder: Arc<Mutex<Option<Arc<Recorder>>>>,
}

impl FrameTap {
    /// A copy of the newest frame; `None` before the first one.
    pub fn snapshot(&self) -> Option<RgbImage> {
        self.latest.latest().view().map(|view| view.to_rgb())
    }

    /// From now on saves each frame that changed the hand with `recorder`; `None` stops.
    pub fn record(&self, recorder: Option<Arc<Recorder>>) {
        *lock(&self.recorder) = recorder;
    }
}

// Holds only an `Option`; a panic elsewhere cannot leave it half-written.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Watcher {
    /// Starts watching `window`. The model is loaded and capture started before this
    /// returns, so those errors come back here; `on_event` then runs on the watcher thread.
    pub fn start(
        environment: &Environment,
        model: &Path,
        window: &WindowInfo,
        profile: Profile,
        on_event: impl FnMut(WatchEvent) + Send + 'static,
    ) -> Result<Self, WatchError> {
        let mut classifier = TileClassifier::load(environment, model)?;
        classifier.warm_up()?;
        let recognizer = TileRecognizer::new(classifier);
        let latest = Arc::new(LatestFrame::new());
        let session = {
            let latest = Arc::clone(&latest);
            let mut gate = FrameGate::new();
            capture::start(window.id, CaptureConfig::default(), move |frame| {
                if gate.changed(frame) {
                    latest.publish(frame);
                }
            })?
        };
        let stop = Arc::new(AtomicBool::new(false));
        let tap = FrameTap {
            latest: Arc::clone(&latest),
            recorder: Arc::new(Mutex::new(None)),
        };
        let thread = {
            let stop = Arc::clone(&stop);
            let recorder = Arc::clone(&tap.recorder);
            let mut state = State {
                recognizer,
                tracker: HandTracker::new(),
                profile,
                detector: ChangeDetector::new(&[WHOLE]),
                watched: WHOLE,
                frame: (0, 0),
                last: None,
            };
            let mut on_event = on_event;
            std::thread::Builder::new()
                .name("watcher".into())
                .spawn(move || {
                    let mut seen = 0;
                    while !stop.load(Ordering::Relaxed) {
                        if session.stats().stopped {
                            on_event(WatchEvent::Stopped);
                            break;
                        }
                        let wait = state
                            .detector
                            .next_check()
                            .map_or(IDLE_WAKE, |t| t.saturating_duration_since(Instant::now()))
                            .min(IDLE_WAKE);
                        if let Some(frame) = latest.wait_newer(seen, Some(wait)) {
                            seen = frame.sequence();
                            if let (Some(view), Some(at)) = (frame.view(), frame.captured()) {
                                state.observe(&view, at);
                            }
                        }
                        if state.detector.settled(Instant::now()).is_empty() {
                            continue;
                        }
                        let frame = latest.latest();
                        if let Some(view) = frame.view()
                            && let Some(event) = state.read(&view)
                        {
                            let recorder = lock(&recorder).clone();
                            let Some(recorder) = recorder else {
                                drop(frame);
                                on_event(event);
                                continue;
                            };
                            // Advice first, so recording never delays it; then copy the
                            // frame that was read before capture can replace it.
                            on_event(event);
                            let image = view.to_rgb();
                            drop(frame);
                            if !recorder.save("auto", image) {
                                tracing::warn!(
                                    "recording: earlier frames are still being written; skipped one"
                                );
                            }
                        }
                    }
                    if let Err(err) = session.stop() {
                        tracing::warn!("stopping capture: {err}");
                    }
                })?
        };
        Ok(Self {
            stop,
            tap,
            thread: Some(thread),
        })
    }

    pub fn tap(&self) -> FrameTap {
        self.tap.clone()
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::error!("the watcher thread panicked");
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct State {
    recognizer: TileRecognizer,
    tracker: HandTracker,
    profile: Profile,
    detector: ChangeDetector,
    watched: Region,
    frame: (u32, u32),
    /// The tiles last reported, to report only changes.
    last: Option<Vec<String>>,
}

impl State {
    fn observe(&mut self, view: &FrameView<'_>, captured: Instant) {
        let size = (view.width(), view.height());
        if size != self.frame {
            // A resized window: start over with the whole window and a fresh cache.
            self.frame = size;
            self.watch(WHOLE);
            self.recognizer.clear_cache();
        }
        self.detector.observe(view, captured);
    }

    fn watch(&mut self, region: Region) {
        if region != self.watched {
            self.watched = region;
            self.detector = ChangeDetector::new(&[region]);
        }
    }

    fn read(&mut self, view: &FrameView<'_>) -> Option<WatchEvent> {
        let settled = Instant::now();
        let still_since = self.detector.last_change(0).unwrap_or(settled);
        let start = Instant::now();
        let Some(layout) = self.tracker.find(view, self.profile.tile_face) else {
            self.watch(WHOLE);
            let changed = self.last.take().is_some();
            return changed.then_some(WatchEvent::NoHand);
        };
        let find = start.elapsed();
        let boxes = layout.tiles();
        let crops: Vec<_> = boxes.iter().map(|b| b.crop(view)).collect();
        let start = Instant::now();
        let tiles = match self.recognizer.recognize(&crops) {
            Ok(tiles) => tiles,
            Err(err) => {
                tracing::warn!("recognizing the hand: {err}");
                return None;
            }
        };
        let recognize = start.elapsed();

        // From now on watch the hand's rows, reaching up for raised tiles.
        let (y0, y1) = layout.band;
        let height = view.height().max(1) as f32;
        let band = (y1 - y0) as f32;
        self.watch(Region {
            rect: RelRect::new(
                0.0,
                (y0 as f32 - 0.3 * band).max(0.0) / height,
                1.0,
                1.3 * band / height,
            ),
            cols: 48,
            rows: 4,
        });

        let names: Vec<String> = tiles
            .iter()
            .map(|t| format!("{}{}", t.tile, if t.doubt.is_some() { "?" } else { "" }))
            .collect();
        if self.last.as_ref() == Some(&names) {
            return None;
        }
        self.last = Some(names);
        Some(WatchEvent::Hand(HandReading {
            tiles,
            boxes,
            drawn: layout.drawn.is_some(),
            frame: (view.width(), view.height()),
            timings: Timings {
                still_since,
                settled,
                find,
                recognize,
            },
        }))
    }
}
