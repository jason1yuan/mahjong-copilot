//! Watches the game window and reads the hand each time it settles (#7): capture, change
//! detection, hand finding and recognition on one background thread, with what each step
//! cost. What to do with the hand is up to the caller (the decision core).
//!
//! Until the hand is found the whole window is watched; after that the rows of the hand (a
//! little above, for raised tiles) and the table above them. When either settles, the hand
//! and the table (rivers, melds, dora indicators; #8) are read together. A table that does
//! not add up with the hand (five of a kind, two red fives of a suit: a misread tile, a
//! results screen, a cut-in) is not trusted: the last one that did is kept.
//!
//! [`FrameTap`] gives the dev build's screenshot button and recording switch the frames
//! (#8); nothing is saved unless they are used.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use image::RgbImage;
use mahjong_core::Tile;
use ort::environment::Environment;

use crate::capture::{
    self, CaptureConfig, CaptureError, FrameView, LatestFrame, RelRect, WindowInfo,
};
use crate::change::{ChangeDetector, FrameGate, Region};
use crate::classifier::{ClassifierError, TileClassifier};
use crate::hand::{HandTracker, TileBox};
use crate::numbers::NumberReader;
use crate::profile::Profile;
use crate::recognizer::{Doubt, Recognition, TileRecognizer, Zone};
use crate::record::Recorder;
use crate::round::RoundReader;
use crate::snapshot::{self, Snapshot};
use crate::table::Seat;
use crate::text::{self, ReadText};

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
    /// Reading the rivers, melds, dora indicators and round.
    pub table: Duration,
}

/// The hand read from one settled frame.
#[derive(Clone, Debug)]
pub struct HandReading {
    /// Concealed tiles left to right, then the drawn tile if [`drawn`](Self::drawn).
    pub tiles: Vec<Recognition>,
    pub boxes: Vec<TileBox>,
    pub drawn: bool,
    /// The rivers, melds and dora indicators; `None` when the profile does not describe the
    /// table.
    pub table: Option<Snapshot>,
    /// Frame size in pixels.
    pub frame: (u32, u32),
    /// Text recognition fails even in a fresh helper, so the round, scores and other
    /// labels cannot be read (#49).
    pub text_failing: bool,
    pub timings: Timings,
}

#[derive(Clone, Debug)]
pub enum WatchEvent {
    /// The hand changed.
    Hand(Box<HandReading>),
    /// The window settled without a hand on screen (lobby, results screen, ...).
    NoHand,
    /// Capture ended on its own, e.g. the window closed.
    Stopped,
}

/// A running watcher; dropping it stops the thread and the capture.
pub struct Watcher {
    stop: Arc<AtomicBool>,
    tap: FrameTap,
    corrector: Corrector,
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

/// Corrections the player made to the hand (#16), taught to the template cache on the
/// watcher thread before it reads the next frame. Cheap to clone.
#[derive(Clone, Default)]
pub struct Corrector {
    pending: Arc<Mutex<Vec<(usize, Tile)>>>,
}

impl Corrector {
    /// The hand's tile `index` (left to right, as last reported) is `tile`.
    pub fn correct(&self, index: usize, tile: Tile) {
        lock(&self.pending).push((index, tile));
    }

    fn take(&self) -> Vec<(usize, Tile)> {
        std::mem::take(&mut *lock(&self.pending))
    }
}

// Holds only an `Option`; a panic elsewhere cannot leave it half-written.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Watcher {
    /// Starts watching `window`, reading labels with `text` (`None`: no text recognition
    /// on this platform). The model is loaded and capture started before this returns, so
    /// those errors come back here; `on_event` then runs on the watcher thread.
    pub fn start(
        environment: &Environment,
        model: &Path,
        window: &WindowInfo,
        profile: Profile,
        text: Option<text::Helper>,
        on_event: impl FnMut(WatchEvent) + Send + 'static,
    ) -> Result<Self, WatchError> {
        let mut classifier = TileClassifier::load(environment, model)?;
        classifier.warm_up()?;
        let recognizer = TileRecognizer::new(classifier);
        // Text recognition loads on first use; get that done while the hand is looked for.
        if let Some(text) = text.clone() {
            std::thread::Builder::new()
                .name("text-warm-up".into())
                .spawn(move || text.warm_up())?;
        }
        let boxed = |text: &text::Helper| Box::new(text.clone()) as Box<dyn ReadText>;
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
        let corrector = Corrector::default();
        let tap = FrameTap {
            latest: Arc::clone(&latest),
            recorder: Arc::new(Mutex::new(None)),
        };
        let thread = {
            let stop = Arc::clone(&stop);
            let recorder = Arc::clone(&tap.recorder);
            let corrections = corrector.clone();
            let mut state = State {
                recognizer,
                rounds: RoundReader::new(text.as_ref().map(boxed)),
                numbers: NumberReader::new(text.as_ref().map(boxed)),
                text,
                tracker: HandTracker::new(),
                profile,
                detector: ChangeDetector::new(&[WHOLE]),
                watched: vec![WHOLE],
                frame: (0, 0),
                last: None,
                last_table: None,
                hand_crops: Vec::new(),
                misfits: 0,
            };
            let mut on_event = on_event;
            std::thread::Builder::new()
                .name("watcher".into())
                .spawn(move || {
                    let mut seen = 0;
                    let mut last_stuck: Option<Instant> = None;
                    while !stop.load(Ordering::Relaxed) {
                        for (index, tile) in corrections.take() {
                            state.teach(index, tile);
                        }
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
                        let Some(view) = frame.view() else {
                            continue;
                        };
                        let event = state.read(&view);
                        let recorder = lock(&recorder).clone();
                        // The recording keeps the frames read; while the hand keeps not
                        // fitting its melds, nothing is read, so it keeps one of those now
                        // and then too: they are the frames to debug from (#77).
                        let label = match &event {
                            Some(_) => Some("auto"),
                            None if state.misfits >= MISFITS_RECORDED
                                && last_stuck
                                    .is_none_or(|t: Instant| t.elapsed() >= STUCK_RECORD_EVERY) =>
                            {
                                last_stuck = Some(Instant::now());
                                Some("stuck")
                            }
                            None => None,
                        };
                        let Some(recorder) = recorder.filter(|_| label.is_some()) else {
                            drop(frame);
                            if let Some(event) = event {
                                on_event(event);
                            }
                            continue;
                        };
                        // Advice first, so recording never delays it; then copy the frame
                        // that was read before capture can replace it.
                        if let Some(event) = event {
                            on_event(event);
                        }
                        let image = view.to_rgb();
                        drop(frame);
                        if !recorder.save(label.unwrap_or("auto"), image) {
                            tracing::warn!(
                                dropped = recorder.dropped(),
                                "recording: earlier frames are still being written; skipped one"
                            );
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
            corrector,
            thread: Some(thread),
        })
    }

    pub fn tap(&self) -> FrameTap {
        self.tap.clone()
    }

    /// Where the player's corrections of hand tiles go (#16).
    pub fn corrector(&self) -> Corrector {
        self.corrector.clone()
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

/// Whether a hand of `tiles` (with the drawn one) fits `melds` called: 13 tiles less three
/// a meld (a kan's fourth tile is replaced by a draw), one more after a draw.
fn hand_fits(tiles: usize, melds: usize) -> bool {
    let waiting = 13usize.saturating_sub(3 * melds);
    tiles == waiting || tiles == waiting + 1
}

/// Frames in a row with a hand that does not fit its melds before its tile size is found
/// afresh.
const FORGET_AFTER_MISFITS: u32 = 5;

/// Frames in a row with a hand that does not fit its melds before the recording keeps
/// them anyway...
const MISFITS_RECORDED: u32 = 3;
/// ...one in this long.
const STUCK_RECORD_EVERY: Duration = Duration::from_secs(5);

/// More than this share of a hand's tiles unsure means it is not a hand.
const MAX_UNSURE: f32 = 0.5;

/// Whether recognized "tiles" are tiles at all: the classifier is unsure of most light
/// blocks that are not tiles (the lobby's buttons). Tiles in doubt only for being small
/// (a small window) still count as tiles.
fn looks_like_tiles(tiles: &[Recognition]) -> bool {
    let unsure = tiles
        .iter()
        .filter(|t| t.doubt == Some(Doubt::LowConfidence))
        .count();
    unsure as f32 <= MAX_UNSURE * tiles.len() as f32
}

struct State {
    recognizer: TileRecognizer,
    rounds: RoundReader,
    numbers: NumberReader,
    /// What `rounds` and `numbers` read with, to tell when it fails.
    text: Option<text::Helper>,
    tracker: HandTracker,
    profile: Profile,
    detector: ChangeDetector,
    watched: Vec<Region>,
    frame: (u32, u32),
    /// What was last reported, to report only changes.
    last: Option<String>,
    /// The last table that added up with its hand.
    last_table: Option<Snapshot>,
    /// The hand's tile pictures as last read, for corrections.
    hand_crops: Vec<RgbImage>,
    /// Frames in a row whose hand did not fit the melds: a few is an animation, many is a
    /// misread meld (#77).
    misfits: u32,
}

impl State {
    /// The player says hand tile `index` of the last reading is `tile`.
    fn teach(&mut self, index: usize, tile: Tile) {
        if let Some(crop) = self.hand_crops.get(index) {
            self.recognizer.teach(Zone::Hand, crop, tile);
        }
    }

    fn observe(&mut self, view: &FrameView<'_>, captured: Instant) {
        let size = (view.width(), view.height());
        if size != self.frame {
            // A resized window: start over with the whole window and a fresh cache.
            self.frame = size;
            self.watch(&[WHOLE]);
            self.recognizer.clear_cache();
            self.rounds.clear();
            self.numbers.clear();
            self.last_table = None;
        }
        self.detector.observe(view, captured);
    }

    fn watch(&mut self, regions: &[Region]) {
        if regions != self.watched.as_slice() {
            self.watched = regions.to_vec();
            self.detector = ChangeDetector::new(regions);
        }
    }

    fn read(&mut self, view: &FrameView<'_>) -> Option<WatchEvent> {
        let settled = Instant::now();
        let still_since = (0..self.watched.len())
            .filter_map(|i| self.detector.last_change(i))
            .max()
            .unwrap_or(settled);
        let start = Instant::now();
        let Some(layout) = self.tracker.find(view, self.profile.tile_face) else {
            self.watch(&[WHOLE]);
            let changed = self.last.take().is_some();
            return changed.then_some(WatchEvent::NoHand);
        };
        let find = start.elapsed();
        let boxes = layout.tiles();
        let crops: Vec<_> = boxes.iter().map(|b| b.crop(view)).collect();
        let start = Instant::now();
        let tiles = self.recognizer.recognize(&crops);
        self.hand_crops = crops;
        let tiles = match tiles {
            Ok(tiles) => tiles,
            Err(err) => {
                tracing::warn!("recognizing the hand: {err}");
                return None;
            }
        };
        let recognize = start.elapsed();
        if !looks_like_tiles(&tiles) {
            // Light blocks the size of tiles, e.g. in the lobby: not a hand, and not a tile
            // size to keep.
            tracing::debug!("the hand reads as unsure; not a hand");
            self.tracker.forget();
            self.watch(&[WHOLE]);
            let changed = self.last.take().is_some();
            return changed.then_some(WatchEvent::NoHand);
        }

        let start = Instant::now();
        let table = match snapshot::read(
            view,
            &self.profile,
            &layout,
            &mut self.recognizer,
            &mut self.rounds,
            &mut self.numbers,
        ) {
            Ok(Some(table)) if snapshot::consistent(&tiles, &table) => {
                self.last_table = Some(table.clone());
                Some(table)
            }
            Ok(Some(read)) => {
                tracing::debug!("the table does not add up with the hand; keeping the last one");
                // The buttons are read apart from the tiles: keep this frame's.
                self.last_table.clone().map(|last| Snapshot {
                    buttons: read.buttons,
                    ..last
                })
            }
            Ok(None) => None,
            Err(err) => {
                tracing::warn!("reading the table: {err}");
                self.last_table.clone()
            }
        };
        let table_time = start.elapsed();
        // Mid-way through the game's sorting animation (tiles sliding together after a
        // discard or a deal, some overlapping) the hand can read with tiles missing. It must
        // fit the player's melds; if not, wait for the next settled frame (#39).
        if let Some(table) = &table
            && !hand_fits(tiles.len(), table.melds[Seat::Own.index()].len())
        {
            self.misfits += 1;
            // Logged at info, as frames dropped here look like a hang from outside (#77).
            tracing::info!(
                tiles = tiles.len(),
                melds = table.melds[Seat::Own.index()].len(),
                frames = self.misfits,
                "the hand does not fit the melds; waiting for it to settle"
            );
            // Longer than an animation: the hand may be cut with a wrong tile size, locked
            // on a results screen's bigger tiles, into a count that never fits. Find it
            // afresh; a right lock is found again.
            if self.misfits >= FORGET_AFTER_MISFITS {
                self.tracker.forget();
            }
            return None;
        }
        self.misfits = 0;

        // From now on watch the hand's rows, reaching up for raised tiles, and the table
        // above them.
        let (y0, y1) = layout.band;
        let height = view.height().max(1) as f32;
        let band = (y1 - y0) as f32;
        let hand_top = (y0 as f32 - 0.3 * band).max(0.0) / height;
        let table_top = layout
            .game_area(&self.profile.hand)
            .map_or(0.0, |area| (area.top / height).clamp(0.0, hand_top));
        self.watch(&[
            Region {
                rect: RelRect::new(0.0, hand_top, 1.0, 1.3 * band / height),
                cols: 48,
                rows: 4,
            },
            Region {
                rect: RelRect::new(0.0, table_top, 1.0, (hand_top - table_top).max(0.01)),
                cols: 48,
                rows: 24,
            },
        ]);

        let mut summary: String = tiles
            .iter()
            .map(|t| format!("{}{} ", t.tile, if t.doubt.is_some() { "?" } else { "" }))
            .collect();
        if let Some(table) = &table {
            summary.push_str(&table.summary());
        }
        if self.last.as_ref() == Some(&summary) {
            return None;
        }
        self.last = Some(summary);
        Some(WatchEvent::Hand(Box::new(HandReading {
            tiles,
            boxes,
            drawn: layout.drawn.is_some(),
            table,
            frame: (view.width(), view.height()),
            text_failing: self.text.as_ref().is_some_and(text::Helper::failing),
            timings: Timings {
                still_since,
                settled,
                find,
                recognize,
                table: table_time,
            },
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognizer::Source;

    fn tile(doubt: Option<Doubt>) -> Recognition {
        Recognition {
            tile: "P".parse().unwrap(),
            confidence: 0.5,
            source: Source::Classifier,
            doubt,
            alternatives: [None; 2],
        }
    }

    #[test]
    fn a_hand_must_fit_the_melds() {
        assert!(hand_fits(13, 0) && hand_fits(14, 0));
        assert!(hand_fits(10, 1) && hand_fits(11, 1));
        assert!(hand_fits(1, 4) && hand_fits(2, 4));
        // Read mid-way through the sorting animation at a deal: 8 tiles, overlapping.
        assert!(!hand_fits(8, 0));
        // A tile lost while the others slide together after a discard.
        assert!(!hand_fits(12, 0));
        assert!(!hand_fits(9, 1));
    }

    #[test]
    fn a_lobby_read_as_unsure_tiles_is_not_a_hand() {
        // The lobby's light buttons read as seven tiles, each unsure.
        let lobby = vec![tile(Some(Doubt::LowConfidence)); 7];
        assert!(!looks_like_tiles(&lobby));
        // A hand with a few unsure tiles, or every tile small in a small window, is a hand.
        let mut hand = vec![tile(None); 11];
        hand.extend([
            tile(Some(Doubt::LowConfidence)),
            tile(Some(Doubt::LowConfidence)),
        ]);
        assert!(looks_like_tiles(&hand));
        assert!(looks_like_tiles(&[tile(Some(Doubt::TooSmall)); 13]));
    }
}
