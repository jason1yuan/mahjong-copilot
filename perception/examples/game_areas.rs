//! Prints the game area found from the hand (#3) in each frame, as JSON for the table
//! calibration (`ml/calibrate/table.py`, #8). Images stay local.
//!     cargo run -p mahjong-perception --release --example game_areas -- recordings/live > /tmp/areas.json

use std::error::Error;
use std::path::PathBuf;

use mahjong_perception::capture::FrameView;
use mahjong_perception::hand::HandFinder;
use mahjong_perception::profile;
use serde_json::json;

fn main() -> Result<(), Box<dyn Error>> {
    let mut paths = Vec::new();
    for arg in std::env::args().skip(1).map(PathBuf::from) {
        if arg.is_dir() {
            for entry in std::fs::read_dir(&arg)? {
                let path = entry?.path();
                if path.extension().is_some_and(|x| x == "png") {
                    paths.push(path);
                }
            }
        } else {
            paths.push(arg);
        }
    }
    paths.sort();
    let profiles = profile::builtin()?;
    let profile = &profiles[0];
    let mut areas = Vec::new();
    for path in paths {
        let rgb = image::open(&path)?.to_rgb8();
        let (w, h) = rgb.dimensions();
        let bgra: Vec<u8> = rgb.pixels().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        let frame = FrameView::new(w, h, w as usize * 4, &bgra, 2.0).ok_or("bad frame")?;
        if let Some(area) = HandFinder::new()
            .find(&frame, profile.tile_face)
            .and_then(|hand| hand.game_area(&profile.hand))
        {
            areas.push(json!({
                "path": path,
                "left": area.left,
                "top": area.top,
                "width": area.width,
                "height": area.height,
            }));
        }
    }
    println!("{}", serde_json::to_string_pretty(&areas)?);
    Ok(())
}
