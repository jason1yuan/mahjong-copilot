//! Reads the four rivers (#8) of recorded frames, in time order with one recognizer as the
//! app would, and checks them against labels. Images stay local.
//!     cargo run -p mahjong-perception --release --example eval_river -- recordings/live
//!     cargo run -p mahjong-perception --release --example eval_river -- recordings/live --labels recordings/live/rivers.json --views /tmp/views
//!
//! Each frame prints one line per seat: tiles in discard order, `*` for the sideways riichi
//! tile, `?` for a tile in doubt. `--views` saves each seat's rectified river for checking
//! by eye.
//!
//! Labels give each round's rivers once, as they are in the round's last frame; every
//! earlier frame of the round must show the start of them, which checks every frame
//! without labelling each one. A tile that was called away is listed under `called` with
//! the frames it lay in the river:
//!
//! ```json
//! {"rounds": [{"first": "1792648832246", "last": "1792648886893",
//!   "rivers": {"own": "6z 1s 2p ...", "right": "...", "across": "...", "left": "3m 4m* ..."},
//!   "called": [{"seat": "own", "index": 6, "tile": "5m", "from": "17926495266", "to": "17926495266"}]}]}
//! ```

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Instant;

use image::RgbImage;
use mahjong_perception::capture::FrameView;
use mahjong_perception::hand::HandFinder;
use mahjong_perception::river::{self, RiverTile};
use mahjong_perception::table::{Seat, Table};
use mahjong_perception::{TileClassifier, TileRecognizer, Zone, profile};
use serde::Deserialize;

const DEFAULT_MODEL: &str = "ml/models/mahjong_soul_vision/model.onnx";
const SEAT_NAMES: [&str; 4] = ["own", "right", "across", "left"];

#[derive(Deserialize)]
struct Labels {
    rounds: Vec<Round>,
}

#[derive(Deserialize)]
struct Round {
    /// Names of the round's first and last frames (or the start of them).
    first: String,
    last: String,
    rivers: BTreeMap<String, String>,
    #[serde(default)]
    called: Vec<Called>,
}

#[derive(Deserialize)]
struct Called {
    seat: String,
    index: usize,
    tile: String,
    from: String,
    to: String,
}

/// One expected tile: its name and whether it lies sideways.
type Expected = (String, bool);

fn parse_river(text: &str) -> Vec<Expected> {
    text.split_whitespace()
        .map(|t| match t.strip_suffix('*') {
            Some(name) => (name.to_owned(), true),
            None => (t.to_owned(), false),
        })
        .collect()
}

fn show(tiles: &[RiverTile]) -> String {
    tiles
        .iter()
        .map(|t| {
            format!(
                "{}{}{}",
                t.tile.tile,
                if t.sideways { "*" } else { "" },
                if t.tile.doubt.is_some() { "?" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
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

/// What `seat`'s river should be in frame `name`, if a round's labels cover it.
fn expected_for(labels: &Labels, name: &str, seat: usize) -> Option<Vec<Expected>> {
    let round = labels.rounds.iter().find(|r| {
        name >= r.first.as_str() && name <= r.last.as_str() || name.starts_with(&r.last)
    })?;
    let mut river = parse_river(round.rivers.get(SEAT_NAMES[seat])?);
    for called in &round.called {
        let inside = name >= called.from.as_str()
            && (name <= called.to.as_str() || name.starts_with(&called.to));
        if called.seat == SEAT_NAMES[seat] && inside && called.index <= river.len() {
            river.insert(called.index, (called.tile.clone(), false));
        }
    }
    Some(river)
}

#[derive(Default)]
struct Score {
    tiles: u32,
    wrong: u32,
    wrong_in_doubt: u32,
    doubt: u32,
    sideways_wrong: u32,
    too_long: u32,
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut paths = Vec::new();
    let mut labels_path = None;
    let mut views = None;
    let mut model = PathBuf::from(DEFAULT_MODEL);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--labels" => {
                labels_path = Some(PathBuf::from(args.next().ok_or("--labels needs a path")?))
            }
            "--views" => views = Some(PathBuf::from(args.next().ok_or("--views needs a folder")?)),
            "--model" => model = args.next().ok_or("--model needs a path")?.into(),
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    let labels: Option<Labels> = match &labels_path {
        Some(path) => Some(serde_json::from_str(&std::fs::read_to_string(path)?)?),
        None => None,
    };
    if let Some(dir) = &views {
        std::fs::create_dir_all(dir)?;
    }
    let profiles = profile::builtin()?;
    let profile = &profiles[0];
    let (geometry, layout) = (
        profile.table.ok_or("the profile has no table")?,
        profile.river.ok_or("the profile has no river")?,
    );

    let environment = ort::init().build()?;
    let mut classifier = TileClassifier::load(&environment, &model)?;
    classifier.warm_up()?;
    let mut recognizer = TileRecognizer::new(classifier);
    let mut size = (0, 0);
    let mut scores: [Score; 4] = Default::default();
    let (mut frames_checked, mut frames_right) = (0, 0);
    let (mut find_ms, mut read_ms) = (Vec::new(), Vec::new());

    for path in frames(&paths)? {
        let name = name_of(&path);
        let rgb = image::open(&path)?.to_rgb8();
        let (w, h) = rgb.dimensions();
        if (w, h) != size {
            size = (w, h);
            recognizer.clear_cache();
        }
        let bgra: Vec<u8> = rgb.pixels().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        let frame = FrameView::new(w, h, w as usize * 4, &bgra, 2.0).ok_or("bad frame")?;
        let Some(area) = HandFinder::new()
            .find(&frame, profile.tile_face)
            .and_then(|hand| hand.game_area(&profile.hand))
        else {
            println!("{name}: no hand");
            continue;
        };
        let Some(table) = Table::new(&geometry, &area) else {
            println!("{name}: no table");
            continue;
        };

        let start = Instant::now();
        let mut found = Vec::new();
        for seat in Seat::ALL {
            let view = river::rectify(&frame, &table, &layout, seat);
            let slots = river::find_tiles(&view, &layout, profile.tile_face);
            if let Some(dir) = &views {
                view.image
                    .save(dir.join(format!("{name}-{}.png", SEAT_NAMES[seat.index()])))?;
            }
            found.push((view, slots));
        }
        find_ms.push(start.elapsed().as_secs_f64() * 1e3);
        let start = Instant::now();
        let mut rivers: [Vec<RiverTile>; 4] = Default::default();
        for (seat, (view, slots)) in Seat::ALL.into_iter().zip(&found) {
            let crops: Vec<RgbImage> = slots.iter().map(|s| river::crop(view, s)).collect();
            let tiles = recognizer.recognize_in(Zone::River(seat), &crops)?;
            rivers[seat.index()] = slots
                .iter()
                .zip(tiles)
                .map(|(slot, tile)| RiverTile {
                    tile,
                    sideways: slot.sideways,
                })
                .collect();
        }
        read_ms.push(start.elapsed().as_secs_f64() * 1e3);

        println!("{name}");
        let mut frame_right = true;
        let mut frame_checked = false;
        for seat in 0..4 {
            let mut marks = String::new();
            if let Some(expected) = labels.as_ref().and_then(|l| expected_for(l, &name, seat)) {
                frame_checked = true;
                let score = &mut scores[seat];
                let read = &rivers[seat];
                if read.len() > expected.len() {
                    score.too_long += 1;
                    frame_right = false;
                    marks.push_str(&format!(
                        "  longer than the label ({} > {})",
                        read.len(),
                        expected.len()
                    ));
                }
                for (i, (got, (want, sideways))) in read.iter().zip(&expected).enumerate() {
                    score.tiles += 1;
                    score.doubt += u32::from(got.tile.doubt.is_some());
                    if got.tile.tile.to_string() != *want {
                        score.wrong += 1;
                        score.wrong_in_doubt += u32::from(got.tile.doubt.is_some());
                        frame_right = false;
                        marks.push_str(&format!("  #{i}: {} not {want}", got.tile.tile));
                    }
                    if got.sideways != *sideways {
                        score.sideways_wrong += 1;
                        frame_right = false;
                        marks.push_str(&format!("  #{i}: sideways {}", got.sideways));
                    }
                }
            }
            println!("  {:6} {}{marks}", SEAT_NAMES[seat], show(&rivers[seat]));
        }
        if frame_checked {
            frames_checked += 1;
            frames_right += u32::from(frame_right);
        }
    }

    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    let p95 = |v: &Vec<f64>| {
        v.get((v.len() as f64 * 0.95) as usize)
            .copied()
            .unwrap_or(0.0)
    };
    println!(
        "\nrectify + find, 4 seats: median {:.1} ms, p95 {:.1} ms; recognize: median {:.1} ms, p95 {:.1} ms",
        median(&mut find_ms),
        p95(&find_ms),
        median(&mut read_ms),
        p95(&read_ms)
    );
    if labels.is_some() {
        for (seat, s) in scores.iter().enumerate() {
            println!(
                "{:6} {} tiles, {} wrong ({} of them in doubt), {} in doubt, {} sideways wrong, {} frames too long",
                SEAT_NAMES[seat],
                s.tiles,
                s.wrong,
                s.wrong_in_doubt,
                s.doubt,
                s.sideways_wrong,
                s.too_long
            );
        }
        println!("frames with all four rivers right: {frames_right} of {frames_checked}");
    }
    Ok(())
}
