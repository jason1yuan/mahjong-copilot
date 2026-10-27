//! Tauri shell: creates the windows and wires perception and core together.
//! Business logic belongs in `mahjong-core` / `mahjong-perception`, not here.

mod devlog;
mod panel;
mod pipeline;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pipeline::{Pipeline, Recording, Status};
use tauri::{AppHandle, Emitter, Manager, State};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Set while a model download runs, so a retry does not start a second one.
static DOWNLOADING: AtomicBool = AtomicBool::new(false);

/// Sent as `model-progress` while a model downloads, for the onboarding screen (#17).
#[derive(Clone, serde::Serialize)]
struct ModelProgress {
    name: String,
    downloaded: u64,
    total: u64,
}

fn models_dir(app: &AppHandle) -> tauri::Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join("models"))
}

/// A model as the dev area shows it, to tell which one a build runs on.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelInfo {
    role: &'static str,
    file: &'static str,
    /// The pinned revision of its Hugging Face repository.
    revision: &'static str,
    size: u64,
    sha256: &'static str,
    /// Downloaded and verified.
    present: bool,
    /// Loaded by the pipeline; `None` where that is not tracked.
    loaded: Option<bool>,
}

/// What this build is and runs on, for the dev area's debug data.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildInfo {
    version: String,
    commit: &'static str,
    /// "release" or "debug".
    profile: &'static str,
    models: Vec<ModelInfo>,
}

/// The build's version and commit, and the models it pins and runs on.
#[tauri::command]
fn build_info(app: AppHandle, pipeline: State<'_, Arc<Pipeline>>) -> Result<BuildInfo, String> {
    let dir = models_dir(&app).map_err(|err| err.to_string())?;
    let info = |role, model: &mahjong_models::ModelFile<'static>, loaded| ModelInfo {
        role,
        file: model.name,
        revision: model
            .url
            .split("/resolve/")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or(""),
        size: model.size,
        sha256: model.sha256,
        present: mahjong_models::is_ready(&dir, model),
        loaded,
    };
    Ok(BuildInfo {
        version: app.package_info().version.to_string(),
        commit: env!("BUILD_COMMIT"),
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        models: vec![
            info("认牌", &mahjong_models::TILE_CLASSIFIER, None),
            info(
                "决策",
                &mahjong_models::DECISION_MODEL,
                Some(pipeline.mortal_loaded()),
            ),
        ],
    })
}

/// Whether every model is downloaded and verified.
#[tauri::command]
fn models_ready(app: AppHandle) -> Result<bool, String> {
    let dir = models_dir(&app).map_err(|err| err.to_string())?;
    Ok(mahjong_models::ALL
        .iter()
        .all(|model| mahjong_models::is_ready(&dir, model)))
}

/// Downloads the missing models in the background (the only time the app goes online),
/// sending `model-progress`, then `models-ready` or `models-error` with the reason, and
/// starts the live pipeline once they are in place. Runs on startup; the onboarding
/// screen calls it again to retry.
#[tauri::command]
fn download_models(app: AppHandle) {
    if DOWNLOADING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let result = fetch_all(&app);
        DOWNLOADING.store(false, Ordering::SeqCst);
        let sent = match result {
            Ok((classifier, mortal)) => {
                app.state::<Arc<Pipeline>>()
                    .start(app.clone(), classifier, mortal);
                app.emit("models-ready", ())
            }
            Err(err) => {
                tracing::error!("model download failed: {err:#}");
                app.emit("models-error", format!("{err:#}"))
            }
        };
        if let Err(err) = sent {
            tracing::warn!("could not tell the window about the models: {err}");
        }
    });
}

/// Fetches every model; returns the tile classifier's path and the decision model's.
/// Progress counts all the models together, so the window shows one bar.
fn fetch_all(app: &AppHandle) -> anyhow::Result<(PathBuf, PathBuf)> {
    let dir = models_dir(app)?;
    let total = mahjong_models::ALL.iter().map(|m| m.size).sum();
    let mut before = 0;
    for model in &mahjong_models::ALL {
        mahjong_models::fetch(&dir, model, &mut |p| {
            let progress = ModelProgress {
                name: model.name.to_owned(),
                downloaded: before + p.downloaded,
                total,
            };
            // Progress is best effort; a closed window must not stop the download.
            let _ = app.emit("model-progress", progress);
        })?;
        before += model.size;
    }
    Ok((
        mahjong_models::path(&dir, &mahjong_models::TILE_CLASSIFIER),
        mahjong_models::path(&dir, &mahjong_models::DECISION_MODEL),
    ))
}

/// What the pipeline is doing; the window asks once on load, then follows
/// `pipeline-status`.
#[tauri::command]
fn pipeline_status(pipeline: State<'_, Arc<Pipeline>>) -> Option<Status> {
    pipeline.status()
}

/// The window drew advice `seq`, or Mortal's decision for it; closes that latency
/// measurement.
#[tauri::command]
fn advice_shown(app: AppHandle, pipeline: State<'_, Arc<Pipeline>>, seq: u64, mortal: bool) {
    pipeline.shown(&app, seq, mortal);
}

/// Whether the app may capture the screen; checking shows no prompt (#17).
#[tauri::command]
fn screen_access() -> bool {
    mahjong_perception::capture::has_screen_access()
}

/// Asks for screen capture: the system's prompt the first time (#17).
#[tauri::command]
fn request_screen_access() -> bool {
    mahjong_perception::capture::request_screen_access()
}

/// Opens System Settings at Screen Recording (#17).
#[tauri::command]
fn open_screen_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
            .spawn()
            .map(|_| ())
            .map_err(|err| err.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    Err("only on macOS".into())
}

/// Starts the app again: a screen recording grant takes effect only then (#17).
#[tauri::command]
fn relaunch(app: AppHandle) {
    app.restart();
}

/// The windows on screen by app and title, to pick the game from (#17). Listing windows
/// asks the window server, so it runs off the main thread, which moves and sizes the panel.
#[tauri::command(async)]
fn window_choices() -> Vec<pipeline::WindowChoice> {
    pipeline::window_choices()
}

/// The game window as it would be found now, `None` when there is none (#17). Off the main
/// thread, as [`window_choices`]; the guide asks every two seconds.
#[tauri::command(async)]
fn game_window(
    pipeline: State<'_, Arc<Pipeline>>,
) -> Result<Option<pipeline::WindowChoice>, String> {
    Ok(pipeline.game_window())
}

/// The player picked the game window (#17).
#[tauri::command]
fn choose_window(pipeline: State<'_, Arc<Pipeline>>, id: u32) {
    pipeline.choose_window(id);
}

/// Pauses (`true`) or resumes recognition (#16).
#[tauri::command]
fn set_paused(pipeline: State<'_, Arc<Pipeline>>, paused: bool) {
    pipeline.set_paused(paused);
}

/// The player says hand tile `index` is `tile` (mjai); advice follows at once (#16).
#[tauri::command]
fn correct_tile(
    app: AppHandle,
    pipeline: State<'_, Arc<Pipeline>>,
    index: usize,
    tile: String,
) -> Result<(), String> {
    let tile = tile.parse().map_err(|err| format!("{err}"))?;
    pipeline.correct(&app, index, tile)
}

/// The dev build's debug recording (#8); `None` in the packaged app, which has none (the
/// packaged dev app does).
#[tauri::command]
fn debug_recording(app: AppHandle, pipeline: State<'_, Arc<Pipeline>>) -> Option<Recording> {
    pipeline.recording(&app)
}

/// Saves the game window's newest frame into the debug folder's `live/` (dev build only)
/// and returns its path. It may wait for earlier frames to be written: off the UI thread.
#[tauri::command]
async fn save_frame(app: AppHandle, pipeline: State<'_, Arc<Pipeline>>) -> Result<String, String> {
    let pipeline = Arc::clone(&pipeline);
    tauri::async_runtime::spawn_blocking(move || pipeline.save_frame(&app))
        .await
        .map_err(|err| err.to_string())?
}

/// Opens the dev tools' debug folder (#80): frames, logs and game logs together.
#[tauri::command]
fn open_dev_folder(app: AppHandle) -> Result<(), String> {
    if !pipeline::dev_tools() {
        return Err("only in the dev build".into());
    }
    let dir = pipeline::debug_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    open_folder(&dir).map_err(|err| err.to_string())
}

#[cfg(target_os = "macos")]
fn open_folder(dir: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new("open")
        .arg(dir)
        .spawn()
        .map(drop)
}

#[cfg(not(target_os = "macos"))]
fn open_folder(_dir: &std::path::Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "opening a folder is macOS only for now",
    ))
}

/// Turns saving each frame that changes the hand on or off (dev build only).
#[tauri::command]
fn set_recording(
    app: AppHandle,
    pipeline: State<'_, Arc<Pipeline>>,
    on: bool,
) -> Result<Recording, String> {
    pipeline.set_recording(&app, on)
}

/// Logs to stderr, and in the dev app also to its log file once [`devlog::start`] runs.
fn init_logging() {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                // ONNX Runtime logs every allocation at info.
                .unwrap_or_else(|_| EnvFilter::new("info,ort=warn")),
        )
        .with(tracing_subscriber::fmt::layer())
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(devlog::DevLog),
        )
        .init();
}

/// Whether this process was started as the text recognition helper (#49).
pub fn is_text_helper() -> bool {
    std::env::args().nth(1).as_deref() == Some(mahjong_perception::text::HELPER_ARG)
}

/// The text recognition helper's whole life, instead of [`run`]: reads labels for the app
/// that started it until the app closes its stdin.
pub fn serve_text_helper() -> std::io::Result<()> {
    init_logging();
    mahjong_perception::text::serve()
}

pub fn run() -> tauri::Result<()> {
    init_logging();
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());
    // Recognition starts paused: the player starts it when a game begins (#16).
    let pipeline = Arc::new(Pipeline::default());
    pipeline.set_paused(true);
    builder
        .manage(pipeline)
        .setup(|app| {
            if pipeline::dev_tools() {
                match devlog::start(&pipeline::debug_dir(app.handle()).join("logs")) {
                    Ok(file) => tracing::info!(
                        file = %file.display(),
                        version = %app.package_info().version,
                        commit = env!("BUILD_COMMIT"),
                        "dev log started"
                    ),
                    Err(err) => tracing::warn!("starting the dev log: {err}"),
                }
            }
            if let Err(err) = panel::set_up(app.handle()) {
                tracing::warn!("setting up the panel: {err:#}");
            }
            download_models(app.handle().clone());
            Ok(())
        })
        .on_window_event(panel::on_window_event)
        .invoke_handler(tauri::generate_handler![
            models_ready,
            download_models,
            pipeline_status,
            advice_shown,
            set_paused,
            correct_tile,
            screen_access,
            request_screen_access,
            open_screen_settings,
            relaunch,
            window_choices,
            game_window,
            choose_window,
            debug_recording,
            save_frame,
            set_recording,
            build_info,
            open_dev_folder
        ])
        .build(tauri::generate_context!())?
        .run(|app, event| match event {
            tauri::RunEvent::ExitRequested { .. } => panel::before_exit(app),
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => panel::reopen(app),
            _ => {}
        });
    Ok(())
}
