//! Tauri shell: creates the windows and wires perception and core together.
//! Business logic belongs in `mahjong-core` / `mahjong-perception`, not here.

use mahjong_core::{Tile, parse_tiles};

/// Fixed hand for the UI until capture (#1) and recognition (#4) supply real ones.
const SAMPLE_HAND: &str = "1m 2m 3m 4m 5m 6m 7m 8m 3p 4p 5s C C N";

#[tauri::command]
fn sample_hand() -> Result<Vec<Tile>, String> {
    parse_tiles(SAMPLE_HAND).map_err(|err| err.to_string())
}

pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![sample_hand])
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
