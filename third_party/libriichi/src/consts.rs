// Modified by mahjong-copilot on 2026-10-23: removed the Python module registration and the
// pyfunction attributes; the constants are unchanged.
// The original file is libriichi/src/consts.rs in Equim-chan/Mortal at commit
// 0cff2b52982be5b1163aa9a62fb01f03ce91e0d2.

pub const MAX_VERSION: u32 = 4;

pub const ACTION_SPACE: usize = 37 // discard | kan (choice)
                              + 1  // riichi
                              + 3  // chi
                              + 1  // pon
                              + 1  // kan (decide)
                              + 1  // agari
                              + 1  // ryukyoku
                              + 1; // pass
// = 46
pub const GRP_SIZE: usize = 7;

#[inline]
pub const fn obs_shape(version: u32) -> (usize, usize) {
    match version {
        1 => (938, 34),
        2 => (942, 34),
        3 => (934, 34),
        4 => (1012, 34),
        _ => unreachable!(),
    }
}

#[inline]
pub const fn oracle_obs_shape(version: u32) -> (usize, usize) {
    match version {
        1 => (211, 34),
        2 | 3 | 4 => (217, 34),
        _ => unreachable!(),
    }
}
