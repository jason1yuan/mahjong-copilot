//! Platforms without window capture yet (Windows comes in P2).

use std::sync::Arc;

use super::{CaptureConfig, CaptureError, Counters, Handler, WindowInfo};

pub(super) struct Session;

impl Session {
    pub(super) fn stop(self) -> Result<(), CaptureError> {
        Ok(())
    }
}

/// No permission to ask for where there is no capture yet.
pub(super) fn has_screen_access() -> bool {
    true
}

pub(super) fn request_screen_access() -> bool {
    true
}

pub(super) fn list_windows() -> Result<Vec<WindowInfo>, CaptureError> {
    Err(CaptureError::Unsupported)
}

pub(super) fn start(
    _window_id: u32,
    _config: CaptureConfig,
    _handler: Handler,
    _counters: Arc<Counters>,
) -> Result<Session, CaptureError> {
    Err(CaptureError::Unsupported)
}
