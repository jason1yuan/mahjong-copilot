//! The floating panel (#2): on macOS a panel that leaves the game's window focused when
//! clicked; ⌥ Space shows and hides it.

use anyhow::Context;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// Turns the main window into the panel and registers ⌥ Space.
pub fn set_up(app: &AppHandle) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    macos::make_panel(app)?;
    let shortcut = Shortcut::new(Some(Modifiers::ALT), Code::Space);
    app.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(move |app, pressed, event| {
                if *pressed == shortcut && event.state() == ShortcutState::Pressed {
                    toggle(app);
                }
            })
            .build(),
    )?;
    app.global_shortcut()
        .register(shortcut)
        .context("registering ⌥ Space")?;
    Ok(())
}

/// Shows the panel if hidden, hides it if shown.
fn toggle(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    macos::toggle(app);
    #[cfg(not(target_os = "macos"))]
    {
        use tauri::Manager;
        let Some(window) = app.get_webview_window("main") else {
            return;
        };
        let shown = window.is_visible().unwrap_or(true);
        let done = if shown { window.hide() } else { window.show() };
        if let Err(err) = done {
            tracing::warn!("showing or hiding the panel: {err}");
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use anyhow::anyhow;
    use tauri::{AppHandle, Manager};
    use tauri_nspanel::{
        CollectionBehavior, ManagerExt, PanelLevel, StyleMask, WebviewWindowExt, tauri_panel,
    };

    tauri_panel! {
        // Floats above the game, never becomes the key window (the panel takes no typing),
        // and stays when the game is the active app.
        panel!(CopilotPanel {
            config: {
                can_become_key_window: false,
                is_floating_panel: true,
                becomes_key_only_if_needed: true,
                hides_on_deactivate: false
            }
        })
    }

    pub fn make_panel(app: &AppHandle) -> anyhow::Result<()> {
        let window = app
            .get_webview_window("main")
            .ok_or_else(|| anyhow!("no main window"))?;
        let panel = window
            .to_panel::<CopilotPanel>()
            .map_err(|err| anyhow!("making the panel: {err:?}"))?;
        // Clicking it does not activate the app, so the game keeps its focus.
        panel
            .add_style_mask(StyleMask::empty().nonactivating_panel().into())
            .map_err(|err| anyhow!("making the panel non-activating: {err:?}"))?;
        panel.set_level(PanelLevel::Floating.into());
        // Beside the game in any Space, a full-screen browser's included.
        panel.set_collection_behavior(
            CollectionBehavior::new()
                .can_join_all_spaces()
                .full_screen_auxiliary()
                .into(),
        );
        Ok(())
    }

    pub fn toggle(app: &AppHandle) {
        let Ok(panel) = app.get_webview_panel("main") else {
            return;
        };
        if panel.is_visible() {
            panel.hide();
        } else {
            panel.show();
        }
    }
}
