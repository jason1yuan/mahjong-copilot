//! The decision model (#22): Mortal v4 with the Mortal-S42 weights, exported to ONNX by
//! `ml/convert/export_mortal.py` and run with ONNX Runtime on the CPU. Its input is
//! libriichi's encoding of the player's view ([`PlayerState::encode_obs`]), its output a
//! Q value for each of the 46 actions; the best legal one is the advice, as in Mortal's
//! own engine (greedy, no sampling).
//!
//! Actions: 0-36 discard that tile (34-36 the red fives), 37 riichi, 38-40 chi with the
//! called tile lowest, in the middle or highest, 41 pon, 42 kan, 43 win, 44 abortive draw
//! (nine terminals), 45 pass. Turning an action into an mjai event follows Mortal's agent
//! (`libriichi/src/agent/mortal.rs` upstream), so red fives are used the same way.

use std::path::Path;

use ort::environment::Environment;
use ort::session::Session;
use ort::session::builder::{AutoDevicePolicy, GraphOptimizationLevel, SessionBuilder};
use ort::value::TensorRef;
use riichi::mjai::Event;
use riichi::state::PlayerState;
use riichi::tile::Tile as RTile;
use serde::Serialize;

use crate::tile::Tile;

/// The observation encoding of Mortal v4.
pub const VERSION: u32 = 4;
/// Rows and columns of one observation.
pub const OBS_SHAPE: [usize; 2] = [1012, 34];
pub const OBS_LEN: usize = OBS_SHAPE[0] * OBS_SHAPE[1];
pub const ACTIONS: usize = 46;

const RIICHI: usize = 37;
const CHI_LOW: usize = 38;
const CHI_MID: usize = 39;
const CHI_HIGH: usize = 40;
const PON: usize = 41;
const KAN: usize = 42;
const HORA: usize = 43;
const RYUKYOKU: usize = 44;

#[derive(Debug, thiserror::Error)]
pub enum MortalError {
    #[error("ONNX Runtime: {0}")]
    Ort(#[from] ort::Error),
    #[error("model output has shape {0:?}, expected [{1}, 46]")]
    OutputShape(Vec<i64>, usize),
    #[error("the player has nothing to decide")]
    NothingToDecide,
    #[error("action {0} does not fit the state: {1}")]
    Unfit(usize, &'static str),
    #[error("the observation is not laid out row by row")]
    Layout,
}

impl From<ort::Error<SessionBuilder>> for MortalError {
    fn from(err: ort::Error<SessionBuilder>) -> Self {
        Self::Ort(err.into())
    }
}

/// One legal action and how the model rates it.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub action: usize,
    /// The action as the player's mjai event (`none` for passing).
    pub event: Event,
    pub q: f32,
}

impl Choice {
    /// The choice for the window: what to do, with which tiles.
    pub fn suggestion(&self) -> Suggestion {
        let tile = |t: &RTile| t.to_string().parse::<Tile>().ok();
        let tiles = |ts: &[RTile]| ts.iter().filter_map(tile).collect();
        let (kind, pai, consumed) = match &self.event {
            Event::Dahai { pai, .. } => (Kind::Discard, tile(pai), Vec::new()),
            Event::Reach { .. } => (Kind::Riichi, None, Vec::new()),
            Event::Chi { pai, consumed, .. } => (Kind::Chi, tile(pai), tiles(consumed)),
            Event::Pon { pai, consumed, .. } => (Kind::Pon, tile(pai), tiles(consumed)),
            Event::Daiminkan { pai, consumed, .. } | Event::Kakan { pai, consumed, .. } => {
                (Kind::Kan, tile(pai), tiles(consumed))
            }
            Event::Ankan { consumed, .. } => (Kind::Kan, tile(&consumed[1]), tiles(consumed)),
            Event::Hora { .. } => (Kind::Win, None, Vec::new()),
            Event::Ryukyoku { .. } => (Kind::Draw, None, Vec::new()),
            _ => (Kind::Pass, None, Vec::new()),
        };
        Suggestion {
            kind,
            tile: pai,
            consumed,
            q: self.q,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Discard,
    Riichi,
    Chi,
    Pon,
    Kan,
    Win,
    /// The abortive draw with nine terminals and honours.
    Draw,
    Pass,
}

/// A choice as the window shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub kind: Kind,
    /// The tile discarded, called or made a kan of.
    pub tile: Option<Tile>,
    /// For a call, the tiles from the hand.
    pub consumed: Vec<Tile>,
    pub q: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    /// Every legal action, best first. Never empty.
    pub choices: Vec<Choice>,
    /// Whether the model was asked: with a single possible discard it is not.
    pub evaluated: bool,
    /// When the best choice is riichi: the discard that declares it.
    pub riichi_discard: Option<Choice>,
}

impl Decision {
    pub fn best(&self) -> &Choice {
        // Invariant: `decide` only makes decisions with at least one legal choice.
        &self.choices[0]
    }
}

pub struct Mortal {
    session: Session,
}

impl Mortal {
    /// Loads the ONNX model to run on the CPU (faster than CoreML for it, see
    /// docs/design.md). `environment` is the process-wide ONNX Runtime environment.
    pub fn load(environment: &Environment, model: &Path) -> Result<Self, MortalError> {
        let session = Session::builder(environment)?
            .with_auto_device(AutoDevicePolicy::PreferCPU)?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .commit_from_file(model)?;
        Ok(Self { session })
    }

    /// Runs the model once, so the first real decision does not pay for ONNX Runtime
    /// setting up its memory.
    pub fn warm_up(&mut self) -> Result<(), MortalError> {
        self.q_values(&vec![0.0; OBS_LEN], &[true; ACTIONS], 1)
            .map(|_| ())
    }

    /// What the player should do now, if it has anything to decide.
    pub fn decide(&mut self, state: &PlayerState) -> Result<Decision, MortalError> {
        let cans = state.last_cans();
        if !cans.can_act() {
            return Err(MortalError::NothingToDecide);
        }
        // As Mortal's quick evaluation: a lone possible discard needs no model.
        let only_discard = cans.can_discard
            && !cans.can_riichi
            && !cans.can_tsumo_agari
            && !cans.can_ankan
            && !cans.can_kakan
            && !cans.can_ryukyoku;
        if only_discard {
            let candidates = state.discard_candidates_aka();
            let mut legal = candidates.iter().enumerate().filter(|(_, ok)| **ok);
            if let (Some((action, _)), None) = (legal.next(), legal.next()) {
                return Ok(Decision {
                    choices: vec![Choice {
                        action,
                        event: event(state, action, None)?,
                        q: 0.0,
                    }],
                    evaluated: false,
                    riichi_discard: None,
                });
            }
        }

        let (q, mask) = self.evaluate(state, false)?;
        let mut order: Vec<usize> = (0..ACTIONS).filter(|&a| mask[a]).collect();
        order.sort_by(|&a, &b| q[b].total_cmp(&q[a]));
        if order.is_empty() {
            return Err(MortalError::NothingToDecide);
        }
        // A kan with more than one tile to choose: the model is asked again which.
        let kan_tile = if order[0] == KAN
            && state.ankan_candidates().len() + state.kakan_candidates().len() > 1
        {
            let (kan_q, kan_mask) = self.evaluate(state, true)?;
            (0..37)
                .filter(|&a| kan_mask[a])
                .max_by(|&a, &b| kan_q[a].total_cmp(&kan_q[b]))
                .and_then(|a| RTile::try_from(a).ok())
        } else {
            None
        };
        let choices = order
            .iter()
            .enumerate()
            .map(|(rank, &action)| {
                let pick = if rank == 0 { kan_tile } else { None };
                Ok(Choice {
                    action,
                    event: event(state, action, pick)?,
                    q: q[action],
                })
            })
            .collect::<Result<Vec<_>, MortalError>>()?;
        // Riichi is declared with a discard, which the model then chooses.
        let riichi_discard = if order[0] == RIICHI {
            let mut declared = state.clone();
            declared
                .update(&Event::Reach {
                    actor: state.player_id(),
                })
                .map_err(|_| MortalError::Unfit(RIICHI, "riichi"))?;
            Some(self.decide(&declared)?.best().clone())
        } else {
            None
        };
        Ok(Decision {
            choices,
            evaluated: true,
            riichi_discard,
        })
    }

    /// Q values and legal actions of the state.
    fn evaluate(
        &mut self,
        state: &PlayerState,
        at_kan_select: bool,
    ) -> Result<([f32; ACTIONS], [bool; ACTIONS]), MortalError> {
        let (obs, mask) = state.encode_obs(VERSION, at_kan_select);
        let mut legal = [false; ACTIONS];
        for (out, m) in legal.iter_mut().zip(mask.iter()) {
            *out = *m;
        }
        // libriichi builds the observation row by row, so this copies nothing.
        let obs = obs.as_standard_layout();
        let obs = obs.as_slice().ok_or(MortalError::Layout)?;
        let q = self.q_values(obs, &legal, 1)?;
        let mut out = [f32::NEG_INFINITY; ACTIONS];
        out.copy_from_slice(&q[..ACTIONS]);
        Ok((out, legal))
    }

    /// Raw Q values: `batch` observations of [`OBS_LEN`] floats and masks of [`ACTIONS`]
    /// in, `batch` rows of 46 out, illegal actions at minus infinity.
    pub fn q_values(
        &mut self,
        obs: &[f32],
        mask: &[bool],
        batch: usize,
    ) -> Result<Vec<f32>, MortalError> {
        let obs = TensorRef::from_array_view((
            [batch, OBS_SHAPE[0], OBS_SHAPE[1]],
            &obs[..batch * OBS_LEN],
        ))?;
        let mask = TensorRef::from_array_view(([batch, ACTIONS], &mask[..batch * ACTIONS]))?;
        let outputs = self
            .session
            .run(ort::inputs!["obs" => obs, "mask" => mask])?;
        let (shape, q) = outputs["q"].try_extract_tensor::<f32>()?;
        if **shape != [batch as i64, ACTIONS as i64] {
            return Err(MortalError::OutputShape(shape.to_vec(), batch));
        }
        Ok(q.to_vec())
    }
}

/// The player's mjai event for a legal `action`; `kan_tile` is the model's pick when it
/// was asked which tile to kan.
pub fn event(
    state: &PlayerState,
    action: usize,
    kan_tile: Option<RTile>,
) -> Result<Event, MortalError> {
    let actor = state.player_id();
    let cans = state.last_cans();
    let akas = state.akas_in_hand();
    // Whether the hand holds the red five of the called tile's suit, for the tiles listed.
    let aka_of = |pai: RTile, fives_near: [u8; 2]| -> bool {
        let id = pai.deaka().as_u8();
        let suit = usize::from(id / 9);
        suit < 3 && fives_near.contains(&(id % 9)) && akas[suit]
    };
    let kawa = || {
        state
            .last_kawa_tile()
            .ok_or(MortalError::Unfit(action, "no discard to call"))
    };
    let target = cans.target_actor;
    Ok(match action {
        0..=36 => {
            let pai = RTile::try_from(action).map_err(|_| MortalError::Unfit(action, "tile"))?;
            Event::Dahai {
                actor,
                pai,
                tsumogiri: state.last_self_tsumo() == Some(pai),
            }
        }
        RIICHI => Event::Reach { actor },
        // Ranks below are 0-based: 2 and 3 are the 3 and 4 that a chi low uses a red five
        // with, and so on.
        CHI_LOW => {
            let pai = kawa()?;
            let first = pai.next();
            let consumed = if aka_of(pai, [2, 3]) {
                [first.akaize(), first.next().akaize()]
            } else {
                [first, first.next()]
            };
            Event::Chi {
                actor,
                target,
                pai,
                consumed,
            }
        }
        CHI_MID => {
            let pai = kawa()?;
            let consumed = if aka_of(pai, [3, 5]) {
                [pai.prev().akaize(), pai.next().akaize()]
            } else {
                [pai.prev(), pai.next()]
            };
            Event::Chi {
                actor,
                target,
                pai,
                consumed,
            }
        }
        CHI_HIGH => {
            let pai = kawa()?;
            let last = pai.prev();
            let consumed = if aka_of(pai, [5, 6]) {
                [last.prev().akaize(), last.akaize()]
            } else {
                [last.prev(), last]
            };
            Event::Chi {
                actor,
                target,
                pai,
                consumed,
            }
        }
        PON => {
            let pai = kawa()?;
            let consumed = if aka_of(pai, [4, 4]) {
                [pai.akaize(), pai.deaka()]
            } else {
                [pai.deaka(); 2]
            };
            Event::Pon {
                actor,
                target,
                pai,
                consumed,
            }
        }
        KAN => kan(state, action, kan_tile)?,
        HORA => Event::Hora {
            actor,
            target,
            deltas: None,
            ura_markers: None,
        },
        RYUKYOKU => Event::Ryukyoku { deltas: None },
        _ => Event::None,
    })
}

fn kan(state: &PlayerState, action: usize, pick: Option<RTile>) -> Result<Event, MortalError> {
    let actor = state.player_id();
    let cans = state.last_cans();
    let ankan = state.ankan_candidates();
    let kakan = state.kakan_candidates();
    let tile = if let Some(tile) = pick.filter(|t| ankan.contains(t) || kakan.contains(t)) {
        tile
    } else if cans.can_daiminkan {
        state
            .last_kawa_tile()
            .ok_or(MortalError::Unfit(action, "no discard to call"))?
    } else if let Some(tile) = ankan.first().or(kakan.first()) {
        *tile
    } else {
        return Err(MortalError::Unfit(action, "no kan"));
    };
    if cans.can_daiminkan {
        let consumed = if tile.is_aka() {
            [tile.deaka(); 3]
        } else {
            [tile.akaize(), tile, tile]
        };
        return Ok(Event::Daiminkan {
            actor,
            target: cans.target_actor,
            pai: tile,
            consumed,
        });
    }
    if cans.can_ankan && ankan.contains(&tile.deaka()) {
        return Ok(Event::Ankan {
            actor,
            consumed: [tile.akaize(), tile, tile, tile],
        });
    }
    let id = tile.deaka().as_u8();
    let red_in_hand = id < 27 && id % 9 == 4 && state.akas_in_hand()[usize::from(id / 9)];
    let (pai, consumed) = if red_in_hand {
        (tile.akaize(), [tile.deaka(); 3])
    } else {
        (tile.deaka(), [tile.akaize(), tile.deaka(), tile.deaka()])
    };
    Ok(Event::Kakan {
        actor,
        pai,
        consumed,
    })
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::game::sim::{Game, play};
    use crate::game::{Engine, Rejected};

    /// The model exported by `ml/convert/export_mortal.py`, kept local (#22).
    fn model_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../ml/models/mortal-s42")
    }

    fn load() -> Mortal {
        // ONNX Runtime allows one environment per process, and tests share the process.
        static ENVIRONMENT: std::sync::OnceLock<Environment> = std::sync::OnceLock::new();
        let environment = ENVIRONMENT.get_or_init(|| ort::init().build().unwrap());
        let mut mortal = Mortal::load(environment, &model_dir().join("model.onnx")).unwrap();
        mortal.warm_up().unwrap();
        mortal
    }

    /// The watched seat's states where it can act, in a simulated game's log as given.
    fn decision_points(game: &Game, log: &[Event]) -> Vec<PlayerState> {
        let mut state = PlayerState::new(game.me);
        let mut out = Vec::new();
        for event in log {
            if state.update(event).unwrap().can_act() {
                out.push(state.clone());
            }
        }
        out
    }

    fn legal(state: &PlayerState, at_kan_select: bool) -> Vec<usize> {
        let (_, mask) = state.encode_obs(VERSION, at_kan_select);
        (0..ACTIONS).filter(|&a| mask[a]).collect()
    }

    /// Every action the encoding calls legal becomes an event libriichi accepts.
    #[test]
    fn every_legal_action_is_a_valid_event() {
        let mut kinds = [0usize; ACTIONS];
        for seed in 0..6 {
            let game = play(seed, 2);
            for state in decision_points(&game, &game.log) {
                for action in legal(&state, false) {
                    let event = event(&state, action, None).unwrap();
                    if action != 45 {
                        state.validate_reaction(&event).unwrap_or_else(|err| {
                            panic!("seed {seed}, action {action}, {event:?}: {err}")
                        });
                    }
                    kinds[action] += 1;
                }
                if state.ankan_candidates().len() + state.kakan_candidates().len() > 1 {
                    for action in legal(&state, true) {
                        let pick = RTile::try_from(action).ok();
                        state
                            .validate_reaction(&event(&state, KAN, pick).unwrap())
                            .unwrap();
                    }
                }
            }
        }
        // The simulated games call, riichi and win, so those paths are taken.
        for action in [CHI_LOW, PON, RIICHI, HORA] {
            assert!(kinds[action] > 0, "action {action} never legal");
        }
    }

    /// The Rust run agrees with PyTorch on real decision points: the first ones written by
    /// the dump_observations example, with PyTorch's Q saved by the export script.
    #[test]
    #[ignore = "needs the local model, observations and reference"]
    fn matches_the_pytorch_reference() {
        let dir = model_dir();
        let reference = std::fs::read(dir.join("reference.bin")).unwrap();
        let want: Vec<f32> = reference
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        let points = want.len() / ACTIONS;
        let raw = std::fs::read(dir.join("observations.bin")).unwrap();
        let size = OBS_LEN * 4 + ACTIONS;
        let mut mortal = load();
        for (i, point) in raw.chunks_exact(size).take(points).enumerate() {
            let obs: Vec<f32> = point[..OBS_LEN * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect();
            let mask: Vec<bool> = point[OBS_LEN * 4..].iter().map(|m| *m != 0).collect();
            let got = mortal.q_values(&obs, &mask, 1).unwrap();
            let want = &want[i * ACTIONS..(i + 1) * ACTIONS];
            for a in 0..ACTIONS {
                if mask[a] {
                    assert!((got[a] - want[a]).abs() < 1e-3, "point {i}, action {a}");
                } else {
                    assert_eq!(got[a], f32::NEG_INFINITY, "point {i}, action {a}");
                }
            }
        }
    }

    /// On simulated games Mortal only ever picks legal actions, and decides in time: the
    /// model within 10 ms (docs/design.md), the whole decision, encoding included, within
    /// 20 ms (P95).
    #[test]
    #[ignore = "needs the local model; run with --release, CARGO_PROFILE_RELEASE_LTO=false"]
    fn plays_simulated_games_legally_and_in_time() {
        let p95 = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            (v[v.len() / 2], v[v.len() * 95 / 100])
        };
        let ms = |start: Instant| start.elapsed().as_secs_f64() * 1e3;
        let mut mortal = load();
        let (mut encode, mut model, mut whole) = (Vec::new(), Vec::new(), Vec::new());
        let mut kinds = [0usize; ACTIONS];
        for seed in 0..20 {
            let game = play(seed, 2);
            for state in decision_points(&game, &game.log) {
                let start = Instant::now();
                let decision = mortal.decide(&state).unwrap();
                if decision.evaluated {
                    whole.push(ms(start));
                    let start = Instant::now();
                    let (obs, mask) = state.encode_obs(VERSION, false);
                    encode.push(ms(start));
                    let mask: Vec<bool> = mask.to_vec();
                    let start = Instant::now();
                    mortal.q_values(obs.as_slice().unwrap(), &mask, 1).unwrap();
                    model.push(ms(start));
                }
                let best = decision.best();
                if best.action != 45 {
                    state.validate_reaction(&best.event).unwrap();
                }
                if best.action == RIICHI {
                    let mut declared = state.clone();
                    declared.update(&best.event).unwrap();
                    let discard = decision.riichi_discard.as_ref().unwrap();
                    declared.validate_reaction(&discard.event).unwrap();
                }
                kinds[best.action] += 1;
            }
        }
        let decisions = whole.len();
        let (encode, model, whole) = (p95(encode), p95(model), p95(whole));
        println!(
            "{decisions} decisions (median, p95): encoding {:.2}, {:.2} ms; model {:.1}, {:.1} ms; whole {:.1}, {:.1} ms",
            encode.0, encode.1, model.0, model.1, whole.0, whole.1
        );
        println!(
            "chosen: discards {}, riichi {}, chi {}, pon {}, kan {}, win {}, pass {}",
            kinds[..37].iter().sum::<usize>(),
            kinds[RIICHI],
            kinds[CHI_LOW] + kinds[CHI_MID] + kinds[CHI_HIGH],
            kinds[PON],
            kinds[KAN],
            kinds[HORA],
            kinds[45]
        );
        assert!(model.1 < 10.0, "model p95 {:.1} ms", model.1);
        assert!(whole.1 < 20.0, "whole p95 {:.1} ms", whole.1);
    }

    /// From a round the engine filled in, opened part way through, Mortal decides legally.
    #[test]
    #[ignore = "needs the local model; run with --release, CARGO_PROFILE_RELEASE_LTO=false"]
    fn decides_on_filled_in_histories() {
        let mut mortal = load();
        let mut decided = 0;
        for seed in 0..10 {
            let game = play(seed, 2);
            let mut engine = Engine::new();
            for moment in &game.moments[game.moments.len() / 3..] {
                match engine.observe(&moment.scene) {
                    Ok(_) | Err(Rejected::Unexplained | Rejected::Covered) => {}
                    Err(err) => panic!("seed {seed}: {err}"),
                }
                let Some(state) = engine.player_state() else {
                    continue;
                };
                if !state.last_cans().can_act() {
                    continue;
                }
                let best = mortal.decide(state).unwrap().best().clone();
                if best.action != 45 {
                    state.validate_reaction(&best.event).unwrap();
                }
                decided += 1;
            }
        }
        assert!(decided > 100, "{decided}");
    }

    /// How much it costs that the screen does not tell the others' draws discarded at once
    /// from their other discards (docs/design.md): Mortal's choices with the real
    /// information against those with every other discard taken as from the hand.
    #[test]
    #[ignore = "needs the local model; run with --release, CARGO_PROFILE_RELEASE_LTO=false"]
    fn tsumogiri_experiment() {
        let mut mortal = load();
        let (mut same, mut total, mut same_discard, mut discards) = (0, 0, 0, 0);
        for seed in 0..40 {
            let game = play(seed, 2);
            let told: Vec<Event> = game
                .log
                .iter()
                .zip(&game.tsumogiri)
                .map(|(e, &tsumogiri)| match e {
                    Event::Dahai { actor, pai, .. } if tsumogiri => Event::Dahai {
                        actor: *actor,
                        pai: *pai,
                        tsumogiri: true,
                    },
                    e => e.clone(),
                })
                .collect();
            let full = decision_points(&game, &told);
            let seen = decision_points(&game, &game.log);
            assert_eq!(full.len(), seen.len());
            for (a, b) in full.iter().zip(&seen) {
                let x = mortal.decide(a).unwrap();
                if !x.evaluated {
                    continue;
                }
                let y = mortal.decide(b).unwrap();
                total += 1;
                same += usize::from(x.best().action == y.best().action);
                if a.last_cans().can_discard {
                    discards += 1;
                    same_discard += usize::from(x.best().action == y.best().action);
                }
            }
        }
        println!(
            "same choice in {same} of {total} decisions ({:.1}%), {same_discard} of {discards} discards ({:.1}%)",
            100.0 * same as f64 / total as f64,
            100.0 * same_discard as f64 / discards as f64
        );
    }
}
