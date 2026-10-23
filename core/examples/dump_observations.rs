//! Replays mjai logs through libriichi's `PlayerState` and writes Mortal's observation and
//! legal-action mask at every point where the player can act (#22), for checking the
//! ONNX export against PyTorch. The logs and the output stay local.
//!     cargo run -p mahjong-core --release --example dump_observations -- out.bin log1.mjson log2.mjson
//!
//! Output: for each point, 1012 × 34 little-endian f32 (the observation), then 46 bytes
//! (the mask, 0 or 1). The player is the seat whose starting hand is known.

use std::error::Error;
use std::io::Write;

use mahjong_core::game::{Event, PlayerState};

/// Mortal's observation encoding version (Mortal-S42 is v4).
const VERSION: u32 = 4;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let out_path = args.next().ok_or("an output path is needed")?;
    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path)?);
    let mut points = 0usize;
    for path in args {
        let text = std::fs::read_to_string(&path)?;
        let events: Vec<Event> = text
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?;
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
        for event in &events {
            let cans = state.update(event)?;
            if !cans.can_act() {
                continue;
            }
            let (obs, mask) = state.encode_obs(VERSION, false);
            for x in obs.iter() {
                out.write_all(&x.to_le_bytes())?;
            }
            for m in mask.iter() {
                out.write_all(&[u8::from(*m)])?;
            }
            points += 1;
        }
    }
    out.flush()?;
    println!("{points} decision points written to {out_path}");
    Ok(())
}
