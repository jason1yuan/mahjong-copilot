//! Call advice (#11) on recorded frames where the chi or pon button shows: reads the hand
//! and table as the app does, works out the discard on offer, and prints every way to call
//! it against not calling. Frames come in time order through one recognizer and hand
//! tracker; only those the button labels mark are printed. Images stay local.
//!     cargo run -p mahjong-perception --release --example eval_calls -- recordings/live --buttons recordings/live/buttons.json

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};

use mahjong_core::calls::{self, CallContext, CallKind};
use mahjong_core::{Tile, parse_tiles};
use mahjong_perception::capture::FrameView;
use mahjong_perception::hand::HandTracker;
use mahjong_perception::numbers::NumberReader;
use mahjong_perception::round::RoundReader;
use mahjong_perception::table::Seat;
use mahjong_perception::{TileClassifier, TileRecognizer, profile, snapshot, text};

const DEFAULT_MODEL: &str = "ml/models/mahjong_soul_vision/model.onnx";

fn main() -> Result<(), Box<dyn Error>> {
    let mut paths = Vec::new();
    let mut buttons_path = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--buttons" => {
                buttons_path = Some(PathBuf::from(args.next().ok_or("--buttons needs a path")?))
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    let labels: BTreeMap<String, Vec<String>> = serde_json::from_str(&std::fs::read_to_string(
        buttons_path.ok_or("--buttons is needed")?,
    )?)?;
    let profiles = profile::builtin()?;
    let profile = &profiles[0];
    let environment = ort::init().build()?;
    let mut classifier = TileClassifier::load(&environment, Path::new(DEFAULT_MODEL))?;
    classifier.warm_up()?;
    let mut recognizer = TileRecognizer::new(classifier);
    let mut tracker = HandTracker::new();
    let mut rounds = RoundReader::new(text::system());
    let mut numbers = NumberReader::new(text::system());
    let dragons = parse_tiles("P F C")?;
    let (mut frames_seen, mut settled, mut advised, mut wins) = (0, 0, 0, 0);

    for path in frames(&paths)? {
        let name = name_of(&path);
        let rgb = image::open(&path)?.to_rgb8();
        let (w, h) = rgb.dimensions();
        let bgra: Vec<u8> = rgb.pixels().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        let frame = FrameView::new(w, h, w as usize * 4, &bgra, 2.0).ok_or("bad frame")?;
        let Some(layout) = tracker.find(&frame, profile.tile_face) else {
            continue;
        };
        let crops: Vec<_> = layout.tiles().iter().map(|b| b.crop(&frame)).collect();
        let hand: Vec<Tile> = recognizer
            .recognize(&crops)?
            .iter()
            .map(|r| r.tile)
            .collect();
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
        let Some(label) = labels.get(&name) else {
            continue;
        };
        let (chi, pon) = (
            label.iter().any(|b| b == "chi"),
            label.iter().any(|b| b == "pon"),
        );
        if !chi && !pon {
            continue;
        }
        frames_seen += 1;
        let latest = [Seat::Right, Seat::Across, Seat::Left]
            .map(|s| table.rivers[s.index()].last().map(|t| t.tile.tile));
        let show = |t: Option<Tile>| t.map_or("-".to_owned(), |t| t.to_string());
        let hand_text: Vec<String> = hand.iter().map(Tile::to_string).collect();
        println!(
            "{name} [{}] hand {} | latest {} {} {}",
            label.join(" "),
            hand_text.join(" "),
            show(latest[0]),
            show(latest[1]),
            show(latest[2]),
        );
        let win = label.iter().any(|b| b == "ron" || b == "tsumo");
        let Some(called) = calls::offered(latest, &hand, chi, pon, win) else {
            if win {
                wins += 1;
                println!("  a win is on offer: no call advice");
            } else {
                println!("  the discard on offer is not settled");
            }
            continue;
        };
        settled += 1;
        let mut value = dragons.clone();
        for wind in [table.round.map(|r| r.wind), table.seat_wind(Seat::Own)]
            .into_iter()
            .flatten()
        {
            value.push(wind.to_string().parse()?);
        }
        let melds: Vec<Vec<Tile>> = table.melds[Seat::Own.index()]
            .iter()
            .map(|m| m.iter().filter_map(|t| t.tile.map(|r| r.tile)).collect())
            .collect();
        let visible = snapshot::seen(&table);
        let ctx = CallContext {
            hand: &hand,
            melds: &melds,
            value_honors: &value,
            visible: &visible,
        };
        let advice = match calls::advise(&ctx, called, chi, pon) {
            Ok(advice) => advice,
            Err(err) => {
                println!("  {called}: {err}");
                continue;
            }
        };
        advised += 1;
        println!(
            "  {called}: stay shanten {} ukeire {}",
            advice.stay.shanten, advice.stay.ukeire_count
        );
        for (i, o) in advice.options.iter().enumerate() {
            let kind = match o.kind {
                CallKind::Chi => "chi",
                CallKind::Pon => "pon",
            };
            println!(
                "  {} {kind} {} {} -> cut {} shanten {} ukeire {} yaku {:?}",
                if advice.take == Some(i) { "*" } else { " " },
                o.with[0],
                o.with[1],
                o.after.discard,
                o.after.shanten,
                o.after.ukeire_count,
                o.yaku
            );
        }
    }
    println!(
        "\nframes with chi or pon: {frames_seen}; discard on offer settled in {settled}; advised in {advised}; with a win on offer {wins}"
    );
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
