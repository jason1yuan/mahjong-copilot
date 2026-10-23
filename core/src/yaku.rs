//! Which yaku a complete hand has (#13): libriichi scores a hand (han, fu, points) but does
//! not name its yaku, which the panel needs for target hands and waits. This names them
//! with the rules libriichi scores by (Tenhou's: no double yakuman, closed honitsu,
//! chinitsu, chanta and junchan one han more), and a test checks the han against
//! libriichi's on random hands. Riichi, tsumo and dora are the caller's to add.
//!
//! Tiles are kinds 0-33 as in [`crate::tile`]: man, pin, sou, then `E S W N P F C`.

use serde::Serialize;

use crate::analysis::TileCounts;

const EAST: u8 = 27;
const WHITE: u8 = 31;
const RED: u8 = 33;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeldKind {
    Chi,
    Pon,
    /// An open kan: called, or added to a pon.
    Minkan,
    Ankan,
}

/// A meld beside the hand; `tile` is the kind of a pon or kan, or the lowest of a chi.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meld {
    pub kind: MeldKind,
    pub tile: u8,
}

/// A complete hand to name the yaku of.
#[derive(Clone, Copy, Debug)]
pub struct WinHand<'a> {
    /// The concealed tiles by kind, winning tile included: 3n + 2 of them.
    pub concealed: &'a TileCounts,
    pub melds: &'a [Meld],
    pub winning: u8,
    pub ron: bool,
    /// Kinds 27-30.
    pub round_wind: u8,
    pub seat_wind: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Yaku {
    Riichi,
    Pinfu,
    Tanyao,
    Iipeikou,
    White,
    Green,
    Red,
    RoundWind,
    SeatWind,
    Sanshoku,
    Ittsu,
    Chanta,
    Junchan,
    Toitoi,
    Sanankou,
    SanshokuDoukou,
    Sankantsu,
    Honroutou,
    Shousangen,
    Chiitoitsu,
    Honitsu,
    Chinitsu,
    Ryanpeikou,
    Kokushi,
    Daisangen,
    Suuankou,
    Tsuuiisou,
    Ryuuiisou,
    Chinroutou,
    Shousuushii,
    Daisuushii,
    Chuuren,
    Suukantsu,
}

impl Yaku {
    /// The name the panel shows.
    pub fn name(self) -> &'static str {
        match self {
            Self::Riichi => "立直",
            Self::Pinfu => "平和",
            Self::Tanyao => "断幺",
            Self::Iipeikou => "一杯口",
            Self::White => "白",
            Self::Green => "發",
            Self::Red => "中",
            Self::RoundWind => "场风",
            Self::SeatWind => "自风",
            Self::Sanshoku => "三色同顺",
            Self::Ittsu => "一气通贯",
            Self::Chanta => "混全带幺九",
            Self::Junchan => "纯全带幺九",
            Self::Toitoi => "对对和",
            Self::Sanankou => "三暗刻",
            Self::SanshokuDoukou => "三色同刻",
            Self::Sankantsu => "三杠子",
            Self::Honroutou => "混老头",
            Self::Shousangen => "小三元",
            Self::Chiitoitsu => "七对子",
            Self::Honitsu => "混一色",
            Self::Chinitsu => "清一色",
            Self::Ryanpeikou => "二杯口",
            Self::Kokushi => "国士无双",
            Self::Daisangen => "大三元",
            Self::Suuankou => "四暗刻",
            Self::Tsuuiisou => "字一色",
            Self::Ryuuiisou => "绿一色",
            Self::Chinroutou => "清老头",
            Self::Shousuushii => "小四喜",
            Self::Daisuushii => "大四喜",
            Self::Chuuren => "九莲宝灯",
            Self::Suukantsu => "四杠子",
        }
    }
}

/// The yaku of a hand, read the way that scores most.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Named {
    /// With their han, in the order above; for a yakuman hand only its yakuman.
    pub yaku: Vec<(Yaku, u8)>,
    pub han: u8,
    /// How many times yakuman; 0 for a normal hand.
    pub yakuman: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Set {
    /// Lowest tile.
    Run(u8),
    Triplet(u8),
}

/// A set of the finished hand: concealed or from a meld.
#[derive(Clone, Copy, Debug)]
struct Group {
    run: bool,
    tile: u8,
    kan: bool,
    concealed: bool,
}

fn is_terminal_or_honor(k: u8) -> bool {
    k >= 27 || k.is_multiple_of(9) || k % 9 == 8
}

fn is_simple(k: u8) -> bool {
    !is_terminal_or_honor(k)
}

/// Every way to split `counts` into runs and triplets (all of them used).
fn split(counts: &mut TileCounts, from: usize, sets: &mut Vec<Set>, out: &mut Vec<Vec<Set>>) {
    let Some(k) = (from..counts.len()).find(|&k| counts[k] > 0) else {
        out.push(sets.clone());
        return;
    };
    let kind = k as u8;
    if counts[k] >= 3 {
        counts[k] -= 3;
        sets.push(Set::Triplet(kind));
        split(counts, k, sets, out);
        sets.pop();
        counts[k] += 3;
    }
    if kind < 27 && kind % 9 <= 6 && counts[k + 1] > 0 && counts[k + 2] > 0 {
        counts[k] -= 1;
        counts[k + 1] -= 1;
        counts[k + 2] -= 1;
        sets.push(Set::Run(kind));
        split(counts, k, sets, out);
        sets.pop();
        counts[k] += 1;
        counts[k + 1] += 1;
        counts[k + 2] += 1;
    }
}

/// Every reading of the concealed tiles as a pair and sets.
fn readings(concealed: &TileCounts) -> Vec<(u8, Vec<Set>)> {
    let mut out = Vec::new();
    for pair in 0..concealed.len() {
        if concealed[pair] < 2 {
            continue;
        }
        let mut counts = *concealed;
        counts[pair] -= 2;
        let mut splits = Vec::new();
        split(&mut counts, 0, &mut Vec::new(), &mut splits);
        out.extend(splits.into_iter().map(|s| (pair as u8, s)));
    }
    out
}

/// The yaku of a complete hand; `None` when it has none, or is not complete.
pub fn name(hand: &WinHand) -> Option<Named> {
    let total: u8 = hand.concealed.iter().sum();
    if total % 3 != 2 || usize::from(total) + 3 * hand.melds.len() != 14 {
        return None;
    }
    let menzen = hand.melds.iter().all(|m| m.kind == MeldKind::Ankan);
    let mut best: Option<Named> = None;
    let mut consider = |named: Named| {
        let better = best
            .as_ref()
            .is_none_or(|b| (named.yakuman, named.han) > (b.yakuman, b.han));
        if better && (named.yakuman > 0 || named.han > 0) {
            best = Some(named);
        }
    };
    if menzen && hand.melds.is_empty() {
        if let Some(named) = kokushi(hand.concealed) {
            consider(named);
        }
        if let Some(named) = chiitoitsu(hand.concealed) {
            consider(named);
        }
    }
    for (pair, sets) in readings(hand.concealed) {
        consider(regular(hand, menzen, pair, &sets));
    }
    best
}

fn kokushi(concealed: &TileCounts) -> Option<Named> {
    let orphans = (0..34u8).filter(|&k| is_terminal_or_honor(k));
    let all = orphans.clone().all(|k| concealed[usize::from(k)] >= 1);
    let only = (0..34u8).all(|k| is_terminal_or_honor(k) || concealed[usize::from(k)] == 0);
    (all && only).then(|| Named {
        yaku: vec![(Yaku::Kokushi, 13)],
        han: 0,
        yakuman: 1,
    })
}

fn chiitoitsu(concealed: &TileCounts) -> Option<Named> {
    if concealed.iter().filter(|&&n| n == 2).count() != 7 {
        return None;
    }
    let pairs: Vec<u8> = (0..34u8)
        .filter(|&k| concealed[usize::from(k)] == 2)
        .collect();
    if pairs.iter().all(|&k| k >= 27) {
        return Some(Named {
            yaku: vec![(Yaku::Tsuuiisou, 13)],
            han: 0,
            yakuman: 1,
        });
    }
    let mut yaku = vec![(Yaku::Chiitoitsu, 2)];
    if pairs.iter().all(|&k| is_simple(k)) {
        yaku.push((Yaku::Tanyao, 1));
    }
    if let Some((y, h)) = flush(pairs.iter().copied(), true) {
        yaku.push((y, h));
    }
    if pairs.iter().all(|&k| is_terminal_or_honor(k)) {
        yaku.push((Yaku::Honroutou, 2));
    }
    Some(total(yaku))
}

/// Honitsu or chinitsu for tiles of one suit (with honors or without).
fn flush(tiles: impl Iterator<Item = u8> + Clone, menzen: bool) -> Option<(Yaku, u8)> {
    let suits: Vec<u8> = tiles.clone().filter(|&k| k < 27).map(|k| k / 9).collect();
    let first = *suits.first()?;
    if suits.iter().any(|&s| s != first) {
        return None;
    }
    let honors = tiles.clone().any(|k| k >= 27);
    Some(if honors {
        (Yaku::Honitsu, 2 + u8::from(menzen))
    } else {
        (Yaku::Chinitsu, 5 + u8::from(menzen))
    })
}

fn total(yaku: Vec<(Yaku, u8)>) -> Named {
    let han = yaku.iter().map(|(_, h)| h).sum();
    Named {
        yaku,
        han,
        yakuman: 0,
    }
}

fn regular(hand: &WinHand, menzen: bool, pair: u8, sets: &[Set]) -> Named {
    let mut groups: Vec<Group> = sets
        .iter()
        .map(|s| match *s {
            Set::Run(t) => Group {
                run: true,
                tile: t,
                kan: false,
                concealed: true,
            },
            Set::Triplet(t) => Group {
                run: false,
                tile: t,
                kan: false,
                concealed: true,
            },
        })
        .collect();
    for m in hand.melds {
        groups.push(Group {
            run: m.kind == MeldKind::Chi,
            tile: m.tile,
            kan: matches!(m.kind, MeldKind::Minkan | MeldKind::Ankan),
            concealed: m.kind == MeldKind::Ankan,
        });
    }
    let w = hand.winning;
    let concealed_runs: Vec<u8> = sets
        .iter()
        .filter_map(|s| match s {
            Set::Run(t) => Some(*t),
            Set::Triplet(_) => None,
        })
        .collect();
    let runs: Vec<u8> = groups.iter().filter(|g| g.run).map(|g| g.tile).collect();
    let triplets: Vec<u8> = groups.iter().filter(|g| !g.run).map(|g| g.tile).collect();
    // A ron completing a concealed triplet opens it, unless a concealed run can take the
    // winning tile instead (libriichi reads it that way, which keeps the triplet).
    let ron_opens = hand.ron
        && sets.contains(&Set::Triplet(w))
        && (w >= 27
            || !concealed_runs
                .iter()
                .any(|&t| t / 9 == w / 9 && t <= w && w <= t + 2));
    let concealed_triplets =
        groups.iter().filter(|g| !g.run && g.concealed).count() - usize::from(ron_opens);
    let kans = groups.iter().filter(|g| g.kan).count();
    let all_tiles = || {
        groups
            .iter()
            .flat_map(|g| {
                if g.run {
                    vec![g.tile, g.tile + 1, g.tile + 2]
                } else {
                    vec![g.tile]
                }
            })
            .chain(std::iter::once(pair))
    };

    // Yakuman first: when there is one, nothing else counts.
    let mut yakuman = Vec::new();
    let dragons = (WHITE..=RED).filter(|t| triplets.contains(t)).count();
    let winds = (EAST..EAST + 4).filter(|t| triplets.contains(t)).count();
    if dragons == 3 {
        yakuman.push(Yaku::Daisangen);
    }
    if concealed_triplets == 4 {
        yakuman.push(Yaku::Suuankou);
    }
    if kans == 4 {
        yakuman.push(Yaku::Suukantsu);
    }
    if all_tiles().all(|k| k >= 27) {
        yakuman.push(Yaku::Tsuuiisou);
    }
    if winds == 4 {
        yakuman.push(Yaku::Daisuushii);
    } else if winds == 3 && (EAST..EAST + 4).contains(&pair) {
        yakuman.push(Yaku::Shousuushii);
    }
    let green = |k: u8| matches!(k, 19 | 20 | 21 | 23 | 25 | 32);
    if runs.iter().all(|&t| t == 19) && triplets.iter().all(|&t| green(t)) && green(pair) {
        yakuman.push(Yaku::Ryuuiisou);
    }
    if runs.is_empty() && all_tiles().all(|k| k < 27 && (k % 9 == 0 || k % 9 == 8)) {
        yakuman.push(Yaku::Chinroutou);
    }
    if menzen && hand.melds.is_empty() && chuuren(hand.concealed) {
        yakuman.push(Yaku::Chuuren);
    }
    if !yakuman.is_empty() {
        return Named {
            yakuman: yakuman.len() as u8,
            yaku: yakuman.into_iter().map(|y| (y, 13)).collect(),
            han: 0,
        };
    }

    let mut yaku = Vec::new();
    let value_pair =
        (WHITE..=RED).contains(&pair) || pair == hand.round_wind || pair == hand.seat_wind;
    let pinfu = menzen
        && hand.melds.is_empty()
        && concealed_runs.len() == 4
        && !value_pair
        && concealed_runs.iter().any(|&t| {
            let num = t % 9 + 1;
            num <= 6 && t == w || num >= 2 && t + 2 == w
        });
    if pinfu {
        yaku.push((Yaku::Pinfu, 1));
    }
    let tanyao = runs.iter().all(|&t| t % 9 > 0 && t % 9 < 6)
        && triplets
            .iter()
            .chain(std::iter::once(&pair))
            .all(|&k| is_simple(k));
    if tanyao {
        yaku.push((Yaku::Tanyao, 1));
    }
    let toitoi = runs.is_empty();
    if toitoi {
        yaku.push((Yaku::Toitoi, 2));
    }
    if let Some(f) = flush(all_tiles(), menzen) {
        yaku.push(f);
    }
    if menzen {
        let mut sorted = concealed_runs.clone();
        sorted.sort_unstable();
        let doubles = sorted
            .chunks(2)
            .filter(|c| c.len() == 2 && c[0] == c[1])
            .count();
        let same = |a: &[u8]| {
            let mut seen = Vec::new();
            a.iter()
                .filter(|&&t| {
                    if seen.contains(&t) {
                        true
                    } else {
                        seen.push(t);
                        false
                    }
                })
                .count()
        };
        if concealed_runs.len() == 4 && doubles == 2 && hand.melds.is_empty() {
            yaku.push((Yaku::Ryanpeikou, 3));
        } else if same(&concealed_runs) >= 1 {
            yaku.push((Yaku::Iipeikou, 1));
        }
    }
    let ittsu = (0..3u8).any(|s| [0u8, 3, 6].iter().all(|n| runs.contains(&(s * 9 + n))));
    if ittsu {
        yaku.push((Yaku::Ittsu, if menzen { 2 } else { 1 }));
    }
    let sanshoku = (0..7u8).any(|n| (0..3u8).all(|s| runs.contains(&(s * 9 + n))));
    if sanshoku {
        yaku.push((Yaku::Sanshoku, if menzen { 2 } else { 1 }));
    } else if (0..9u8).any(|n| (0..3u8).all(|s| triplets.contains(&(s * 9 + n)))) {
        yaku.push((Yaku::SanshokuDoukou, 2));
    }
    if concealed_triplets == 3 {
        yaku.push((Yaku::Sanankou, 2));
    }
    if kans == 3 {
        yaku.push((Yaku::Sankantsu, 2));
    }
    if !tanyao {
        if triplets.contains(&hand.round_wind) {
            yaku.push((Yaku::RoundWind, 1));
        }
        if triplets.contains(&hand.seat_wind) {
            yaku.push((Yaku::SeatWind, 1));
        }
        for (tile, y) in [
            (WHITE, Yaku::White),
            (WHITE + 1, Yaku::Green),
            (RED, Yaku::Red),
        ] {
            if triplets.contains(&tile) {
                yaku.push((y, 1));
            }
        }
        if dragons == 2 && (WHITE..=RED).contains(&pair) {
            yaku.push((Yaku::Shousangen, 2));
        }
        let outside = triplets
            .iter()
            .chain(std::iter::once(&pair))
            .all(|&k| is_terminal_or_honor(k));
        if outside {
            let honors = triplets
                .iter()
                .chain(std::iter::once(&pair))
                .any(|&k| k >= 27);
            if toitoi {
                if honors {
                    yaku.push((Yaku::Honroutou, 2));
                }
            } else if runs.iter().all(|&t| t % 9 == 0 || t % 9 == 6) {
                if honors {
                    yaku.push((Yaku::Chanta, 1 + u8::from(menzen)));
                } else {
                    yaku.push((Yaku::Junchan, 2 + u8::from(menzen)));
                }
            }
        }
    }
    yaku.sort();
    total(yaku)
}

/// Nine gates: 1112345678999 of one suit plus any tile of it.
fn chuuren(concealed: &TileCounts) -> bool {
    (0..3usize).any(|s| {
        let suit = &concealed[s * 9..s * 9 + 9];
        let base = [3u8, 1, 1, 1, 1, 1, 1, 1, 3];
        concealed.iter().sum::<u8>() == 14
            && suit.iter().sum::<u8>() == 14
            && suit.iter().zip(base).all(|(&n, b)| n >= b)
    })
}

#[cfg(test)]
mod tests {
    use riichi::algo::agari::{Agari, AgariCalculator};

    use super::*;
    use crate::tile::parse_tiles;

    fn counts(s: &str) -> TileCounts {
        let mut c = [0; 34];
        for t in parse_tiles(s).unwrap() {
            c[usize::from(t.kind())] += 1;
        }
        c
    }

    fn kind(s: &str) -> u8 {
        parse_tiles(s).unwrap()[0].kind()
    }

    fn names(hand: &WinHand) -> Vec<&'static str> {
        name(hand).map_or_else(Vec::new, |n| n.yaku.iter().map(|(y, _)| y.name()).collect())
    }

    fn closed<'a>(concealed: &'a TileCounts, winning: &str, ron: bool) -> WinHand<'a> {
        WinHand {
            concealed,
            melds: &[],
            winning: kind(winning),
            ron,
            round_wind: EAST,
            seat_wind: EAST + 1,
        }
    }

    #[test]
    fn names_common_hands() {
        let c = counts("2m 3m 4m 5m 6m 7m 3p 4p 5p 6s 7s 8s 9p 9p");
        assert_eq!(names(&closed(&c, "8s", true)), ["平和"]);
        let c = counts("1m 2m 3m 4m 5m 6m 7m 8m 9m 3p 4p 5p C C");
        assert_eq!(names(&closed(&c, "9m", true)), ["一气通贯"]);
        let c = counts("1m 1m 2m 2m 3m 3m 5m 5m 6m 6m 7m 7m 9m 9m");
        assert_eq!(names(&closed(&c, "9m", true)), ["清一色", "二杯口"]);
        let c = counts("1m 1m 1m 2m 3m 4m 5m 6m 7m 8m 9m 9m 9m 5m");
        let named = name(&closed(&c, "5m", true)).unwrap();
        assert_eq!(named.yakuman, 1);
        assert_eq!(named.yaku[0].0, Yaku::Chuuren);
    }

    #[test]
    fn an_open_hand_needs_a_yaku() {
        let c = counts("2m 3m 4m 5p 6p 7p 1p 1p");
        let melds = [
            Meld {
                kind: MeldKind::Pon,
                tile: kind("9s"),
            },
            Meld {
                kind: MeldKind::Chi,
                tile: kind("6s"),
            },
        ];
        let hand = WinHand {
            concealed: &c,
            melds: &melds,
            winning: kind("1p"),
            ron: true,
            round_wind: EAST,
            seat_wind: EAST + 1,
        };
        assert_eq!(name(&hand), None);
    }

    /// xorshift64*, as the simulator uses.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn below(&mut self, n: u64) -> u8 {
            (self.next() % n) as u8
        }
    }

    /// A random complete hand: four sets and a pair, some called, from a few suits so that
    /// flushes, outside hands and honors come up often.
    fn random_hand(rng: &mut Rng) -> Option<(TileCounts, Vec<Meld>, u8, bool, u8, u8)> {
        if rng.below(8) == 0 {
            return random_pairs(rng);
        }
        let suits: Vec<u8> = match rng.below(4) {
            0 => vec![rng.below(3)],
            1 => vec![rng.below(3), 3],
            _ => vec![0, 1, 2, 3],
        };
        let pick_kind = |rng: &mut Rng, run: bool| -> Option<u8> {
            let suit = suits[usize::from(rng.below(suits.len() as u64))];
            if suit == 3 {
                (!run).then(|| 27 + rng.below(7))
            } else if run {
                Some(suit * 9 + rng.below(7))
            } else {
                Some(suit * 9 + rng.below(9))
            }
        };
        let mut all = [0u8; 34];
        let mut concealed = [0u8; 34];
        let mut melds = Vec::new();
        for _ in 0..4 {
            let run = rng.below(2) == 0;
            let tile = pick_kind(rng, run)?;
            let open = rng.below(4) == 0;
            let kan = !run && rng.below(8) == 0;
            let tiles: Vec<u8> = if run {
                vec![tile, tile + 1, tile + 2]
            } else {
                vec![tile; if kan { 4 } else { 3 }]
            };
            for &t in &tiles {
                all[usize::from(t)] += 1;
            }
            if open || kan {
                let kind = match (run, kan, open) {
                    (true, _, _) => MeldKind::Chi,
                    (false, true, true) => MeldKind::Minkan,
                    (false, true, false) => MeldKind::Ankan,
                    (false, false, _) => MeldKind::Pon,
                };
                melds.push(Meld { kind, tile });
            } else {
                for &t in &tiles {
                    concealed[usize::from(t)] += 1;
                }
            }
        }
        let pair = pick_kind(rng, false)?;
        all[usize::from(pair)] += 2;
        concealed[usize::from(pair)] += 2;
        if all.iter().any(|&n| n > 4) {
            return None;
        }
        let held: Vec<u8> = (0..34u8)
            .filter(|&k| concealed[usize::from(k)] > 0)
            .collect();
        let winning = held[usize::from(rng.below(held.len() as u64))];
        let ron = rng.below(2) == 0;
        let round = 27 + rng.below(2);
        let seat = 27 + rng.below(4);
        Some((concealed, melds, winning, ron, round, seat))
    }

    /// Seven pairs of different kinds, from one suit with honors or from all.
    fn random_pairs(rng: &mut Rng) -> Option<(TileCounts, Vec<Meld>, u8, bool, u8, u8)> {
        let pool: Vec<u8> = match rng.below(3) {
            0 => {
                let s = rng.below(3);
                (s * 9..s * 9 + 9).chain(27..34).collect()
            }
            1 => (0..34u8).filter(|&k| is_terminal_or_honor(k)).collect(),
            _ => (0..34u8).collect(),
        };
        let mut concealed = [0u8; 34];
        let mut picked = 0;
        while picked < 7 {
            let k = pool[usize::from(rng.below(pool.len() as u64))];
            if concealed[usize::from(k)] == 0 {
                concealed[usize::from(k)] = 2;
                picked += 1;
            }
        }
        let held: Vec<u8> = (0..34u8)
            .filter(|&k| concealed[usize::from(k)] > 0)
            .collect();
        let winning = held[usize::from(rng.below(7))];
        Some((concealed, Vec::new(), winning, rng.below(2) == 0, 27, 28))
    }

    /// Our han and yakuman agree with libriichi's on random complete hands.
    #[test]
    fn agrees_with_libriichi_on_random_hands() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut checked = 0;
        let mut seen = std::collections::BTreeSet::new();
        while checked < 30_000 {
            let Some((concealed, melds, winning, ron, round, seat)) = random_hand(&mut rng) else {
                continue;
            };
            let kinds = |k: MeldKind| -> Vec<u8> {
                melds
                    .iter()
                    .filter(|m| m.kind == k)
                    .map(|m| m.tile)
                    .collect()
            };
            let (chis, pons, minkans, ankans) = (
                kinds(MeldKind::Chi),
                kinds(MeldKind::Pon),
                kinds(MeldKind::Minkan),
                kinds(MeldKind::Ankan),
            );
            let theirs = AgariCalculator {
                tehai: &concealed,
                is_menzen: chis.is_empty() && pons.is_empty() && minkans.is_empty(),
                chis: &chis,
                pons: &pons,
                minkans: &minkans,
                ankans: &ankans,
                bakaze: round,
                jikaze: seat,
                winning_tile: winning,
                is_ron: ron,
            }
            .search_yakus();
            let ours = name(&WinHand {
                concealed: &concealed,
                melds: &melds,
                winning,
                ron,
                round_wind: round,
                seat_wind: seat,
            });
            let want = match theirs {
                None => None,
                Some(Agari::Normal { han, .. }) => Some((0, han)),
                Some(Agari::Yakuman(n)) => Some((n, 0)),
            };
            let got = ours.as_ref().map(|n| (n.yakuman, n.han));
            assert_eq!(
                got, want,
                "concealed {concealed:?} melds {melds:?} winning {winning} ron {ron} round {round} seat {seat}: {ours:?}"
            );
            if let Some(n) = ours {
                seen.extend(n.yaku.iter().map(|(y, _)| *y));
            }
            checked += 1;
        }
        // The random hands reach most yaku, so most rules are checked.
        assert!(seen.len() >= 20, "{seen:?}");
    }
}
