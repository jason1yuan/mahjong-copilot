//! Game state and decision core. Independent of platform, UI and perception.

pub mod analysis;
pub mod calls;
pub mod defense;
pub mod game;
pub mod mortal;
pub mod scoring;
pub mod target;
pub mod tile;
pub mod yaku;

pub use analysis::{
    AnalysisError, DiscardOption, Outlook, TileCounts, outlook, rank_discards, shanten, unseen,
};
pub use calls::{CallAdvice, CallContext, CallKind, CallOption, Yaku};
pub use tile::{ParseTileError, Tile, parse_tiles};
