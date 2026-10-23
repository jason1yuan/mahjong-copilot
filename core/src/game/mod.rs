//! The game state engine (#10): settled scenes from the screen in, mjai events out, kept
//! in libriichi's `PlayerState` for the advice and Mortal.

mod engine;
mod scene;
#[cfg(test)]
mod sim;

pub use engine::{Engine, Observed, Rejected};
pub use scene::{MeldView, RiverTile, RoundLabel, Scene};
