//! Target hands (#13): the complete hands within reach of the current one, what each is
//! worth and roughly how likely it is, so the panel can show up to three ways to go.
//!
//! - Every complete hand reachable by exchanging no more than shanten + 1 tiles (one more
//!   when one or two steps from tenpai) is enumerated: a pair and sets, or seven pairs,
//!   built from the hand's tiles first. Tiles that are all seen cannot be waited for.
//! - Each is named with [`crate::yaku`] and scored by libriichi as a ron, with riichi when
//!   closed (an open hand needs a yaku of its own), dora counted, ura dora not; the last
//!   missing tile to arrive is taken to be the one that scores most.
//! - Its probability is that of drawing every missing tile in the player's draws left,
//!   from the tiles not yet seen. It ignores calls, others winning first and the wall's
//!   unseen part of others' hands, so it is a rough guide.
//! - Targets are grouped by their yaku (riichi aside), each group keeps the best by
//!   probability times points, and the three best groups are kept.
//! - Up to four steps from tenpai the targets are searched; three or four steps away
//!   they are marked rough, as the hand can still change a lot. Five or more steps away,
//!   only directions are given.

use std::collections::{BTreeMap, HashSet};

use riichi::algo::agari::{Agari, AgariCalculator};
use serde::Serialize;

use crate::analysis::{self, TileCounts};
use crate::game::{MeldView, Scene};
use crate::tile::{KINDS, Tile};
use crate::yaku::{self, Meld, MeldKind, WinHand, Yaku};

/// Targets shown at most.
const TARGETS: usize = 3;
/// From this shanten on, targets are rough.
const ROUGH: i8 = 3;
/// From this shanten on, only directions.
const FAR: i8 = 5;

/// The hand and table to search from.
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    /// Concealed tiles: 3n + 1 waiting for a draw, or 3n + 2 to discard from.
    pub hand: &'a [Tile],
    pub melds: &'a [Meld],
    /// The melds' tiles, for counting dora and red fives.
    pub meld_tiles: &'a [Tile],
    pub dora_indicators: &'a [Tile],
    /// Tiles seen outside the hand: rivers, melds, dora indicators.
    pub visible: &'a TileCounts,
    /// Kinds 27-30.
    pub round_wind: u8,
    pub seat_wind: u8,
    pub dealer: bool,
    /// The player's own draws left this round.
    pub draws_left: u8,
}

/// A complete hand within reach.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    /// The concealed tiles of the finished hand, in order, missing ones included.
    pub tiles: Vec<Tile>,
    /// The tiles still to draw.
    pub missing: Vec<Tile>,
    /// With riichi first when assumed.
    pub yaku: Vec<Yaku>,
    pub han: u8,
    /// 0 when it does not matter (five han or more, or yakuman).
    pub fu: u8,
    pub yakuman: u8,
    /// As a ron, without ura dora.
    pub points: i32,
    pub probability: f32,
}

impl Target {
    fn expected(&self) -> f32 {
        self.probability * self.points as f32
    }
}

/// A broad way to go when the hand is far from tenpai.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Direction {
    /// One suit (0 man, 1 pin, 2 sou) with honors.
    Flush {
        suit: u8,
    },
    Chiitoitsu,
    /// A value honor the hand holds a pair of or more.
    Yakuhai {
        tile: Tile,
    },
    Tanyao,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Targets {
    pub targets: Vec<Target>,
    pub directions: Vec<Direction>,
    /// Three or more steps from tenpai: the targets are only a rough guide.
    pub rough: bool,
}

/// A wait of a tenpai hand and what winning on it is worth.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitValue {
    pub tile: Tile,
    pub yaku: Vec<Yaku>,
    pub han: u8,
    pub fu: u8,
    pub yakuman: u8,
    pub points: i32,
}

/// What the panel shows from a scene: the waits' values when the player waits in tenpai,
/// otherwise the targets.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outlook {
    #[serde(flatten)]
    pub targets: Targets,
    pub waits: Vec<WaitValue>,
}

/// Draws taken to be left when the screen does not show the tiles left.
const DEFAULT_DRAWS: u8 = 12;

/// Targets or wait values for the player in `scene`.
pub fn from_scene(scene: &Scene) -> Outlook {
    let hand = scene.own_tiles();
    let melds: Vec<Meld> = scene.melds[0].iter().filter_map(meld_of).collect();
    if melds.len() != scene.melds[0].len() {
        return Outlook::default();
    }
    let meld_tiles: Vec<Tile> = scene.melds[0].iter().flat_map(MeldView::seen).collect();
    let mut visible = [0u8; KINDS as usize];
    for t in scene
        .rivers
        .iter()
        .flatten()
        .map(|r| r.tile)
        .chain(scene.melds.iter().flatten().flat_map(MeldView::seen))
        .chain(scene.dora.iter().copied())
    {
        visible[usize::from(t.kind())] += 1;
    }
    let round_wind = scene.round.map_or(27, |r| r.wind.kind());
    // The player's wind: east for the dealer, then south, west, north counterclockwise.
    let seat_wind = scene.dealer.map_or(28, |d| 27 + (4 - d) % 4);
    let ctx = Context {
        hand: &hand,
        melds: &melds,
        meld_tiles: &meld_tiles,
        dora_indicators: &scene.dora,
        visible: &visible,
        round_wind,
        seat_wind,
        dealer: scene.dealer == Some(0),
        draws_left: scene.tiles_left.map_or(DEFAULT_DRAWS, |n| n.div_ceil(4)),
    };
    if hand.len() % 3 == 1 && analysis::shanten(&hand) == Ok(0) {
        Outlook {
            targets: Targets::default(),
            waits: waits(&ctx),
        }
    } else {
        Outlook {
            targets: search(&ctx),
            waits: Vec::new(),
        }
    }
}

/// A meld as the yaku rules see it; `None` when its tiles are not read.
fn meld_of(view: &MeldView) -> Option<Meld> {
    let tiles: Vec<u8> = view.seen().map(|t| t.kind()).collect();
    let first = *tiles.first()?;
    if view.is_concealed_kan() {
        return Some(Meld {
            kind: MeldKind::Ankan,
            tile: first,
        });
    }
    let same = tiles.iter().all(|&k| k == first);
    let kind = match (tiles.len(), same) {
        (4, true) => MeldKind::Minkan,
        (3, true) => MeldKind::Pon,
        (3, false) => MeldKind::Chi,
        _ => return None,
    };
    let tile = *tiles.iter().min()?;
    Some(Meld { kind, tile })
}

/// Up to three target hands, or directions when the hand is far from tenpai.
pub fn search(ctx: &Context) -> Targets {
    let Ok(shanten) = analysis::shanten(ctx.hand) else {
        return Targets::default();
    };
    if shanten < 0 {
        return Targets::default();
    }
    if shanten >= FAR {
        return Targets {
            targets: Vec::new(),
            directions: directions(ctx),
            rough: true,
        };
    }
    let held = analysis::counts(ctx.hand);
    let max_exchanges = (shanten + 1 + i8::from(shanten <= 1)) as u8;
    let mut finished = HashSet::new();
    let size = 14 - 3 * ctx.melds.len();
    let mut search = Search {
        held,
        unseen: unseen(ctx, &held),
        budget: max_exchanges,
        size,
        out: &mut finished,
    };
    search.regular();
    if ctx.melds.is_empty() {
        search.pairs(0, &mut [0; KINDS as usize], 0, 0);
    }
    let mut best: BTreeMap<Vec<Yaku>, Target> = BTreeMap::new();
    for counts in finished {
        let Some(target) = score(ctx, &held, &counts) else {
            continue;
        };
        let route: Vec<Yaku> = target
            .yaku
            .iter()
            .copied()
            .filter(|y| *y != Yaku::Riichi)
            .collect();
        let better = best
            .get(&route)
            .is_none_or(|b| target.expected() > b.expected());
        if better {
            best.insert(route, target);
        }
    }
    let mut targets: Vec<Target> = best.into_values().collect();
    targets.sort_by(|a, b| b.expected().total_cmp(&a.expected()));
    targets.truncate(TARGETS);
    Targets {
        targets,
        directions: Vec::new(),
        rough: shanten >= ROUGH,
    }
}

/// What winning on each wait of a tenpai hand (3n + 1 tiles) is worth, as a ron with
/// riichi when closed; `None` for a wait the hand cannot win on (no yaku when open).
pub fn waits(ctx: &Context) -> Vec<WaitValue> {
    if ctx.hand.len() % 3 != 1 {
        return Vec::new();
    }
    let held = analysis::counts(ctx.hand);
    let mut out = Vec::new();
    for k in 0..KINDS {
        let mut counts = held;
        counts[usize::from(k)] += 1;
        if counts[usize::from(k)] > 4 {
            continue;
        }
        if let Some(v) = value(ctx, &counts, k, &held) {
            out.push(WaitValue {
                tile: Tile::from_kind(k).unwrap_or_else(|| unreachable!("k < KINDS")),
                yaku: v.yaku,
                han: v.han,
                fu: v.fu,
                yakuman: v.yakuman,
                points: v.points,
            });
        }
    }
    out
}

/// Copies of each kind still unseen: not in the hand, the melds or `visible`.
fn unseen(ctx: &Context, held: &TileCounts) -> TileCounts {
    let mut out = [0; KINDS as usize];
    for k in 0..usize::from(KINDS) {
        out[k] = 4u8.saturating_sub(held[k]).saturating_sub(ctx.visible[k]);
    }
    out
}

struct Search<'a> {
    held: TileCounts,
    unseen: TileCounts,
    budget: u8,
    /// Concealed tiles in a finished hand.
    size: usize,
    out: &'a mut HashSet<TileCounts>,
}

impl Search<'_> {
    /// Missing copies of `kind` when `want` of it are in the finished hand; `None` when
    /// not enough are left to draw.
    fn missing(&self, kind: usize, want: u8) -> Option<u8> {
        let missing = want.saturating_sub(self.held[kind]);
        (missing <= self.unseen[kind]).then_some(missing)
    }

    /// Pair and sets, sets in a fixed order so each hand comes once per reading.
    fn regular(&mut self) {
        let sets = (self.size - 2) / 3;
        for pair in 0..usize::from(KINDS) {
            let mut counts = [0; KINDS as usize];
            counts[pair] = 2;
            let Some(cost) = self.missing(pair, 2) else {
                continue;
            };
            if cost <= self.budget {
                self.sets(&mut counts, cost, 0, sets);
            }
        }
    }

    /// Adds `left` sets from set index `from` on: triplets are 0-33, runs 34 on.
    fn sets(&mut self, counts: &mut TileCounts, cost: u8, from: usize, left: usize) {
        if left == 0 {
            self.out.insert(*counts);
            return;
        }
        for index in from..usize::from(KINDS) + 21 {
            let tiles: [usize; 3] = if index < usize::from(KINDS) {
                [index; 3]
            } else {
                let run = index - usize::from(KINDS);
                let start = run / 7 * 9 + run % 7;
                [start, start + 1, start + 2]
            };
            for &t in &tiles {
                counts[t] += 1;
            }
            let mut extra = 0u8;
            let mut ok = true;
            let mut seen = [usize::MAX; 3];
            for (i, &t) in tiles.iter().enumerate() {
                if seen.contains(&t) {
                    continue;
                }
                seen[i] = t;
                if counts[t] > 4 {
                    ok = false;
                    break;
                }
                let before = counts[t] - tiles.iter().filter(|&&x| x == t).count() as u8;
                match (self.missing(t, counts[t]), self.missing(t, before)) {
                    (Some(now), Some(was)) => extra += now - was,
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok && cost + extra <= self.budget {
                self.sets(counts, cost + extra, index, left - 1);
            }
            for &t in &tiles {
                counts[t] -= 1;
            }
        }
    }

    /// Seven pairs of different kinds, from kind `from` on.
    fn pairs(&mut self, from: usize, counts: &mut TileCounts, picked: usize, cost: u8) {
        if picked == 7 {
            self.out.insert(*counts);
            return;
        }
        for k in from..usize::from(KINDS) {
            let Some(extra) = self.missing(k, 2) else {
                continue;
            };
            if cost + extra > self.budget {
                continue;
            }
            counts[k] = 2;
            self.pairs(k + 1, counts, picked + 1, cost + extra);
            counts[k] = 0;
        }
    }
}

struct Value {
    yaku: Vec<Yaku>,
    han: u8,
    fu: u8,
    yakuman: u8,
    points: i32,
}

fn menzen(ctx: &Context) -> bool {
    ctx.melds.iter().all(|m| m.kind == MeldKind::Ankan)
}

/// The finished concealed `counts`, won on `winning` (a kind): its yaku and points.
fn value(ctx: &Context, counts: &TileCounts, winning: u8, held: &TileCounts) -> Option<Value> {
    let hand = WinHand {
        concealed: counts,
        melds: ctx.melds,
        winning,
        ron: true,
        round_wind: ctx.round_wind,
        seat_wind: ctx.seat_wind,
    };
    let named = yaku::name(&hand);
    let riichi = menzen(ctx);
    if named.is_none() && !riichi {
        return None;
    }
    let kinds = |k: MeldKind| -> Vec<u8> {
        ctx.melds
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
    let agari = AgariCalculator {
        tehai: counts,
        is_menzen: riichi,
        chis: &chis,
        pons: &pons,
        minkans: &minkans,
        ankans: &ankans,
        bakaze: ctx.round_wind,
        jikaze: ctx.seat_wind,
        winning_tile: winning,
        is_ron: true,
    }
    .agari(u8::from(riichi), dora(ctx, counts, held))?;
    let mut yaku: Vec<Yaku> =
        named.map_or_else(Vec::new, |n| n.yaku.iter().map(|(y, _)| *y).collect());
    let (han, fu, yakuman) = match agari {
        Agari::Normal { fu, han } => {
            if riichi {
                yaku.insert(0, Yaku::Riichi);
            }
            (han, if han >= 5 { 0 } else { fu }, 0)
        }
        Agari::Yakuman(n) => (0, 0, n),
    };
    Some(Value {
        yaku,
        han,
        fu,
        yakuman,
        points: agari.point(ctx.dealer).ron,
    })
}

/// Dora in the finished hand: indicated kinds in it and the melds, and the red fives the
/// hand keeps (missing tiles are taken to be plain).
fn dora(ctx: &Context, counts: &TileCounts, held: &TileCounts) -> u8 {
    let mut n = 0u8;
    for indicator in ctx.dora_indicators {
        let k = next(indicator.kind());
        n += counts[usize::from(k)];
        n += ctx.meld_tiles.iter().filter(|t| t.kind() == k).count() as u8;
    }
    for t in ctx.hand.iter().filter(|t| t.is_red()) {
        let k = usize::from(t.kind());
        // A red five counts while the finished hand keeps all the fives the hand has.
        if counts[k] >= held[k] {
            n += 1;
        }
    }
    n + ctx.meld_tiles.iter().filter(|t| t.is_red()).count() as u8
}

/// The dora a kind indicates.
fn next(k: u8) -> u8 {
    match k {
        0..=26 => k / 9 * 9 + (k % 9 + 1) % 9,
        27..=30 => 27 + (k - 27 + 1) % 4,
        _ => 31 + (k - 31 + 1) % 3,
    }
}

/// The target for finished `counts`: the missing tiles, the best last tile to win on and
/// the probability of drawing them all.
fn score(ctx: &Context, held: &TileCounts, counts: &TileCounts) -> Option<Target> {
    let mut missing = Vec::new();
    for k in 0..KINDS {
        let want = counts[usize::from(k)];
        let have = held[usize::from(k)];
        for _ in have..want.max(have) {
            missing.push(Tile::from_kind(k)?);
        }
    }
    if missing.is_empty() {
        return None;
    }
    let mut kinds: Vec<u8> = missing.iter().map(|t| t.kind()).collect();
    kinds.dedup();
    let best = kinds
        .iter()
        .filter_map(|&w| value(ctx, counts, w, held))
        .max_by_key(|v| (v.yakuman, v.points, v.han))?;
    let probability = draw_all(&missing, &unseen(ctx, held), ctx.draws_left);
    let mut tiles = Vec::new();
    for k in 0..KINDS {
        for _ in 0..counts[usize::from(k)] {
            tiles.push(Tile::from_kind(k)?);
        }
    }
    Some(Target {
        tiles,
        missing,
        yaku: best.yaku,
        han: best.han,
        fu: best.fu,
        yakuman: best.yakuman,
        points: best.points,
        probability,
    })
}

/// The probability of drawing every tile of `missing` within `draws` draws from the
/// unseen tiles, each draw equally likely to be any of them.
fn draw_all(missing: &[Tile], unseen: &TileCounts, draws: u8) -> f32 {
    let mut need: Vec<(usize, u8)> = Vec::new();
    for t in missing {
        let k = usize::from(t.kind());
        match need.iter_mut().find(|(kind, _)| *kind == k) {
            Some((_, n)) => *n += 1,
            None => need.push((k, 1)),
        }
    }
    let pool: u32 = unseen.iter().map(|&n| u32::from(n)).sum();
    if need.len() > 4 || pool == 0 {
        return 0.0;
    }
    // States: how many of each needed kind are drawn so far (capped at the need).
    let dims: Vec<usize> = need.iter().map(|&(_, n)| usize::from(n) + 1).collect();
    let states: usize = dims.iter().product();
    let decode = |mut s: usize| -> Vec<u8> {
        dims.iter()
            .map(|&d| {
                let v = (s % d) as u8;
                s /= d;
                v
            })
            .collect()
    };
    let encode = |got: &[u8]| -> usize {
        let mut s = 0;
        for (i, &g) in got.iter().enumerate().rev() {
            s = s * dims[i] + usize::from(g);
        }
        s
    };
    let mut p = vec![0f64; states];
    p[0] = 1.0;
    let done = encode(&need.iter().map(|&(_, n)| n).collect::<Vec<_>>());
    for drawn in 0..u32::from(draws) {
        let left = f64::from(pool.saturating_sub(drawn));
        if left <= 0.0 {
            break;
        }
        let mut next = vec![0f64; states];
        for s in 0..states {
            if p[s] == 0.0 {
                continue;
            }
            if s == done {
                next[s] += p[s];
                continue;
            }
            let got = decode(s);
            let mut moved = 0.0;
            for (i, &(k, n)) in need.iter().enumerate() {
                if got[i] >= n {
                    continue;
                }
                // Copies of this kind still in the pool, approximately: the drawn ones of
                // it are gone.
                let copies = f64::from(unseen[k]) - f64::from(got[i]);
                if copies <= 0.0 {
                    continue;
                }
                let q = copies / left;
                let mut g = got.clone();
                g[i] += 1;
                next[encode(&g)] += p[s] * q;
                moved += q;
            }
            next[s] += p[s] * (1.0 - moved).max(0.0);
        }
        p = next;
    }
    p[done] as f32
}

/// Broad directions for a hand far from tenpai.
fn directions(ctx: &Context) -> Vec<Direction> {
    let held = analysis::counts(ctx.hand);
    let mut all = held;
    for t in ctx.meld_tiles {
        all[usize::from(t.kind())] += 1;
    }
    let total: u8 = all.iter().sum();
    let honors: u8 = all[27..].iter().sum();
    let mut out = Vec::new();
    let suit = (0..3u8)
        .max_by_key(|&s| {
            all[usize::from(s) * 9..usize::from(s) * 9 + 9]
                .iter()
                .sum::<u8>()
        })
        .unwrap_or(0);
    let in_suit: u8 = all[usize::from(suit) * 9..usize::from(suit) * 9 + 9]
        .iter()
        .sum();
    if in_suit + honors + 3 >= total {
        out.push(Direction::Flush { suit });
    }
    if ctx.melds.is_empty() && held.iter().filter(|&&n| n >= 2).count() >= 4 {
        out.push(Direction::Chiitoitsu);
    }
    for k in [31, 32, 33, ctx.round_wind, ctx.seat_wind] {
        if held[usize::from(k)] >= 2
            && let Some(tile) = Tile::from_kind(k)
            && !out.contains(&Direction::Yakuhai { tile })
        {
            out.push(Direction::Yakuhai { tile });
        }
    }
    let outside: u8 = (0..KINDS)
        .filter(|&k| k >= 27 || k % 9 == 0 || k % 9 == 8)
        .map(|k| all[usize::from(k)])
        .sum();
    if outside <= 2 {
        out.push(Direction::Tanyao);
    }
    out.truncate(3);
    out
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::tile::parse_tiles;

    fn ctx<'a>(hand: &'a [Tile], visible: &'a TileCounts, dora: &'a [Tile]) -> Context<'a> {
        Context {
            hand,
            melds: &[],
            meld_tiles: &[],
            dora_indicators: dora,
            visible,
            round_wind: 27,
            seat_wind: 28,
            dealer: false,
            draws_left: 12,
        }
    }

    fn names(t: &Target) -> Vec<&'static str> {
        t.yaku.iter().map(|y| y.name()).collect()
    }

    /// The design's example: 1-8m, 3-4p, 5s, CC and a drawn N. Ittsu with riichi is
    /// among the targets, worth more than riichi alone.
    #[test]
    fn finds_the_designs_example_routes() {
        let hand = parse_tiles("1m 2m 3m 4m 5m 6m 7m 8m 3p 4p 5s C C N").unwrap();
        let visible = [0; 34];
        let dora = parse_tiles("2p").unwrap();
        let found = search(&ctx(&hand, &visible, &dora));
        for t in &found.targets {
            let tiles: Vec<String> = t.tiles.iter().map(|x| x.to_string()).collect();
            let missing: Vec<String> = t.missing.iter().map(|x| x.to_string()).collect();
            println!(
                "{:?} {} han {} fu {} points {:.0}% | {} | missing {}",
                names(t),
                t.han,
                t.fu,
                t.points,
                100.0 * t.probability,
                tiles.join(""),
                missing.join(" ")
            );
        }
        assert!(found.directions.is_empty());
        assert!(!found.targets.is_empty() && found.targets.len() <= 3);
        let ittsu = found
            .targets
            .iter()
            .find(|t| t.yaku.contains(&Yaku::Ittsu))
            .expect("an ittsu target");
        assert_eq!(names(ittsu)[0], "立直");
        assert!(ittsu.missing.iter().any(|t| t.to_string() == "9m"));
        for t in &found.targets {
            assert_eq!(t.tiles.len(), 14);
            assert!(t.probability > 0.0 && t.probability <= 1.0, "{t:?}");
            assert!(t.points >= 1000);
        }
    }

    /// A tenpai hand's waits with what each is worth: 23m on 1m (riichi only) and 4m
    /// (riichi, tanyao? no: the 9p pair is a terminal).
    #[test]
    fn values_each_wait() {
        let hand = parse_tiles("2m 3m 4m 5m 6m 7m 3p 4p 5p 9p 9p 6s 7s").unwrap();
        let visible = [0; 34];
        let found = waits(&ctx(&hand, &visible, &[]));
        let tiles: Vec<String> = found.iter().map(|w| w.tile.to_string()).collect();
        assert_eq!(tiles, ["5s", "8s"]);
        for w in &found {
            let n: Vec<&str> = w.yaku.iter().map(|y| y.name()).collect();
            assert_eq!(n, ["立直", "平和"]);
            assert_eq!((w.han, w.fu, w.points), (2, 30, 2000));
        }
    }

    /// Tiles that are all seen are not waited for.
    #[test]
    fn does_not_wait_for_tiles_all_seen() {
        let hand = parse_tiles("1m 2m 3m 4m 5m 6m 7m 8m 3p 4p 5s C C N").unwrap();
        let mut visible = [0; 34];
        visible[8] = 4;
        let found = search(&ctx(&hand, &visible, &[]));
        assert!(
            found
                .targets
                .iter()
                .all(|t| t.missing.iter().all(|m| m.kind() != 8))
        );
    }

    #[test]
    fn far_hands_get_directions() {
        let hand = parse_tiles("1m 4m 7m 9m 2p 5p 8p 3s 6s 9s E S C C").unwrap();
        let visible = [0; 34];
        let found = search(&ctx(&hand, &visible, &[]));
        assert!(found.targets.is_empty());
        assert!(found.directions.contains(&Direction::Yakuhai {
            tile: parse_tiles("C").unwrap()[0]
        }));
    }

    /// Hands three or four steps from tenpai get targets, marked rough.
    #[test]
    fn far_hands_get_rough_targets() {
        let visible = [0; 34];
        let (mut far, mut found_any) = (0, 0);
        for seed in 0..4 {
            let game = crate::game::sim::play(seed, 2);
            for moment in &game.moments {
                let hand = moment.scene.own_tiles();
                let shanten = analysis::shanten(&hand).unwrap_or(0);
                if !moment.scene.melds[0].is_empty() || !(ROUGH..FAR).contains(&shanten) {
                    continue;
                }
                far += 1;
                let found = search(&ctx(&hand, &visible, &[]));
                assert!(found.rough && found.directions.is_empty(), "{hand:?}");
                found_any += usize::from(!found.targets.is_empty());
            }
        }
        assert!(far > 0 && found_any == far, "{found_any} of {far}");
    }

    #[test]
    fn drawing_probability_behaves() {
        let mut unseen = [3u8; 34];
        unseen[0] = 4;
        let one = parse_tiles("1m").unwrap();
        let p1 = draw_all(&one, &unseen, 10);
        let p2 = draw_all(&one, &unseen, 20);
        assert!(p1 > 0.2 && p1 < p2 && p2 < 1.0, "{p1} {p2}");
        let two = parse_tiles("1m 1m").unwrap();
        assert!(draw_all(&two, &unseen, 10) < p1);
        unseen[0] = 0;
        assert_eq!(draw_all(&one, &unseen, 10), 0.0);
    }

    /// Search time on the simulated games' hands (run with --release for the budget).
    #[test]
    fn search_time() {
        let mut times = Vec::new();
        // Fewer games in a debug build, where only the release timing counts.
        let games = if cfg!(debug_assertions) { 2 } else { 10 };
        for seed in 0..games {
            let game = crate::game::sim::play(seed, 2);
            for moment in &game.moments {
                let hand: Vec<Tile> = moment
                    .scene
                    .hand
                    .iter()
                    .copied()
                    .chain(moment.scene.drawn)
                    .collect();
                let visible = [0; 34];
                let c = Context {
                    melds: &[],
                    ..ctx(&hand, &visible, &[])
                };
                if !moment.scene.melds[0].is_empty() {
                    continue;
                }
                let start = Instant::now();
                let _ = search(&c);
                times.push(start.elapsed().as_secs_f64() * 1e3);
            }
        }
        times.sort_by(f64::total_cmp);
        let p95 = times[times.len() * 95 / 100];
        println!(
            "{} searches: median {:.2} ms, p95 {p95:.2} ms, max {:.2} ms",
            times.len(),
            times[times.len() / 2],
            times[times.len() - 1]
        );
        if !cfg!(debug_assertions) {
            assert!(p95 < 30.0, "p95 {p95:.1} ms");
        }
    }
}
