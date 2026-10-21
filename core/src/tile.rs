//! Tiles in mjai notation: `1m`-`9m`, `1p`-`9p`, `1s`-`9s`, red fives `5mr` `5pr` `5sr`,
//! honors `E S W N P F C` (P = white, F = green, C = red dragon).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

const SUITS: [char; 3] = ['m', 'p', 's'];
const HONORS: [char; 7] = ['E', 'S', 'W', 'N', 'P', 'F', 'C'];

/// Number of distinct tile kinds, ignoring red fives.
pub const KINDS: u8 = 34;

/// A single tile. `kind` is 0-8 for man 1-9, 9-17 for pin, 18-26 for sou and 27-33 for
/// honors in `E S W N P F C` order; `red` is only ever set on the three fives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tile {
    kind: u8,
    red: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid mjai tile: {0:?}")]
pub struct ParseTileError(String);

impl Tile {
    pub fn from_kind(kind: u8) -> Option<Self> {
        (kind < KINDS).then_some(Self { kind, red: false })
    }

    pub fn kind(self) -> u8 {
        self.kind
    }

    pub fn is_red(self) -> bool {
        self.red
    }

    pub fn is_honor(self) -> bool {
        self.kind >= 27
    }
}

impl FromStr for Tile {
    type Err = ParseTileError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseTileError(s.to_owned());
        let chars: Vec<char> = s.chars().collect();
        match chars.as_slice() {
            [h] => HONORS
                .iter()
                .position(|c| c == h)
                .map(|i| Self {
                    kind: 27 + i as u8,
                    red: false,
                })
                .ok_or_else(err),
            [rank, suit, rest @ ..] => {
                let rank = rank
                    .to_digit(10)
                    .filter(|r| (1..=9).contains(r))
                    .ok_or_else(err)?;
                let suit = SUITS.iter().position(|c| c == suit).ok_or_else(err)?;
                let red = match rest {
                    [] => false,
                    ['r'] if rank == 5 => true,
                    _ => return Err(err()),
                };
                Ok(Self {
                    kind: suit as u8 * 9 + rank as u8 - 1,
                    red,
                })
            }
            _ => Err(err()),
        }
    }
}

impl fmt::Display for Tile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_honor() {
            return write!(f, "{}", HONORS[usize::from(self.kind - 27)]);
        }
        let suit = SUITS[usize::from(self.kind / 9)];
        write!(f, "{}{}", self.kind % 9 + 1, suit)?;
        if self.red {
            f.write_str("r")?;
        }
        Ok(())
    }
}

impl Serialize for Tile {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Tile {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Parses whitespace-separated mjai tiles, e.g. `"1m 2m 5mr C"`.
pub fn parse_tiles(s: &str) -> Result<Vec<Tile>, ParseTileError> {
    s.split_whitespace().map(str::parse).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_kind() {
        for kind in 0..KINDS {
            let tile = Tile::from_kind(kind).unwrap();
            assert_eq!(tile.to_string().parse::<Tile>().unwrap(), tile);
        }
        assert_eq!(Tile::from_kind(KINDS), None);
    }

    #[test]
    fn parses_numbers_honors_and_red_fives() {
        assert_eq!("1m".parse::<Tile>().unwrap().kind(), 0);
        assert_eq!("9s".parse::<Tile>().unwrap().kind(), 26);
        assert_eq!("E".parse::<Tile>().unwrap().kind(), 27);
        assert_eq!("C".parse::<Tile>().unwrap().kind(), 33);

        let red = "5pr".parse::<Tile>().unwrap();
        assert!(red.is_red());
        assert_eq!(red.kind(), "5p".parse::<Tile>().unwrap().kind());
        assert_eq!(red.to_string(), "5pr");
    }

    #[test]
    fn rejects_invalid_tiles() {
        for bad in ["", "0m", "10m", "1x", "4mr", "5mx", "Z", "m1", "5m r"] {
            assert!(bad.parse::<Tile>().is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn parses_a_hand_and_serializes_as_strings() {
        let hand = parse_tiles("1m 5mr  C\tN").unwrap();
        assert_eq!(
            serde_json::to_string(&hand).unwrap(),
            r#"["1m","5mr","C","N"]"#
        );
        assert!(parse_tiles("1m 4mr").is_err());
    }
}
