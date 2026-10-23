//! A small seeded game simulator for the state engine's tests (#10): four simple bots
//! play whole rounds (draws, discards, chi, pon, the three kans, riichi, wins after riichi
//! and draws at the end of the wall), and from one seat it records the scenes the screen
//! would show and the mjai log that seat would see. No outside game records are needed.

use riichi::mjai::Event;
use riichi::state::PlayerState;
use riichi::tile::Tile as RTile;

use super::engine::{called_view, concealed_kan_view};
use super::scene::{MeldView, RiverTile, RoundLabel, Scene};
use super::{Engine, Rejected};
use crate::analysis::{rank_discards, shanten};
use crate::tile::Tile;

/// xorshift64*: enough randomness for tests, the same for a seed everywhere.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

#[derive(Default, Clone)]
struct Player {
    hand: Vec<Tile>,
    melds: Vec<MeldView>,
    river: Vec<RiverTile>,
    riichi: bool,
    sideways_next: bool,
}

impl Player {
    fn is_closed(&self) -> bool {
        self.melds.iter().all(MeldView::is_concealed_kan)
    }

    fn take(&mut self, tile: Tile) {
        if let Some(i) = self.hand.iter().position(|t| *t == tile) {
            self.hand.remove(i);
        }
    }

    fn count(&self, kind: u8) -> usize {
        self.hand.iter().filter(|t| t.kind() == kind).count()
    }

    /// The hand's tiles of `kind`, plain fives before red ones.
    fn of_kind(&self, kind: u8, n: usize) -> Vec<Tile> {
        let mut tiles: Vec<Tile> = self
            .hand
            .iter()
            .filter(|t| t.kind() == kind)
            .copied()
            .collect();
        tiles.sort_by_key(|t| t.is_red());
        tiles.truncate(n);
        tiles
    }
}

/// A recorded moment: the scene from the watched seat, and how much of the log it shows.
pub struct Moment {
    pub scene: Scene,
    pub events: usize,
    /// The round's last moment or its first: never skipped.
    pub keep: bool,
}

pub struct Game {
    /// The watched seat's log.
    pub log: Vec<Event>,
    pub moments: Vec<Moment>,
    pub me: u8,
}

struct Sim {
    rng: Rng,
    me: u8,
    players: [Player; 4],
    wall: Vec<Tile>,
    dead: Vec<Tile>,
    dora: Vec<Tile>,
    kans: usize,
    pending_dora: usize,
    kyoku: u8,
    oya: u8,
    log: Vec<Event>,
    moments: Vec<Moment>,
    /// The watched seat's last draw, shown apart until it discards or melds.
    drawn: Option<Tile>,
    /// Kinds the caller may not discard right after its call (swap-calling).
    forbidden: Vec<u8>,
}

fn rt(tile: Tile) -> RTile {
    tile.to_string().parse().unwrap()
}

fn unknown() -> RTile {
    "?".parse().unwrap()
}

fn all_tiles() -> Vec<Tile> {
    let mut tiles = Vec::new();
    for kind in 0..34u8 {
        for copy in 0..4 {
            let tile = Tile::from_kind(kind).unwrap();
            tiles.push(if copy == 0 {
                tile.to_red().unwrap_or(tile)
            } else {
                tile
            });
        }
    }
    tiles
}

/// Whether 3n+2 tiles make a winning hand.
fn complete(tiles: &[Tile]) -> bool {
    shanten(tiles).is_ok_and(|s| s < 0)
}

impl Sim {
    /// Seat `seat` as the watched seat sees it.
    fn rel(&self, seat: u8) -> u8 {
        (seat + 4 - self.me) % 4
    }

    fn record(&mut self, keep: bool) {
        let me = usize::from(self.me);
        let mut hand = self.players[me].hand.clone();
        let drawn = self.drawn;
        if let Some(tile) = drawn {
            let i = hand.iter().position(|t| *t == tile).unwrap();
            hand.remove(i);
        }
        let mut scene = Scene {
            hand,
            drawn,
            dora: self.dora.clone(),
            round: Some(RoundLabel {
                wind: "E".parse().unwrap(),
                number: self.kyoku,
            }),
            dealer: Some(self.rel(self.oya)),
            honba: Some(0),
            riichi_sticks: Some(0),
            scores: Some([25000; 4]),
            tiles_left: Some(u8::try_from(self.wall.len()).unwrap()),
            ..Scene::default()
        };
        for seat in 0..4u8 {
            let r = usize::from(self.rel(seat));
            scene.rivers[r].clone_from(&self.players[usize::from(seat)].river);
            scene.melds[r].clone_from(&self.players[usize::from(seat)].melds);
        }
        self.moments.push(Moment {
            scene,
            events: self.log.len(),
            keep,
        });
    }

    /// Logs an event: the watched seat sees others' draws and starting hands as unknown,
    /// and others' discards as from the hand.
    fn emit(&mut self, event: Event) {
        let event = match event {
            Event::Tsumo { actor, .. } if actor != self.me => Event::Tsumo {
                actor,
                pai: unknown(),
            },
            Event::Dahai { actor, pai, .. } if actor != self.me => Event::Dahai {
                actor,
                pai,
                tsumogiri: false,
            },
            Event::StartKyoku {
                bakaze,
                dora_marker,
                kyoku,
                honba,
                kyotaku,
                oya,
                scores,
                mut tehais,
            } => {
                for (seat, tehai) in tehais.iter_mut().enumerate() {
                    if seat != usize::from(self.me) {
                        *tehai = [unknown(); 13];
                    }
                }
                Event::StartKyoku {
                    bakaze,
                    dora_marker,
                    kyoku,
                    honba,
                    kyotaku,
                    oya,
                    scores,
                    tehais,
                }
            }
            other => other,
        };
        self.log.push(event);
    }

    fn reveal_dora(&mut self) {
        let tile = self.dead[4 + 2 * self.dora.len()];
        self.dora.push(tile);
        self.emit(Event::Dora {
            dora_marker: rt(tile),
        });
    }

    fn play_round(&mut self, kyoku: u8) {
        self.kyoku = kyoku;
        self.oya = kyoku - 1;
        let mut tiles = all_tiles();
        for i in (1..tiles.len()).rev() {
            let j = self.rng.below(i + 1);
            tiles.swap(i, j);
        }
        self.dead = tiles.split_off(tiles.len() - 14);
        self.wall = tiles;
        self.players = Default::default();
        self.kans = 0;
        self.pending_dora = 0;
        self.drawn = None;
        self.dora = vec![self.dead[4]];
        for seat in 0..4 {
            let hand: Vec<Tile> = self.wall.drain(..13).collect();
            self.players[seat].hand = hand;
        }
        let tehais = self.players.clone().map(|p| {
            let tiles: Vec<RTile> = p.hand.iter().map(|t| rt(*t)).collect();
            <[RTile; 13]>::try_from(tiles).unwrap()
        });
        self.emit(Event::StartKyoku {
            bakaze: "E".parse().unwrap(),
            dora_marker: rt(self.dora[0]),
            kyoku,
            honba: 0,
            kyotaku: 0,
            oya: self.oya,
            scores: [25000; 4],
            tehais,
        });
        self.record(true);

        let mut actor = self.oya;
        let mut draw = Some(false); // Some(rinshan) when the actor draws first
        loop {
            let a = usize::from(actor);
            if let Some(rinshan) = draw {
                let tile = if rinshan {
                    // The dead wall takes one tile from the live wall's end in exchange.
                    if !self.wall.is_empty() {
                        self.wall.remove(0);
                    }
                    self.dead.remove(0)
                } else if let Some(tile) = self.wall.pop() {
                    tile
                } else {
                    self.emit(Event::Ryukyoku { deltas: None });
                    self.emit(Event::EndKyoku);
                    self.keep_last();
                    return;
                };
                self.players[a].hand.push(tile);
                self.emit(Event::Tsumo {
                    actor,
                    pai: rt(tile),
                });
                if actor == self.me {
                    self.drawn = Some(tile);
                    self.record(false);
                }
                if self.players[a].riichi && complete(&self.players[a].hand) {
                    self.emit(Event::Hora {
                        actor,
                        target: actor,
                        deltas: None,
                        ura_markers: None,
                    });
                    self.emit(Event::EndKyoku);
                    self.keep_last();
                    return;
                }
                // A kan, then the replacement draw.
                if !self.players[a].riichi && self.kans < 3 && !self.wall.is_empty() {
                    if let Some(kind) = (0..34u8).find(|k| self.players[a].count(*k) == 4)
                        && self.rng.chance(60)
                    {
                        let tiles = self.players[a].of_kind(kind, 4);
                        for tile in &tiles {
                            self.players[a].take(*tile);
                        }
                        self.players[a].melds.push(concealed_kan_view(&tiles));
                        self.kans += 1;
                        self.emit(Event::Ankan {
                            actor,
                            consumed: <[RTile; 4]>::try_from(
                                tiles.iter().map(|t| rt(*t)).collect::<Vec<_>>(),
                            )
                            .unwrap(),
                        });
                        self.reveal_dora();
                        if actor == self.me {
                            self.drawn = None;
                        }
                        self.record(false);
                        draw = Some(true);
                        continue;
                    }
                    if let Some(i) = self.players[a].melds.iter().position(|m| {
                        m.tiles.len() == 3
                            && m.added.is_none()
                            && m.tiles.iter().flatten().all(|t| t.kind() == tile.kind())
                    }) && self.rng.chance(60)
                    {
                        self.players[a].take(tile);
                        self.players[a].melds[i].added = Some(tile);
                        let consumed: Vec<RTile> = self.players[a].melds[i]
                            .tiles
                            .iter()
                            .flatten()
                            .map(|t| rt(*t))
                            .collect();
                        self.kans += 1;
                        self.pending_dora += 1;
                        self.emit(Event::Kakan {
                            actor,
                            pai: rt(tile),
                            consumed: <[RTile; 3]>::try_from(consumed).unwrap(),
                        });
                        if actor == self.me {
                            self.drawn = None;
                        }
                        self.record(false);
                        draw = Some(true);
                        continue;
                    }
                }
            }
            // The discard.
            let player = &self.players[a];
            let forbidden = std::mem::take(&mut self.forbidden);
            let tile = if player.riichi {
                *player.hand.last().unwrap()
            } else {
                self.choose_discard(a, &forbidden)
            };
            let reach =
                !self.players[a].riichi && self.players[a].is_closed() && self.wall.len() >= 4 && {
                    let mut rest = self.players[a].hand.clone();
                    let i = rest.iter().position(|t| *t == tile).unwrap();
                    rest.remove(i);
                    shanten(&rest).is_ok_and(|s| s == 0)
                };
            if reach {
                self.emit(Event::Reach { actor });
            }
            let tsumogiri = self.players[a].hand.last() == Some(&tile) && draw.is_some();
            self.players[a].take(tile);
            let sideways = reach || self.players[a].sideways_next;
            self.players[a].sideways_next = false;
            if reach {
                self.players[a].riichi = true;
            }
            self.players[a].river.push(RiverTile { tile, sideways });
            self.emit(Event::Dahai {
                actor,
                pai: rt(tile),
                tsumogiri,
            });
            if actor == self.me {
                self.drawn = None;
            }
            // A win off the discard, by a player in riichi.
            for step in 1..4u8 {
                let seat = (actor + step) % 4;
                let p = &self.players[usize::from(seat)];
                let mut with = p.hand.clone();
                with.push(tile);
                if p.riichi && complete(&with) {
                    self.emit(Event::Hora {
                        actor: seat,
                        target: actor,
                        deltas: None,
                        ura_markers: None,
                    });
                    self.emit(Event::EndKyoku);
                    self.keep_last();
                    return;
                }
            }
            if reach {
                self.emit(Event::ReachAccepted { actor });
            }
            while self.pending_dora > 0 {
                self.pending_dora -= 1;
                self.reveal_dora();
            }
            self.record(false);

            // A call of the discard.
            if let Some((caller, next)) = self.call(actor, tile) {
                actor = caller;
                draw = next;
                continue;
            }
            actor = (actor + 1) % 4;
            draw = Some(false);
        }
    }

    /// Marks the round's last recorded moment as one to keep.
    fn keep_last(&mut self) {
        if let Some(last) = self.moments.last_mut() {
            last.keep = true;
        }
    }

    /// The bot's discard: the most efficient one not in `forbidden`, the drawn tile when
    /// it is the same tile.
    fn choose_discard(&self, seat: usize, forbidden: &[u8]) -> Tile {
        let hand = &self.players[seat].hand;
        let options = rank_discards(hand, &[0; 34]).unwrap();
        let best = options
            .iter()
            .map(|o| o.discard)
            .find(|t| !forbidden.contains(&t.kind()))
            .unwrap_or(hand[0]);
        // Of equal tiles, the one drawn last.
        *hand.iter().rev().find(|t| **t == best).unwrap()
    }

    /// Another player calls `tile` sometimes: the caller, and how it goes on (a draw for
    /// an open kan, none for chi or pon).
    fn call(&mut self, from: u8, tile: Tile) -> Option<(u8, Option<bool>)> {
        let kind = tile.kind();
        for step in 1..4u8 {
            let c = (from + step) % 4;
            let cu = usize::from(c);
            if self.players[cu].riichi {
                continue;
            }
            let held = self.players[cu].count(kind);
            let kan = held == 3 && self.kans < 3 && !self.wall.is_empty() && self.rng.chance(40);
            let pon = held >= 2 && self.rng.chance(35);
            if !kan && !pon {
                continue;
            }
            let consumed = self.players[cu].of_kind(kind, if kan { 3 } else { 2 });
            if !kan && !self.can_discard_after(cu, &consumed, &[kind]) {
                continue;
            }
            if !kan {
                self.forbidden = vec![kind];
            }
            self.take_call(from, c, tile, &consumed);
            let (target, actor, pai) = (from, c, rt(tile));
            let list: Vec<RTile> = consumed.iter().map(|t| rt(*t)).collect();
            if kan {
                self.kans += 1;
                self.pending_dora += 1;
            }
            self.emit(if kan {
                Event::Daiminkan {
                    actor,
                    target,
                    pai,
                    consumed: <[RTile; 3]>::try_from(list).unwrap(),
                }
            } else {
                Event::Pon {
                    actor,
                    target,
                    pai,
                    consumed: <[RTile; 2]>::try_from(list).unwrap(),
                }
            });
            self.record(false);
            return Some((c, kan.then_some(true)));
        }
        // Chi, by the next player only, and not of honors.
        let c = (from + 1) % 4;
        let cu = usize::from(c);
        if tile.is_honor() || self.players[cu].riichi || !self.rng.chance(30) {
            return None;
        }
        let rank = i8::try_from(kind % 9).unwrap();
        for (a, b) in [(-2i8, -1i8), (-1, 1), (1, 2)] {
            if !(0..9).contains(&(rank + a)) || !(0..9).contains(&(rank + b)) {
                continue;
            }
            let ka = u8::try_from(i8::try_from(kind).unwrap() + a).unwrap();
            let kb = u8::try_from(i8::try_from(kind).unwrap() + b).unwrap();
            if self.players[cu].count(ka) == 0 || self.players[cu].count(kb) == 0 {
                continue;
            }
            let consumed = vec![
                self.players[cu].of_kind(ka, 1)[0],
                self.players[cu].of_kind(kb, 1)[0],
            ];
            let mut forbidden = vec![kind];
            if a == 1 && kind % 9 + 3 <= 8 {
                forbidden.push(kind + 3);
            }
            if b == -1 && kind % 9 >= 3 {
                forbidden.push(kind - 3);
            }
            if !self.can_discard_after(cu, &consumed, &forbidden) {
                continue;
            }
            self.take_call(from, c, tile, &consumed);
            self.emit(Event::Chi {
                actor: c,
                target: from,
                pai: rt(tile),
                consumed: [rt(consumed[0]), rt(consumed[1])],
            });
            self.record(false);
            self.forbidden = forbidden;
            return Some((c, None));
        }
        None
    }

    fn can_discard_after(&self, seat: usize, consumed: &[Tile], forbidden: &[u8]) -> bool {
        let mut rest = self.players[seat].hand.clone();
        for tile in consumed {
            let i = rest.iter().position(|t| t == tile).unwrap();
            rest.remove(i);
        }
        rest.iter().any(|t| !forbidden.contains(&t.kind()))
    }

    fn take_call(&mut self, from: u8, caller: u8, tile: Tile, consumed: &[Tile]) {
        let f = usize::from(from);
        let last = self.players[f].river.pop().unwrap();
        if last.sideways
            && self.players[f].riichi
            && self.players[f].river.iter().all(|t| !t.sideways)
        {
            self.players[f].sideways_next = true;
        }
        for t in consumed {
            self.players[usize::from(caller)].take(*t);
        }
        let off = (from + 4 - caller) % 4;
        self.players[usize::from(caller)]
            .melds
            .push(called_view(tile, consumed, off));
        if caller == self.me {
            self.drawn = None;
        }
    }
}

/// Plays `rounds` rounds watched from one seat.
pub fn play(seed: u64, rounds: u8) -> Game {
    let mut sim = Sim {
        rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
        me: (seed % 4) as u8,
        players: Default::default(),
        wall: Vec::new(),
        dead: Vec::new(),
        dora: Vec::new(),
        kans: 0,
        pending_dora: 0,
        kyoku: 1,
        oya: 0,
        log: Vec::new(),
        moments: Vec::new(),
        drawn: None,
        forbidden: Vec::new(),
    };
    for kyoku in 1..=rounds {
        sim.play_round(kyoku);
    }
    Game {
        log: sim.log,
        moments: sim.moments,
        me: sim.me,
    }
}

/// The events the engine should infer: the log without what the screen cannot show yet
/// (round ends), with others' discards counted as from the hand and melded tiles sorted.
fn comparable(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter(|e| {
            !matches!(
                e,
                Event::Hora { .. } | Event::Ryukyoku { .. } | Event::EndKyoku
            )
        })
        .map(|e| {
            let mut e = e.clone();
            match &mut e {
                Event::Chi { consumed, .. } | Event::Pon { consumed, .. } => {
                    consumed.sort_by_key(|t| t.as_u8());
                }
                Event::Daiminkan { consumed, .. } | Event::Kakan { consumed, .. } => {
                    consumed.sort_by_key(|t| t.as_u8());
                }
                Event::Ankan { consumed, .. } => consumed.sort_by_key(|t| t.as_u8()),
                _ => {}
            }
            serde_json::to_string(&e).unwrap()
        })
        .collect()
}

/// The log up to each round's last kept moment: what the engine can know.
fn expected(game: &Game, kept: &[&Moment]) -> Vec<String> {
    let mut out = Vec::new();
    let starts: Vec<usize> = game
        .log
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e, Event::StartKyoku { .. }))
        .map(|(i, _)| i)
        .collect();
    for (r, &start) in starts.iter().enumerate() {
        let end = starts.get(r + 1).copied().unwrap_or(game.log.len());
        let seen = kept
            .iter()
            .filter(|m| m.events > start && m.events <= end)
            .map(|m| m.events)
            .max()
            .unwrap_or(start + 1);
        out.extend(comparable(&game.log[start..seen]));
    }
    out
}

/// Feeds `moments` to a new engine; every one must be taken.
fn run(moments: &[&Moment]) -> Engine {
    let mut engine = Engine::new();
    for (i, moment) in moments.iter().enumerate() {
        if let Err(err) = engine.observe(&moment.scene) {
            panic!("moment {i}: {err}\n{:#?}", moment.scene);
        }
    }
    engine
}

#[test]
fn the_simulated_logs_are_legal() {
    for seed in 0..20 {
        let game = play(seed, 4);
        let mut state = PlayerState::new(game.me);
        for event in &game.log {
            state.update(event).unwrap();
        }
    }
}

#[test]
fn every_scene_gives_back_the_log() {
    for seed in 0..40 {
        let game = play(seed, 4);
        let kept: Vec<&Moment> = game.moments.iter().collect();
        let engine = run(&kept);
        assert_eq!(
            comparable(&engine.events()),
            expected(&game, &kept),
            "seed {seed}"
        );
        assert_eq!(engine.rewrites(), 0);
    }
}

/// Games whose events come back exactly, of 100, with `percent` of the scenes kept.
fn exact_games(percent: usize) -> usize {
    (0..100)
        .filter(|&seed| {
            let game = play(seed, 4);
            let mut rng = Rng(seed + 99);
            let kept: Vec<&Moment> = game
                .moments
                .iter()
                .filter(|m| m.keep || rng.chance(percent))
                .collect();
            let engine = run(&kept);
            comparable(&engine.events()) == expected(&game, &kept)
        })
        .count()
}

/// With scenes missed, several players act between two of them; the engine still
/// explains every scene with legal events. Exactly the same events come back unless the
/// screen cannot tell: a called discard leaves no mark in the river, so with no scene
/// in between, two discards of a seat (one of them called) or two kept draws of the
/// player can come in either order, and a called riichi declaration looks like a riichi
/// at the next discard, which lies sideways.
#[test]
fn scenes_far_apart_give_back_the_log() {
    let (most, many) = (exact_games(75), exact_games(60));
    assert!(most >= 90, "75% of scenes: {most} of 100 exact");
    assert!(many >= 80, "60% of scenes: {many} of 100 exact");
}

#[test]
fn impossible_scenes_are_turned_away() {
    let game = play(1, 1);
    let mut engine = Engine::new();
    engine.observe(&game.moments[0].scene).unwrap();
    let mut five = game.moments[1].scene.clone();
    let tile = five.hand[0];
    five.hand[1..5].fill(tile);
    assert!(matches!(
        engine.observe(&five),
        Err(Rejected::Impossible(_))
    ));
    let mut short = game.moments[1].scene.clone();
    short.hand.pop();
    short.drawn = None;
    short.hand.pop();
    assert!(matches!(
        engine.observe(&short),
        Err(Rejected::Impossible(_))
    ));
    // The engine is unchanged: the right scene still follows.
    engine.observe(&game.moments[1].scene).unwrap();
}

#[test]
fn a_tile_read_wrongly_once_is_corrected_later() {
    let mut corrected = 0;
    for seed in 0..30 {
        let game = play(seed, 2);
        let mut rng = Rng(seed + 7);
        let mut moments: Vec<Moment> = game
            .moments
            .iter()
            .map(|m| Moment {
                scene: m.scene.clone(),
                events: m.events,
                keep: m.keep,
            })
            .collect();
        // Misread the newest river tile of some seat in one scene.
        let candidates: Vec<usize> = (1..moments.len() - 1)
            .filter(|&i| !moments[i].keep && moments[i].scene.rivers.iter().any(|r| !r.is_empty()))
            .collect();
        let i = candidates[rng.below(candidates.len())];
        let seat = (0..4)
            .find(|&s| !moments[i].scene.rivers[s].is_empty())
            .unwrap();
        let river = &mut moments[i].scene.rivers[seat];
        let last = river.last_mut().unwrap();
        let wrong = Tile::from_kind((last.tile.kind() + 9) % 27).unwrap();
        last.tile = wrong;
        if moments[i].scene.check().is_err() {
            continue;
        }
        let kept: Vec<&Moment> = moments.iter().collect();
        let mut engine = Engine::new();
        for moment in &kept {
            match engine.observe(&moment.scene) {
                Ok(_) | Err(Rejected::Unexplained) => {}
                Err(err) => panic!("seed {seed}: {err}"),
            }
        }
        assert_eq!(
            comparable(&engine.events()),
            expected(&game, &kept),
            "seed {seed}"
        );
        corrected += usize::from(engine.rewrites() > 0);
    }
    assert!(corrected > 10, "only {corrected} games needed a correction");
}
