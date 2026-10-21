//! Game state and decision core. Independent of platform, UI and perception.

pub mod tile;

pub use tile::{ParseTileError, Tile, parse_tiles};
