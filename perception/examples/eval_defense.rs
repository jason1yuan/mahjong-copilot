//! Defense read off the table (#105) on recorded frames: reads each frame as the app does,
//! feeds the state engine, and rates the hand both from the table alone and, when the
//! engine followed the frame, from its log. Counts the frames with a threat, those the
//! engine missed (where only the table's defense is there), the efficiency-only discards
//! the defense changed, and any tile the table calls safe that the log does not. Images
//! and logs stay local.
//!     cargo run -p mahjong-perception --release --example eval_defense -- recordings/live

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};

use mahjong_core::defense::{self, Level, Own};
use mahjong_core::game::{Engine, Event};
use mahjong_core::{Tile, rank_discards};
use mahjong_perception::capture::FrameView;
use mahjong_perception::hand::HandTracker;
use mahjong_perception::numbers::NumberReader;
use mahjong_perception::round::RoundReader;
use mahjong_perception::{TileClassifier, TileRecognizer, profile, snapshot, text};

const DEFAULT_MODEL: &str = "ml/models/mahjong_soul_vision/model.onnx";
/// Frames further apart than this are separate sessions: the game in between was not seen.
const SESSION_GAP_MS: u64 = 10 * 60 * 1000;

fn main() -> Result<(), Box<dyn Error>> {
    let paths: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let profiles = profile::builtin()?;
    let profile = &profiles[0];
    let environment = ort::init().build()?;
    let mut classifier = TileClassifier::load(&environment, Path::new(DEFAULT_MODEL))?;
    classifier.warm_up()?;
    let mut recognizer = TileRecognizer::new(classifier);
    let mut tracker = HandTracker::new();
    let mut rounds = RoundReader::new(text::system());
    let mut numbers = NumberReader::new(text::system());
    let mut engine = Engine::new();
    let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
    let mut last_time = None;

    for path in frames(&paths)? {
        let name = name_of(&path);
        let time: u64 = name
            .split('-')
            .next()
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);
        if last_time.is_some_and(|t: u64| time.saturating_sub(t) > SESSION_GAP_MS) {
            engine = Engine::new();
        }
        last_time = Some(time);
        let rgb = image::open(&path)?.to_rgb8();
        let (w, h) = rgb.dimensions();
        let bgra: Vec<u8> = rgb.pixels().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        let frame = FrameView::new(w, h, w as usize * 4, &bgra, 2.0).ok_or("bad frame")?;
        let Some(layout) = tracker.find(&frame, profile.tile_face) else {
            continue;
        };
        let crops: Vec<_> = layout.tiles().iter().map(|b| b.crop(&frame)).collect();
        let hand_read = recognizer.recognize(&crops)?;
        let Some(table) = snapshot::read(
            &frame,
            profile,
            &layout,
            &mut recognizer,
            &mut rounds,
            &mut numbers,
        )?
        else {
            continue;
        };
        if !snapshot::consistent(&hand_read, &table) {
            continue;
        }
        let scene = table.scene(&hand_read, layout.drawn.is_some());
        if scene.check().is_err() {
            continue;
        }
        *counts.entry("frames").or_default() += 1;
        let taken = engine.observe(&scene).is_ok();
        let hand: Vec<Tile> = hand_read.iter().map(|r| r.tile).collect();
        let mut visible = [0u8; 34];
        for t in scene.seen() {
            visible[usize::from(t.kind())] += 1;
        }
        for t in &hand {
            visible[usize::from(t.kind())] = visible[usize::from(t.kind())].saturating_sub(1);
        }
        let options = rank_discards(&hand, &visible)
            .ok()
            .filter(|_| hand.len() % 3 == 2);
        let own = match options.as_ref().and_then(|o| o.first()) {
            Some(best) => Own {
                shanten: best.shanten,
                ukeire: best.ukeire_count,
                points: None,
            },
            None => match mahjong_core::outlook(&hand, &visible) {
                Ok(o) => Own {
                    shanten: o.shanten,
                    ukeire: o.ukeire_count,
                    points: None,
                },
                Err(_) => continue,
            },
        };
        let read = defense::assess_scene(&scene, &hand, &visible, &own);
        if read.threats.is_empty() {
            continue;
        }
        *counts.entry("with a threat on the table").or_default() += 1;
        if read.threats.iter().any(|t| t.riichi) {
            *counts.entry("with a riichi on the table").or_default() += 1;
        }
        if !taken {
            *counts
                .entry("with a threat, engine not following")
                .or_default() += 1;
        }
        if let Some(options) = &options {
            *counts.entry("with a threat, to discard").or_default() += 1;
            let pick = defense::guarded(options, &read).unwrap_or(0);
            if pick != 0 {
                *counts
                    .entry("with a threat, to discard: defense changed the cut")
                    .or_default() += 1;
                if std::env::var_os("EVAL_DEBUG").is_some() {
                    println!(
                        "{name}: {:?} cuts {} instead of {}",
                        read.verdict, options[pick].discard, options[0].discard
                    );
                }
            }
        }
        // Against the log, when the engine followed this frame.
        let me = engine
            .player_state()
            .filter(|_| taken)
            .map(|s| s.player_id());
        let games = engine.games();
        let (Some(me), Some(game)) = (me, games.last()) else {
            continue;
        };
        let start = game
            .events
            .iter()
            .rposition(|e| matches!(e, Event::StartKyoku { .. }))
            .unwrap_or(0);
        let logged = defense::assess(&game.events[start..], me, &hand, &visible, &own);
        *counts.entry("compared with the log").or_default() += 1;
        let riichi = |d: &defense::Defense| -> Vec<u8> {
            d.threats
                .iter()
                .filter(|t| t.riichi)
                .map(|t| t.seat)
                .collect()
        };
        if riichi(&read) != riichi(&logged) {
            *counts.entry("compared: riichi players differ").or_default() += 1;
            continue;
        }
        for t in read.tiles.iter().filter(|t| t.level == Level::Safe) {
            let in_log = logged.tiles.iter().find(|x| x.tile == t.tile);
            if in_log.is_some_and(|x| x.level != Level::Safe) {
                *counts
                    .entry("compared: safe on the table only")
                    .or_default() += 1;
                println!("{name}: {} safe on the table only", t.tile);
            }
        }
    }
    for (what, n) in &counts {
        println!("{what}: {n}");
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
