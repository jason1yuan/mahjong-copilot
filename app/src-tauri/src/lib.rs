//! Tauri shell: creates the windows and wires perception and core together.
//! Business logic belongs in `mahjong-core` / `mahjong-perception`, not here.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use mahjong_core::{Tile, parse_tiles};
use tauri::{AppHandle, Emitter, Manager};

/// Fixed hand for the UI until capture (#1) and recognition (#4) supply real ones.
const SAMPLE_HAND: &str = "1m 2m 3m 4m 5m 6m 7m 8m 3p 4p 5s C C N";

/// Set while a model download runs, so a retry does not start a second one.
static DOWNLOADING: AtomicBool = AtomicBool::new(false);

/// Sent as `model-progress` while a model downloads, for the onboarding screen (#17).
#[derive(Clone, serde::Serialize)]
struct ModelProgress {
    name: String,
    downloaded: u64,
    total: u64,
}

#[tauri::command]
fn sample_hand() -> Result<Vec<Tile>, String> {
    parse_tiles(SAMPLE_HAND).map_err(|err| err.to_string())
}

fn models_dir(app: &AppHandle) -> tauri::Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join("models"))
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
/// sending `model-progress`, then `models-ready` or `models-error` with the reason. Runs
/// on startup; the onboarding screen calls it again to retry.
#[tauri::command]
fn download_models(app: AppHandle) {
    if DOWNLOADING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let result = fetch_all(&app);
        DOWNLOADING.store(false, Ordering::SeqCst);
        let sent = match result {
            Ok(()) => app.emit("models-ready", ()),
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

fn fetch_all(app: &AppHandle) -> anyhow::Result<()> {
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
    Ok(())
}

pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .setup(|app| {
            download_models(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            sample_hand,
            models_ready,
            download_models
        ])
        .run(tauri::generate_context!())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_hand_is_valid() {
        assert_eq!(sample_hand().unwrap().len(), 14);
    }
}
