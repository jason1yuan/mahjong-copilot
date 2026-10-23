//! What the screen shows of a game at one settled moment, from the player's seat: the
//! input of the state engine. Perception fills it in; nothing here depends on how.
//!
//! Seats are relative to the player in turn order: 0 is the player, 1 the right player
//! (next to act), 2 across, 3 the left player.

use crate::tile::Tile;

/// A tile in a river, in discard order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RiverTile {
    pub tile: Tile,
    /// Lying sideways: the riichi declaration, or the discard after a declaration that
    /// was called.
    pub sideways: bool,
}

/// A meld as it lies on the table, its tiles left to right as its owner sees them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeldView {
    /// `None` for a tile lying face down (a concealed kan).
    pub tiles: Vec<Option<Tile>>,
    /// Index into `tiles` of the called tile, which lies sideways: leftmost when it came
    /// from the left player, in the middle from across, rightmost from the right player.
    pub sideways: Option<usize>,
    /// The tile added to a pon, making it a kan.
    pub added: Option<Tile>,
}

impl MeldView {
    /// The seat the called tile came from, relative to the owner: 1 right, 2 across, 3
    /// left; `None` for a concealed kan.
    pub fn source(&self) -> Option<u8> {
        let i = self.sideways?;
        let n = self.tiles.len();
        Some(if i == 0 {
            3
        } else if i + 1 == n {
            1
        } else {
            2
        })
    }

    /// The called tile.
    pub fn called(&self) -> Option<Tile> {
        self.tiles.get(self.sideways?).copied().flatten()
    }

    /// Lying face down, or four alike with none called: a concealed kan.
    pub fn is_concealed_kan(&self) -> bool {
        self.sideways.is_none() && self.tiles.len() == 4
    }

    /// Every tile that can be seen, the added one included.
    pub fn seen(&self) -> impl Iterator<Item = Tile> + '_ {
        self.tiles.iter().flatten().copied().chain(self.added)
    }
}

/// The round from the label in the middle of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundLabel {
    /// `E`, `S`, `W` or `N`.
    pub wind: Tile,
    /// 1-4.
    pub number: u8,
}

/// Everything the engine reads from one settled frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scene {
    /// The concealed hand, without the tile just drawn.
    pub hand: Vec<Tile>,
    /// The tile just drawn, set apart on the right.
    pub drawn: Option<Tile>,
    /// Each seat's river, in discard order; called tiles are gone from it.
    pub rivers: [Vec<RiverTile>; 4],
    /// Each seat's melds, in the order they were made.
    pub melds: [Vec<MeldView>; 4],
    pub dora: Vec<Tile>,
    pub round: Option<RoundLabel>,
    /// The dealer's seat.
    pub dealer: Option<u8>,
    pub honba: Option<u8>,
    pub riichi_sticks: Option<u8>,
    /// Tiles left to draw (余牌数): one less for every draw, so it tells how many draws
    /// happened between two scenes, which the screen does not show otherwise.
    pub tiles_left: Option<u8>,
    /// By seat.
    pub scores: Option<[i32; 4]>,
}

impl Scene {
    /// The player's tiles in hand, the drawn one included.
    pub fn own_tiles(&self) -> Vec<Tile> {
        self.hand.iter().copied().chain(self.drawn).collect()
    }

    /// Every tile face up, the player's hand included.
    pub fn seen(&self) -> impl Iterator<Item = Tile> + '_ {
        self.own_tiles()
            .into_iter()
            .chain(self.rivers.iter().flatten().map(|t| t.tile))
            .chain(self.melds.iter().flatten().flat_map(MeldView::seen))
            .chain(self.dora.iter().copied())
    }

    /// Whether the scene can be true at all: no more than four of a kind or one red five
    /// of a suit in sight, and a hand that fits the player's melds (13 less three a meld,
    /// one more on the player's turn).
    pub fn check(&self) -> Result<(), &'static str> {
        let mut kinds = [0u8; 34];
        let mut reds = [0u8; 3];
        for tile in self.seen() {
            kinds[usize::from(tile.kind())] += 1;
            if tile.is_red() {
                reds[usize::from(tile.kind() / 9)] += 1;
            }
        }
        if kinds.iter().any(|&n| n > 4) {
            return Err("more than four of a kind");
        }
        if reds.iter().any(|&n| n > 1) {
            return Err("two red fives of a suit");
        }
        let waiting = 13usize.saturating_sub(3 * self.melds[0].len());
        let held = self.own_tiles().len();
        if held != waiting && held != waiting + 1 {
            return Err("the hand does not fit the melds");
        }
        Ok(())
    }
}
