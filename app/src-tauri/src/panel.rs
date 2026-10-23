//! The floating panel (#2): on macOS a panel that leaves the game's window focused when
//! clicked; ⌥ Space shows and hides it.
//!
//! Before the window closes (its close button, or the app quitting) it is turned back
//! into a plain window, as tauri-nspanel requires: closing it as a panel made WebKit throw
//! while taking its view off the window, and the app crashed.

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

/// Handles the main window's close request: on macOS, turns the panel back into a window
/// and closes that instead.
pub fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    #[cfg(target_os = "macos")]
    {
        use tauri::Manager;
        let app = window.app_handle();
        if let tauri::WindowEvent::CloseRequested { api, .. } = event
            && macos::unpanel(app, window.label())
        {
            api.prevent_close();
            if let Some(window) = app.get_webview_window(window.label())
                && let Err(err) = window.close()
            {
                tracing::warn!("closing the window: {err}");
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (window, event);
}

/// Turns the panel back into a window before the app quits, so it is not torn down as
/// a panel either.
pub fn before_exit(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    macos::unpanel(app, "main");
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

/// Clicking the app's Dock icon: brings the panel back, from the Dock when it was
/// minimized there.
pub fn reopen(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    macos::bring_back(app);
    #[cfg(not(target_os = "macos"))]
    let _ = app;
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

    /// Turns panel `label` back into a plain window; false when it is not a panel (any
    /// more).
    pub fn unpanel<R: tauri::Runtime>(app: &AppHandle<R>, label: &str) -> bool {
        app.get_webview_panel(label)
            .ok()
            .and_then(|panel| panel.to_window())
            .is_some()
    }

    fn minimized(app: &AppHandle) -> bool {
        app.get_webview_window("main")
            .is_some_and(|w| w.is_minimized().unwrap_or(false))
    }

    /// Shows the panel, restoring it from the Dock first when it was minimized.
    pub fn bring_back(app: &AppHandle) {
        if minimized(app)
            && let Some(window) = app.get_webview_window("main")
            && let Err(err) = window.unminimize()
        {
            tracing::warn!("restoring the panel: {err}");
        }
        if let Ok(panel) = app.get_webview_panel("main") {
            panel.show();
        }
    }

    pub fn toggle(app: &AppHandle) {
        let Ok(panel) = app.get_webview_panel("main") else {
            return;
        };
        // A minimized panel is not visible, and showing it does not bring it back from
        // the Dock: it is restored first.
        if minimized(app) {
            bring_back(app);
            return;
        }
        if panel.is_visible() {
            panel.hide();
        } else {
            panel.show();
        }
    }
}
