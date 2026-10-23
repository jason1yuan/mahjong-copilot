//! The state engine (#10): from settled scenes to mjai events.
//!
//! - The engine keeps the round's events and a model of the table they imply. A new scene
//!   is explained by the shortest run of events, in turn order, that turns the model into
//!   the scene: draws, discards (with riichi), calls of the last discard and kans. A turn
//!   order walk settles what happened between two scenes however many things did; a call
//!   skips the players in between.
//! - Other players' draws are `?` and their discards count as from the hand (`tsumogiri`
//!   false): the screen does not tell. The player's own draws are worked out from the hand.
//!   How many draws there were comes from the tiles left to draw: a called discard leaves
//!   no mark in the river, so without it a call could be put a turn early, one draw short.
//! - Every event also goes into libriichi's `PlayerState`, which rejects what cannot be.
//! - When no run of events explains a scene, a tile read wrongly earlier is the likely
//!   cause: the engine tries the scene's tile in its place (a river tile, or a tile the
//!   player drew), replays the round, and tries again. A scene that still cannot be
//!   explained is left out; the next ones may settle it.
//! - Kan dora: shown at once after a concealed kan, after the next discard for the other
//!   kans.
//! - Round ends (wins, draws) and opening mid-round come with the results screen (#10,
//!   step 2): for now a round starts from a scene with nothing discarded.

use riichi::mjai::Event;
use riichi::state::PlayerState;
use riichi::tile::Tile as RTile;

use super::scene::{MeldView, RiverTile, RoundLabel, Scene};
use crate::tile::Tile;

/// Most groups of events looked for between two scenes: four players' draw and discard,
/// four or five times round the table.
const MAX_STEPS: usize = 40;
/// Tiles left to draw when a round starts: 136 less the dead wall's 14 and four hands.
const TILES_LEFT: u8 = 70;

/// Why a scene was not taken.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Rejected {
    #[error("the scene cannot be: {0}")]
    Impossible(&'static str),
    #[error("no round has started and this scene does not start one")]
    NoRound,
    #[error("no run of events explains the scene")]
    Unexplained,
}

/// What a scene added to the log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observed {
    /// The new events, in order.
    pub events: Vec<Event>,
    /// Earlier events of the round were corrected from this scene (a tile read wrongly
    /// before): the round's log was rebuilt.
    pub rewritten: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Phase {
    /// The seat is to draw.
    #[default]
    Draw,
    /// The seat has drawn or called and is to discard.
    Discard,
}

/// The table as the round's events leave it, by seat relative to the player.
#[derive(Debug, Clone, Default)]
struct Model {
    hand: Vec<Tile>,
    drawn: Option<Tile>,
    rivers: [Vec<RiverTile>; 4],
    melds: [Vec<MeldView>; 4],
    dora: Vec<Tile>,
    riichi: [bool; 4],
    /// Declared riichi, waiting for the declaration's discard.
    declared: [bool; 4],
    /// The declaration's discard was called: the next discard lies sideways.
    sideways_next: [bool; 4],
    turn: u8,
    phase: Phase,
    /// The last discard, while it can still be called.
    last_discard: Option<(u8, Tile)>,
    /// Kan dora still to be shown.
    pending_dora: u8,
    tiles_left: u8,
}

struct Round {
    label: RoundLabel,
    honba: Option<u8>,
    /// The player's seat in mjai's numbering.
    me: u8,
    events: Vec<Event>,
    model: Model,
    state: PlayerState,
}

/// The state engine for one game.
#[derive(Default)]
pub struct Engine {
    /// Events of the rounds before the current one.
    earlier: Vec<Event>,
    round: Option<Round>,
    /// How many times earlier events were corrected.
    rewrites: u32,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every event so far, the current round's included.
    pub fn events(&self) -> Vec<Event> {
        let mut all = self.earlier.clone();
        if let Some(round) = &self.round {
            all.extend(round.events.iter().cloned());
        }
        all
    }

    /// The game state the events lead to, from the player's seat.
    pub fn player_state(&self) -> Option<&PlayerState> {
        self.round.as_ref().map(|r| &r.state)
    }

    /// How many times earlier events were corrected from a later scene.
    pub fn rewrites(&self) -> u32 {
        self.rewrites
    }

    /// Takes a settled scene: the events it implies, or why it was not taken.
    pub fn observe(&mut self, scene: &Scene) -> Result<Observed, Rejected> {
        scene.check().map_err(Rejected::Impossible)?;
        if self.starts_round(scene) {
            return self.start_round(scene);
        }
        let Some(round) = self.round.as_mut() else {
            return Err(Rejected::NoRound);
        };
        if let Some(events) = explain(&round.model, scene, round.me) {
            for event in &events {
                round
                    .model
                    .apply(event, round.me)
                    .map_err(|_| Rejected::Unexplained)?;
                round
                    .state
                    .update(event)
                    .map_err(|_| Rejected::Unexplained)?;
            }
            round.events.extend(events.iter().cloned());
            return Ok(Observed {
                events,
                rewritten: false,
            });
        }
        // Perhaps a tile was read wrongly before: try the scene's in its place.
        for patched in corrections(round, scene) {
            let Some(rebuilt) = replay(&patched, round.me) else {
                continue;
            };
            let Some(events) = explain(&rebuilt.0, scene, round.me) else {
                continue;
            };
            let (mut model, mut state) = rebuilt;
            let mut ok = true;
            for event in &events {
                ok &= model.apply(event, round.me).is_ok() && state.update(event).is_ok();
            }
            if !ok {
                continue;
            }
            round.events = patched;
            round.events.extend(events.iter().cloned());
            round.model = model;
            round.state = state;
            self.rewrites += 1;
            return Ok(Observed {
                events,
                rewritten: true,
            });
        }
        Err(Rejected::Unexplained)
    }

    /// A scene starts a new round when there is none, the round label or honba changed,
    /// or nothing is discarded while the current round has discards.
    fn starts_round(&self, scene: &Scene) -> bool {
        let empty = scene.rivers.iter().all(Vec::is_empty) && scene.melds.iter().all(Vec::is_empty);
        match &self.round {
            None => empty,
            Some(round) => {
                let label_changed = scene.round.is_some_and(|l| l != round.label)
                    || (scene.honba.is_some() && scene.honba != round.honba);
                let discarded = round.model.rivers.iter().any(|r| !r.is_empty())
                    || round.model.melds.iter().any(|m| !m.is_empty());
                empty && (label_changed || discarded)
            }
        }
    }

    fn start_round(&mut self, scene: &Scene) -> Result<Observed, Rejected> {
        let (Some(label), Some(dealer), Some(&dora)) =
            (scene.round, scene.dealer, scene.dora.first())
        else {
            return Err(Rejected::NoRound);
        };
        let oya = label.number.saturating_sub(1) % 4;
        let me = (oya + 4 - dealer % 4) % 4;
        let mut tehai: Vec<Tile> = scene.hand.clone();
        tehai.truncate(13);
        if tehai.len() != 13 {
            return Err(Rejected::Impossible("a round starts with 13 tiles in hand"));
        }
        let unknown = [unknown(); 13];
        let mut tehais = [unknown; 4];
        for (slot, tile) in tehais[usize::from(me)].iter_mut().zip(&tehai) {
            *slot = rtile(*tile);
        }
        let mut scores = [25000; 4];
        if let Some(seen) = scene.scores {
            for (rel, score) in seen.iter().enumerate() {
                scores[(usize::from(me) + rel) % 4] = *score;
            }
        }
        let start = Event::StartKyoku {
            bakaze: rtile(label.wind),
            dora_marker: rtile(dora),
            kyoku: label.number,
            honba: scene.honba.unwrap_or(0),
            kyotaku: scene.riichi_sticks.unwrap_or(0),
            oya,
            scores,
            tehais,
        };
        let mut model = Model::default();
        let mut state = PlayerState::new(me);
        model.apply(&start, me).map_err(Rejected::Impossible)?;
        state.update(&start).map_err(|_| Rejected::NoRound)?;
        let mut events = vec![start];
        // The dealer's first draw, if it shows already.
        if let Some(more) = explain(&model, scene, me) {
            for event in &more {
                model.apply(event, me).map_err(Rejected::Impossible)?;
                state.update(event).map_err(|_| Rejected::Unexplained)?;
            }
            events.extend(more);
        }
        if let Some(old) = self.round.take() {
            self.earlier.extend(old.events);
        }
        self.round = Some(Round {
            label,
            honba: scene.honba,
            me,
            events: events.clone(),
            model,
            state,
        });
        Ok(Observed {
            events,
            rewritten: false,
        })
    }
}

/// The round's events with one tile replaced by what `scene` shows, for each tile the
/// scene contradicts: river tiles at the same place, and tiles the player drew.
fn corrections(round: &Round, scene: &Scene) -> Vec<Vec<Event>> {
    let mut out = Vec::new();
    let me = round.me;
    // River tiles: the scene's tile where the model's differs.
    for seat in 0..4u8 {
        let model = &round.model.rivers[usize::from(seat)];
        let seen = &scene.rivers[usize::from(seat)];
        let Some(i) = model.iter().zip(seen).position(|(m, s)| m.tile != s.tile) else {
            continue;
        };
        let right = seen[i].tile;
        // The i-th discard of the seat that stayed in its river.
        let mut kept = 0;
        for (at, event) in round.events.iter().enumerate() {
            let Event::Dahai { actor, pai, .. } = *event else {
                continue;
            };
            if rel(actor, me) != seat || called_next(&round.events, at) {
                continue;
            }
            if kept == i {
                let mut patched = round.events.clone();
                replace_discard(&mut patched, at, pai, rtile(right));
                out.push(patched);
                break;
            }
            kept += 1;
        }
    }
    // The player's tiles: a tile the model holds but the scene does not came in wrong;
    // try each tile the scene holds but the model does not in its place.
    let held = round.model.hand.clone();
    let shown = scene.own_tiles();
    let extra = minus(&held, &shown);
    let missing = minus(&shown, &held);
    for wrong in &extra {
        for right in &missing {
            // The latest draw of it, or else the starting hand.
            let at = round.events.iter().rposition(|e| {
                matches!(*e, Event::Tsumo { actor, pai } if actor == me && core(pai) == Some(*wrong))
            });
            let mut patched = round.events.clone();
            match at {
                Some(at) => {
                    if let Event::Tsumo { pai, .. } = &mut patched[at] {
                        *pai = rtile(*right);
                    }
                }
                None => {
                    let Some(Event::StartKyoku { tehais, .. }) = patched.first_mut() else {
                        continue;
                    };
                    let mine = &mut tehais[usize::from(me)];
                    let Some(slot) = mine.iter_mut().find(|t| core(**t) == Some(*wrong)) else {
                        continue;
                    };
                    *slot = rtile(*right);
                }
            }
            out.push(patched);
        }
    }
    out
}

/// Replaces the discard at `at` with `right`, and the draw before it when that was the
/// tile drawn (the player's own tsumogiri).
fn replace_discard(events: &mut [Event], at: usize, wrong: RTile, right: RTile) {
    if let Event::Dahai {
        actor,
        pai,
        tsumogiri,
    } = &mut events[at]
    {
        *pai = right;
        let (actor, tsumogiri) = (*actor, *tsumogiri);
        if tsumogiri
            && let Some(Event::Tsumo {
                actor: a,
                pai: drawn,
            }) = at.checked_sub(1).map(|i| &mut events[i])
            && *a == actor
            && *drawn == wrong
        {
            *drawn = right;
        }
    }
}

/// Whether the discard at `at` was called by the next event.
fn called_next(events: &[Event], at: usize) -> bool {
    let next = events[at + 1..]
        .iter()
        .find(|e| !matches!(e, Event::ReachAccepted { .. } | Event::Dora { .. }));
    matches!(
        next,
        Some(Event::Chi { .. } | Event::Pon { .. } | Event::Daiminkan { .. })
    )
}

/// The model and game state the events lead to; `None` if they do not hold together.
fn replay(events: &[Event], me: u8) -> Option<(Model, PlayerState)> {
    let mut model = Model::default();
    let mut state = PlayerState::new(me);
    for event in events {
        model.apply(event, me).ok()?;
        state.update(event).ok()?;
    }
    Some((model, state))
}

/// The shortest run of events that turns `model` into `scene`, if any.
fn explain(model: &Model, scene: &Scene, me: u8) -> Option<Vec<Event>> {
    // The player's draws to come: what the scene shows in hand, discarded or melded,
    // less what the model holds.
    let came_in = own_tiles_after(model, scene)?;
    let draws = minus(&came_in, &model.hand);
    if !minus(&model.hand, &came_in).is_empty() {
        return None;
    }
    if model.matches(scene) && draws.is_empty() {
        return Some(Vec::new());
    }
    for limit in 1..=MAX_STEPS {
        let mut path = Vec::new();
        if search(model, scene, me, &draws, limit, &mut path) {
            return Some(path.into_iter().flatten().collect());
        }
    }
    None
}

fn search(
    model: &Model,
    scene: &Scene,
    me: u8,
    draws: &[Tile],
    limit: usize,
    path: &mut Vec<Vec<Event>>,
) -> bool {
    if model.matches(scene) && draws.is_empty() {
        return true;
    }
    if path.len() == limit || !model.prefix_of(scene) {
        return false;
    }
    for (events, used) in steps(model, scene, me, draws) {
        let mut next = model.clone();
        if events.iter().any(|e| next.apply(e, me).is_err()) {
            continue;
        }
        let left = match used {
            Some(tile) => {
                let mut left = draws.to_vec();
                let Some(i) = left.iter().position(|t| *t == tile) else {
                    continue;
                };
                left.remove(i);
                left
            }
            None => draws.to_vec(),
        };
        path.push(events);
        if search(&next, scene, me, &left, limit, path) {
            return true;
        }
        path.pop();
    }
    false
}

/// The groups of events that can come next, each with the drawn tile it uses up.
fn steps(model: &Model, scene: &Scene, me: u8, draws: &[Tile]) -> Vec<(Vec<Event>, Option<Tile>)> {
    let seat = model.turn;
    let actor = abs(seat, me);
    let mut out = Vec::new();
    match model.phase {
        Phase::Draw => {
            // A call of the last discard comes first.
            if let Some((from, tile)) = model.last_discard {
                for caller in (0..4u8).filter(|c| *c != from) {
                    let Some(meld) = new_meld(model, scene, caller) else {
                        continue;
                    };
                    let Some(off) = meld.source() else {
                        continue;
                    };
                    if (caller + off) % 4 != from || meld.called() != Some(tile) {
                        continue;
                    }
                    let others: Vec<RTile> = without_called(meld).into_iter().map(rtile).collect();
                    let (actor, target, pai) = (abs(caller, me), abs(from, me), rtile(tile));
                    let event = match others.as_slice() {
                        [a, b, c] => Event::Daiminkan {
                            actor,
                            target,
                            pai,
                            consumed: [*a, *b, *c],
                        },
                        [a, b] if same_kind(meld) => Event::Pon {
                            actor,
                            target,
                            pai,
                            consumed: [*a, *b],
                        },
                        [a, b] if (from + 1) % 4 == caller => Event::Chi {
                            actor,
                            target,
                            pai,
                            consumed: [*a, *b],
                        },
                        _ => continue,
                    };
                    out.push((vec![event], None));
                }
            }
            if seat == 0 {
                for tile in draw_order(model, scene, draws) {
                    out.push((
                        vec![Event::Tsumo {
                            actor,
                            pai: rtile(tile),
                        }],
                        Some(tile),
                    ));
                }
            } else {
                out.push((
                    vec![Event::Tsumo {
                        actor,
                        pai: unknown(),
                    }],
                    None,
                ));
            }
        }
        Phase::Discard => {
            // A concealed kan.
            if let Some(meld) = new_meld(model, scene, seat)
                && meld.is_concealed_kan()
                && let Some(consumed) = concealed_kan_tiles(model, meld, seat)
            {
                let mut events = vec![Event::Ankan {
                    actor,
                    consumed: consumed.map(rtile),
                }];
                if let Some(dora) = scene.dora.get(model.dora.len()) {
                    events.push(Event::Dora {
                        dora_marker: rtile(*dora),
                    });
                }
                out.push((events, None));
            }
            // A tile added to a pon.
            for (i, meld) in model.melds[usize::from(seat)].iter().enumerate() {
                let Some(seen) = scene.melds[usize::from(seat)].get(i) else {
                    continue;
                };
                if meld.added.is_none()
                    && meld.tiles.len() == 3
                    && let Some(added) = seen.added
                {
                    let consumed: Vec<RTile> =
                        meld.tiles.iter().flatten().map(|t| rtile(*t)).collect();
                    if let [a, b, c] = consumed.as_slice() {
                        out.push((
                            vec![Event::Kakan {
                                actor,
                                pai: rtile(added),
                                consumed: [*a, *b, *c],
                            }],
                            None,
                        ));
                    }
                }
            }
            // A discard that stays in the river.
            let river = &scene.rivers[usize::from(seat)];
            if let Some(next) = river.get(model.rivers[usize::from(seat)].len()) {
                let reach = next.sideways
                    && !model.riichi[usize::from(seat)]
                    && !model.sideways_next[usize::from(seat)];
                out.push((discard(model, scene, me, next.tile, reach), None));
            }
            // A discard that was called, so it is not in the river.
            for caller in (0..4u8).filter(|c| *c != seat) {
                if let Some(meld) = new_meld(model, scene, caller)
                    && let (Some(off), Some(tile)) = (meld.source(), meld.called())
                    && (caller + off) % 4 == seat
                    && river
                        .get(model.rivers[usize::from(seat)].len())
                        .map(|t| t.tile)
                        != Some(tile)
                {
                    out.push((discard(model, scene, me, tile, false), None));
                }
            }
        }
    }
    out
}

/// A discard by the seat whose turn it is, with riichi and any kan dora due.
fn discard(model: &Model, scene: &Scene, me: u8, tile: Tile, reach: bool) -> Vec<Event> {
    let actor = abs(model.turn, me);
    let mut events = Vec::new();
    if reach {
        events.push(Event::Reach { actor });
    }
    events.push(Event::Dahai {
        actor,
        pai: rtile(tile),
        tsumogiri: model.turn == 0 && model.drawn == Some(tile),
    });
    if reach {
        events.push(Event::ReachAccepted { actor });
    }
    for dora in scene
        .dora
        .iter()
        .skip(model.dora.len())
        .take(usize::from(model.pending_dora))
    {
        events.push(Event::Dora {
            dora_marker: rtile(*dora),
        });
    }
    events
}

/// The tiles the player may draw next, likeliest first: the one the next discard or kan
/// needs, then the others, the one shown drawn last.
fn draw_order(model: &Model, scene: &Scene, draws: &[Tile]) -> Vec<Tile> {
    let next_discard = scene.rivers[0].get(model.rivers[0].len()).map(|t| t.tile);
    let kan_kind =
        new_meld(model, scene, 0).and_then(|m| m.tiles.iter().flatten().next().map(|t| t.kind()));
    let mut order: Vec<Tile> = draws.to_vec();
    order.sort();
    order.dedup();
    order.sort_by_key(|t| {
        let rank = if Some(*t) == next_discard {
            0
        } else if Some(t.kind()) == kan_kind {
            1
        } else if Some(*t) == scene.drawn {
            3
        } else {
            2
        };
        // The drawn tile shown is the last draw unless it is the only one.
        if draws.len() == 1 { 0 } else { rank }
    });
    order
}

/// The scene's next meld for `seat` that the model does not have yet.
fn new_meld<'a>(model: &Model, scene: &'a Scene, seat: u8) -> Option<&'a MeldView> {
    scene.melds[usize::from(seat)].get(model.melds[usize::from(seat)].len())
}

fn same_kind(meld: &MeldView) -> bool {
    let mut kinds = meld.tiles.iter().flatten().map(|t| t.kind());
    let first = kinds.next();
    kinds.all(|k| Some(k) == first)
}

/// A called meld's tiles other than the called one.
fn without_called(meld: &MeldView) -> Vec<Tile> {
    meld.tiles
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != meld.sideways)
        .filter_map(|(_, t)| *t)
        .collect()
}

/// The four tiles of a concealed kan: the player's from the hand, another player's from
/// the faces shown, the rest plain.
fn concealed_kan_tiles(model: &Model, meld: &MeldView, seat: u8) -> Option<[Tile; 4]> {
    let kind = meld.tiles.iter().flatten().next()?.kind();
    let tiles: Vec<Tile> = if seat == 0 {
        model
            .hand
            .iter()
            .filter(|t| t.kind() == kind)
            .copied()
            .collect()
    } else {
        let plain = Tile::from_kind(kind)?;
        let mut faces: Vec<Tile> = meld.tiles.iter().flatten().copied().collect();
        faces.resize(4, plain);
        faces
    };
    tiles.try_into().ok()
}

/// Every tile the player holds, has discarded or has melded from the hand in `scene`,
/// beyond what `model` already accounts for: what the model's hand must grow into.
fn own_tiles_after(model: &Model, scene: &Scene) -> Option<Vec<Tile>> {
    let mut tiles = scene.own_tiles();
    let river = &scene.rivers[0];
    // New discards start after the model's, or in place of its last one if that was
    // called away since.
    let mut known = model.rivers[0].len();
    if let Some((0, tile)) = model.last_discard
        && river.get(known - 1).map(|t| t.tile) != Some(tile)
    {
        known -= 1;
    }
    tiles.extend(river.iter().skip(known).map(|t| t.tile));
    // Discards others called, but for the model's last discard, already counted.
    let mut pending = match model.last_discard {
        Some((0, tile)) => Some(tile),
        _ => None,
    };
    for caller in 1..4u8 {
        for meld in scene.melds[usize::from(caller)]
            .iter()
            .skip(model.melds[usize::from(caller)].len())
        {
            if meld.source().is_some_and(|off| (caller + off) % 4 == 0)
                && let Some(tile) = meld.called()
            {
                if pending == Some(tile) {
                    pending = None;
                } else {
                    tiles.push(tile);
                }
            }
        }
    }
    // Melds made from the hand.
    let own = &scene.melds[0];
    for (i, meld) in own.iter().enumerate() {
        match model.melds[0].get(i) {
            None if meld.is_concealed_kan() => {
                let kind = meld.tiles.iter().flatten().next()?.kind();
                let mut faces: Vec<Tile> = meld.tiles.iter().flatten().copied().collect();
                let plain = Tile::from_kind(kind)?;
                faces.resize(4, plain);
                tiles.extend(faces);
            }
            None => {
                tiles.extend(without_called(meld));
                tiles.extend(meld.added);
            }
            Some(known) if known.added.is_none() => tiles.extend(meld.added),
            Some(_) => {}
        }
    }
    Some(tiles)
}

impl Model {
    /// Applies one event; `Err` when it does not fit the model.
    fn apply(&mut self, event: &Event, me: u8) -> Result<(), &'static str> {
        match *event {
            Event::StartKyoku {
                dora_marker,
                oya,
                tehais,
                ..
            } => {
                *self = Self::default();
                self.hand = tehais[usize::from(me)]
                    .iter()
                    .map(|t| core(*t).ok_or("an unknown tile in hand"))
                    .collect::<Result<_, _>>()?;
                self.dora = vec![core(dora_marker).ok_or("an unknown dora")?];
                self.turn = rel(oya, me);
                self.phase = Phase::Draw;
                self.tiles_left = TILES_LEFT;
            }
            Event::Tsumo { actor, pai } => {
                let seat = rel(actor, me);
                if seat != self.turn || self.phase != Phase::Draw {
                    return Err("a draw out of turn");
                }
                self.tiles_left = self
                    .tiles_left
                    .checked_sub(1)
                    .ok_or("a draw from an empty wall")?;
                if seat == 0 {
                    let tile = core(pai).ok_or("the player drew an unknown tile")?;
                    self.hand.push(tile);
                    self.drawn = Some(tile);
                }
                self.phase = Phase::Discard;
                self.last_discard = None;
            }
            Event::Dahai { actor, pai, .. } => {
                let seat = rel(actor, me);
                if seat != self.turn || self.phase != Phase::Discard {
                    return Err("a discard out of turn");
                }
                let tile = core(pai).ok_or("an unknown discard")?;
                if seat == 0 {
                    take(&mut self.hand, &[tile])?;
                    self.drawn = None;
                }
                let s = usize::from(seat);
                let sideways = self.declared[s] || self.sideways_next[s];
                if self.declared[s] {
                    self.declared[s] = false;
                    self.riichi[s] = true;
                } else {
                    self.sideways_next[s] = false;
                }
                self.rivers[s].push(RiverTile { tile, sideways });
                self.last_discard = Some((seat, tile));
                self.turn = (seat + 1) % 4;
                self.phase = Phase::Draw;
            }
            Event::Reach { actor } => {
                if rel(actor, me) != self.turn || self.phase != Phase::Discard {
                    return Err("riichi out of turn");
                }
                self.declared[usize::from(rel(actor, me))] = true;
            }
            Event::Chi {
                actor,
                target,
                pai,
                consumed,
            }
            | Event::Pon {
                actor,
                target,
                pai,
                consumed,
            } => self.call(rel(actor, me), rel(target, me), pai, &consumed)?,
            Event::Daiminkan {
                actor,
                target,
                pai,
                consumed,
            } => {
                self.call(rel(actor, me), rel(target, me), pai, &consumed)?;
                self.phase = Phase::Draw;
                self.pending_dora += 1;
            }
            Event::Kakan { actor, pai, .. } => {
                let seat = rel(actor, me);
                if seat != self.turn || self.phase != Phase::Discard {
                    return Err("a kan out of turn");
                }
                let tile = core(pai).ok_or("an unknown kan tile")?;
                let meld = self.melds[usize::from(seat)]
                    .iter_mut()
                    .find(|m| {
                        m.tiles.len() == 3
                            && m.added.is_none()
                            && m.tiles.iter().flatten().all(|t| t.kind() == tile.kind())
                    })
                    .ok_or("a tile added to no pon")?;
                meld.added = Some(tile);
                if seat == 0 {
                    take(&mut self.hand, &[tile])?;
                    self.drawn = None;
                }
                self.phase = Phase::Draw;
                self.pending_dora += 1;
                self.last_discard = None;
            }
            Event::Ankan { actor, consumed } => {
                let seat = rel(actor, me);
                if seat != self.turn || self.phase != Phase::Discard {
                    return Err("a kan out of turn");
                }
                let tiles: Vec<Tile> = consumed
                    .iter()
                    .map(|t| core(*t).ok_or("an unknown kan tile"))
                    .collect::<Result<_, _>>()?;
                if seat == 0 {
                    take(&mut self.hand, &tiles)?;
                    self.drawn = None;
                }
                self.melds[usize::from(seat)].push(concealed_kan_view(&tiles));
                self.phase = Phase::Draw;
                // Shown at once, by the next event; if not, with the next discard.
                self.pending_dora += 1;
            }
            Event::Dora { dora_marker } => {
                self.dora.push(core(dora_marker).ok_or("an unknown dora")?);
                self.pending_dora = self.pending_dora.saturating_sub(1);
            }
            _ => {}
        }
        Ok(())
    }

    /// A chi, pon or open kan of the last discard.
    fn call(
        &mut self,
        caller: u8,
        from: u8,
        pai: RTile,
        consumed: &[RTile],
    ) -> Result<(), &'static str> {
        let tile = core(pai).ok_or("an unknown called tile")?;
        if self.phase != Phase::Draw || self.last_discard != Some((from, tile)) {
            return Err("a call of a tile that is not the last discard");
        }
        let river = &mut self.rivers[usize::from(from)];
        let last = river.pop().ok_or("a call from an empty river")?;
        if last.sideways
            && self.riichi[usize::from(from)]
            && self.rivers[usize::from(from)].iter().all(|t| !t.sideways)
        {
            self.sideways_next[usize::from(from)] = true;
        }
        let others: Vec<Tile> = consumed
            .iter()
            .map(|t| core(*t).ok_or("an unknown tile in a call"))
            .collect::<Result<_, _>>()?;
        if caller == 0 {
            take(&mut self.hand, &others)?;
        }
        let off = (from + 4 - caller) % 4;
        self.melds[usize::from(caller)].push(called_view(tile, &others, off));
        self.turn = caller;
        self.phase = Phase::Discard;
        self.last_discard = None;
        Ok(())
    }

    /// Whether the model shows what the scene shows.
    fn matches(&self, scene: &Scene) -> bool {
        let mut held = self.hand.clone();
        let mut shown = scene.own_tiles();
        held.sort();
        shown.sort();
        // A drawn tile set apart must be the one drawn; one not set apart is in the hand.
        let drawn_ok = scene.drawn.is_none_or(|tile| {
            self.turn == 0 && self.phase == Phase::Discard && self.drawn == Some(tile)
        });
        held == shown
            && drawn_ok
            && scene.tiles_left.is_none_or(|n| n == self.tiles_left)
            && self.rivers == scene.rivers
            && self.dora == scene.dora
            && (0..4).all(|s| {
                self.melds[s].len() == scene.melds[s].len()
                    && self.melds[s]
                        .iter()
                        .zip(&scene.melds[s])
                        .all(|(a, b)| same_meld(a, b))
            })
    }

    /// Whether the scene can still be reached from the model: rivers, melds and dora
    /// only grow, but for the last discard, which a call may take away.
    fn prefix_of(&self, scene: &Scene) -> bool {
        let rivers = (0..4u8).all(|s| {
            let mine = &self.rivers[usize::from(s)];
            let seen = &scene.rivers[usize::from(s)];
            if seen.starts_with(mine) {
                return true;
            }
            // The last discard, about to be called.
            matches!(self.last_discard, Some((from, _)) if from == s)
                && mine
                    .split_last()
                    .is_some_and(|(_, rest)| seen.starts_with(rest))
        });
        let melds = (0..4).all(|s| {
            self.melds[s].len() <= scene.melds[s].len()
                && self.melds[s].iter().zip(&scene.melds[s]).all(|(a, b)| {
                    same_meld(a, b)
                        || (a.added.is_none()
                            && b.added.is_some()
                            && same_meld(
                                a,
                                &MeldView {
                                    added: None,
                                    ..b.clone()
                                },
                            ))
                })
        });
        rivers
            && melds
            && scene.dora.starts_with(&self.dora)
            && scene.tiles_left.is_none_or(|n| n <= self.tiles_left)
    }
}

/// Melds compare by their tiles in sight, the called tile's place and the added tile.
fn same_meld(a: &MeldView, b: &MeldView) -> bool {
    let key = |m: &MeldView| {
        let mut tiles: Vec<Tile> = m.tiles.iter().flatten().copied().collect();
        tiles.sort();
        (tiles, m.tiles.len(), m.source(), m.called(), m.added)
    };
    key(a) == key(b)
}

/// How a called meld lies: the called tile sideways on the side it came from (`off`: 1
/// right, 2 across, 3 left, relative to the caller).
pub(crate) fn called_view(called: Tile, others: &[Tile], off: u8) -> MeldView {
    let mut others: Vec<Option<Tile>> = others.iter().map(|t| Some(*t)).collect();
    others.sort();
    let at = match off {
        3 => 0,
        2 => 1,
        _ => others.len(),
    };
    others.insert(at, Some(called));
    MeldView {
        tiles: others,
        sideways: Some(at),
        added: None,
    }
}

/// How a concealed kan lies: face down at both ends, the red five face up if there is one.
pub(crate) fn concealed_kan_view(tiles: &[Tile]) -> MeldView {
    let mut sorted = tiles.to_vec();
    sorted.sort_by_key(|t| std::cmp::Reverse(t.is_red()));
    MeldView {
        tiles: vec![None, sorted.first().copied(), sorted.get(1).copied(), None],
        sideways: None,
        added: None,
    }
}

/// `from` less one of each tile in `less`.
fn minus(from: &[Tile], less: &[Tile]) -> Vec<Tile> {
    let mut out = from.to_vec();
    for tile in less {
        if let Some(i) = out.iter().position(|t| t == tile) {
            out.remove(i);
        }
    }
    out
}

/// Takes `tiles` out of `hand`; `Err` if one is not there.
fn take(hand: &mut Vec<Tile>, tiles: &[Tile]) -> Result<(), &'static str> {
    for tile in tiles {
        let i = hand
            .iter()
            .position(|t| t == tile)
            .ok_or("a tile that is not in hand")?;
        hand.remove(i);
    }
    Ok(())
}

/// A seat in mjai's numbering relative to the player's.
fn rel(actor: u8, me: u8) -> u8 {
    (actor + 4 - me) % 4
}

fn abs(seat: u8, me: u8) -> u8 {
    (seat + me) % 4
}

fn rtile(tile: Tile) -> RTile {
    tile.to_string().parse().unwrap_or_default()
}

fn unknown() -> RTile {
    "?".parse().unwrap_or_default()
}

/// The tile, or `None` for an unknown one.
fn core(tile: RTile) -> Option<Tile> {
    if tile.is_unknown() {
        return None;
    }
    tile.to_string().parse().ok()
}
