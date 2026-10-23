//! Reads the action buttons (#9) of recorded frames and checks them against labels. Images
//! stay local.
//!     cargo run -p mahjong-perception --release --example eval_buttons -- recordings/live
//!     cargo run -p mahjong-perception --release --example eval_buttons -- recordings/live --labels recordings/live/buttons.json
//!
//! Prints each frame where buttons are read, then how often each set of buttons was seen
//! and how long reading took. Labels list the frames that show buttons, left to right;
//! every other frame should show none:
//!
//! ```json
//! {"1792718593003-auto": ["pon", "kan"], "1792707402095-manual": ["chi"]}
//! ```

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Instant;

use mahjong_perception::buttons::{self, Button};
use mahjong_perception::capture::FrameView;
use mahjong_perception::hand::HandTracker;
use mahjong_perception::profile;

fn main() -> Result<(), Box<dyn Error>> {
    let mut paths = Vec::new();
    let mut labels_path = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--labels" => {
                labels_path = Some(PathBuf::from(args.next().ok_or("--labels needs a path")?))
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    let labels: Option<BTreeMap<String, Vec<String>>> = match &labels_path {
        Some(path) => Some(serde_json::from_str(&std::fs::read_to_string(path)?)?),
        None => None,
    };
    let profiles = profile::builtin()?;
    let profile = &profiles[0];
    let layout = profile.buttons.ok_or("the profile has no buttons")?;

    let mut tracker = HandTracker::new();
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    let mut times = Vec::new();
    let (mut checked, mut wrong) = (0u32, Vec::new());
    for path in frames(&paths)? {
        let name = name_of(&path);
        let rgb = image::open(&path)?.to_rgb8();
        let (w, h) = rgb.dimensions();
        let bgra: Vec<u8> = rgb.pixels().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        let frame = FrameView::new(w, h, w as usize * 4, &bgra, 2.0).ok_or("bad frame")?;
        // As in the app: the game area comes from the hand, found at a locked tile size.
        let read = tracker
            .find(&frame, profile.tile_face)
            .and_then(|hand| hand.game_area(&profile.hand))
            .map(|area| {
                let start = Instant::now();
                let read = buttons::read(&frame, &area, &layout);
                times.push(start.elapsed().as_secs_f64() * 1e3);
                read
            })
            .unwrap_or_default();
        let names: Vec<&str> = read.iter().map(|b| Button::as_str(*b)).collect();
        if !names.is_empty() {
            println!("{name}: {}", names.join(" "));
        }
        *seen.entry(names.join(" ")).or_default() += 1;
        if let Some(labels) = &labels {
            checked += 1;
            let want: Vec<&str> = labels
                .get(&name)
                .map(|l| l.iter().map(String::as_str).collect())
                .unwrap_or_default();
            if want != names {
                wrong.push(format!("{name}: read {names:?}, labelled {want:?}"));
            }
        }
    }

    println!();
    for (set, count) in &seen {
        let set = if set.is_empty() { "(none)" } else { set };
        println!("{count:5}  {set}");
    }
    times.sort_by(f64::total_cmp);
    if !times.is_empty() {
        println!(
            "reading the buttons: median {:.2} ms, p95 {:.2} ms, max {:.2} ms",
            times[times.len() / 2],
            times[times.len() * 95 / 100],
            times[times.len() - 1]
        );
    }
    if labels.is_some() {
        println!("labelled frames: {checked}, wrong {}", wrong.len());
        for line in &wrong {
            println!("  {line}");
        }
    }
    Ok(())
}

fn frames(args: &[PathBuf]) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut out = Vec::new();
    for arg in args {
        if arg.is_dir() {
            for entry in std::fs::read_dir(arg)? {
                let path = entry?.path();
                if path.extension().is_some_and(|x| x == "png") {
                    out.push(path);
                }
            }
        } else {
            out.push(arg.clone());
        }
    }
    out.sort();
    Ok(out)
}

fn name_of(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}
