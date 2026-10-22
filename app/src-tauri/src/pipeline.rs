//! Wiring for the live advice (#7): finds the game window, watches it with perception's
//! [`Watcher`] once the models are in place, turns each hand into advice with the decision
//! core, sends it to the window, and measures how long it took from the screen going still
//! to the advice being shown. Assembly only: the reading lives in perception, the advice
//! in core.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use mahjong_core::{Tile, TileCounts, outlook, rank_discards};
use mahjong_perception::capture::{self, WindowInfo};
use mahjong_perception::profile::{self, Profile};
use mahjong_perception::{Doubt, HandReading, WatchEvent, Watcher};
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
    pub times: StepTimes,
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

/// Shared between the watcher thread, the window's commands and the status query.
#[derive(Default)]
pub struct Pipeline {
    started: AtomicBool,
    seq: AtomicU64,
    status: Mutex<Option<Status>>,
    /// Advice sent and not yet shown: when its screen went still.
    pending: Mutex<HashMap<u64, Instant>>,
    shown: Mutex<VecDeque<f64>>,
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
                let advice = self.advise(reading);
                tracing::info!(
                    tiles = advice.tiles.len(),
                    find_ms = advice.times.find_ms,
                    recognize_ms = advice.times.recognize_ms,
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

    fn advise(&self, reading: HandReading) -> Advice {
        let start = Instant::now();
        let hand: Vec<Tile> = reading.tiles.iter().map(|r| r.tile).collect();
        // Rivers, melds and dora indicators are not read yet (#8).
        let visible: TileCounts = [0; 34];
        let mut advice = Advice {
            seq: self.seq.fetch_add(1, Ordering::Relaxed) + 1,
            tiles: reading
                .tiles
                .iter()
                .map(|r| TileView {
                    tile: r.tile,
                    doubt: r.doubt.map(|d| match d {
                        Doubt::TooSmall => "small",
                        Doubt::LowConfidence => "unsure",
                    }),
                })
                .collect(),
            drawn: reading.drawn,
            discard: None,
            options: Vec::new(),
            shanten: None,
            ukeire: Vec::new(),
            ukeire_count: None,
            times: StepTimes {
                settle_ms: ms(reading.timings.settled - reading.timings.still_since),
                find_ms: ms(reading.timings.find),
                recognize_ms: ms(reading.timings.recognize),
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
