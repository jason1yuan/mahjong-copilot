//! Basic defense (#12): who threatens to win, how dangerous each tile in hand is against
//! them and why, and whether to push on, play carefully or fold.
//!
//! - Threats: every player in riichi, and open hands that look close (three calls, or two
//!   with a value honor or a dora among them). Open-hand threats are a guess.
//! - Danger of a tile against a threat, from safest:
//!   - safe: in the threat's river, or (against riichi) discarded by anyone after the
//!     declaration without a ron (the riichi player would be furiten on it);
//!   - nearly safe: an honor with three copies seen; a number every two-sided wait on
//!     which is ruled out by a wall (all four of a neighbour seen);
//!   - somewhat: suji (the tiles three away on each side that a two-sided wait would need
//!     are safe), one chance (a neighbour with three seen), other honors, terminals;
//!   - dangerous: anything else; a dora or a tile next to one is one level worse.
//!   - against several threats a tile counts as its worst.
//! - Verdict, against at least one threat: push when tenpai with four or more tiles to
//!   win on or a hand worth a mangan; play carefully when tenpai otherwise, or one step
//!   away with a mangan in reach; fold otherwise. Mortal decides the play; this explains.
//!
//! Seats are counted from the player in turn order: 1 right, 2 across, 3 left.

use riichi::mjai::Event;
use riichi::tile::Tile as RTile;
use serde::Serialize;

use crate::analysis::{self, TileCounts};
use crate::tile::{KINDS, Tile};

/// Points of a mangan for a non-dealer: worth pushing for.
const MANGAN: i32 = 8000;
/// Tiles to win on that make a wait worth pushing.
const GOOD_WAIT: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Safe,
    NearlySafe,
    Somewhat,
    Dangerous,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    /// In the threat's river, or passed after the riichi.
    Genbutsu,
    /// Three or four copies of the honor are seen.
    HonorSeen,
    /// Two-sided waits on it are walled off.
    Wall,
    Suji,
    OneChance,
    Honor,
    Terminal,
    /// No protection.
    Plain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Threat {
    /// 1 right, 2 across, 3 left.
    pub seat: u8,
    pub riichi: bool,
    pub calls: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TileDanger {
    pub tile: Tile,
    pub level: Level,
    pub reason: Reason,
    /// Raised a level for being a dora or next to one.
    pub dora: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Push,
    Careful,
    Fold,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Defense {
    pub threats: Vec<Threat>,
    /// One entry a kind in the hand, in kind order; empty without threats.
    pub tiles: Vec<TileDanger>,
    pub verdict: Option<Verdict>,
    /// The safest tile in hand, when there is something to defend against.
    pub safest: Option<Tile>,
}

/// The player's own standing, for the verdict.
#[derive(Clone, Copy, Debug, Default)]
pub struct Own {
    /// After the best discard on the player's turn.
    pub shanten: i8,
    /// Tiles that improve the hand (or win it, when tenpai).
    pub ukeire: u8,
    /// The best target hand's or wait's points, if known.
    pub points: Option<i32>,
}

/// What a seat did this round, from the log.
#[derive(Clone, Debug, Default)]
struct Seat {
    discards: Vec<u8>,
    /// Index in the round's discards of the riichi declaration's discard.
    riichi_at: Option<usize>,
    calls: u8,
    /// Called a value honor or a dora.
    valuable_call: bool,
}

fn core_tile(t: RTile) -> Option<Tile> {
    t.to_string().parse().ok()
}

fn kind(t: RTile) -> Option<u8> {
    core_tile(t).map(Tile::kind)
}

/// The dora a kind indicates.
fn indicated(k: u8) -> u8 {
    match k {
        0..=26 => k / 9 * 9 + (k % 9 + 1) % 9,
        27..=30 => 27 + (k - 27 + 1) % 4,
        _ => 31 + (k - 31 + 1) % 3,
    }
}

/// How dangerous each tile in `hand` is, and the verdict. `events` are the round's, from
/// its start; `me` is the player's absolute seat; `visible` counts the tiles seen outside
/// the hand.
pub fn assess(events: &[Event], me: u8, hand: &[Tile], visible: &TileCounts, own: &Own) -> Defense {
    let mut seats: [Seat; 4] = Default::default();
    // Every discard of the round in order, by absolute seat.
    let mut order: Vec<(u8, u8)> = Vec::new();
    let mut dora: Vec<u8> = Vec::new();
    let mut declaring = [false; 4];
    let mut value_honors = vec![31u8, 32, 33];
    for event in events {
        match event {
            Event::StartKyoku {
                bakaze,
                dora_marker,
                oya,
                ..
            } => {
                seats = Default::default();
                order.clear();
                dora.clear();
                declaring = [false; 4];
                value_honors = vec![31, 32, 33];
                if let Some(k) = kind(*bakaze) {
                    value_honors.push(k);
                }
                // The player's seat wind.
                value_honors.push(27 + (me + 4 - oya) % 4);
                dora.extend(kind(*dora_marker).map(indicated));
            }
            Event::Dora { dora_marker } => dora.extend(kind(*dora_marker).map(indicated)),
            Event::Reach { actor } => declaring[usize::from(*actor)] = true,
            Event::Dahai { actor, pai, .. } => {
                let Some(k) = kind(*pai) else { continue };
                let a = usize::from(*actor);
                if declaring[a] {
                    declaring[a] = false;
                    seats[a].riichi_at = Some(order.len());
                }
                seats[a].discards.push(k);
                order.push((*actor, k));
            }
            Event::Chi { actor, pai, .. }
            | Event::Pon { actor, pai, .. }
            | Event::Daiminkan { actor, pai, .. } => {
                let a = usize::from(*actor);
                seats[a].calls += 1;
                if let Some(k) = kind(*pai)
                    && (dora.contains(&k)
                        || (!matches!(event, Event::Chi { .. }) && value_honors.contains(&k)))
                {
                    seats[a].valuable_call = true;
                }
            }
            _ => {}
        }
    }

    // Value honors for an open threat are its own, but the player's are a fair guess
    // (dragons are the same for all); the check above is a rough one.
    let threats: Vec<Threat> = (1..4u8)
        .filter_map(|rel| {
            let s = &seats[usize::from((me + rel) % 4)];
            let riichi = s.riichi_at.is_some();
            let open = s.calls >= 3 || (s.calls >= 2 && s.valuable_call);
            (riichi || open).then_some(Threat {
                seat: rel,
                riichi,
                calls: s.calls,
            })
        })
        .collect();
    if threats.is_empty() {
        return Defense::default();
    }

    let held = analysis::counts(hand);
    let mut seen = [0u8; KINDS as usize];
    for k in 0..usize::from(KINDS) {
        seen[k] = held[k] + visible[k];
    }
    let mut tiles = Vec::new();
    for k in 0..KINDS {
        if held[usize::from(k)] == 0 {
            continue;
        }
        let worst = threats
            .iter()
            .map(|t| {
                let seat = &seats[usize::from((me + t.seat) % 4)];
                let safe: Vec<u8> = safe_against(seat, &order);
                danger(k, &safe, &seen, &dora)
            })
            .max_by_key(|d| (d.level, !matches!(d.reason, Reason::Genbutsu)))
            .unwrap_or_else(|| unreachable!("threats is not empty"));
        tiles.push(TileDanger {
            tile: Tile::from_kind(k).unwrap_or_else(|| unreachable!("k < KINDS")),
            ..worst
        });
    }
    let safest = tiles
        .iter()
        .min_by_key(|d| (d.level, d.dora))
        .map(|d| d.tile);
    Defense {
        threats,
        tiles,
        verdict: Some(verdict(own)),
        safest,
    }
}

/// Kinds safe against a seat: its own discards, and if in riichi everything discarded
/// after its declaration.
fn safe_against(seat: &Seat, order: &[(u8, u8)]) -> Vec<u8> {
    let mut safe = seat.discards.clone();
    if let Some(at) = seat.riichi_at {
        safe.extend(order[at..].iter().map(|&(_, k)| k));
    }
    safe.sort_unstable();
    safe.dedup();
    safe
}

fn danger(k: u8, safe: &[u8], seen: &TileCounts, dora: &[u8]) -> TileDanger {
    let tile = Tile::from_kind(k).unwrap_or_else(|| unreachable!("k < KINDS"));
    if safe.contains(&k) {
        return TileDanger {
            tile,
            level: Level::Safe,
            reason: Reason::Genbutsu,
            dora: false,
        };
    }
    let (level, reason) = if k >= 27 {
        if seen[usize::from(k)] >= 3 {
            (Level::NearlySafe, Reason::HonorSeen)
        } else {
            (Level::Somewhat, Reason::Honor)
        }
    } else {
        number_danger(k, safe, seen)
    };
    // A dora, or a number next to one, is worth more to the threat: one level worse.
    let near_dora = dora
        .iter()
        .any(|&d| d == k || (k < 27 && d < 27 && d / 9 == k / 9 && d.abs_diff(k) == 1));
    let level = if near_dora {
        match level {
            Level::Safe => Level::Safe,
            Level::NearlySafe => Level::Somewhat,
            Level::Somewhat | Level::Dangerous => Level::Dangerous,
        }
    } else {
        level
    };
    TileDanger {
        tile,
        level,
        reason,
        dora: near_dora,
    }
}

/// A number tile not in the threat's safe tiles.
fn number_danger(k: u8, safe: &[u8], seen: &TileCounts) -> (Level, Reason) {
    let n = k % 9;
    let base = k - n;
    let tile = |m: i8| -> Option<u8> { (0..9).contains(&m).then(|| base + m as u8) };
    let n = n as i8;
    // Two-sided waits winning on this tile: holding n+1, n+2 (waiting on n and n+3) or
    // n-2, n-1 (waiting on n-3 and n).
    let shapes: Vec<[u8; 2]> = [
        tile(n + 1)
            .zip(tile(n + 2))
            .filter(|_| n + 3 <= 8 || n <= 5),
        tile(n - 2).zip(tile(n - 1)),
    ]
    .into_iter()
    .flatten()
    .map(|(a, b)| [a, b])
    .collect();
    // Wall: every shape needs a tile with all four seen.
    let walled = !shapes.is_empty()
        && shapes
            .iter()
            .all(|s| s.iter().any(|&t| seen[usize::from(t)] >= 4));
    if walled {
        return (Level::NearlySafe, Reason::Wall);
    }
    // Suji: the other end of every two-sided wait is safe.
    let suji =
        [tile(n - 3), tile(n + 3)]
            .into_iter()
            .enumerate()
            .all(|(side, other)| match other {
                Some(o) => safe.contains(&o),
                // No wait from that side: 1-3 have none below, 7-9 none above.
                None => (side == 0 && n < 3) || (side == 1 && n > 5),
            });
    if suji {
        return (Level::Somewhat, Reason::Suji);
    }
    let one_chance = shapes
        .iter()
        .all(|s| s.iter().any(|&t| seen[usize::from(t)] >= 3));
    if !shapes.is_empty() && one_chance {
        return (Level::Somewhat, Reason::OneChance);
    }
    if n == 0 || n == 8 {
        return (Level::Somewhat, Reason::Terminal);
    }
    (Level::Dangerous, Reason::Plain)
}

fn verdict(own: &Own) -> Verdict {
    let big = own.points.is_some_and(|p| p >= MANGAN);
    match own.shanten {
        s if s <= 0 && (own.ukeire >= GOOD_WAIT || big) => Verdict::Push,
        s if s <= 0 => Verdict::Careful,
        1 if big => Verdict::Careful,
        _ => Verdict::Fold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::parse_tiles;

    fn ev(json: &str) -> Event {
        serde_json::from_str(json).unwrap()
    }

    fn start(dora_marker: &str) -> Event {
        let unknown = format!("[{}]", vec!["\"?\""; 13].join(","));
        ev(&format!(
            r#"{{"type":"start_kyoku","bakaze":"E","dora_marker":"{dora_marker}","kyoku":1,"honba":0,"kyotaku":0,"oya":0,"scores":[25000,25000,25000,25000],"tehais":[{unknown},{unknown},{unknown},{unknown}]}}"#
        ))
    }

    fn discard(actor: u8, pai: &str) -> Event {
        ev(&format!(
            r#"{{"type":"dahai","actor":{actor},"pai":"{pai}","tsumogiri":false}}"#
        ))
    }

    fn reach(actor: u8) -> Event {
        ev(&format!(r#"{{"type":"reach","actor":{actor}}}"#))
    }

    fn levels(d: &Defense) -> Vec<(String, Level, Reason)> {
        d.tiles
            .iter()
            .map(|t| (t.tile.to_string(), t.level, t.reason))
            .collect()
    }

    fn visible_of(events: &[Event]) -> TileCounts {
        let mut v = [0u8; 34];
        for e in events {
            if let Event::Dahai { pai, .. } = e {
                v[usize::from(kind(*pai).unwrap())] += 1;
            }
        }
        v
    }

    fn own(shanten: i8, ukeire: u8, points: Option<i32>) -> Own {
        Own {
            shanten,
            ukeire,
            points,
        }
    }

    /// Against a riichi on the right: its river and what came after the declaration are
    /// safe, suji of a safe 4 is somewhat safe, a plain middle tile is dangerous.
    #[test]
    fn rates_tiles_against_a_riichi() {
        let events = vec![
            start("C"),
            discard(0, "1p"),
            discard(1, "4m"),
            discard(2, "9s"),
            discard(3, "W"),
            discard(0, "2p"),
            reach(1),
            discard(1, "8p"),
            discard(2, "3s"),
        ];
        let hand = parse_tiles("1m 4m 7m 5p 3s 9s E E 6p").unwrap();
        let d = assess(&events, 0, &hand, &visible_of(&events), &own(2, 10, None));
        assert_eq!(
            d.threats,
            [Threat {
                seat: 1,
                riichi: true,
                calls: 0
            }]
        );
        let l = levels(&d);
        let get = |t: &str| l.iter().find(|(x, _, _)| x == t).unwrap().clone();
        assert_eq!(get("4m").1, Level::Safe, "own discard");
        assert_eq!(
            get("3s"),
            ("3s".into(), Level::Safe, Reason::Genbutsu),
            "passed after riichi"
        );
        assert_eq!(
            get("1m"),
            ("1m".into(), Level::Somewhat, Reason::Suji),
            "suji of 4m"
        );
        assert_eq!(get("7m"), ("7m".into(), Level::Somewhat, Reason::Suji));
        assert_eq!(get("5p").1, Level::Dangerous);
        assert_eq!(get("E"), ("E".into(), Level::Somewhat, Reason::Honor));
        assert_eq!(d.verdict, Some(Verdict::Fold));
        let safest = d.safest.unwrap().to_string();
        assert!(
            safest == "4m" || safest == "3s" || safest == "9s",
            "{safest}"
        );
    }

    /// A 9s can only be won on two-sided by 7s-8s: with all four 8s seen it is walled.
    #[test]
    fn a_wall_makes_a_tile_nearly_safe() {
        let events = vec![start("C"), reach(1), discard(1, "1m")];
        let hand = parse_tiles("9s").unwrap();
        let eight = usize::from(parse_tiles("8s").unwrap()[0].kind());
        let mut visible = visible_of(&events);
        visible[eight] = 3;
        let d = assess(&events, 0, &hand, &visible, &own(3, 0, None));
        assert_eq!(
            levels(&d)[0],
            ("9s".into(), Level::Somewhat, Reason::OneChance)
        );
        visible[eight] = 4;
        let d = assess(&events, 0, &hand, &visible, &own(3, 0, None));
        assert_eq!(
            levels(&d)[0],
            ("9s".into(), Level::NearlySafe, Reason::Wall)
        );
    }

    /// A dora is one level more dangerous.
    #[test]
    fn dora_raise_the_danger() {
        let events = vec![start("3p"), reach(2), discard(2, "1s")];
        let hand = parse_tiles("4p E").unwrap();
        let d = assess(&events, 0, &hand, &visible_of(&events), &own(3, 0, None));
        let four = d.tiles.iter().find(|t| t.tile.to_string() == "4p").unwrap();
        assert!(four.dora);
        assert_eq!(four.level, Level::Dangerous);
    }

    /// Open hands count as threats only when they look close.
    #[test]
    fn open_hands_threaten_when_close() {
        let pon = |actor: u8, pai: &str| {
            ev(&format!(
                r#"{{"type":"pon","actor":{actor},"target":0,"pai":"{pai}","consumed":["{pai}","{pai}"]}}"#
            ))
        };
        let events = vec![start("S"), pon(3, "2m"), pon(3, "5p")];
        let hand = parse_tiles("1m").unwrap();
        assert!(
            assess(&events, 0, &hand, &[0; 34], &own(2, 0, None))
                .threats
                .is_empty()
        );
        let events = vec![start("S"), pon(3, "C"), pon(3, "5p")];
        let d = assess(&events, 0, &hand, &[0; 34], &own(2, 0, None));
        assert_eq!(
            d.threats,
            [Threat {
                seat: 3,
                riichi: false,
                calls: 2
            }]
        );
    }

    #[test]
    fn no_threat_no_defense() {
        let events = vec![start("S"), discard(1, "1m")];
        let d = assess(
            &events,
            0,
            &parse_tiles("5m").unwrap(),
            &[0; 34],
            &own(1, 8, None),
        );
        assert_eq!(d, Defense::default());
    }

    /// In the simulated games, once another player is in riichi every tile in hand has a
    /// danger, and the riichi player's own discards are safe.
    #[test]
    fn every_tile_has_a_danger_after_a_riichi() {
        let mut checked = 0;
        for seed in 0..20 {
            let game = crate::game::sim::play(seed, 2);
            for moment in &game.moments {
                let log = &game.log[..moment.events];
                let start = log
                    .iter()
                    .rposition(|e| matches!(e, Event::StartKyoku { .. }))
                    .unwrap_or(0);
                let round = &log[start..];
                let riichi = round
                    .iter()
                    .any(|e| matches!(e, Event::ReachAccepted { actor, .. } if *actor != game.me));
                if !riichi {
                    continue;
                }
                let hand = moment.scene.own_tiles();
                let mut visible = [0u8; 34];
                for t in moment.scene.seen() {
                    visible[usize::from(t.kind())] += 1;
                }
                for t in &hand {
                    visible[usize::from(t.kind())] -= 1;
                }
                let d = assess(round, game.me, &hand, &visible, &own(2, 0, None));
                assert!(d.threats.iter().any(|t| t.riichi), "seed {seed}");
                let mut kinds: Vec<u8> = hand.iter().map(|t| t.kind()).collect();
                kinds.sort_unstable();
                kinds.dedup();
                let rated: Vec<u8> = d.tiles.iter().map(|t| t.tile.kind()).collect();
                assert_eq!(rated, kinds, "seed {seed}");
                assert!(d.safest.is_some() && d.verdict.is_some());
                checked += 1;
            }
        }
        assert!(checked > 20, "{checked}");
    }

    #[test]
    fn verdicts() {
        assert_eq!(verdict(&own(0, 6, Some(2000))), Verdict::Push);
        assert_eq!(verdict(&own(0, 2, Some(1300))), Verdict::Careful);
        assert_eq!(verdict(&own(0, 2, Some(8000))), Verdict::Push);
        assert_eq!(verdict(&own(1, 20, Some(8000))), Verdict::Careful);
        assert_eq!(verdict(&own(1, 20, Some(3900))), Verdict::Fold);
        assert_eq!(verdict(&own(2, 30, None)), Verdict::Fold);
    }
}
