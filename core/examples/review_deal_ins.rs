//! Reviews the player's deal-ins in saved game logs (#12): for each win off the player's
//! discard, replays the log to that discard and prints what Mortal would have played (its
//! best three discards) and how the defense rated the tile discarded and the hand. Logs
//! and the model stay local.
//!     cargo run -p mahjong-core --release --example review_deal_ins -- ml/models/mortal-s42/model.onnx log.mjson...

use std::error::Error;
use std::path::Path;

use mahjong_core::defense::{self, Own};
use mahjong_core::game::{Event, PlayerState};
use mahjong_core::mortal::{Kind, Mortal};
use mahjong_core::{Tile, parse_tiles};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let model = args.next().ok_or("a model path is needed")?;
    let environment = ort::init().build()?;
    let mut mortal = Mortal::load(&environment, Path::new(&model))?;
    for path in args {
        let text = std::fs::read_to_string(&path)?;
        let events: Vec<Event> = text
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?;
        review(&mut mortal, &path, &events)?;
    }
    Ok(())
}

fn hand_of(state: &PlayerState) -> Vec<Tile> {
    let mut out = Vec::new();
    let akas = state.akas_in_hand();
    for (k, &n) in state.tehai().iter().enumerate() {
        for i in 0..n {
            let mut t = Tile::from_kind(k as u8).unwrap_or_else(|| unreachable!("k < 34"));
            // The first five of a suit is the red one, when the hand holds it.
            if i == 0 && matches!(k, 4 | 13 | 22) && akas[k / 9] {
                t = t.to_red().unwrap_or(t);
            }
            out.push(t);
        }
    }
    out
}

fn review(mortal: &mut Mortal, path: &str, events: &[Event]) -> Result<(), Box<dyn Error>> {
    let me = events
        .iter()
        .find_map(|e| match e {
            Event::StartKyoku { tehais, .. } => tehais
                .iter()
                .position(|t| !t[0].is_unknown())
                .map(|p| p as u8),
            _ => None,
        })
        .ok_or("no known starting hand")?;
    let mut state = PlayerState::new(me);
    let mut round_start = 0;
    // The player's last discard decision: its state, index and the tile discarded.
    let mut last: Option<(PlayerState, usize, String)> = None;
    for (i, event) in events.iter().enumerate() {
        if matches!(event, Event::StartKyoku { .. }) {
            round_start = i;
        }
        if let Event::Dahai { actor, pai, .. } = event
            && *actor == me
        {
            last = Some((state.clone(), i, pai.to_string()));
        }
        if let Event::Hora { actor, target, .. } = event
            && *target == me
            && *actor != me
            && let Some((before, at, tile)) = &last
        {
            println!("{path}: event {i}, seat {actor} wins off the player's {tile} (event {at})");
            let hand = hand_of(before);
            let names: Vec<String> = hand.iter().map(Tile::to_string).collect();
            println!("  hand {} | shanten {}", names.join(" "), before.shanten());
            match mortal.decide(before) {
                Ok(d) => {
                    let top: Vec<String> = d
                        .choices
                        .iter()
                        .map(|c| c.suggestion())
                        .filter(|s| s.kind == Kind::Discard)
                        .take(3)
                        .map(|s| s.tile.map_or("?".into(), |t| t.to_string()))
                        .collect();
                    let best = d.best().suggestion();
                    println!(
                        "  Mortal: {:?} {} | discards best first: {}",
                        best.kind,
                        best.tile.map_or(String::new(), |t| t.to_string()),
                        top.join(" ")
                    );
                }
                Err(err) => println!("  Mortal: {err}"),
            }
            let mut visible = [0u8; 34];
            for e in &events[round_start..*at] {
                let tiles: Vec<String> = match e {
                    Event::Dahai { pai, .. } => vec![pai.to_string()],
                    Event::Dora { dora_marker } => vec![dora_marker.to_string()],
                    Event::StartKyoku { dora_marker, .. } => vec![dora_marker.to_string()],
                    Event::Chi { consumed, .. } | Event::Pon { consumed, .. } => {
                        consumed.iter().map(|t| t.to_string()).collect()
                    }
                    Event::Daiminkan { consumed, .. } | Event::Kakan { consumed, .. } => {
                        consumed.iter().map(|t| t.to_string()).collect()
                    }
                    Event::Ankan { consumed, .. } => {
                        consumed.iter().map(|t| t.to_string()).collect()
                    }
                    _ => Vec::new(),
                };
                for t in tiles {
                    if let Ok(t) = parse_tiles(&t) {
                        visible[usize::from(t[0].kind())] += 1;
                    }
                }
            }
            let own = Own {
                shanten: before.shanten(),
                ukeire: 0,
                points: None,
            };
            let d = defense::assess(&events[round_start..*at], me, &hand, &visible, &own);
            let threats: Vec<String> = d
                .threats
                .iter()
                .map(|t| {
                    format!(
                        "seat +{} {}",
                        t.seat,
                        if t.riichi { "riichi" } else { "open" }
                    )
                })
                .collect();
            let rated: Vec<String> = d
                .tiles
                .iter()
                .map(|t| format!("{}:{:?}/{:?}", t.tile, t.level, t.reason))
                .collect();
            println!(
                "  defense: threats [{}] verdict {:?} safest {:?}",
                threats.join(", "),
                d.verdict,
                d.safest.map(|t| t.to_string())
            );
            if !rated.is_empty() {
                println!("  tiles {}", rated.join(" "));
            }
        }
        state.update(event)?;
    }
    Ok(())
}
