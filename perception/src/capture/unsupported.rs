//! Platforms without window capture yet (Windows comes in P2).

use std::sync::Arc;

use super::{CaptureConfig, CaptureError, Counters, Handler, WindowInfo};

pub(super) struct Session;

impl Session {
    pub(super) fn stop(self) -> Result<(), CaptureError> {
        Ok(())
    }
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
