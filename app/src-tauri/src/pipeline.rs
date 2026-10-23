//! Wiring for the live advice (#7): finds the game window, watches it with perception's
//! [`Watcher`] once the models are in place, turns each hand into advice with the decision
//! core, sends it to the window, and measures how long it took from the screen going still
//! to the advice being shown. Assembly only: the reading lives in perception, the advice
//! in core.
//!
//! The dev build also saves frames for the table evaluation set (#8): a screenshot button
//! and a switch that saves each frame changing the hand, into `recordings/live/`.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use mahjong_core::{Tile, TileCounts, outlook, rank_discards};
use mahjong_perception::capture::{self, WindowInfo};
use mahjong_perception::profile::{self, Profile};
use mahjong_perception::snapshot::{self, Snapshot};
use mahjong_perception::table::Seat;
use mahjong_perception::{
    Doubt, FrameTap, HandReading, Recognition, Recorder, WatchEvent, Watcher,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// How long to wait before looking for the game window again.
const RETRY: Duration = Duration::from_secs(2);
/// Latencies kept for the percentiles.
const KEEP: usize = 200;
/// Discard options shown besides the best one.
const OPTIONS: usize = 3;

/// What the pipeline is doing, for the window's status line.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    NoWindow,
    Watching { app: String, title: String },
    Error { message: String },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TileView {
    pub tile: Tile,
    /// "small" (window too small) or "unsure" (low confidence); `None` when sure.
    pub doubt: Option<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionView {
    pub discard: Tile,
    pub shanten: i8,
    pub ukeire_count: u8,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepTimes {
    /// Still screen to settled (the change detector's wait).
    pub settle_ms: f64,
    pub find_ms: f64,
    pub recognize_ms: f64,
    /// Reading the rivers, melds and dora indicators.
    pub table_ms: f64,
    pub decide_ms: f64,
    /// Still screen to the advice being sent; the window adds its render time.
    pub sent_ms: f64,
}

/// Advice for one hand, sent as `advice`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Advice {
    pub seq: u64,
    pub tiles: Vec<TileView>,
    pub drawn: bool,
    /// Index into `tiles` of the suggested discard, for a hand that must discard.
    pub discard: Option<usize>,
    pub options: Vec<OptionView>,
    /// Shanten after the best discard, or of the waiting hand; 0 is tenpai.
    pub shanten: Option<i8>,
    pub ukeire: Vec<Tile>,
    pub ukeire_count: Option<u8>,
    /// The dora indicators, left to right.
    pub dora: Vec<TileView>,
    /// Tiles face up on the table (rivers, melds, dora indicators), which the counts above
    /// leave out; `None` when the table is not read.
    pub seen: Option<u32>,
    /// The table as read, for the window's debug view and the player's own melds.
    pub table: Option<TableView>,
    /// The round and the numbers on the table (#9); `None` when not read.
    pub game: Option<GameView>,
    pub times: StepTimes,
}

/// The round, seat wind, numbers on the table and action buttons (#9), for the window.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameView {
    /// mjai-style, e.g. `E2` for 東2局.
    pub round: Option<String>,
    /// The player's wind: `E`, `S`, `W` or `N`.
    pub seat_wind: Option<String>,
    /// In turn order from the player.
    pub scores: Option<[u32; 4]>,
    pub tiles_left: Option<u8>,
    pub honba: Option<u8>,
    pub riichi_sticks: Option<u8>,
    /// The action buttons shown, left to right: `chi`, `pon`, `kan`, `ron`, `riichi`,
    /// `tsumo`.
    pub buttons: Vec<&'static str>,
}

impl GameView {
    fn of(table: &Snapshot) -> Self {
        Self {
            round: table.round.map(|r| r.to_string()),
            seat_wind: table.seat_wind(Seat::Own).map(|w| w.to_string()),
            scores: table.numbers.scores,
            tiles_left: table.numbers.tiles_left,
            honba: table.numbers.honba,
            riichi_sticks: table.numbers.riichi_sticks,
            buttons: table.buttons.iter().map(|b| b.as_str()).collect(),
        }
    }
}

/// One tile on the table as read.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableTile {
    /// `None` for a tile lying face down.
    pub tile: Option<Tile>,
    pub sideways: bool,
    /// The tile added to a called pon, making it a kan.
    pub added: Option<Tile>,
    pub doubt: bool,
}

/// Rivers and melds by seat, in turn order from the player.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableView {
    pub rivers: [Vec<TableTile>; 4],
    /// Each seat's melds in the order they were made, each left to right.
    pub melds: [Vec<Vec<TableTile>>; 4],
}

impl TableView {
    fn of(table: &Snapshot) -> Self {
        Self {
            rivers: table.rivers.clone().map(|river| {
                river
                    .iter()
                    .map(|t| TableTile {
                        tile: Some(t.tile.tile),
                        sideways: t.sideways,
                        added: None,
                        doubt: t.tile.doubt.is_some(),
                    })
                    .collect()
            }),
            melds: table.melds.clone().map(|melds| {
                melds
                    .iter()
                    .map(|meld| {
                        meld.iter()
                            .map(|t| TableTile {
                                tile: t.tile.map(|r| r.tile),
                                sideways: t.sideways,
                                added: t.added.map(|r| r.tile),
                                doubt: t.tile.is_some_and(|r| r.doubt.is_some())
                                    || t.added.is_some_and(|r| r.doubt.is_some()),
                            })
                            .collect()
                    })
                    .collect()
            }),
        }
    }
}

/// Still screen to advice on screen, over the last readings; sent as `latency`.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Latency {
    pub count: usize,
    pub last_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
}

/// The dev build's debug recording, for the window's debug row.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recording {
    /// Saving each frame that changes the hand.
    pub on: bool,
    /// Frames written since the app started.
    pub saved: u64,
    pub dir: String,
}

/// Shared between the watcher thread, the window's commands and the status query.
#[derive(Default)]
pub struct Pipeline {
    started: AtomicBool,
    seq: AtomicU64,
    status: Mutex<Option<Status>>,
    /// Advice sent and not yet shown: when its screen went still.
    pending: Mutex<HashMap<u64, Instant>>,
    shown: Mutex<VecDeque<f64>>,
    /// The running watcher's frames, for the debug recording.
    tap: Mutex<Option<FrameTap>>,
    /// Started on first use, in the dev build only.
    recorder: Mutex<Option<Arc<Recorder>>>,
    recording: AtomicBool,
    saved: Arc<AtomicU64>,
}

/// `recordings/live/` in the repository, which git ignores. The dev build runs from the
/// repository, two levels above this crate.
fn recordings_dir() -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    crate_dir
        .ancestors()
        .nth(2)
        .unwrap_or(crate_dir)
        .join("recordings/live")
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // Plain bookkeeping: a panic elsewhere cannot leave it inconsistent in a harmful way.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Pipeline {
    pub fn status(&self) -> Option<Status> {
        lock(&self.status).clone()
    }

    fn set_status(&self, app: &AppHandle, status: Status) {
        *lock(&self.status) = Some(status.clone());
        if let Err(err) = app.emit("pipeline-status", status) {
            tracing::warn!("sending the status: {err}");
        }
    }

    /// Starts the pipeline once; later calls do nothing.
    pub fn start(self: &Arc<Self>, app: AppHandle, model: PathBuf) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let pipeline = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("pipeline".into())
            .spawn(move || pipeline.supervise(&app, &model));
        if let Err(err) = spawned {
            tracing::error!("could not start the pipeline: {err}");
        }
    }

    /// Finds the game window, watches it until capture stops, and starts over.
    fn supervise(self: &Arc<Self>, app: &AppHandle, model: &std::path::Path) {
        mahjong_core::analysis::warm_up();
        let (environment, profiles) = match (ort::init().build(), profile::builtin()) {
            (Ok(environment), Ok(profiles)) => (environment, profiles),
            (Err(err), _) => {
                return self.set_status(
                    app,
                    Status::Error {
                        message: err.to_string(),
                    },
                );
            }
            (_, Err(err)) => {
                return self.set_status(
                    app,
                    Status::Error {
                        message: err.to_string(),
                    },
                );
            }
        };
        loop {
            let Some((window, profile)) = find_window(&profiles) else {
                self.set_status(app, Status::NoWindow);
                std::thread::sleep(RETRY);
                continue;
            };
            let (stopped, wait) = mpsc::channel();
            let pipeline = Arc::clone(self);
            let events = app.clone();
            let watcher = Watcher::start(&environment, model, &window, profile, move |event| {
                if matches!(event, WatchEvent::Stopped) {
                    let _ = stopped.send(());
                }
                pipeline.handle(&events, event);
            });
            match watcher {
                Ok(watcher) => {
                    let tap = watcher.tap();
                    if self.recording.load(Ordering::Relaxed) {
                        tap.record(lock(&self.recorder).clone());
                    }
                    *lock(&self.tap) = Some(tap);
                    tracing::info!(app = %window.app, title = %window.title, "watching the game window");
                    self.set_status(
                        app,
                        Status::Watching {
                            app: window.app.clone(),
                            title: window.title.clone(),
                        },
                    );
                    // Until capture stops (the window closed) or the watcher goes away.
                    let _ = wait.recv();
                    lock(&self.tap).take();
                    watcher.stop();
                }
                Err(err) => {
                    tracing::warn!("cannot watch {}: {err}", window.title);
                    self.set_status(
                        app,
                        Status::Error {
                            message: err.to_string(),
                        },
                    );
                    std::thread::sleep(RETRY);
                }
            }
        }
    }

    fn handle(&self, app: &AppHandle, event: WatchEvent) {
        let sent = match event {
            WatchEvent::Hand(reading) => {
                let advice = self.advise(&reading);
                tracing::info!(
                    tiles = advice.tiles.len(),
                    find_ms = advice.times.find_ms,
                    recognize_ms = advice.times.recognize_ms,
                    table_ms = advice.times.table_ms,
                    seen = advice.seen,
                    decide_ms = advice.times.decide_ms,
                    sent_ms = advice.times.sent_ms,
                    "advice"
                );
                app.emit("advice", advice)
            }
            WatchEvent::NoHand => app.emit("advice", None::<Advice>),
            WatchEvent::Stopped => Ok(()),
        };
        if let Err(err) = sent {
            tracing::warn!("sending advice: {err}");
        }
    }

    fn advise(&self, reading: &HandReading) -> Advice {
        let start = Instant::now();
        let hand: Vec<Tile> = reading.tiles.iter().map(|r| r.tile).collect();
        let visible = seen_counts(&hand, reading.table.as_ref());
        let mut advice = Advice {
            seq: self.seq.fetch_add(1, Ordering::Relaxed) + 1,
            tiles: reading.tiles.iter().map(view).collect(),
            drawn: reading.drawn,
            discard: None,
            options: Vec::new(),
            shanten: None,
            ukeire: Vec::new(),
            ukeire_count: None,
            dora: reading
                .table
                .as_ref()
                .map_or_else(Vec::new, |t| t.dora.iter().map(view).collect()),
            seen: reading
                .table
                .as_ref()
                .map(|_| visible.iter().map(|&n| u32::from(n)).sum()),
            table: reading.table.as_ref().map(TableView::of),
            game: reading.table.as_ref().map(GameView::of),
            times: StepTimes {
                settle_ms: ms(reading.timings.settled - reading.timings.still_since),
                find_ms: ms(reading.timings.find),
                recognize_ms: ms(reading.timings.recognize),
                table_ms: ms(reading.timings.table),
                decide_ms: 0.0,
                sent_ms: 0.0,
            },
        };
        if hand.len() % 3 == 2 {
            if let Ok(options) = rank_discards(&hand, &visible)
                && let Some(best) = options.first()
            {
                // Point at the drawn tile when it is the one to cut.
                advice.discard = hand.iter().rposition(|t| *t == best.discard);
                advice.shanten = Some(best.shanten);
                advice.ukeire.clone_from(&best.ukeire);
                advice.ukeire_count = Some(best.ukeire_count);
                advice.options = options
                    .iter()
                    .take(OPTIONS)
                    .map(|o| OptionView {
                        discard: o.discard,
                        shanten: o.shanten,
                        ukeire_count: o.ukeire_count,
                    })
                    .collect();
            }
        } else if let Ok(outlook) = outlook(&hand, &visible) {
            advice.shanten = Some(outlook.shanten);
            advice.ukeire = outlook.ukeire;
            advice.ukeire_count = Some(outlook.ukeire_count);
        }
        advice.times.decide_ms = ms(start.elapsed());
        advice.times.sent_ms = ms(reading.timings.still_since.elapsed());
        lock(&self.pending).insert(advice.seq, reading.timings.still_since);
        advice
    }

    /// The window drew advice `seq`: record still screen → shown.
    pub fn shown(&self, app: &AppHandle, seq: u64) {
        let Some(still_since) = lock(&self.pending).remove(&seq) else {
            return;
        };
        let total = ms(still_since.elapsed());
        let latency = {
            let mut shown = lock(&self.shown);
            shown.push_back(total);
            if shown.len() > KEEP {
                shown.pop_front();
            }
            let mut sorted: Vec<f64> = shown.iter().copied().collect();
            sorted.sort_by(f64::total_cmp);
            let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
            Latency {
                count: sorted.len(),
                last_ms: total,
                p50_ms: at(0.5),
                p95_ms: at(0.95),
            }
        };
        tracing::info!(
            total_ms = latency.last_ms,
            p50_ms = latency.p50_ms,
            p95_ms = latency.p95_ms,
            count = latency.count,
            "still screen to advice shown"
        );
        if let Err(err) = app.emit("latency", latency) {
            tracing::warn!("sending latency: {err}");
        }
    }
}

impl Pipeline {
    /// The debug recording's state; `None` outside the dev build.
    pub fn recording(&self) -> Option<Recording> {
        tauri::is_dev().then(|| Recording {
            on: self.recording.load(Ordering::Relaxed),
            saved: self.saved.load(Ordering::Relaxed),
            dir: recordings_dir().display().to_string(),
        })
    }

    /// Saves the newest frame of the game window (dev build only).
    pub fn save_frame(&self, app: &AppHandle) -> Result<(), String> {
        let recorder = self.recorder(app)?;
        let tap = lock(&self.tap)
            .clone()
            .ok_or("not watching a game window")?;
        let frame = tap.snapshot().ok_or("no frame captured yet")?;
        if recorder.save("manual", frame) {
            Ok(())
        } else {
            Err("still writing earlier frames".into())
        }
    }

    /// Starts or stops saving each frame that changes the hand (dev build only).
    pub fn set_recording(&self, app: &AppHandle, on: bool) -> Result<Recording, String> {
        let recorder = if on { Some(self.recorder(app)?) } else { None };
        self.recording.store(on, Ordering::Relaxed);
        if let Some(tap) = lock(&self.tap).as_ref() {
            tap.record(recorder);
        }
        self.recording()
            .ok_or_else(|| "only in the dev build".into())
    }

    fn recorder(&self, app: &AppHandle) -> Result<Arc<Recorder>, String> {
        if !tauri::is_dev() {
            return Err("only in the dev build".into());
        }
        let mut recorder = lock(&self.recorder);
        if let Some(recorder) = recorder.as_ref() {
            return Ok(Arc::clone(recorder));
        }
        let saved = Arc::clone(&self.saved);
        let app = app.clone();
        let started = Recorder::start(recordings_dir(), move |path, count| {
            saved.store(count, Ordering::Relaxed);
            tracing::info!(path = %path.display(), "saved a frame");
            // Best effort: the count is only for the debug row.
            let _ = app.emit("frame-saved", count);
        })
        .map(Arc::new)
        .map_err(|err| err.to_string())?;
        *recorder = Some(Arc::clone(&started));
        Ok(started)
    }
}

fn view(r: &Recognition) -> TileView {
    TileView {
        tile: r.tile,
        doubt: r.doubt.map(|d| match d {
            Doubt::TooSmall => "small",
            Doubt::LowConfidence => "unsure",
        }),
    }
}

/// Tiles of each kind face up on the table, capped so that with the hand there are never
/// more than four: a misread tile must not make the hand impossible to advise on.
fn seen_counts(hand: &[Tile], table: Option<&Snapshot>) -> TileCounts {
    let mut seen = table.map_or([0; 34], snapshot::seen);
    for (kind, count) in seen.iter_mut().enumerate() {
        let in_hand = hand
            .iter()
            .filter(|t| usize::from(t.kind()) == kind)
            .count() as u8;
        *count = (*count).min(4u8.saturating_sub(in_hand));
    }
    seen
}

/// The first window a built-in profile recognises, with that profile.
fn find_window(profiles: &[Profile]) -> Option<(WindowInfo, Profile)> {
    let windows = match capture::list_windows() {
        Ok(windows) => windows,
        Err(err) => {
            tracing::warn!("listing windows: {err}");
            return None;
        }
    };
    windows
        .into_iter()
        .find_map(|w| profile::for_window(profiles, &w).cloned().map(|p| (w, p)))
}

#[cfg(test)]
mod tests {
    use mahjong_perception::river::RiverTile;
    use mahjong_perception::{Recognition, Source};

    use super::*;

    #[test]
    fn seen_tiles_never_make_more_than_four_with_the_hand() {
        let tile = |name: &str| Recognition {
            tile: name.parse().unwrap(),
            confidence: 0.99,
            source: Source::Classifier,
            doubt: None,
        };
        let mut table = Snapshot::default();
        // Three East discarded, one misread too many, and a red five.
        table.rivers[2] = ["E", "E", "E", "5pr"]
            .iter()
            .map(|n| RiverTile {
                tile: tile(n),
                sideways: false,
            })
            .collect();
        table.dora = vec![tile("E")];
        let hand: Vec<Tile> = ["E", "E", "5p"].map(|n| n.parse().unwrap()).to_vec();
        let seen = seen_counts(&hand, Some(&table));
        assert_eq!(seen[27], 2);
        assert_eq!(seen[13], 1);
        assert_eq!(seen_counts(&hand, None), [0; 34]);
    }
}
