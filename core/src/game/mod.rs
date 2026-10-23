//! The game state engine (#10): settled scenes from the screen in, mjai events out, kept
//! in libriichi's `PlayerState` for the advice and Mortal.

mod ending;
mod engine;
mod scene;
#[cfg(test)]
pub(crate) mod sim;

pub use ending::{Ending, ending};
pub use engine::{Engine, GameLog, Mark, MarkKind, Observed, Rejected};
pub use riichi::mjai::Event;
pub use riichi::state::PlayerState;
pub use scene::{MeldView, RiverTile, RoundLabel, Scene};
