//! Watches the game window and prints the hand whenever it changes (#3): the tiles found,
//! what they are, which are in doubt, the game area and how long each step took. Prints
//! text only; nothing is saved.
//!
//! Finds the window through the built-in profiles (e.g. Mahjong Soul in a browser):
//!     cargo run -p mahjong-perception --release --example hand_probe -- 60
//!     cargo run -p mahjong-perception --release --example hand_probe -- 60 --model path/to/model.onnx

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mahjong_perception::capture::{self, CaptureConfig, LatestFrame, RelRect};
use mahjong_perception::change::{ChangeDetector, FrameGate, Region};
use mahjong_perception::hand::HandTracker;
use mahjong_perception::{Doubt, Source, TileClassifier, TileRecognizer, profile};

const DEFAULT_MODEL: &str = "ml/models/mahjong_soul_vision/model.onnx";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut seconds = 60u64;
    let mut model = PathBuf::from(DEFAULT_MODEL);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--model" => model = args.next().ok_or("--model needs a path")?.into(),
            s => seconds = s.parse()?,
        }
    }
    let profiles = profile::builtin()?;
    let (window, profile) = capture::list_windows()?
        .into_iter()
        .find_map(|w| profile::for_window(&profiles, &w).cloned().map(|p| (w, p)))
        .ok_or("no game window found; open the game first")?;
    println!(
        "{} — {}: {:.0}x{:.0} pt, profile {}",
        window.app, window.title, window.width, window.height, profile.id
    );

    let environment = ort::init().build()?;
    let mut recognizer = TileRecognizer::new(TileClassifier::load(&environment, &model)?);
    let latest = Arc::new(LatestFrame::new());
    let session = {
        let latest = Arc::clone(&latest);
        let mut gate = FrameGate::new();
        capture::start(window.id, CaptureConfig::default(), move |frame| {
            if gate.changed(frame) {
                latest.publish(frame);
            }
        })?
    };

    // Until a hand is found, watch the whole window; then only the hand's rows.
    let whole = Region {
        rect: RelRect::FULL,
        cols: 32,
        rows: 18,
    };
    let mut detector = ChangeDetector::new(&[whole]);
    let mut finder = HandTracker::new();
    let mut watched = whole;
    let mut last_hand = String::new();
    let mut seen = 0;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(seconds) {
        let wait = detector
            .next_check()
            .map_or(Duration::from_millis(500), |t| {
                t.saturating_duration_since(Instant::now())
            });
        let Some(frame) = latest.wait_newer(seen, Some(wait.min(Duration::from_millis(500))))
        else {
            if detector.settled(Instant::now()).is_empty() {
                continue;
            }
            // Settled with no new frame: use the latest one.
            let frame = latest.latest();
            let Some(view) = frame.view() else { continue };
            report(
                &view,
                &mut finder,
                &mut recognizer,
                &profile,
                &mut watched,
                &mut detector,
                &mut last_hand,
                start,
            )?;
            continue;
        };
        seen = frame.sequence();
        let Some(view) = frame.view() else { continue };
        detector.observe(&view, Instant::now());
        if !detector.settled(Instant::now()).is_empty() {
            report(
                &view,
                &mut finder,
                &mut recognizer,
                &profile,
                &mut watched,
                &mut detector,
                &mut last_hand,
                start,
            )?;
        }
    }
    let stats = session.stats();
    session.stop()?;
    println!(
        "done: {} frames, {} templates in the cache, stopped by the system: {}",
        stats.frames,
        recognizer.cache().len(),
        stats.stopped
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn report(
    view: &capture::FrameView<'_>,
    finder: &mut HandTracker,
    recognizer: &mut TileRecognizer,
    profile: &profile::Profile,
    watched: &mut Region,
    detector: &mut ChangeDetector,
    last_hand: &mut String,
    start: Instant,
) -> Result<(), Box<dyn std::error::Error>> {
    let t0 = Instant::now();
    let Some(layout) = finder.find(view, profile.tile_face) else {
        if *last_hand != "none" {
            println!(
                "{:7.2}s  no hand found ({}x{} px)",
                start.elapsed().as_secs_f64(),
                view.width(),
                view.height()
            );
            *last_hand = "none".into();
        }
        return Ok(());
    };
    let find_ms = t0.elapsed().as_secs_f64() * 1e3;
    let boxes = layout.tiles();
    let crops: Vec<_> = boxes.iter().map(|b| b.crop(view)).collect();
    let t1 = Instant::now();
    let results = recognizer.recognize(&crops)?;
    let recognize_ms = t1.elapsed().as_secs_f64() * 1e3;
    let names: Vec<String> = results
        .iter()
        .map(|r| match r.doubt {
            None => r.tile.to_string(),
            Some(Doubt::TooSmall) => format!("{}?small", r.tile),
            Some(Doubt::LowConfidence) => format!("{}?", r.tile),
        })
        .collect();
    let hand = names.join(" ");
    if hand != *last_hand {
        let hits = results.iter().filter(|r| r.source == Source::Cache).count();
        let area = layout.game_area(&profile.hand);
        let (w, h) = (view.width() as f32, view.height() as f32);
        println!(
            "{:7.2}s  {} + {}  [{}]",
            start.elapsed().as_secs_f64(),
            layout.concealed.len(),
            usize::from(layout.drawn.is_some()),
            hand
        );
        println!(
            "          tiles {}x{} px at y {}, pitch {:.1}; game area {}; find {:.1} ms, recognize {:.1} ms ({hits} from cache)",
            boxes[0].width(),
            boxes[0].height(),
            boxes[0].top,
            layout.pitch,
            area.map_or("?".into(), |a| format!(
                "{:.0},{:.0} {:.0}x{:.0} of {}x{}",
                a.left, a.top, a.width, a.height, w, h
            )),
            find_ms,
            recognize_ms
        );
        *last_hand = hand;
    }
    // Watch the hand's rows (a little above for raised tiles) from now on.
    let (y0, y1) = layout.band;
    let band = (y1 - y0) as f32;
    let rect = RelRect::new(
        0.0,
        (y0 as f32 - band * 0.3) / h_of(view),
        1.0,
        band * 1.3 / h_of(view),
    );
    if rect != watched.rect {
        *watched = Region {
            rect,
            cols: 48,
            rows: 4,
        };
        *detector = ChangeDetector::new(&[*watched]);
    }
    Ok(())
}

fn h_of(view: &capture::FrameView<'_>) -> f32 {
    view.height() as f32
}
