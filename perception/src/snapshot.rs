//! What is on the table besides the hand (#8): the four rivers, the four seats' melds, the
//! dora indicators, the round and seat winds and the action buttons (#9), read together from
//! one frame, and a check that they and the hand can all be true at once.

use mahjong_core::Tile;

use crate::buttons::{self, Button};
use crate::capture::FrameView;
use crate::classifier::ClassifierError;
use crate::dora;
use crate::hand::HandLayout;
use crate::meld::{self, Melds};
use crate::numbers::{self, NumberReader, Numbers};
use crate::profile::Profile;
use crate::recognizer::{Classify, Recognition, TileRecognizer};
use crate::river::{self, Rivers};
use crate::round::{self, Round, RoundReader, Wind};
use crate::table::{Seat, Table};

/// The table as read from one frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub rivers: Rivers,
    pub melds: Melds,
    pub dora: Vec<Recognition>,
    /// The round from the label in the middle of the table; `None` when it could not be
    /// read.
    pub round: Option<Round>,
    /// Who deals, from the red wind marker; `None` when the box is not in sight.
    pub dealer: Option<Seat>,
    /// Scores, tiles left, riichi sticks and honba (#9).
    pub numbers: Numbers,
    /// The action buttons shown, left to right (#9).
    pub buttons: Vec<Button>,
}

impl Snapshot {
    /// Every face-up tile on the table, with whether it is in doubt.
    pub fn tiles(&self) -> impl Iterator<Item = &Recognition> {
        let rivers = self.rivers.iter().flatten().map(|t| &t.tile);
        let melds = self
            .melds
            .iter()
            .flatten()
            .flatten()
            .flat_map(|t| t.tile.iter().chain(t.added.iter()));
        rivers.chain(melds).chain(self.dora.iter())
    }

    /// `seat`'s wind; `None` when the dealer is not known.
    pub fn seat_wind(&self, seat: Seat) -> Option<Wind> {
        self.dealer.map(|dealer| round::seat_wind(dealer, seat))
    }

    /// A short text of everything read, to tell whether anything changed.
    pub fn summary(&self) -> String {
        self.tiles()
            .map(|t| format!("{}{}", t.tile, if t.doubt.is_some() { "?" } else { "" }))
            .collect::<Vec<_>>()
            .join(" ")
            + &format!(
                " / {:?} / {:?} / {} {} / {:?}",
                self.melds.iter().map(Vec::len).collect::<Vec<_>>(),
                self.numbers,
                self.round.map_or("-".to_owned(), |r| r.to_string()),
                self.seat_wind(Seat::Own)
                    .map_or("-".to_owned(), |w| w.to_string()),
                self.buttons,
            )
    }
}

/// Reads the table of a frame whose hand is `hand`; `None` when the profile does not describe
/// the table or the game area is degenerate. The round is read only while a dealer marker
/// shows, so a results screen costs no text recognition.
pub fn read<C: Classify>(
    frame: &FrameView<'_>,
    profile: &Profile,
    hand: &HandLayout,
    recognizer: &mut TileRecognizer<C>,
    rounds: &mut RoundReader,
    number_reader: &mut NumberReader,
) -> Result<Option<Snapshot>, ClassifierError> {
    let (Some(geometry), Some(river_layout), Some(meld_layout), Some(dora_layout)) =
        (profile.table, profile.river, profile.melds, profile.dora)
    else {
        return Ok(None);
    };
    let Some(area) = hand.game_area(&profile.hand) else {
        return Ok(None);
    };
    let Some(table) = Table::new(&geometry, &area) else {
        return Ok(None);
    };
    let face = profile.tile_face;
    let dealer = profile
        .centre
        .and_then(|centre| round::dealer(frame, &table, &centre));
    let round = match (profile.centre, dealer) {
        (Some(centre), Some(_)) => rounds.read(&round::rectify_label(frame, &table, &centre)),
        _ => None,
    };
    let dora_boxes = dora::find(frame, &area, hand.pitch, &dora_layout, face);
    // Like the round, the numbers are read only while the box is in sight.
    let numbers = match (profile.centre, dealer) {
        (Some(centre), Some(_)) => numbers::read(
            frame,
            &table,
            &centre,
            &dora_layout,
            dora_boxes.first(),
            hand.pitch,
            number_reader,
        ),
        _ => Numbers::default(),
    };
    Ok(Some(Snapshot {
        rivers: river::read(frame, &table, &river_layout, face, recognizer)?,
        melds: meld::read(frame, &table, &meld_layout, face, recognizer)?,
        dora: dora::recognize(frame, &dora_boxes, recognizer)?,
        round,
        dealer,
        numbers,
        buttons: profile
            .buttons
            .map(|layout| buttons::read(frame, &area, &layout))
            .unwrap_or_default(),
    }))
}

/// Whether the hand and the table can both be right: no more than four of a kind and no
/// more than one red five of a suit in sight. A misread tile, or a frame that is not the
/// table (a results screen, a cut-in), usually breaks this.
pub fn consistent(hand: &[Recognition], table: &Snapshot) -> bool {
    let mut kinds = [0u8; 34];
    let mut reds = [0u8; 3];
    for tile in hand.iter().chain(table.tiles()).map(|r| r.tile) {
        kinds[usize::from(tile.kind())] += 1;
        if tile.is_red() {
            reds[usize::from(tile.kind() / 9)] += 1;
        }
    }
    kinds.iter().all(|&n| n <= 4) && reds.iter().all(|&n| n <= 1)
}

/// How many of each kind are face up on the table, for the advice: red fives count as fives.
pub fn seen(table: &Snapshot) -> [u8; 34] {
    let mut counts = [0u8; 34];
    for tile in table.tiles().map(|r| r.tile) {
        let k = usize::from(Tile::kind(tile));
        counts[k] = counts[k].saturating_add(1);
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognizer::Source;
    use crate::river::RiverTile;

    fn tile(name: &str) -> Recognition {
        Recognition {
            tile: name.parse().unwrap(),
            confidence: 0.99,
            source: Source::Classifier,
            doubt: None,
        }
    }

    fn table(river: &[&str], dora: &[&str]) -> Snapshot {
        let mut snapshot = Snapshot::default();
        snapshot.rivers[1] = river
            .iter()
            .map(|n| RiverTile {
                tile: tile(n),
                sideways: false,
            })
            .collect();
        snapshot.dora = dora.iter().map(|n| tile(n)).collect();
        snapshot
    }

    #[test]
    fn seat_winds_and_round_show_in_the_summary() {
        let mut snapshot = table(&[], &[]);
        assert!(snapshot.summary().ends_with("- - / []"));
        snapshot.dealer = Some(Seat::Left);
        snapshot.round = round::parse_round(&["東2局".to_owned()]);
        assert_eq!(snapshot.seat_wind(Seat::Own), Some(Wind::South));
        assert_eq!(snapshot.seat_wind(Seat::Left), Some(Wind::East));
        assert!(snapshot.summary().ends_with("E2 S / []"));
    }

    #[test]
    fn counts_red_fives_as_fives() {
        let counts = seen(&table(&["5m", "5mr", "E"], &["E"]));
        assert_eq!(counts[4], 2);
        assert_eq!(counts[27], 2);
    }

    #[test]
    fn five_of_a_kind_or_two_red_fives_cannot_be() {
        let hand = [tile("E"), tile("E")];
        assert!(consistent(&hand, &table(&["E"], &["E"])));
        assert!(!consistent(&hand, &table(&["E", "E"], &["E"])));
        assert!(!consistent(&[tile("5pr")], &table(&["5pr"], &[])));
    }
}
