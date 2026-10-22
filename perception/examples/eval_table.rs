//! Reads the table (#8) of recorded frames: the four rivers, the four seats' melds and the
//! dora indicators, in time order with one recognizer as the app would, and checks them
//! against labels. Images stay local.
//!     cargo run -p mahjong-perception --release --example eval_table -- recordings/live
//!     cargo run -p mahjong-perception --release --example eval_table -- recordings/live --labels recordings/live/table.json --views /tmp/views
//!
//! Each frame prints one line per seat: river tiles in discard order, then melds in the
//! order they were made (`|` between melds, tiles left to right as their owner sees them);
//! `*` marks a sideways tile, `+X` the tile added to a kan, `x` a tile face down and `?` a
//! tile in doubt. `--views` saves each seat's rectified river and meld band.
//!
//! Labels give each round's table once, as it is in the round's last frame; every earlier
//! frame must show the start of it, which checks every frame without labelling each one. A
//! river tile that was called away is listed under `called` with the frames it lay there,
//! and an added kan tile may still be missing earlier:
//!
//! ```json
//! {"rounds": [{"first": "1792648832246", "last": "1792648886893",
//!   "rivers": {"own": "N E 1s ...", "right": "...", "across": "...", "left": "3m 4m* ..."},
//!   "melds": {"own": "2m* 3m 4m | S S* S", "left": "5m 5mr 5m*"},
//!   "dora": "W N",
//!   "called": [{"seat": "own", "index": 6, "tile": "5m", "from": "17926495266", "to": "17926495266"}]}]}
//! ```

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Instant;

use mahjong_perception::capture::FrameView;
use mahjong_perception::hand::HandFinder;
use mahjong_perception::meld::{self, Meld, MeldTile};
use mahjong_perception::river::{self, RiverTile};
use mahjong_perception::table::{Seat, Table};
use mahjong_perception::{Recognition, TileClassifier, TileRecognizer, dora, profile};
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
    melds: BTreeMap<String, String>,
    #[serde(default)]
    dora: String,
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

fn name(tile: Option<&Recognition>) -> String {
    tile.map_or("x".to_owned(), |t| {
        format!("{}{}", t.tile, if t.doubt.is_some() { "?" } else { "" })
    })
}

fn show_river(tiles: &[RiverTile]) -> String {
    tiles
        .iter()
        .map(|t| {
            format!(
                "{}{}",
                name(Some(&t.tile)),
                if t.sideways { "*" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn show_tile(t: &MeldTile) -> String {
    format!(
        "{}{}{}",
        name(t.tile.as_ref()),
        if t.sideways { "*" } else { "" },
        t.added
            .map_or(String::new(), |a| format!("+{}", name(Some(&a))))
    )
}

fn show_melds(melds: &[Meld]) -> String {
    melds
        .iter()
        .map(|m| m.iter().map(show_tile).collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// A label without the `?` marks, for comparing.
fn plain(text: &str) -> String {
    text.replace('?', "")
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

fn round_of<'a>(labels: &'a Labels, frame: &str) -> Option<&'a Round> {
    labels.rounds.iter().find(|r| {
        frame >= r.first.as_str() && (frame <= r.last.as_str() || frame.starts_with(&r.last))
    })
}

/// What `seat`'s river should be in `frame`: tile names, `*` for sideways.
fn expected_river(round: &Round, frame: &str, seat: usize) -> Option<Vec<String>> {
    let mut river: Vec<String> = round
        .rivers
        .get(SEAT_NAMES[seat])?
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    for called in &round.called {
        let inside = frame >= called.from.as_str()
            && (frame <= called.to.as_str() || frame.starts_with(&called.to));
        if called.seat == SEAT_NAMES[seat] && inside && called.index <= river.len() {
            river.insert(called.index, called.tile.clone());
        }
    }
    Some(river)
}

/// Whether a meld as read matches the label's, which may already hold the added kan tile.
fn meld_matches(read: &str, label: &str) -> bool {
    let read: Vec<&str> = read.split_whitespace().collect();
    let label: Vec<&str> = label.split_whitespace().collect();
    read.len() == label.len()
        && read
            .iter()
            .zip(&label)
            .all(|(r, l)| r == l || (!r.contains('+') && l.split('+').next() == Some(*r)))
}

#[derive(Default)]
struct Score {
    tiles: u32,
    wrong: u32,
    doubt: u32,
}

impl Score {
    fn add(&mut self, other: &Score) {
        self.tiles += other.tiles;
        self.wrong += other.wrong;
        self.doubt += other.doubt;
    }
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
    let geometry = profile.table.ok_or("the profile has no table")?;
    let river_layout = profile.river.ok_or("the profile has no river")?;
    let meld_layout = profile.melds.ok_or("the profile has no melds")?;
    let dora_layout = profile.dora.ok_or("the profile has no dora box")?;
    let face = profile.tile_face;

    let environment = ort::init().build()?;
    let mut classifier = TileClassifier::load(&environment, &model)?;
    classifier.warm_up()?;
    let mut recognizer = TileRecognizer::new(classifier);
    let mut size = (0, 0);
    let (mut rivers_score, mut melds_score, mut dora_score) =
        (Score::default(), Score::default(), Score::default());
    let (mut frames_checked, mut frames_right) = (0, 0);
    let mut read_ms = Vec::new();
    let mut part_ms: [Vec<f64>; 3] = Default::default();

    for path in frames(&paths)? {
        let frame_name = name_of(&path);
        let rgb = image::open(&path)?.to_rgb8();
        let (w, h) = rgb.dimensions();
        if (w, h) != size {
            size = (w, h);
            recognizer.clear_cache();
        }
        let bgra: Vec<u8> = rgb.pixels().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        let frame = FrameView::new(w, h, w as usize * 4, &bgra, 2.0).ok_or("bad frame")?;
        let Some(hand) = HandFinder::new().find(&frame, face) else {
            println!("{frame_name}: no hand");
            continue;
        };
        let Some(area) = hand.game_area(&profile.hand) else {
            continue;
        };
        let Some(table) = Table::new(&geometry, &area) else {
            println!("{frame_name}: no table");
            continue;
        };

        let start = Instant::now();
        let rivers = river::read(&frame, &table, &river_layout, face, &mut recognizer)?;
        let after_rivers = start.elapsed();
        let melds = meld::read(&frame, &table, &meld_layout, face, &mut recognizer)?;
        let after_melds = start.elapsed();
        let indicators = dora::read(
            &frame,
            &area,
            hand.pitch,
            &dora_layout,
            face,
            &mut recognizer,
        )?;
        let ms = |d: std::time::Duration| d.as_secs_f64() * 1e3;
        part_ms[0].push(ms(after_rivers));
        part_ms[1].push(ms(after_melds - after_rivers));
        part_ms[2].push(ms(start.elapsed() - after_melds));
        read_ms.push(start.elapsed().as_secs_f64() * 1e3);
        if let Some(dir) = &views {
            for seat in Seat::ALL {
                let seat_name = SEAT_NAMES[seat.index()];
                river::rectify(&frame, &table, &river_layout, seat)
                    .image
                    .save(dir.join(format!("{frame_name}-{seat_name}-river.png")))?;
                meld::rectify(&frame, &table, &meld_layout, seat)
                    .image
                    .save(dir.join(format!("{frame_name}-{seat_name}-melds.png")))?;
            }
        }

        let dora_text = indicators
            .iter()
            .map(|t| name(Some(t)))
            .collect::<Vec<_>>()
            .join(" ");
        println!("{frame_name}  dora {dora_text}");
        let round = labels.as_ref().and_then(|l| round_of(l, &frame_name));
        let mut right = true;
        for seat in 0..4 {
            let river_text = show_river(&rivers[seat]);
            let meld_text = show_melds(&melds[seat]);
            let mut marks = String::new();
            if let Some(round) = round {
                // Rivers: the start of the label, tile by tile.
                let mut score = Score::default();
                if let Some(want) = expected_river(round, &frame_name, seat) {
                    let got: Vec<String> = plain(&river_text)
                        .split_whitespace()
                        .map(str::to_owned)
                        .collect();
                    if got.len() > want.len() {
                        marks.push_str(&format!(
                            "  river longer than the label ({} > {})",
                            got.len(),
                            want.len()
                        ));
                        score.wrong += 1;
                    }
                    for (i, (g, wnt)) in got.iter().zip(&want).enumerate() {
                        score.tiles += 1;
                        if g != wnt {
                            score.wrong += 1;
                            marks.push_str(&format!("  river #{i}: {g} not {wnt}"));
                        }
                    }
                    score.doubt += river_text.matches('?').count() as u32;
                }
                right &= score.wrong == 0;
                rivers_score.add(&score);

                // Melds: the first ones of the label, meld by meld.
                let mut score = Score::default();
                let want: Vec<&str> = round.melds.get(SEAT_NAMES[seat]).map_or(Vec::new(), |m| {
                    m.split('|')
                        .map(str::trim)
                        .filter(|m| !m.is_empty())
                        .collect()
                });
                let got_text = plain(&meld_text);
                let got: Vec<&str> = got_text
                    .split('|')
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                    .collect();
                if got.len() > want.len() {
                    marks.push_str(&format!(
                        "  more melds than the label ({} > {})",
                        got.len(),
                        want.len()
                    ));
                    score.wrong += 1;
                }
                for (i, (g, wnt)) in got.iter().zip(&want).enumerate() {
                    score.tiles += g.split_whitespace().count() as u32;
                    if !meld_matches(g, wnt) {
                        score.wrong += 1;
                        marks.push_str(&format!("  meld #{i}: {g} not {wnt}"));
                    }
                }
                score.doubt += meld_text.matches('?').count() as u32;
                right &= score.wrong == 0;
                melds_score.add(&score);
            }
            println!(
                "  {:6} {}{}{}{marks}",
                SEAT_NAMES[seat],
                river_text,
                if meld_text.is_empty() { "" } else { "  ||  " },
                meld_text
            );
        }
        if let Some(round) = round {
            let want: Vec<&str> = round.dora.split_whitespace().collect();
            let got_text = plain(&dora_text);
            let got: Vec<&str> = got_text.split_whitespace().collect();
            let mut score = Score {
                tiles: got.len() as u32,
                wrong: 0,
                doubt: dora_text.matches('?').count() as u32,
            };
            if got.len() > want.len()
                || got.iter().zip(&want).any(|(g, w)| g != w)
                || got.is_empty()
            {
                score.wrong += 1;
                println!("  dora {got_text} not the start of {}", round.dora);
            }
            right &= score.wrong == 0;
            dora_score.add(&score);
            frames_checked += 1;
            frames_right += u32::from(right);
        }
    }

    for (part, times) in ["rivers", "melds", "dora"].iter().zip(&mut part_ms) {
        times.sort_by(f64::total_cmp);
        let median = times.get(times.len() / 2).copied().unwrap_or(0.0);
        println!("{part}: median {median:.1} ms");
    }
    read_ms.sort_by(f64::total_cmp);
    let at = |q: f64| {
        read_ms
            .get(((read_ms.len() as f64 - 1.0) * q).round() as usize)
            .copied()
            .unwrap_or(0.0)
    };
    println!(
        "\nread rivers, melds and dora: median {:.1} ms, p95 {:.1} ms",
        at(0.5),
        at(0.95)
    );
    if labels.is_some() {
        for (what, s) in [
            ("rivers", &rivers_score),
            ("melds", &melds_score),
            ("dora", &dora_score),
        ] {
            println!(
                "{what:6} {} tiles, {} wrong, {} in doubt",
                s.tiles, s.wrong, s.doubt
            );
        }
        println!("frames with the whole table right: {frames_right} of {frames_checked}");
    }
    Ok(())
}
