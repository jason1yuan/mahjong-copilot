// Modified by mahjong-copilot on 2026-10-22: removed the Python module registration
// and the bot module (the Python-facing Mortal bot).
// The original file is libriichi/src/mjai/mod.rs in Equim-chan/Mortal at commit
// 0cff2b52982be5b1163aa9a62fb01f03ce91e0d2.

mod event;

pub use event::{Event, EventExt, EventWithCanAct, Metadata, OutOfBoundError};
