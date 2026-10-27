//! Wiring for the live advice (#7): finds the game window, watches it with perception's
//! [`Watcher`] once the models are in place, turns each hand into advice with the decision
//! core, sends it to the window, and measures how long it took from the screen going still
//! to the advice being shown. Assembly only: the reading lives in perception, the advice
//! in core.
//!
//! It also asks the decision model (#22), Mortal, what to do whenever the state engine
//! has followed the table to a point where the player can act, and sends that as
//! `mortal` right after the advice, which it comes before in the window.
//!
//! The dev build also saves frames for the table evaluation set (#8): a screenshot button
//! and a switch that saves each frame changing the hand, into `recordings/live/`. So does
//! the packaged dev app (the `dev-tools` feature), into its own data folder.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use mahjong_core::calls::{self, CallAdvice, CallContext, CallKind, Yaku};
use mahjong_core::defense::{self, Defense, Own};
use mahjong_core::game::{Engine, Event, MarkKind, PlayerState, Rejected};
use mahjong_core::mortal::{Kind, Mortal, Suggestion};
use mahjong_core::scoring::{self, Value};
use mahjong_core::target::{self, Outlook};
use mahjong_core::{Tile, TileCounts, outlook, parse_tiles, rank_discards, unseen};
use mahjong_perception::buttons::Button;
use mahjong_perception::capture::{self, WindowInfo};
use mahjong_perception::profile::{self, Profile};
use mahjong_perception::snapshot::{self, Snapshot};
use mahjong_perception::table::Seat;
use mahjong_perception::text;
use mahjong_perception::{
    Corrector, Doubt, FrameTap, HandReading, Recognition, Recorder, WatchEvent, Watcher,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

/// How often the pause switch is checked while waiting.
const PAUSE_POLL: Duration = Duration::from_millis(200);
/// How long to wait before looking for the game window again.
const RETRY: Duration = Duration::from_secs(2);
/// Latencies kept for the percentiles.
const KEEP: usize = 200;
/// Mortal's discards sent: the best and two more.
const OPTIONS: usize = 3;
/// Game log events sent for the dev area.
const RECENT: usize = 3;

/// What the pipeline is doing, for the window's status line.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    NoWindow,
    /// The player paused recognition (#16): no capture, no reading, no recording.
    Paused,
    /// `game` is the profile's name, e.g. 雀魂（网页版）.
    Watching {
        app: String,
        title: String,
        game: String,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TileView {
    pub tile: Tile,
    /// "small" (window too small) or "unsure" (low confidence); `None` when sure.
    pub doubt: Option<&'static str>,
    /// The classifier's next likeliest tiles, offered when correcting it (#16).
    pub alternatives: Vec<Tile>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionView {
    pub discard: Tile,
    pub shanten: i8,
    pub ukeire: Vec<TileLeft>,
    pub ukeire_count: u8,
}

/// A tile that improves the hand (or wins it), and how many copies are still unseen.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TileLeft {
    pub tile: Tile,
    pub left: u8,
}

fn tiles_left(tiles: &[Tile], hand: &[Tile], visible: &TileCounts) -> Vec<TileLeft> {
    tiles
        .iter()
        .map(|&tile| TileLeft {
            tile,
            left: unseen(hand, visible, tile),
        })
        .collect()
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
    pub ukeire: Vec<TileLeft>,
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
    /// Whether and how to call the discard on offer (#11); `None` without a chi or pon
    /// button.
    pub call: Option<CallView>,
    pub times: StepTimes,
}

/// Call advice (#11) for the window.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallView {
    pub called: Tile,
    /// Who discarded it: `right`, `across` or `left`; `None` when two of them did.
    pub from: Option<&'static str>,
    /// Shanten and effective draws if not calling.
    pub stay_shanten: i8,
    pub stay_ukeire_count: u8,
    /// Every way to call, best first.
    pub options: Vec<CallOptionView>,
    /// Index into `options` of the call to take; `None` to let it go.
    pub take: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallOptionView {
    /// `chi` or `pon`.
    pub kind: &'static str,
    pub with: [Tile; 2],
    pub discard: Tile,
    pub shanten: i8,
    pub ukeire_count: u8,
    /// `yakuhai`, `tanyao`, `honitsu` or `toitoi`.
    pub yaku: Vec<&'static str>,
}

impl CallView {
    fn of(called: Tile, from: Option<&'static str>, advice: &CallAdvice) -> Self {
        Self {
            called,
            from,
            stay_shanten: advice.stay.shanten,
            stay_ukeire_count: advice.stay.ukeire_count,
            options: advice
                .options
                .iter()
                .map(|o| CallOptionView {
                    kind: match o.kind {
                        CallKind::Chi => "chi",
                        CallKind::Pon => "pon",
                    },
                    with: o.with,
                    discard: o.after.discard,
                    shanten: o.after.shanten,
                    ukeire_count: o.after.ukeire_count,
                    yaku: o
                        .yaku
                        .iter()
                        .map(|y| match y {
                            Yaku::Yakuhai => "yakuhai",
                            Yaku::Tanyao => "tanyao",
                            Yaku::Honitsu => "honitsu",
                            Yaku::Toitoi => "toitoi",
                        })
                        .collect(),
                })
                .collect(),
            take: advice.take,
        }
    }
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
    /// On frames with action buttons: the advice, and Mortal's decision (#11).
    pub buttons: Option<Spread>,
    pub buttons_mortal: Option<Spread>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Spread {
    pub count: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
}

/// Keeps the latest [`KEEP`] latencies.
fn record(kept: &Mutex<VecDeque<f64>>, ms: f64) {
    let mut kept = lock(kept);
    kept.push_back(ms);
    if kept.len() > KEEP {
        kept.pop_front();
    }
}

fn spread(kept: &Mutex<VecDeque<f64>>) -> Spread {
    let mut sorted: Vec<f64> = lock(kept).iter().copied().collect();
    sorted.sort_by(f64::total_cmp);
    let at = |q: f64| {
        sorted
            .get((sorted.len().saturating_sub(1) as f64 * q).round() as usize)
            .copied()
            .unwrap_or(0.0)
    };
    Spread {
        count: sorted.len(),
        p50_ms: at(0.5),
        p95_ms: at(0.95),
    }
}

/// The dev build's debug recording, for the window's debug row.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recording {
    /// Saving each frame that changes the hand.
    pub on: bool,
    /// Frames written since the app started.
    pub saved: u64,
    /// Frames the continuous recording dropped while earlier ones were being written.
    pub dropped: u64,
    pub dir: String,
}

/// Shared between the watcher thread, the window's commands and the status query.
#[derive(Default)]
pub struct Pipeline {
    started: AtomicBool,
    seq: AtomicU64,
    status: Mutex<Option<Status>>,
    /// Advice sent and not yet shown: when its screen went still, and whether it showed
    /// action buttons.
    pending: Mutex<HashMap<u64, Pending>>,
    shown: Mutex<VecDeque<f64>>,
    /// Still screen to shown on frames with action buttons (#11): the advice, and
    /// Mortal's decision.
    shown_buttons: Mutex<VecDeque<f64>>,
    shown_buttons_mortal: Mutex<VecDeque<f64>>,
    /// The running watcher's frames, for the debug recording.
    tap: Mutex<Option<FrameTap>>,
    /// Started on first use, in the dev build only.
    recorder: Mutex<Option<Arc<Recorder>>>,
    recording: AtomicBool,
    saved: Arc<AtomicU64>,
    /// The player paused recognition (#16).
    paused: AtomicBool,
    /// The window the player picked when none was found by its title (#17).
    chosen: Mutex<Option<u32>>,
    /// The running watcher's way to learn the player's corrections (#16).
    corrector: Mutex<Option<Corrector>>,
    /// The last hand read, to correct and advise on again (#16).
    last_reading: Mutex<Option<HandReading>>,
    /// Scenes in a row the state engine did not take, for the status light (#16).
    stuck: AtomicU32,
    /// The state engine (#10) and the file each game's log goes to.
    game: Mutex<GameRecord>,
    /// The decision model (#22), when it is in the models folder.
    mortal: Mutex<Option<Mortal>>,
}

/// What the win on offer is worth (#11), for advice `seq`, sent as `win`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WinView {
    pub seq: u64,
    #[serde(flatten)]
    pub value: Value,
}

/// How dangerous the hand's tiles are and whether to push (#12), for advice `seq`, sent
/// as `defense`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefenseView {
    pub seq: u64,
    #[serde(flatten)]
    pub defense: Defense,
}

/// What the state engine followed of the table: the player's state when it can act, and
/// the round so far with the player's seat, when the engine took the scene.
struct Followed {
    state: Option<PlayerState>,
    round: Option<(Vec<Event>, u8)>,
}

/// Target hands or the waits' values (#13) for advice `seq`, sent as `targets`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetsView {
    pub seq: u64,
    #[serde(flatten)]
    pub outlook: Outlook,
    pub search_ms: f64,
}

/// Mortal's decision (#22) for advice `seq`, sent as `mortal`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MortalView {
    pub seq: u64,
    pub best: Suggestion,
    /// When the best is riichi, the discard declaring it.
    pub riichi_discard: Option<Suggestion>,
    /// The best discards, best first, when discarding is a choice.
    pub discards: Vec<Suggestion>,
    pub decide_ms: f64,
}

#[derive(Clone, Copy)]
struct Pending {
    still_since: Instant,
    buttons: bool,
    /// Whether the advice and Mortal's decision were shown.
    advice: bool,
    mortal: bool,
}

/// Advice kept waiting to be shown; older is dropped (a decision that never came).
const PENDING: u64 = 64;

#[derive(Default)]
struct GameRecord {
    engine: Engine,
    /// One a game, in the order they started.
    files: Vec<PathBuf>,
}

/// What the state engine (#10) made of the table, for the window.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineView {
    /// Events in the current game's log.
    pub events: usize,
    /// The current round, mjai-style (`E2`), and its honba.
    pub round: Option<String>,
    pub honba: Option<u8>,
    /// The round's start was not seen: its history so far is filled in.
    pub filled: bool,
    /// Stretches of the game filled in again after the engine lost track.
    pub gaps: usize,
    /// How the last scene went: `taken`, `covered`, `unexplained` or `impossible`.
    pub last: &'static str,
    /// Text recognition fails even in a fresh helper: the round and scores cannot be read
    /// until the app restarts, unless a later retry works (#49).
    pub text_failing: bool,
    /// The game log's latest events, newest first, each after its index (dev build).
    pub recent: Vec<String>,
    /// Scenes in a row the engine did not take: the advice cannot follow the game (#16).
    pub stuck: u32,
}

/// Whether the dev tools are on: under `tauri dev`, or in the packaged dev app.
pub fn dev_tools() -> bool {
    tauri::is_dev() || cfg!(feature = "dev-tools")
}

/// The dev tools' one folder for what they keep: `live/` frames, `logs/` the log files and
/// `game-logs/` the game logs. Dev builds are built in the repository (two levels above
/// this crate), and its `recordings/`, which git ignores, is used while it is there; else
/// the app's data folder's `recordings/`.
pub fn debug_dir(app: &AppHandle) -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repository = crate_dir.ancestors().nth(2).unwrap_or(crate_dir);
    if repository.join(".git").exists() {
        return repository.join("recordings");
    }
    app.path().app_data_dir().map_or_else(
        |_| repository.join("recordings"),
        |data| data.join("recordings"),
    )
}

/// Where frames are saved: the debug folder's `live/`.
fn recordings_dir(app: &AppHandle) -> PathBuf {
    debug_dir(app).join("live")
}

/// Where game logs are saved: in the dev tools' debug folder, beside the frames and logs;
/// else the app's data folder, as the player's own record.
fn game_logs_dir(app: &AppHandle) -> Option<PathBuf> {
    if dev_tools() {
        return Some(debug_dir(app).join("game-logs"));
    }
    app.path().app_data_dir().ok().map(|d| d.join("game-logs"))
}

/// Writes the current game's log, and the one before when a new game has just begun
/// (`all`: every game's), as mjai JSON Lines with the marks beside them. Off the watcher
/// thread: the files are small but disk writes can stall.
fn save_games(game: &GameRecord, all: bool) {
    let games = game.engine.games();
    let from = if all {
        0
    } else {
        games.len().saturating_sub(1)
    };
    let writes: Vec<(PathBuf, String, String)> = games
        .iter()
        .enumerate()
        .skip(from)
        .filter_map(|(i, log)| {
            let file = game.files.get(i)?.clone();
            let marks = serde_json::to_string(&log.marks).ok()?;
            Some((file, log.lines(), marks))
        })
        .collect();
    std::thread::spawn(move || {
        for (file, lines, marks) in writes {
            let marks_file = file.with_extension("marks.json");
            let written = file
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&file, lines))
                .and_then(|()| std::fs::write(&marks_file, marks));
            if let Err(err) = written {
                tracing::warn!("saving the game log {}: {err}", file.display());
            }
        }
    });
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

impl Pipeline {
    /// Whether the decision model loaded and gives advice (#22).
    pub fn mortal_loaded(&self) -> bool {
        lock(&self.mortal).is_some()
    }
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
    /// Starts watching with the tile classifier at `model` and the decision model at
    /// `mortal`.
    pub fn start(self: &Arc<Self>, app: AppHandle, model: PathBuf, mortal: PathBuf) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let pipeline = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("pipeline".into())
            .spawn(move || pipeline.supervise(&app, &model, &mortal));
        if let Err(err) = spawned {
            tracing::error!("could not start the pipeline: {err}");
        }
    }

    /// Finds the game window, watches it until capture stops, and starts over.
    fn supervise(self: &Arc<Self>, app: &AppHandle, model: &Path, mortal: &Path) {
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
        // Without the decision model the panel still shows the rule-based advice.
        match Mortal::load(&environment, mortal).and_then(|mut m| m.warm_up().map(|()| m)) {
            Ok(m) => {
                tracing::info!(path = %mortal.display(), "decision model loaded");
                *lock(&self.mortal) = Some(m);
            }
            Err(err) => tracing::warn!("the decision model did not load: {err}"),
        }
        // Labels are read in a helper process, this executable started again, which is
        // replaced when the system's text recognition breaks down (#49).
        let text = match std::env::current_exe() {
            Ok(exe) => text::helper(exe),
            Err(err) => {
                tracing::warn!("no text recognition: cannot find this executable: {err}");
                None
            }
        };
        loop {
            if self.paused.load(Ordering::Relaxed) {
                if lock(&self.status)
                    .as_ref()
                    .is_none_or(|s| !matches!(s, Status::Paused))
                {
                    self.set_status(app, Status::Paused);
                }
                std::thread::sleep(PAUSE_POLL);
                continue;
            }
            let chosen = *lock(&self.chosen);
            let Some((window, profile)) = find_window(&profiles, chosen) else {
                self.set_status(app, Status::NoWindow);
                std::thread::sleep(RETRY);
                continue;
            };
            let (stopped, wait) = mpsc::channel();
            let pipeline = Arc::clone(self);
            let events = app.clone();
            let game = profile.name.clone();
            let watcher = Watcher::start(
                &environment,
                model,
                &window,
                profile,
                text.clone(),
                move |event| {
                    if matches!(event, WatchEvent::Stopped) {
                        let _ = stopped.send(());
                    }
                    pipeline.handle(&events, event);
                },
            );
            match watcher {
                Ok(watcher) => {
                    let tap = watcher.tap();
                    if self.recording.load(Ordering::Relaxed) {
                        tap.record(lock(&self.recorder).clone());
                    }
                    *lock(&self.tap) = Some(tap);
                    *lock(&self.corrector) = Some(watcher.corrector());
                    tracing::info!(app = %window.app, title = %window.title, "watching the game window");
                    self.set_status(
                        app,
                        Status::Watching {
                            app: window.app.clone(),
                            title: window.title.clone(),
                            game,
                        },
                    );
                    // Until capture stops (the window closed), the watcher goes away, or the
                    // player pauses recognition.
                    loop {
                        match wait.recv_timeout(PAUSE_POLL) {
                            Err(mpsc::RecvTimeoutError::Timeout)
                                if !self.paused.load(Ordering::Relaxed) => {}
                            _ => break,
                        }
                    }
                    lock(&self.tap).take();
                    lock(&self.corrector).take();
                    watcher.stop();
                    if self.paused.load(Ordering::Relaxed) {
                        self.pause_game(app);
                    }
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

    /// The game window as it would be found now, even while paused (#17).
    pub fn game_window(&self) -> Option<WindowChoice> {
        let profiles = profile::builtin().ok()?;
        let chosen = *lock(&self.chosen);
        find_window(&profiles, chosen).map(|(w, _)| WindowChoice {
            id: w.id,
            app: w.app,
            title: w.title,
        })
    }

    /// The player picked the game window (#17); the supervisor looks for it next.
    pub fn choose_window(&self, id: u32) {
        *lock(&self.chosen) = Some(id);
    }

    /// Pauses or resumes recognition (#16); the supervisor stops or restarts the watcher.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    /// Recognition just paused: ends the game being logged, saves it and clears the panel.
    fn pause_game(&self, app: &AppHandle) {
        {
            let mut game = lock(&self.game);
            game.engine.end_game();
            save_games(&game, true);
        }
        lock(&self.last_reading).take();
        self.stuck.store(0, Ordering::Relaxed);
        if let Err(err) = app.emit("advice", None::<Advice>) {
            tracing::warn!("clearing the advice: {err}");
        }
    }

    /// The player says hand tile `index` of the last reading is `tile` (#16): advises
    /// again at once on the corrected hand, and teaches the watcher for the next frames.
    pub fn correct(&self, app: &AppHandle, index: usize, tile: Tile) -> Result<(), String> {
        let mut reading = lock(&self.last_reading)
            .clone()
            .ok_or("no hand to correct")?;
        let read = reading
            .tiles
            .get_mut(index)
            .ok_or("no such tile in the hand")?;
        read.tile = tile;
        read.confidence = 1.0;
        read.doubt = None;
        read.alternatives = [None; 2];
        if let Some(corrector) = lock(&self.corrector).as_ref() {
            corrector.correct(index, tile);
        }
        tracing::info!(index, %tile, "the player corrected a tile");
        self.handle(app, WatchEvent::Hand(Box::new(reading)));
        Ok(())
    }

    fn handle(&self, app: &AppHandle, event: WatchEvent) {
        let sent = match event {
            WatchEvent::Hand(reading) => {
                *lock(&self.last_reading) = Some((*reading).clone());
                let advice = self.advise(&reading);
                tracing::info!(
                    tiles = advice.tiles.len(),
                    find_ms = advice.times.find_ms,
                    recognize_ms = advice.times.recognize_ms,
                    table_ms = advice.times.table_ms,
                    seen = advice.seen,
                    buttons = ?advice.game.as_ref().map(|g| &g.buttons),
                    decide_ms = advice.times.decide_ms,
                    sent_ms = advice.times.sent_ms,
                    "advice"
                );
                let seq = advice.seq;
                let shanten = advice.shanten;
                let ukeire = advice.ukeire_count;
                let sent = app.emit("advice", advice);
                let points = self.aim(app, seq, &reading);
                let followed = self.follow(app, &reading);
                if let (Some((events, me)), Some(shanten)) = (&followed.round, shanten) {
                    let own = Own {
                        shanten,
                        ukeire: ukeire.unwrap_or(0),
                        points,
                    };
                    self.defend(app, seq, &reading, events, *me, &own);
                }
                if let Some(state) = followed.state {
                    if let Some(value) = scoring::value(&state) {
                        let sent = app.emit("win", WinView { seq, value });
                        if let Err(err) = sent {
                            tracing::warn!("sending the win's value: {err}");
                        }
                    }
                    self.consult(app, seq, &state);
                }
                sent
            }
            WatchEvent::NoHand => app.emit("advice", None::<Advice>),
            WatchEvent::Stopped => {
                let mut game = lock(&self.game);
                game.engine.end_game();
                save_games(&game, true);
                Ok(())
            }
        };
        if let Err(err) = sent {
            tracing::warn!("sending advice: {err}");
        }
    }

    /// Feeds the table to the state engine, after the advice so that it adds nothing to
    /// its latency, and saves the log when a round or game starts. Returns the player's
    /// state when the engine took the scene and the player can act on it, and the round so
    /// far when the engine took the scene.
    fn follow(&self, app: &AppHandle, reading: &HandReading) -> Followed {
        let Some(table) = reading.table.as_ref() else {
            return Followed {
                state: None,
                round: None,
            };
        };
        let scene = table.scene(&reading.tiles, reading.drawn);
        let mut game = lock(&self.game);
        let (last, added, new_round, new_game) = match game.engine.observe(&scene) {
            Ok(observed) => {
                let starts = |f: fn(&Event) -> bool| observed.events.iter().any(f);
                (
                    "taken",
                    observed.events.len(),
                    starts(|e| matches!(e, Event::StartKyoku { .. })) || observed.rewritten,
                    starts(|e| matches!(e, Event::StartGame { .. })),
                )
            }
            Err(Rejected::Covered | Rejected::NoRound) => ("covered", 0, false, false),
            Err(Rejected::Unexplained) => ("unexplained", 0, false, false),
            Err(Rejected::Impossible(why)) => {
                tracing::info!("a scene that cannot be: {why}");
                ("impossible", 0, false, false)
            }
        };
        if new_game {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis());
            if let Some(dir) = game_logs_dir(app) {
                game.files.push(dir.join(format!("{stamp}.mjson")));
            }
        }
        // The log is saved as it grows (#80), not only when a round begins, so a round cut
        // short still leaves what was followed of it.
        if new_round || added > 0 {
            save_games(&game, new_game);
        }
        let stuck = if last == "taken" {
            self.stuck.store(0, Ordering::Relaxed);
            0
        } else {
            self.stuck.fetch_add(1, Ordering::Relaxed) + 1
        };
        tracing::info!(scene = last, added, stuck, "engine");
        let games = game.engine.games();
        let current = games.last();
        let view = EngineView {
            stuck,
            events: current.map_or(0, |g| g.events.len()),
            round: game
                .engine
                .round()
                .map(|(l, _)| format!("{}{}", l.wind, l.number)),
            honba: game.engine.round().and_then(|(_, h)| h),
            filled: game.engine.filled(),
            gaps: current.map_or(0, |g| {
                g.marks.iter().filter(|m| m.kind == MarkKind::Gap).count()
            }),
            last,
            text_failing: reading.text_failing,
            recent: current.map_or_else(Vec::new, |g| {
                let first = g.events.len().saturating_sub(RECENT);
                g.events[first..]
                    .iter()
                    .enumerate()
                    .rev()
                    .map(|(i, e)| {
                        let json = serde_json::to_string(e).unwrap_or_default();
                        format!("{} {json}", first + i)
                    })
                    .collect()
            }),
        };
        if let Err(err) = app.emit("engine", view) {
            tracing::warn!("sending the engine state: {err}");
        }
        let taken = game.engine.player_state().filter(|_| last == "taken");
        let round = taken.zip(current).map(|(s, g)| {
            let start = g
                .events
                .iter()
                .rposition(|e| matches!(e, Event::StartKyoku { .. }))
                .unwrap_or(0);
            (g.events[start..].to_vec(), s.player_id())
        });
        Followed {
            state: taken.filter(|s| s.last_cans().can_act()).cloned(),
            round,
        }
    }

    /// Rates the hand's tiles against the players threatening to win and sends it, with
    /// whether to push, for advice `seq`.
    fn defend(
        &self,
        app: &AppHandle,
        seq: u64,
        reading: &HandReading,
        events: &[Event],
        me: u8,
        own: &Own,
    ) {
        let hand: Vec<Tile> = reading.tiles.iter().map(|r| r.tile).collect();
        let visible = seen_counts(&hand, reading.table.as_ref());
        let view = DefenseView {
            seq,
            defense: defense::assess(events, me, &hand, &visible, own),
        };
        if let Err(err) = app.emit("defense", view) {
            tracing::warn!("sending the defense: {err}");
        }
    }

    /// Searches the target hands, or values the waits, for advice `seq` and sends them,
    /// after the advice so that it adds nothing to its latency.
    /// Returns the best target's or wait's points, for the push-or-fold verdict.
    fn aim(&self, app: &AppHandle, seq: u64, reading: &HandReading) -> Option<i32> {
        let table = reading.table.as_ref()?;
        let start = Instant::now();
        let outlook = target::from_scene(&table.scene(&reading.tiles, reading.drawn));
        let points = outlook
            .targets
            .targets
            .iter()
            .map(|t| t.points)
            .chain(outlook.waits.iter().map(|w| w.points))
            .max();
        let view = TargetsView {
            seq,
            outlook,
            search_ms: ms(start.elapsed()),
        };
        if let Err(err) = app.emit("targets", view) {
            tracing::warn!("sending the target hands: {err}");
        }
        points
    }

    /// Asks Mortal what to do in `state`, the table of advice `seq`, and sends it.
    fn consult(&self, app: &AppHandle, seq: u64, state: &PlayerState) {
        let mut mortal = lock(&self.mortal);
        let Some(mortal) = mortal.as_mut() else {
            return;
        };
        let start = Instant::now();
        let decision = match mortal.decide(state) {
            Ok(decision) => decision,
            Err(err) => {
                tracing::warn!("the decision model: {err}");
                return;
            }
        };
        let discards = decision
            .choices
            .iter()
            .map(|c| c.suggestion())
            .filter(|s| s.kind == Kind::Discard)
            .take(OPTIONS)
            .collect();
        let view = MortalView {
            seq,
            best: decision.best().suggestion(),
            riichi_discard: decision.riichi_discard.as_ref().map(|c| c.suggestion()),
            discards,
            decide_ms: ms(start.elapsed()),
        };
        tracing::info!(
            kind = ?view.best.kind,
            tile = ?view.best.tile.map(|t| t.to_string()),
            decide_ms = view.decide_ms,
            "mortal"
        );
        if let Err(err) = app.emit("mortal", view) {
            tracing::warn!("sending the decision: {err}");
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
            call: None,
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
                advice.ukeire = tiles_left(&best.ukeire, &hand, &visible);
                advice.ukeire_count = Some(best.ukeire_count);
                // Every discard, so the window can show the one Mortal picks.
                advice.options = options
                    .iter()
                    .map(|o| OptionView {
                        discard: o.discard,
                        shanten: o.shanten,
                        ukeire: tiles_left(&o.ukeire, &hand, &visible),
                        ukeire_count: o.ukeire_count,
                    })
                    .collect();
            }
        } else if let Ok(outlook) = outlook(&hand, &visible) {
            advice.shanten = Some(outlook.shanten);
            advice.ukeire = tiles_left(&outlook.ukeire, &hand, &visible);
            advice.ukeire_count = Some(outlook.ukeire_count);
            if let Some(table) = &reading.table {
                advice.call = call_view(&hand, table, &visible);
            }
        }
        advice.times.decide_ms = ms(start.elapsed());
        advice.times.sent_ms = ms(reading.timings.still_since.elapsed());
        let buttons = reading
            .table
            .as_ref()
            .is_some_and(|t| !t.buttons.is_empty());
        let mut pending = lock(&self.pending);
        pending.retain(|&seq, _| seq + PENDING > advice.seq);
        pending.insert(
            advice.seq,
            Pending {
                still_since: reading.timings.still_since,
                buttons,
                advice: false,
                mortal: false,
            },
        );
        advice
    }

    /// The window drew advice `seq`, or Mortal's decision for it: record still screen →
    /// shown, and for frames with action buttons apart (#11).
    pub fn shown(&self, app: &AppHandle, seq: u64, mortal: bool) {
        let (total, buttons) = {
            let mut pending = lock(&self.pending);
            let Some(entry) = pending.get_mut(&seq) else {
                return;
            };
            let done = if mortal {
                &mut entry.mortal
            } else {
                &mut entry.advice
            };
            if *done {
                return;
            }
            *done = true;
            let found = (ms(entry.still_since.elapsed()), entry.buttons);
            // Without buttons only the advice is measured; with them, Mortal's decision
            // too (or the entry ages out when none comes).
            if entry.advice && (entry.mortal || !entry.buttons) {
                pending.remove(&seq);
            }
            found
        };
        match (mortal, buttons) {
            (false, _) => {
                record(&self.shown, total);
                if buttons {
                    record(&self.shown_buttons, total);
                }
            }
            (true, true) => record(&self.shown_buttons_mortal, total),
            (true, false) => return,
        }
        let all = spread(&self.shown);
        let latency = Latency {
            count: all.count,
            last_ms: total,
            p50_ms: all.p50_ms,
            p95_ms: all.p95_ms,
            buttons: Some(spread(&self.shown_buttons)).filter(|s| s.count > 0),
            buttons_mortal: Some(spread(&self.shown_buttons_mortal)).filter(|s| s.count > 0),
        };
        tracing::info!(
            total_ms = total,
            mortal,
            buttons,
            p50_ms = latency.p50_ms,
            p95_ms = latency.p95_ms,
            count = latency.count,
            buttons_p95_ms = latency.buttons.as_ref().map(|s| s.p95_ms),
            buttons_mortal_p95_ms = latency.buttons_mortal.as_ref().map(|s| s.p95_ms),
            "still screen to shown"
        );
        if let Err(err) = app.emit("latency", latency) {
            tracing::warn!("sending latency: {err}");
        }
    }

    pub fn recording(&self, app: &AppHandle) -> Option<Recording> {
        dev_tools().then(|| Recording {
            on: self.recording.load(Ordering::Relaxed),
            saved: self.saved.load(Ordering::Relaxed),
            dropped: lock(&self.recorder).as_ref().map_or(0, |r| r.dropped()),
            dir: recordings_dir(app).display().to_string(),
        })
    }

    /// Saves the newest frame of the game window (dev build only), waiting for frames
    /// still being written rather than dropping it (#80). Returns the file's path.
    pub fn save_frame(&self, app: &AppHandle) -> Result<String, String> {
        let recorder = self.recorder(app)?;
        let tap = lock(&self.tap)
            .clone()
            .ok_or("not watching a game window")?;
        let frame = tap.snapshot().ok_or("no frame captured yet")?;
        let path = recorder
            .save_waiting("manual", frame)
            .ok_or("the recorder has stopped")?;
        tracing::info!(path = %path.display(), "screenshot queued");
        Ok(path.display().to_string())
    }

    /// Starts or stops saving each frame that changes the hand (dev build only).
    pub fn set_recording(&self, app: &AppHandle, on: bool) -> Result<Recording, String> {
        let recorder = if on { Some(self.recorder(app)?) } else { None };
        self.recording.store(on, Ordering::Relaxed);
        if let Some(tap) = lock(&self.tap).as_ref() {
            tap.record(recorder);
        }
        self.recording(app)
            .ok_or_else(|| "only in the dev build".into())
    }

    fn recorder(&self, app: &AppHandle) -> Result<Arc<Recorder>, String> {
        if !dev_tools() {
            return Err("only in the dev build".into());
        }
        let mut recorder = lock(&self.recorder);
        if let Some(recorder) = recorder.as_ref() {
            return Ok(Arc::clone(recorder));
        }
        let saved = Arc::clone(&self.saved);
        let dir = recordings_dir(app);
        let app = app.clone();
        let started = Recorder::start(dir, move |path, count| {
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
        alternatives: r.alternatives.iter().flatten().copied().collect(),
    }
}

/// Call advice for the discard the table's buttons offer, if any.
fn call_view(hand: &[Tile], table: &Snapshot, visible: &TileCounts) -> Option<CallView> {
    let shown = |b: Button| table.buttons.contains(&b);
    let (chi, pon) = (shown(Button::Chi), shown(Button::Pon));
    let win = shown(Button::Ron) || shown(Button::Tsumo);
    let latest = [Seat::Right, Seat::Across, Seat::Left]
        .map(|seat| table.rivers[seat.index()].last().map(|t| t.tile.tile));
    let called = calls::offered(latest, hand, chi, pon, win)?;
    let seats: Vec<&'static str> = ["right", "across", "left"]
        .into_iter()
        .zip(latest)
        .filter(|(_, t)| t.is_some_and(|t| t.kind() == called.kind()))
        .map(|(seat, _)| seat)
        .collect();
    let from = if chi {
        Some("left")
    } else {
        match seats.as_slice() {
            [one] => Some(*one),
            _ => None,
        }
    };
    // The dragons, and the round and seat winds when they are read.
    let mut value_honors = parse_tiles("P F C").unwrap_or_default();
    for wind in [table.round.map(|r| r.wind), table.seat_wind(Seat::Own)]
        .into_iter()
        .flatten()
    {
        if let Ok(tile) = wind.to_string().parse() {
            value_honors.push(tile);
        }
    }
    let melds: Vec<Vec<Tile>> = table.melds[Seat::Own.index()]
        .iter()
        .map(|meld| meld.iter().filter_map(|t| t.tile.map(|r| r.tile)).collect())
        .collect();
    let ctx = CallContext {
        hand,
        melds: &melds,
        value_honors: &value_honors,
        visible,
    };
    calls::advise(&ctx, called, chi, pon)
        .ok()
        .map(|advice| CallView::of(called, from, &advice))
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
/// The game window: the one the player picked if it is still there, read with the first
/// profile; otherwise the first window a profile recognises by its app and title.
fn find_window(profiles: &[Profile], chosen: Option<u32>) -> Option<(WindowInfo, Profile)> {
    let windows = match capture::list_windows() {
        Ok(windows) => windows,
        Err(err) => {
            tracing::warn!("listing windows: {err}");
            return None;
        }
    };
    if let Some(id) = chosen
        && let Some(window) = windows.iter().find(|w| w.id == id)
        && let Some(profile) = profiles.first()
    {
        return Some((window.clone(), profile.clone()));
    }
    windows
        .into_iter()
        .find_map(|w| profile::for_window(profiles, &w).cloned().map(|p| (w, p)))
}

/// A window the player can pick as the game (#17): its app and title only, no picture.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowChoice {
    pub id: u32,
    pub app: String,
    pub title: String,
}

/// The windows on screen, for the player to pick the game from.
pub fn window_choices() -> Vec<WindowChoice> {
    capture::list_windows()
        .unwrap_or_default()
        .into_iter()
        .filter(|w| w.on_screen && !w.title.is_empty())
        .map(|w| WindowChoice {
            id: w.id,
            app: w.app,
            title: w.title,
        })
        .collect()
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
            alternatives: [None; 2],
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
