//! What a win is worth (#11): han, fu and points of the hand the player can win with
//! now, scored by libriichi from the player's state (`PlayerState::agari_value`), which
//! counts riichi, ippatsu, the last tile, rinshan, chankan and dora as they happened.
//! The rules are libriichi's (Tenhou's): no rounding up to mangan. Ura dora are not known
//! before the win, so a riichi win is counted without them.

use riichi::algo::agari::Agari;
use riichi::state::PlayerState;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Value {
    /// Won off a discard (ron), or on the player's own draw (tsumo).
    pub ron: bool,
    /// 0 for a yakuman.
    pub han: u8,
    /// 0 when it does not matter (five han or more, or a yakuman).
    pub fu: u8,
    /// How many times yakuman; 0 for a normal hand.
    pub yakuman: u8,
    /// The hand's own points: from the discarder, or the sum of the tsumo payments.
    pub points: i32,
    /// For a tsumo, what the dealer pays (0 when the player is the dealer) and what
    /// each other player pays.
    pub dealer_pays: i32,
    pub others_pay: i32,
    /// What the player gains, with the honba and the riichi sticks on the table.
    pub total: i32,
}

/// The value of the win the player can declare now; `None` when it cannot win.
pub fn value(state: &PlayerState) -> Option<Value> {
    let cans = state.last_cans();
    let ron = cans.can_ron_agari;
    if !ron && !cans.can_tsumo_agari {
        return None;
    }
    let agari = state.agari_value(ron, &[]).ok()?;
    let dealer = state.is_oya();
    let point = agari.point(dealer);
    let (han, fu, yakuman) = match agari {
        Agari::Normal { fu, han } => (han, if han >= 5 { 0 } else { fu }, 0),
        Agari::Yakuman(n) => (0, 0, n),
    };
    let (points, dealer_pays, others_pay) = if ron {
        (point.ron, 0, 0)
    } else if dealer {
        (point.tsumo_ko * 3, 0, point.tsumo_ko)
    } else {
        (
            point.tsumo_oya + point.tsumo_ko * 2,
            point.tsumo_oya,
            point.tsumo_ko,
        )
    };
    let total = points + 300 * i32::from(state.honba()) + 1000 * i32::from(state.kyotaku());
    Some(Value {
        ron,
        han,
        fu,
        yakuman,
        points,
        dealer_pays,
        others_pay,
        total,
    })
}

#[cfg(test)]
mod tests {
    use riichi::mjai::Event;

    use super::*;

    /// A round watched from seat 0: `tehai` is its starting hand, the others' unknown.
    fn start(oya: u8, honba: u8, kyotaku: u8, dora_marker: &str, tehai: &str) -> String {
        let tiles: Vec<String> = crate::parse_tiles(tehai)
            .unwrap()
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect();
        assert_eq!(tiles.len(), 13, "{tehai}");
        let unknown = format!("[{}]", vec!["\"?\""; 13].join(","));
        format!(
            r#"{{"type":"start_kyoku","bakaze":"E","dora_marker":"{dora_marker}","kyoku":{},"honba":{honba},"kyotaku":{kyotaku},"oya":{oya},"scores":[25000,25000,25000,25000],"tehais":[[{}],{unknown},{unknown},{unknown}]}}"#,
            oya + 1,
            tiles.join(",")
        )
    }

    fn draw(actor: u8, pai: &str) -> String {
        format!(r#"{{"type":"tsumo","actor":{actor},"pai":"{pai}"}}"#)
    }

    fn discard(actor: u8, pai: &str, tsumogiri: bool) -> String {
        format!(r#"{{"type":"dahai","actor":{actor},"pai":"{pai}","tsumogiri":{tsumogiri}}}"#)
    }

    /// Another player draws and discards `pai`.
    fn turn(actor: u8, pai: &str) -> [String; 2] {
        [draw(actor, "?"), discard(actor, pai, false)]
    }

    fn play(lines: &[String]) -> PlayerState {
        let mut state = PlayerState::new(0);
        for line in lines {
            let event: Event = serde_json::from_str(line).unwrap();
            state.update(&event).unwrap();
        }
        state
    }

    fn normal(ron: bool, han: u8, fu: u8, points: i32, pays: (i32, i32), total: i32) -> Value {
        Value {
            ron,
            han,
            fu,
            yakuman: 0,
            points,
            dealer_pays: pays.0,
            others_pay: pays.1,
            total,
        }
    }

    fn line(text: &str) -> String {
        text.to_owned()
    }

    /// Seats 1 to 3 each draw and discard `pai`.
    fn others(lines: &mut Vec<String>, pai: &str) {
        for actor in 1..4 {
            lines.extend(turn(actor, pai));
        }
    }

    /// The player's first turn passes with the tile drawn, so a later win is not a
    /// first-turn win (chiihou) and a riichi not a double riichi.
    fn first_turn(lines: &mut Vec<String>) {
        lines.push(draw(0, "W"));
        lines.push(discard(0, "W", true));
    }

    /// Pinfu and menzen tsumo, 20 fu, not the dealer: 700 + 400 + 400. Two honba and a
    /// riichi stick on the table go on top.
    #[test]
    fn pinfu_tsumo() {
        let mut lines = vec![start(
            1,
            2,
            1,
            "N",
            "2m 3m 4m 5m 6m 7m 3p 4p 5p 9p 9p 6s 7s",
        )];
        others(&mut lines, "1p");
        first_turn(&mut lines);
        others(&mut lines, "9m");
        lines.push(draw(0, "8s"));
        let want = normal(false, 2, 20, 1500, (700, 400), 1500 + 600 + 1000);
        assert_eq!(value(&play(&lines)), Some(want));
    }

    /// Seven pairs off a discard, as the dealer: 2 han 25 fu, 2400.
    #[test]
    fn chiitoitsu_ron_as_dealer() {
        let mut lines = vec![start(0, 0, 0, "S", "1m 1m 3p 3p 5s 5s 7s 7s E E P P 9m")];
        lines.push(draw(0, "1s"));
        lines.push(discard(0, "1s", true));
        lines.extend(turn(1, "9m"));
        assert_eq!(
            value(&play(&lines)),
            Some(normal(true, 2, 25, 2400, (0, 0), 2400))
        );
    }

    /// Riichi, ippatsu, menzen tsumo and tanyao on a single wait: 4 han 30 fu, 2000 from
    /// each other player and 3900 from the dealer, plus the player's own riichi stick.
    #[test]
    fn riichi_ippatsu_tsumo() {
        let mut lines = vec![start(
            1,
            0,
            0,
            "N",
            "2m 3m 4m 6p 7p 8p 3s 4s 5s 6s 7s 8s 5m",
        )];
        others(&mut lines, "1p");
        first_turn(&mut lines);
        others(&mut lines, "9m");
        lines.push(draw(0, "N"));
        lines.push(line(r#"{"type":"reach","actor":0}"#));
        lines.push(discard(0, "N", true));
        lines.push(line(
            r#"{"type":"reach_accepted","actor":0,"deltas":[-1000,0,0,0],"scores":[24000,25000,25000,25000]}"#,
        ));
        others(&mut lines, "1s");
        lines.push(draw(0, "5m"));
        let want = normal(false, 4, 30, 7900, (3900, 2000), 7900 + 1000);
        assert_eq!(value(&play(&lines)), Some(want));
    }

    /// Big three dragons off a discard: a yakuman, 32000 when not the dealer.
    #[test]
    fn daisangen_ron() {
        let mut lines = vec![start(1, 0, 0, "N", "P P P F F F C C 1m 2m 3m 9s 9s")];
        others(&mut lines, "1p");
        first_turn(&mut lines);
        lines.extend(turn(1, "C"));
        let want = Value {
            ron: true,
            han: 0,
            fu: 0,
            yakuman: 1,
            points: 32000,
            dealer_pays: 0,
            others_pay: 0,
            total: 32000,
        };
        assert_eq!(value(&play(&lines)), Some(want));
    }

    /// The hand after a call, waiting on a pair: a pon and a discard to set it up.
    fn after_pon(dora_marker: &str, tehai: &str, pon: &str, wait: &str) -> PlayerState {
        let mut lines = vec![start(1, 0, 0, dora_marker, tehai)];
        lines.extend(turn(1, "1m"));
        lines.extend(turn(2, "1m"));
        lines.extend(turn(3, pon));
        lines.push(format!(
            r#"{{"type":"pon","actor":0,"target":3,"pai":"{pon}","consumed":["{pon}","{pon}"]}}"#
        ));
        lines.push(discard(0, "N", false));
        lines.extend(turn(1, wait));
        play(&lines)
    }

    /// An open hand with no yaku cannot win.
    #[test]
    fn an_open_hand_without_yaku_cannot_win() {
        let state = after_pon("N", "9s 9s 2m 3m 4m 5p 6p 7p 6s 7s 8s 1p N", "9s", "1p");
        assert!(!state.last_cans().can_ron_agari);
        assert_eq!(value(&state), None);
    }

    /// Open tanyao (allowed in Mahjong Soul): 1 han 30 fu, 1000.
    #[test]
    fn open_tanyao_ron() {
        let state = after_pon("N", "2s 2s 3m 4m 5m 5p 6p 7p 6s 7s 8s 4p N", "2s", "4p");
        assert_eq!(value(&state), Some(normal(true, 1, 30, 1000, (0, 0), 1000)));
    }

    /// Dora in a meld count: the pon of the dora 2s adds 3 han, 4 han 30 fu, 7700.
    #[test]
    fn dora_in_a_meld_count() {
        let state = after_pon("1s", "2s 2s 3m 4m 5m 5p 6p 7p 6s 7s 8s 4p N", "2s", "4p");
        assert_eq!(value(&state), Some(normal(true, 4, 30, 7700, (0, 0), 7700)));
    }

    /// A red five is a dora: tanyao and a red five, 2 han 30 fu, 2000.
    #[test]
    fn a_red_five_counts() {
        let state = after_pon("N", "2s 2s 3m 4m 5mr 5p 6p 7p 6s 7s 8s 4p N", "2s", "4p");
        assert_eq!(value(&state), Some(normal(true, 2, 30, 2000, (0, 0), 2000)));
    }

    /// Every win the simulated games offer has a value, and only those.
    #[test]
    fn every_win_offered_in_simulated_games_has_a_value() {
        let mut wins = 0;
        for seed in 0..30 {
            let game = crate::game::sim::play(seed, 2);
            let mut state = PlayerState::new(game.me);
            for event in &game.log {
                let cans = state.update(event).unwrap();
                let got = value(&state);
                assert_eq!(got.is_some(), cans.can_agari(), "seed {seed}");
                if let Some(v) = got {
                    assert!(
                        v.points >= 1000 && v.total >= v.points,
                        "seed {seed}: {v:?}"
                    );
                    wins += 1;
                }
            }
        }
        assert!(wins > 10, "{wins}");
    }
}
