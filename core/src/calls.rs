//! Call advice (#11, first step): when another player's discard can be called, every way to
//! chi or pon it, compared with not calling, by speed alone (shanten, then effective draws)
//! and only among calls that keep a yaku within reach.
//!
//! - After a call the hand must discard; the discard is ranked as usual, leaving out the
//!   tiles swap-calling forbids (the called kind, and for an end chi the tile at the other
//!   end of the run). A call whose every discard is forbidden is not offered.
//! - An open hand needs a yaku to win. One is within reach when the hand, called tiles
//!   included, already has a triplet of a value honor (dragons, the round and seat winds),
//!   or can still become all simples, one suit with honors, or all triplets no more than
//!   one step slower than it is.
//! - A call is advised when it lowers the shanten and keeps a yaku within reach, or when it
//!   is a pon of a value honor, which players take at once; between such calls, the lower
//!   shanten and then the more effective draws win. Hand value and safety are left for
//!   later (#11); calling also gives up riichi, which this ignores.
//! - While a win is on offer there is no call advice: winning comes first.

use riichi::algo::shanten;
use serde::Serialize;

use crate::analysis::{self, AnalysisError, DiscardOption, Outlook, TileCounts};
use crate::tile::{KINDS, Tile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CallKind {
    Chi,
    Pon,
}

/// A yaku an open hand can still reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Yaku {
    /// A triplet of a dragon, the round wind or the seat wind.
    Yakuhai,
    /// All simples (open tanyao is allowed).
    Tanyao,
    /// One suit, with or without honors.
    Honitsu,
    /// All triplets.
    Toitoi,
}

/// One way to call, and the hand after it and its best allowed discard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallOption {
    pub kind: CallKind,
    pub called: Tile,
    /// The two tiles from the hand that go with the called one.
    pub with: [Tile; 2],
    /// The best discard after the call, and where the hand stands then.
    pub after: DiscardOption,
    /// Yaku within reach after the call; empty means the hand could not win.
    pub yaku: Vec<Yaku>,
}

/// The ways to call a discard, best first, and the hand if it does not call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallAdvice {
    pub stay: Outlook,
    pub options: Vec<CallOption>,
    /// The option to take, if any is worth it.
    pub take: Option<usize>,
}

/// What the call advice needs to know about the player.
pub struct CallContext<'a> {
    /// The concealed hand, 3n+1 tiles, waiting on another player's discard.
    pub hand: &'a [Tile],
    /// The player's melds so far, each with the tiles that can be seen.
    pub melds: &'a [Vec<Tile>],
    /// Honors whose triplet is a yaku: the dragons, the round wind and the seat wind.
    pub value_honors: &'a [Tile],
    /// Tiles seen outside the hand, the called one included.
    pub visible: &'a TileCounts,
}

/// Every way to call `called` (a chi only when `chi`, a pon only when `pon`), compared with
/// not calling.
pub fn advise(
    ctx: &CallContext<'_>,
    called: Tile,
    chi: bool,
    pon: bool,
) -> Result<CallAdvice, AnalysisError> {
    let stay = analysis::outlook(ctx.hand, ctx.visible)?;
    let mut ways: Vec<(CallKind, [Tile; 2])> = Vec::new();
    if pon {
        ways.extend(pick(ctx.hand, [called.kind(), called.kind()]).map(|w| (CallKind::Pon, w)));
    }
    if chi && !called.is_honor() {
        let (kind, rank) = (called.kind(), called.kind() % 9);
        for (a, b) in [(-2i8, -1i8), (-1, 1), (1, 2)] {
            let (ra, rb) = (rank as i8 + a, rank as i8 + b);
            if !(0..9).contains(&ra) || !(0..9).contains(&rb) {
                continue;
            }
            let pair = [(kind as i8 + a) as u8, (kind as i8 + b) as u8];
            ways.extend(pick(ctx.hand, pair).map(|w| (CallKind::Chi, w)));
        }
    }

    let mut options = Vec::new();
    for (kind, with) in ways {
        let mut rest = ctx.hand.to_vec();
        for tile in with {
            if let Some(i) = rest.iter().position(|t| *t == tile) {
                rest.remove(i);
            }
        }
        let forbidden = forbidden_discards(kind, called, with);
        let Some(after) = analysis::rank_discards(&rest, ctx.visible)?
            .into_iter()
            .find(|o| !forbidden.contains(&o.discard.kind()))
        else {
            continue;
        };
        let mut melds: Vec<Vec<Tile>> = ctx.melds.to_vec();
        melds.push(vec![called, with[0], with[1]]);
        let mut left = rest.clone();
        if let Some(i) = left.iter().position(|t| *t == after.discard) {
            left.remove(i);
        }
        let yaku = reachable_yaku(&left, &melds, ctx.value_honors, after.shanten);
        options.push(CallOption {
            kind,
            called,
            with,
            after,
            yaku,
        });
    }
    options.sort_by_key(|o| {
        (
            o.yaku.is_empty(),
            o.after.shanten,
            std::cmp::Reverse(o.after.ukeire_count),
        )
    });
    let value_pon = |o: &CallOption| {
        o.kind == CallKind::Pon && ctx.value_honors.iter().any(|t| t.kind() == called.kind())
    };
    let take = options.iter().position(|o| {
        !o.yaku.is_empty()
            && (o.after.shanten < stay.shanten || (o.after.shanten == stay.shanten && value_pon(o)))
    });
    Ok(CallAdvice {
        stay,
        options,
        take,
    })
}

/// The discard the buttons offer to call, from each other player's latest discard (right,
/// across, left): a chi can only be of the left player's, so with the chi button it is
/// that one; with only pon, it is the one discard the hand holds a pair of. `None` when
/// that does not settle it, or when a win is on offer too (`win`), which comes first.
pub fn offered(
    latest: [Option<Tile>; 3],
    hand: &[Tile],
    chi: bool,
    pon: bool,
    win: bool,
) -> Option<Tile> {
    if win {
        return None;
    }
    if chi {
        return latest[2].filter(|t| !t.is_honor());
    }
    if !pon {
        return None;
    }
    let held = |t: &Tile| hand.iter().filter(|h| h.kind() == t.kind()).count() >= 2;
    let mut kinds: Vec<Tile> = latest.into_iter().flatten().filter(held).collect();
    kinds.sort_by_key(|t| t.kind());
    kinds.dedup_by_key(|t| t.kind());
    match kinds.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

/// Two tiles of kinds `pair` from `hand`, plain ones before red fives; `None` when the
/// hand lacks them.
fn pick(hand: &[Tile], pair: [u8; 2]) -> Option<[Tile; 2]> {
    let mut sorted = hand.to_vec();
    sorted.sort_by_key(|t| t.is_red());
    let mut out = Vec::new();
    for kind in pair {
        let i = sorted.iter().position(|t| t.kind() == kind)?;
        out.push(sorted.remove(i));
    }
    Some([out[0], out[1]])
}

/// Kinds that may not be discarded right after the call (swap-calling).
fn forbidden_discards(kind: CallKind, called: Tile, with: [Tile; 2]) -> Vec<u8> {
    let c = called.kind();
    let mut out = vec![c];
    if kind == CallKind::Chi {
        let (lo, hi) = {
            let (a, b) = (with[0].kind(), with[1].kind());
            (a.min(b), a.max(b))
        };
        // An end chi: 3 with 4-5 also forbids 6; 6 with 4-5 forbids 3.
        if lo == c + 1 && hi % 9 < 8 {
            out.push(hi + 1);
        }
        if hi + 1 == c && lo % 9 > 0 {
            out.push(lo - 1);
        }
    }
    out
}

fn is_simple(kind: u8) -> bool {
    kind < 27 && !matches!(kind % 9, 0 | 8)
}

/// Yaku the open hand can still reach: `left` is the concealed 3n+1 hand after the
/// discard, `melds` every meld including the new one, `shanten` where the hand stands.
fn reachable_yaku(
    left: &[Tile],
    melds: &[Vec<Tile>],
    value_honors: &[Tile],
    shanten: i8,
) -> Vec<Yaku> {
    let counts = analysis::counts(left);
    let groups = (left.len() / 3) as u8;
    let within = |path: i8| path <= shanten + 1;
    let kinds = |meld: &Vec<Tile>| meld.iter().map(|t| t.kind()).collect::<Vec<_>>();
    let is_triplet = |meld: &Vec<Tile>| {
        let k = kinds(meld);
        k.len() >= 3 && k.iter().all(|x| *x == k[0])
    };
    let mut yaku = Vec::new();

    let value: Vec<u8> = value_honors.iter().map(|t| t.kind()).collect();
    if melds
        .iter()
        .any(|m| is_triplet(m) && value.contains(&m[0].kind()))
        || value.iter().any(|k| counts[usize::from(*k)] >= 3)
    {
        yaku.push(Yaku::Yakuhai);
    }

    if melds.iter().flatten().all(|t| is_simple(t.kind())) {
        let only = keep(&counts, is_simple);
        if within(shanten::calc_normal(&only, groups)) {
            yaku.push(Yaku::Tanyao);
        }
    }

    for suit in 0..3u8 {
        let in_suit = |k: u8| k >= 27 || k / 9 == suit;
        if melds.iter().flatten().all(|t| in_suit(t.kind())) {
            let only = keep(&counts, in_suit);
            if within(shanten::calc_normal(&only, groups)) {
                yaku.push(Yaku::Honitsu);
                break;
            }
        }
    }

    if melds.iter().all(is_triplet) && within(triplets_shanten(&counts, groups)) {
        yaku.push(Yaku::Toitoi);
    }
    yaku
}

/// `counts` with only the kinds `keep_kind` accepts.
fn keep(counts: &TileCounts, keep_kind: impl Fn(u8) -> bool) -> TileCounts {
    let mut out = [0; KINDS as usize];
    for kind in 0..KINDS {
        if keep_kind(kind) {
            out[usize::from(kind)] = counts[usize::from(kind)];
        }
    }
    out
}

/// Shanten of `counts` towards `groups` triplets and a pair.
fn triplets_shanten(counts: &TileCounts, groups: u8) -> i8 {
    let mut held: Vec<u8> = counts.iter().map(|c| (*c).min(3)).collect();
    held.sort_unstable_by(|a, b| b.cmp(a));
    let top = &held[..=usize::from(groups)];
    // Missing tiles with the pair taken from each of the best kinds in turn.
    let missing = (0..top.len())
        .map(|pair| {
            top.iter()
                .enumerate()
                .map(|(i, c)| {
                    if i == pair {
                        2u8.saturating_sub(*c)
                    } else {
                        3 - c
                    }
                })
                .sum::<u8>()
        })
        .min()
        .unwrap_or(u8::MAX);
    missing as i8 - 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::parse_tiles;

    fn tiles(s: &str) -> Vec<Tile> {
        parse_tiles(s).unwrap()
    }

    fn tile(s: &str) -> Tile {
        s.parse().unwrap()
    }

    fn advise_on(hand: &str, melds: &[&str], called: &str, chi: bool, pon: bool) -> CallAdvice {
        let hand = tiles(hand);
        let melds: Vec<Vec<Tile>> = melds.iter().map(|m| tiles(m)).collect();
        let value = tiles("P F C E S");
        let mut visible = [0u8; KINDS as usize];
        visible[usize::from(tile(called).kind())] += 1;
        let ctx = CallContext {
            hand: &hand,
            melds: &melds,
            value_honors: &value,
            visible: &visible,
        };
        advise(&ctx, tile(called), chi, pon).unwrap()
    }

    fn names(option: &CallOption) -> String {
        format!("{} {}", option.with[0], option.with[1])
    }

    #[test]
    fn lists_every_way_to_chi() {
        // 3p can go with 1p-2p, 2p-4p or 4p-5p.
        let advice = advise_on(
            "1p 2p 4p 5p 2m 3m 4m 6s 7s 8s 9m 9m N",
            &[],
            "3p",
            true,
            false,
        );
        let mut ways: Vec<String> = advice.options.iter().map(names).collect();
        ways.sort();
        assert_eq!(ways, ["1p 2p", "2p 4p", "4p 5p"]);
    }

    #[test]
    fn a_call_that_speeds_up_an_all_simples_hand_is_taken() {
        // One-away with all simples but N; chi 3p with 4p-5p makes it tenpai.
        let advice = advise_on(
            "4p 5p 6m 7m 2m 3m 4m 6s 7s 8s 5s 5s N",
            &[],
            "3p",
            true,
            false,
        );
        let take = &advice.options[advice.take.expect("a call to take")];
        assert_eq!(take.kind, CallKind::Chi);
        assert!(take.after.shanten < advice.stay.shanten);
        assert!(take.yaku.contains(&Yaku::Tanyao));
        assert_eq!(take.after.discard, tile("N"));
    }

    #[test]
    fn a_call_without_any_yaku_is_not_taken() {
        // Terminals and two suits in hand, no value honors: nothing to win with when open.
        let advice = advise_on(
            "1p 2p 1m 9m 9m 7s 8s 9s 1s 1s 2m 3m E",
            &[],
            "3p",
            true,
            false,
        );
        assert!(advice.options.iter().all(|o| o.yaku.is_empty()));
        assert_eq!(advice.take, None);
    }

    #[test]
    fn a_pon_of_a_dragon_is_a_yaku() {
        let advice = advise_on("C C 1m 9m 1p 4p 7p 2s 5s 8s E S W", &[], "C", false, true);
        assert_eq!(advice.options.len(), 1);
        assert!(advice.options[0].yaku.contains(&Yaku::Yakuhai));
    }

    #[test]
    fn a_pon_of_a_value_honor_is_taken_even_at_the_same_shanten() {
        // Four partial runs and the dragon pair: three-away either way, as the hand has
        // room for only three of the partials once the pon is a group.
        let advice = advise_on("C C 2m 3m 6m 7m 2p 3p 5s 6s 9p N W", &[], "C", false, true);
        assert_eq!(advice.options[0].after.shanten, advice.stay.shanten);
        assert_eq!(advice.take, Some(0));
    }

    #[test]
    fn a_call_that_does_not_lower_shanten_is_not_taken() {
        // Already tenpai; a pon of 9m only swaps the pair for a triplet.
        let advice = advise_on(
            "9m 9m 2p 3p 4p 5p 6p 7p 3s 4s 6s 7s 8s",
            &[],
            "9m",
            false,
            true,
        );
        assert_eq!(advice.take, None);
    }

    #[test]
    fn finds_the_offered_discard() {
        let hand = tiles("3p 4p 9m 9m 1s 2s 5s 6s 7s E E W N");
        let latest = [Some(tile("9m")), Some(tile("C")), Some(tile("2p"))];
        // A chi is always of the left player's discard.
        assert_eq!(offered(latest, &hand, true, true, false), Some(tile("2p")));
        // A pon of the one discard the hand holds a pair of.
        assert_eq!(offered(latest, &hand, false, true, false), Some(tile("9m")));
        // Two discards it could pon: not settled.
        let two = [Some(tile("9m")), Some(tile("E")), None];
        assert_eq!(offered(two, &hand, false, true, false), None);
        assert_eq!(offered(latest, &hand, false, false, false), None);
        // Winning comes first.
        assert_eq!(offered(latest, &hand, true, true, true), None);
    }

    #[test]
    fn swap_calling_discards_are_not_suggested() {
        // Chi 3p with 4p-5p: neither 3p nor 6p may be discarded right after.
        assert_eq!(
            forbidden_discards(CallKind::Chi, tile("3p"), [tile("4p"), tile("5p")]),
            [tile("3p").kind(), tile("6p").kind()]
        );
        assert_eq!(
            forbidden_discards(CallKind::Chi, tile("6p"), [tile("4p"), tile("5p")]),
            [tile("6p").kind(), tile("3p").kind()]
        );
        assert_eq!(
            forbidden_discards(CallKind::Chi, tile("4p"), [tile("3p"), tile("5p")]),
            [tile("4p").kind()]
        );
        assert_eq!(
            forbidden_discards(CallKind::Chi, tile("7p"), [tile("8p"), tile("9p")]),
            [tile("7p").kind()]
        );
    }

    #[test]
    fn plain_fives_go_into_the_call_before_red_ones() {
        assert_eq!(
            pick(&tiles("5pr 5p 4p"), [tile("4p").kind(), tile("5p").kind()]),
            Some([tile("4p"), tile("5p")])
        );
    }

    #[test]
    fn triplets_shanten_counts_missing_tiles() {
        // 1m1m1m 2p2p2p 3s3s 4s: one tile from a pair-and-triplet hand of 3 groups.
        let counts = analysis::counts(&tiles("1m 1m 1m 2p 2p 2p 3s 3s 4s 4s"));
        assert_eq!(triplets_shanten(&counts, 3), 0);
        let counts = analysis::counts(&tiles("1m 2m 3m 4m 5m 6m 7m 8m 9m E"));
        assert_eq!(triplets_shanten(&counts, 3), 6);
    }
}
