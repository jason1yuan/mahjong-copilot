//! Game state and decision core. Independent of platform, UI and perception.

pub mod analysis;
pub mod calls;
pub mod game;
pub mod tile;

pub use analysis::{
    AnalysisError, DiscardOption, Outlook, TileCounts, outlook, rank_discards, shanten,
};
pub use calls::{CallAdvice, CallContext, CallKind, CallOption, Yaku};
pub use tile::{ParseTileError, Tile, parse_tiles};
