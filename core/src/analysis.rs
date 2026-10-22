//! Hand efficiency: shanten, effective draws (ukeire) and discard ranking.
//! Shanten numbers come from libriichi's lookup tables (`third_party/libriichi`).

use riichi::algo::shanten;
use serde::Serialize;

use crate::tile::{KINDS, Tile};

/// Tile counts indexed by [`Tile::kind`], red fives folded into their kind.
pub type TileCounts = [u8; KINDS as usize];

/// Largest concealed hand: four groups plus a pair.
const MAX_GROUPS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AnalysisError {
    #[error("a concealed hand must have 3n+1 or 3n+2 tiles (n <= 4), got {0}")]
    BadHandSize(usize),
    #[error("a discard needs a hand of 3n+2 tiles (n <= 4), got {0}")]
    NotDiscardSize(usize),
    #[error("more than 4 copies of {0} between the hand and the visible tiles")]
    TooManyCopies(Tile),
}

/// One possible discard and what the hand looks like afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscardOption {
    pub discard: Tile,
    /// Shanten after the discard; 0 is tenpai.
    pub shanten: i8,
    /// Tile kinds that would lower the shanten and still have unseen copies.
    pub ukeire: Vec<Tile>,
    /// Unseen copies of those kinds, after subtracting the hand and `visible`.
    pub ukeire_count: u8,
}

/// Where a 3n+1 hand stands while waiting for its next draw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Outlook {
    /// 0 is tenpai.
    pub shanten: i8,
    /// Tile kinds that would lower the shanten and still have unseen copies.
    pub ukeire: Vec<Tile>,
    /// Unseen copies of those kinds, after subtracting the hand and `visible`.
    pub ukeire_count: u8,
}

/// Decompresses the shanten tables, which otherwise happens on first use.
pub fn warm_up() {
    shanten::ensure_init();
}

pub fn counts(tiles: &[Tile]) -> TileCounts {
    let mut counts = [0; KINDS as usize];
    for tile in tiles {
        counts[usize::from(tile.kind())] += 1;
    }
    counts
}

/// Shanten of a concealed hand of 3n+1 or 3n+2 tiles, considering regular hands,
/// seven pairs and thirteen orphans. -1 means the hand is already complete.
pub fn shanten(hand: &[Tile]) -> Result<i8, AnalysisError> {
    let groups = groups_for(hand.len()).ok_or(AnalysisError::BadHandSize(hand.len()))?;
    let counts = counts(hand);
    check_copies(&counts, &[0; KINDS as usize])?;
    Ok(shanten::calc_all(&counts, groups))
}

/// Ranks every distinct discard of a 3n+2 concealed hand: lower shanten first, then
/// more unseen effective draws. `visible` holds tiles seen outside the hand (all
/// discards, melds, dora indicators) and only affects the unseen counts.
pub fn rank_discards(
    hand: &[Tile],
    visible: &TileCounts,
) -> Result<Vec<DiscardOption>, AnalysisError> {
    if hand.len() % 3 != 2 {
        return Err(AnalysisError::NotDiscardSize(hand.len()));
    }
    let groups = groups_for(hand.len()).ok_or(AnalysisError::NotDiscardSize(hand.len()))?;
    let mut counts = counts(hand);
    check_copies(&counts, visible)?;

    let mut discards: Vec<Tile> = hand.to_vec();
    discards.sort();
    discards.dedup();

    let mut options: Vec<DiscardOption> = discards
        .into_iter()
        .map(|discard| {
            let kind = usize::from(discard.kind());
            counts[kind] -= 1;
            let option = evaluate(discard, &mut counts, visible, groups);
            counts[kind] += 1;
            option
        })
        .collect();

    // Ties keep the cheaper tile first: a plain five ahead of its red five.
    options.sort_by_key(|o| {
        (
            o.shanten,
            std::cmp::Reverse(o.ukeire_count),
            o.discard.is_red(),
            o.discard.kind(),
        )
    });
    Ok(options)
}

/// Shanten and effective draws of a 3n+1 concealed hand waiting for its next tile, e.g.
/// on another player's turn. `visible` only affects the unseen counts.
pub fn outlook(hand: &[Tile], visible: &TileCounts) -> Result<Outlook, AnalysisError> {
    if hand.len() % 3 != 1 {
        return Err(AnalysisError::BadHandSize(hand.len()));
    }
    let groups = groups_for(hand.len()).ok_or(AnalysisError::BadHandSize(hand.len()))?;
    let mut counts = counts(hand);
    check_copies(&counts, visible)?;
    let option = evaluate(hand[0], &mut counts, visible, groups);
    Ok(Outlook {
        shanten: option.shanten,
        ukeire: option.ukeire,
        ukeire_count: option.ukeire_count,
    })
}

/// Shanten and ukeire of the 3n+1 hand left in `counts` after `discard`.
fn evaluate(
    discard: Tile,
    counts: &mut TileCounts,
    visible: &TileCounts,
    groups: u8,
) -> DiscardOption {
    let current = shanten::calc_all(counts, groups);
    let mut ukeire = Vec::new();
    let mut ukeire_count = 0;
    for kind in 0..KINDS {
        let k = usize::from(kind);
        let unseen = 4u8.saturating_sub(counts[k] + visible[k]);
        if unseen == 0 {
            continue;
        }
        counts[k] += 1;
        let improves = shanten::calc_all(counts, groups) < current;
        counts[k] -= 1;
        if improves && let Some(tile) = Tile::from_kind(kind) {
            ukeire.push(tile);
            ukeire_count += unseen;
        }
    }
    DiscardOption {
        discard,
        shanten: current,
        ukeire,
        ukeire_count,
    }
}

/// Number of groups a concealed hand of `len` tiles is built towards.
fn groups_for(len: usize) -> Option<u8> {
    let groups = len / 3;
    (!len.is_multiple_of(3) && groups <= MAX_GROUPS).then_some(groups as u8)
}

fn check_copies(hand: &TileCounts, visible: &TileCounts) -> Result<(), AnalysisError> {
    for kind in 0..KINDS {
        let k = usize::from(kind);
        if hand[k] + visible[k] > 4 {
            return Err(AnalysisError::TooManyCopies(
                Tile::from_kind(kind).unwrap_or_else(|| unreachable!("kind < KINDS")),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::parse_tiles;

    fn hand(s: &str) -> Vec<Tile> {
        parse_tiles(s).unwrap()
    }

    fn tile(s: &str) -> Tile {
        s.parse().unwrap()
    }

    const NONE_VISIBLE: TileCounts = [0; KINDS as usize];

    // Cases taken from libriichi's own shanten tests, rewritten in mjai notation.
    #[test]
    fn shanten_matches_libriichi_reference_hands() {
        let cases = [
            ("1m 1m 1m 1m 3p 3p 3p 2s 2s 2s N N N", 1),
            ("1m 4m 7m 2p 5p 8p 3s 6s 9s E S W N", 6),
            ("1m 9m 1p 9p 1s 5s 9s E S W N P F", 1),
            ("2m 3m 4m 4m 4m 5m 6m 1p 4p 1s 2s 7s S 7p", 3),
            ("2m 3m 4m 4m 4m 5m 6m 1p 4p 1s 2s 7s S 5p", 2),
            ("1m 2m 2m 3m 3m 4m 6p 7p 8p 3s 7s S S 5s", 0),
            ("1m 2m 2m 2m 3m 4m 5m 6m 7p 8p 8p 8p 9p 2m", -1),
            ("3p 4p 7p 7p 8p", 0),
            ("5m 5m", -1),
        ];
        for (tiles, expected) in cases {
            assert_eq!(shanten(&hand(tiles)).unwrap(), expected, "{tiles}");
        }
    }

    #[test]
    fn outlook_of_a_waiting_hand() {
        let ready = outlook(&hand("1m 2m 3m 4p 5p 6p 7s 8s 9s E E 5m 6m"), &NONE_VISIBLE).unwrap();
        assert_eq!(ready.shanten, 0);
        assert_eq!(ready.ukeire, hand("4m 7m"));
        assert_eq!(ready.ukeire_count, 8);
        // Seen elsewhere, two 4m are no longer drawable.
        let mut seen = NONE_VISIBLE;
        seen[usize::from(tile("4m").kind())] = 2;
        let ready = outlook(&hand("1m 2m 3m 4p 5p 6p 7s 8s 9s E E 5m 6m"), &seen).unwrap();
        assert_eq!(ready.ukeire_count, 6);
        assert_eq!(
            outlook(&hand("1m 2m 3m 4p"), &NONE_VISIBLE)
                .unwrap()
                .shanten,
            0,
            "a single-tile wait after three melds"
        );
        assert!(matches!(
            outlook(&hand("1m 2m"), &NONE_VISIBLE),
            Err(AnalysisError::BadHandSize(2))
        ));
    }

    #[test]
    fn isolated_honor_is_the_only_tenpai_discard() {
        let options = rank_discards(
            &hand("1m 2m 3m 4p 5p 6p 7s 8s 9s 2m 3m 5p 5p N"),
            &NONE_VISIBLE,
        )
        .unwrap();
        let best = &options[0];
        assert_eq!(best.discard, tile("N"));
        assert_eq!(best.shanten, 0);
        assert_eq!(best.ukeire, hand("1m 4m"));
        // One 1m is already in the hand, so 3 + 4 copies are left.
        assert_eq!(best.ukeire_count, 7);
        assert!(options[1..].iter().all(|o| o.shanten > 0));
    }

    #[test]
    fn visible_tiles_reduce_ukeire() {
        let mut visible = NONE_VISIBLE;
        visible[usize::from(tile("4m").kind())] = 4;
        visible[usize::from(tile("1m").kind())] = 1;
        let best =
            &rank_discards(&hand("1m 2m 3m 4p 5p 6p 7s 8s 9s 2m 3m 5p 5p N"), &visible).unwrap()[0];
        assert_eq!(best.discard, tile("N"));
        assert_eq!(best.ukeire, hand("1m"));
        assert_eq!(best.ukeire_count, 2);
    }

    #[test]
    fn prefers_the_wider_wait() {
        // 6s and 3s both keep tenpai: 34s waits on 2s/5s (8 tiles), 46s only on 5s (4).
        let options = rank_discards(
            &hand("1m 2m 3m 4p 5p 6p 7s 8s 9s E E 3s 4s 6s"),
            &NONE_VISIBLE,
        )
        .unwrap();
        // 9s works as well as 6s: 6-7-8s becomes the group and 34s still waits on 2s/5s.
        for (option, discard) in options.iter().zip(["6s", "9s"]) {
            assert_eq!(option.discard, tile(discard));
            assert_eq!(option.shanten, 0);
            assert_eq!(option.ukeire, hand("2s 5s"));
            assert_eq!(option.ukeire_count, 8);
        }
        assert_eq!(options[2].discard, tile("3s"));
        assert_eq!(options[2].shanten, 0);
        assert_eq!(options[2].ukeire, hand("5s"));
        assert_eq!(options[2].ukeire_count, 4);
        assert!(options[3..].iter().all(|o| o.shanten > 0));
    }

    #[test]
    fn plain_five_is_discarded_before_red_five() {
        let options = rank_discards(
            &hand("5m 5mr 1p 2p 3p 4s 5s 6s 7s 8s 9s E E N"),
            &NONE_VISIBLE,
        )
        .unwrap();
        let fives: Vec<Tile> = options
            .iter()
            .map(|o| o.discard)
            .filter(|t| t.kind() == tile("5m").kind())
            .collect();
        assert_eq!(fives, vec![tile("5m"), tile("5mr")]);
    }

    #[test]
    fn rejects_bad_sizes_and_impossible_counts() {
        assert_eq!(
            rank_discards(
                &hand("1m 2m 3m 4m 5m 6m 7m 8m 9m 1p 2p 3p 4p"),
                &NONE_VISIBLE
            ),
            Err(AnalysisError::NotDiscardSize(13))
        );
        assert_eq!(
            shanten(&hand("1m 2m 3m")),
            Err(AnalysisError::BadHandSize(3))
        );
        assert_eq!(
            shanten(&hand("1m 1m 1m 1m 1m 2m 3m 4m 5m 6m 7m 8m 9m 9m")),
            Err(AnalysisError::TooManyCopies(tile("1m")))
        );
        let mut visible = NONE_VISIBLE;
        visible[usize::from(tile("E").kind())] = 3;
        assert_eq!(
            rank_discards(&hand("1m 2m 3m 4p 5p 6p 7s 8s 9s 2m 3m 5p E E"), &visible),
            Err(AnalysisError::TooManyCopies(tile("E")))
        );
    }

    #[test]
    fn hands_with_melds_use_fewer_groups() {
        // Two melds called: 8 concealed tiles, tenpai on 3-6s after discarding 9m.
        let options = rank_discards(&hand("2m 3m 4m 4s 5s 7p 7p 9m"), &NONE_VISIBLE).unwrap();
        assert_eq!(options[0].discard, tile("9m"));
        assert_eq!(options[0].shanten, 0);
        assert_eq!(options[0].ukeire, hand("3s 6s"));
    }

    #[test]
    fn ranking_a_full_hand_is_fast() {
        warm_up();
        let tiles = hand("1m 2m 3m 4m 5m 6m 7m 8m 3p 4p 5s C C N");
        let runs: u32 = 200;
        let start = std::time::Instant::now();
        for _ in 0..runs {
            rank_discards(&tiles, &NONE_VISIBLE).unwrap();
        }
        let per_call = start.elapsed() / runs;
        // The 1 ms budget applies to release builds; debug builds are far slower.
        let budget = if cfg!(debug_assertions) { 20 } else { 1 };
        assert!(
            per_call.as_millis() < budget,
            "rank_discards took {per_call:?}"
        );
    }
}
