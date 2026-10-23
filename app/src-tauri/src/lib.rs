//! Tauri shell: creates the windows and wires perception and core together.
//! Business logic belongs in `mahjong-core` / `mahjong-perception`, not here.

mod panel;
mod pipeline;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pipeline::{Pipeline, Recording, Status};
use tauri::{AppHandle, Emitter, Manager, State};
use tracing_subscriber::EnvFilter;

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

/// The decision model (#22), put in the models folder by `ml/convert/export_mortal.py
/// --install`. It is not downloaded until its distribution is settled; without it the
/// panel shows the rule-based advice only.
const LOCAL_MORTAL: &str = "mortal-s42.onnx";

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

/// Fetches every model; returns the tile classifier's path and the decision model's, if
/// it is there.
fn fetch_all(app: &AppHandle) -> anyhow::Result<(PathBuf, Option<PathBuf>)> {
    let dir = models_dir(app)?;
    for model in &mahjong_models::ALL {
        mahjong_models::fetch(&dir, model, &mut |p| {
            let progress = ModelProgress {
                name: model.name.to_owned(),
                downloaded: p.downloaded,
                total: p.total,
            };
            // Progress is best effort; a closed window must not stop the download.
            let _ = app.emit("model-progress", progress);
        })?;
    }
    let mortal = Some(dir.join(LOCAL_MORTAL)).filter(|p| p.is_file());
    Ok((
        mahjong_models::path(&dir, &mahjong_models::TILE_CLASSIFIER),
        mortal,
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

/// The dev build's debug recording (#8); `None` in a packaged app, which has none.
#[tauri::command]
fn debug_recording(pipeline: State<'_, Arc<Pipeline>>) -> Option<Recording> {
    pipeline.recording()
}

/// Saves the game window's newest frame into `recordings/live/` (dev build only).
#[tauri::command]
fn save_frame(app: AppHandle, pipeline: State<'_, Arc<Pipeline>>) -> Result<(), String> {
    pipeline.save_frame(&app)
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

fn init_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                // ONNX Runtime logs every allocation at info.
                .unwrap_or_else(|_| EnvFilter::new("info,ort=warn")),
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
    builder
        .manage(Arc::new(Pipeline::default()))
        .setup(|app| {
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
            debug_recording,
            save_frame,
            set_recording
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
