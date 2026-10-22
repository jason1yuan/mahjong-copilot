//! Game state and decision core. Independent of platform, UI and perception.

pub mod analysis;
pub mod tile;

pub use analysis::{
    AnalysisError, DiscardOption, Outlook, TileCounts, outlook, rank_discards, shanten,
};
pub use tile::{ParseTileError, Tile, parse_tiles};
