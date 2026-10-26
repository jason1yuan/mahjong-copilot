//! The state engine (#10) on recorded frames: reads each frame's hand and table as the
//! app does, in time order, turns them into scenes and feeds them to one engine. Prints
//! what the engine made of each round and how many scenes it took, could not explain or
//! found impossible. Images and logs stay local.
//!     cargo run -p mahjong-perception --release --example eval_engine -- recordings/live
//!     cargo run -p mahjong-perception --release --example eval_engine -- recordings/live --logs /tmp/logs
//!
//! `--logs` writes each game's mjai log (JSON Lines) and its marks there. Recordings are
//! made only when the hand changes, so scenes here are further apart than in the app,
//! which also reads the table when only the table changes.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Instant;

use mahjong_core::game::{Engine, Event, Rejected};
use mahjong_perception::capture::FrameView;
use mahjong_perception::hand::HandTracker;
use mahjong_perception::numbers::NumberReader;
use mahjong_perception::round::RoundReader;
use mahjong_perception::{TileClassifier, TileRecognizer, profile, snapshot, text};

const DEFAULT_MODEL: &str = "ml/models/mahjong_soul_vision/model.onnx";
/// Frames further apart than this are separate sessions: the game in between was not seen.
const SESSION_GAP_MS: u64 = 10 * 60 * 1000;

fn main() -> Result<(), Box<dyn Error>> {
    let mut paths = Vec::new();
    let mut logs = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--logs" => logs = Some(PathBuf::from(args.next().ok_or("--logs needs a folder")?)),
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    if let Some(dir) = &logs {
        std::fs::create_dir_all(dir)?;
    }
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
    let mut times = Vec::new();
    let mut last_time = None;
    let mut session = 0;
    let mut last_taken: Option<String> = None;
    let mut last_scores: Option<(Option<[i32; 4]>, Option<u8>)> = None;

    for path in frames(&paths)? {
        let name = name_of(&path);
        let time: u64 = name
            .split('-')
            .next()
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);
        if last_time.is_some_and(|t: u64| time.saturating_sub(t) > SESSION_GAP_MS) {
            finish(&mut engine, logs.as_deref(), session)?;
            session += 1;
            engine = Engine::new();
        }
        last_time = Some(time);
        let rgb = image::open(&path)?.to_rgb8();
        let (w, h) = rgb.dimensions();
        let bgra: Vec<u8> = rgb.pixels().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        let frame = FrameView::new(w, h, w as usize * 4, &bgra, 2.0).ok_or("bad frame")?;
        let Some(layout) = tracker.find(&frame, profile.tile_face) else {
            *counts.entry("no hand").or_default() += 1;
            continue;
        };
        let crops: Vec<_> = layout.tiles().iter().map(|b| b.crop(&frame)).collect();
        let hand = recognizer.recognize(&crops)?;
        let Some(table) = snapshot::read(
            &frame,
            profile,
            &layout,
            &mut recognizer,
            &mut rounds,
            &mut numbers,
        )?
        else {
            *counts.entry("no table").or_default() += 1;
            continue;
        };
        if !snapshot::consistent(&hand, &table) {
            *counts.entry("table does not add up").or_default() += 1;
            continue;
        }
        let scene = table.scene(&hand, layout.drawn.is_some());
        // As the app's watcher: a hand that does not fit the melds is mid-animation (#39).
        if scene.check() == Err("the hand does not fit the melds") {
            *counts.entry("hand does not fit the melds").or_default() += 1;
            continue;
        }
        let start = Instant::now();
        let result = engine.observe(&scene);
        times.push(start.elapsed().as_secs_f64() * 1e3);
        match result {
            Ok(observed) => {
                if std::env::var_os("EVAL_DEBUG").is_some()
                    && observed
                        .events
                        .iter()
                        .any(|e| matches!(e, Event::StartKyoku { .. }))
                {
                    println!(
                        "    round change: last {:?} -> now {:?} {:?}",
                        last_scores, scene.scores, scene.riichi_sticks
                    );
                }
                if scene.scores.is_some() {
                    last_scores = Some((scene.scores, scene.riichi_sticks));
                }
                last_taken = Some(describe(&scene));
                *counts.entry("taken").or_default() += 1;
                if observed.rewritten {
                    *counts.entry("taken, rewriting the round").or_default() += 1;
                }
                for event in &observed.events {
                    match event {
                        Event::StartKyoku {
                            bakaze,
                            kyoku,
                            honba,
                            ..
                        } => println!(
                            "{name}: start {bakaze}{kyoku} {honba} honba{}",
                            if engine.filled() { " (filled in)" } else { "" }
                        ),
                        Event::StartGame { .. } => println!("{name}: start game"),
                        _ => {}
                    }
                }
                if observed.rewritten {
                    println!("{name}: rewritten");
                }
            }
            Err(Rejected::Impossible(why)) => {
                *counts.entry("impossible").or_default() += 1;
                println!("{name}: impossible: {why}");
            }
            Err(Rejected::NoRound) => *counts.entry("no round").or_default() += 1,
            Err(Rejected::Covered) => {
                let why = match (scene.round.is_some(), scene.dealer.is_some()) {
                    (false, false) => "covered: no round, no dealer",
                    (false, true) => "covered: no round",
                    (true, false) => "covered: no dealer",
                    (true, true) => "covered: read short",
                };
                *counts.entry(why).or_default() += 1;
                // Covered frames are many; each is printed only when debugging.
                if std::env::var_os("EVAL_DEBUG").is_some() {
                    println!("{name}: {why}");
                    println!("    last taken {}", last_taken.as_deref().unwrap_or("-"));
                    println!("    this       {}", describe(&scene));
                }
            }
            Err(Rejected::Unexplained) => {
                *counts.entry("unexplained").or_default() += 1;
                println!("{name}: unexplained");
                if std::env::var_os("EVAL_DEBUG").is_some() {
                    println!("    last taken {}", last_taken.as_deref().unwrap_or("-"));
                    println!("    this       {}", describe(&scene));
                }
            }
        }
    }
    finish(&mut engine, logs.as_deref(), session)?;

    println!();
    for (what, n) in &counts {
        println!("{n:6}  {what}");
    }
    times.sort_by(f64::total_cmp);
    if !times.is_empty() {
        println!(
            "engine: median {:.3} ms, p95 {:.3} ms, max {:.3} ms",
            times[times.len() / 2],
            times[times.len() * 95 / 100],
            times[times.len() - 1]
        );
    }
    Ok(())
}

/// Ends the session's game and prints how its rounds ended; writes its log if asked.
fn finish(engine: &mut Engine, logs: Option<&Path>, session: usize) -> Result<(), Box<dyn Error>> {
    engine.end_game();
    let events = engine.events();
    if events.is_empty() {
        return Ok(());
    }
    let endings: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            Event::Hora { actor, target, .. } if actor == target => {
                Some(format!("tsumo by {actor}"))
            }
            Event::Hora { actor, target, .. } => Some(format!("ron by {actor} off {target}")),
            Event::Ryukyoku { .. } => Some("draw".to_owned()),
            _ => None,
        })
        .collect();
    let marks = engine.marks();
    println!(
        "session {session}: {} events, endings [{}], marks {:?}, corrections {}",
        events.len(),
        endings.join(", "),
        marks
            .iter()
            .map(|m| format!("{:?} {}..{}", m.kind, m.first, m.last))
            .collect::<Vec<_>>(),
        engine.rewrites()
    );
    if let Some(dir) = logs {
        std::fs::write(
            dir.join(format!("session-{session}.mjson")),
            engine.log_lines(),
        )?;
        std::fs::write(
            dir.join(format!("session-{session}.marks.json")),
            serde_json::to_string(&marks)?,
        )?;
    }
    Ok(())
}

/// A scene in one line: hand, rivers, melds, dora, round, tiles left, sticks.
fn describe(scene: &mahjong_core::game::Scene) -> String {
    let tiles =
        |t: &[mahjong_core::Tile]| t.iter().map(|t| t.to_string()).collect::<Vec<_>>().join("");
    let rivers: Vec<String> = scene
        .rivers
        .iter()
        .map(|r| {
            r.iter()
                .map(|t| format!("{}{}", t.tile, if t.sideways { "*" } else { "" }))
                .collect::<Vec<_>>()
                .join("")
        })
        .collect();
    let melds: Vec<String> = scene
        .melds
        .iter()
        .map(|m| {
            m.iter()
                .map(|v| {
                    v.tiles
                        .iter()
                        .map(|t| t.map_or("x".to_owned(), |t| t.to_string()))
                        .collect::<String>()
                        + &v.sideways.map_or(String::new(), |i| format!("@{i}"))
                        + &v.added.map_or(String::new(), |t| format!("+{t}"))
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect();
    format!(
        "hand {}{} | rivers {:?} | melds {:?} | dora {} | {:?} dealer {:?} honba {:?} left {:?} sticks {:?} scores {:?}",
        tiles(&scene.hand),
        scene.drawn.map_or(String::new(), |t| format!(" +{t}")),
        rivers,
        melds,
        tiles(&scene.dora),
        scene.round.map(|r| format!("{}{}", r.wind, r.number)),
        scene.dealer,
        scene.honba,
        scene.tiles_left,
        scene.riichi_sticks,
        scene.scores
    )
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
