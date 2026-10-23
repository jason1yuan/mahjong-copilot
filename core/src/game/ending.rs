//! How a round ended, from the scores (#10): the results screen is not read; the scores
//! the round ended on and the next round's tell who won from whom, or that it was a draw.
//!
//! - A win pays the winner from the loser (ron) or from the three others (tsumo), and
//!   the riichi sticks on the table go to the winner. Two winners and one loser is a
//!   double ron.
//! - A draw pays only for being tenpai: 3000 points from the players not tenpai to those
//!   who are (+3000 / −1000 × 3, ±1500, +1000 × 3 / −3000), or nothing, and the sticks stay.
//! - A dealer's tsumo for 1000 all with no honba and no sticks pays like a draw where only
//!   the dealer was tenpai; it is taken as the draw.

/// A round's ending. Seats are relative to the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// One or two winners, each with whom they won from (themselves for a tsumo).
    Win {
        winners: Vec<(u8, u8)>,
        deltas: [i32; 4],
    },
    Draw {
        deltas: [i32; 4],
    },
}

/// The ending that turns `before` (the last scores of the round, riichi deposits taken)
/// with `sticks` riichi sticks on the table into `after` with `sticks_after`; `None` when
/// the scores fit no ending.
pub fn ending(before: [i32; 4], sticks: u8, after: [i32; 4], sticks_after: u8) -> Option<Ending> {
    let deltas: [i32; 4] = std::array::from_fn(|i| after[i] - before[i]);
    let sum: i32 = deltas.iter().sum();
    let gained: Vec<u8> = (0..4u8).filter(|&s| deltas[usize::from(s)] > 0).collect();
    let paid: Vec<u8> = (0..4u8).filter(|&s| deltas[usize::from(s)] < 0).collect();
    if sticks_after == sticks && sum == 0 && is_tenpai_payment(&deltas) {
        return Some(Ending::Draw { deltas });
    }
    if sticks_after != 0 || sum != 1000 * i32::from(sticks) {
        return None;
    }
    let winners = match (gained.as_slice(), paid.as_slice()) {
        // Ron, one winner or two; the second in turn order after the loser.
        ([_], [loser]) | ([_, _], [loser]) => {
            let mut winners = gained.clone();
            winners.sort_by_key(|w| (w + 4 - loser) % 4);
            winners.into_iter().map(|w| (w, *loser)).collect()
        }
        ([winner], [_, _, _]) => vec![(*winner, *winner)],
        _ => return None,
    };
    Some(Ending::Win { winners, deltas })
}

/// Whether `deltas` are what a draw pays for being tenpai.
fn is_tenpai_payment(deltas: &[i32; 4]) -> bool {
    let tenpai = deltas.iter().filter(|&&d| d > 0).count();
    let (gain, pay) = match tenpai {
        1 => (3000, -1000),
        2 => (1500, -1500),
        3 => (1000, -3000),
        _ => return deltas.iter().all(|&d| d == 0),
    };
    deltas.iter().all(|&d| d == gain || d == pay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ron_pays_the_winner_and_the_sticks_go_with_it() {
        // Seat 2 wins 3900 off seat 1 with one stick on the table.
        let before = [25000, 25000, 24000, 25000];
        let after = [25000, 21100, 28900, 25000];
        assert_eq!(
            ending(before, 1, after, 0),
            Some(Ending::Win {
                winners: vec![(2, 1)],
                deltas: [0, -3900, 4900, 0]
            })
        );
    }

    #[test]
    fn a_tsumo_is_paid_by_the_three_others() {
        let before = [25000; 4];
        let after = [24000, 23000, 29000, 24000];
        let Some(Ending::Win { winners, .. }) = ending(before, 0, after, 0) else {
            panic!("a win");
        };
        assert_eq!(winners, [(2, 2)]);
    }

    #[test]
    fn a_double_ron_names_both_winners_in_turn_order() {
        // Seat 3 deals into seats 1 and 0: seat 0 comes first after seat 3.
        let before = [25000; 4];
        let after = [27000, 28900, 25000, 19100];
        let Some(Ending::Win { winners, .. }) = ending(before, 0, after, 0) else {
            panic!("a win");
        };
        assert_eq!(winners, [(0, 3), (1, 3)]);
    }

    #[test]
    fn a_draw_pays_for_tenpai_and_keeps_the_sticks() {
        let before = [24000, 25000, 25000, 25000];
        for (after, sticks) in [
            ([24000, 25000, 25000, 25000], 1),
            ([27000, 24000, 24000, 24000], 1),
            ([25500, 26500, 23500, 23500], 1),
            ([25000, 26000, 26000, 22000], 1),
        ] {
            assert!(
                matches!(ending(before, 1, after, sticks), Some(Ending::Draw { .. })),
                "{after:?}"
            );
        }
    }

    #[test]
    fn scores_that_fit_nothing_are_not_an_ending() {
        // Sticks gone but nobody gained them.
        assert_eq!(ending([25000; 4], 1, [25000; 4], 0), None);
        // A misread score.
        assert_eq!(ending([25000; 4], 0, [25000, 25000, 25000, 26000], 0), None);
    }
}
