//! The numbers on the table (#9): the four scores and the tiles left in the box in the
//! middle of the table, and the riichi sticks and honba under the dora indicators.
//!
//! - Numbers are read with the system's text recognition (`text`) as rarely as possible:
//!   - the scores and the tiles left are remembered digit by digit. Each digit glyph's look
//!     is kept with its digit, learnt from the times the whole number had to be recognized;
//!     once a digit has shown, it is read by comparing pixels. Glyphs are cut to their own
//!     ink, so a pixel's shift or a glow beside the number does not make a new look;
//!   - the riichi stick and honba counters, small and rarely changing, are remembered as
//!     whole thumbnails, as the round label is (`round`).
//! - The scores and the riichi sticks must add up: in a four-player game the scores and
//!   1,000 per stick on the table always make 100,000. Scores that do not (a digit hidden
//!   by an effect) are not trusted.
//! - The tiles left are read from their digits alone: the stylised 余 in front of them makes
//!   the recognition garble the whole label about one time in nine.

use std::collections::HashMap;

use image::{GrayImage, RgbImage, imageops};

use crate::capture::FrameView;
use crate::hand::TileBox;
use crate::profile::{CentreLayout, DoraLayout};
use crate::round::{MAX_DISTANCE, distance, thumbnail};
use crate::table::{Seat, Table};
use crate::text::ReadText;

/// The scores and 1,000 per riichi stick on the table always make this in a four-player
/// game.
pub const TOTAL_POINTS: u32 = 100_000;
/// Pixels per tile width the centre box's numbers are sampled at, as the round label.
const SCALE: f64 = 96.0;
/// Whole looks remembered per counter.
const MAX_SEEN: usize = 32;
/// A digit glyph is shrunk to this size to be compared.
const GLYPH: (u32, u32) = (12, 18);
/// A glyph matches a remembered one this close (mean absolute difference, 0-255)...
const GLYPH_DISTANCE: f32 = 24.0;
/// ...when it is this much closer than to any other digit.
const GLYPH_MARGIN: f32 = 8.0;
/// Glyph looks remembered per digit.
const MAX_PER_DIGIT: usize = 4;

/// Which number a picture is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    Score(Seat),
    TilesLeft,
    RiichiSticks,
    Honba,
}

/// The numbers as read; each `None` when it could not be read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Numbers {
    /// In turn order from the player ([`Seat::index`]); only when they add up.
    pub scores: Option<[u32; 4]>,
    pub tiles_left: Option<u8>,
    pub riichi_sticks: Option<u8>,
    pub honba: Option<u8>,
}

/// The first number in recognized text, half or full width digits (`x1`, `余58`, `25000`).
pub fn parse_number(lines: &[String]) -> Option<u32> {
    lines.iter().find_map(|line| {
        let digits: String = line
            .chars()
            .map(|c| match c {
                '０'..='９' => {
                    char::from_u32(u32::from(c) - u32::from('０') + u32::from('0')).unwrap_or(c)
                }
                c => c,
            })
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    })
}

/// Whether scores and riichi sticks make the game's total.
pub fn adds_up(scores: [u32; 4], sticks: u8) -> bool {
    scores.iter().sum::<u32>() + 1000 * u32::from(sticks) == TOTAL_POINTS
}

/// The digits' colour: which pixels are their ink.
#[derive(Clone, Copy, Debug)]
enum Ink {
    /// The scores: orange.
    Orange,
    /// The tiles left: cyan, unlike the grey frame around them.
    Cyan,
}

impl Ink {
    fn is_ink(self, [r, g, b]: [u8; 3]) -> bool {
        match self {
            Ink::Orange => r >= 150 && g >= 90 && b.saturating_add(60) <= r,
            Ink::Cyan => g >= 110 && b >= 110 && r.saturating_add(40) <= g.min(b),
        }
    }
}

/// The digit glyphs in a picture, left to right, each shrunk to [`GLYPH`]: within the
/// tallest band of rows with ink (so a glow line above or below is left out), runs of
/// columns with ink, cut to the rows their ink spans. Empty when a run is wider than tall,
/// which no digit is.
fn split_glyphs(picture: &RgbImage, ink: Ink) -> Vec<GrayImage> {
    let (w, h) = picture.dimensions();
    let at = |x: u32, y: u32| ink.is_ink(picture.get_pixel(x, y).0);
    // The tallest band of rows with ink.
    let inked_rows: Vec<bool> = (0..h).map(|y| (0..w).any(|x| at(x, y))).collect();
    let mut band = (0, 0);
    let mut y = 0;
    while y < h {
        if !inked_rows[y as usize] {
            y += 1;
            continue;
        }
        let start = y;
        while y < h && inked_rows[y as usize] {
            y += 1;
        }
        if y - start > band.1 - band.0 {
            band = (start, y);
        }
    }
    let (y0, y1) = band;
    let inked: Vec<bool> = (0..w).map(|x| (y0..y1).any(|y| at(x, y))).collect();
    let mut glyphs = Vec::new();
    let mut x = 0;
    while x < w {
        if !inked[x as usize] {
            x += 1;
            continue;
        }
        let start = x;
        while x < w && inked[x as usize] {
            x += 1;
        }
        // Specks narrower than an eighth of the band are not digits.
        if (x - start) * 8 < y1 - y0 {
            continue;
        }
        let rows: Vec<u32> = (y0..y1)
            .filter(|&y| (start..x).any(|xx| at(xx, y)))
            .collect();
        let (top, bottom) = (rows[0], rows[rows.len() - 1] + 1);
        if x - start >= bottom - top {
            return Vec::new();
        }
        let crop = imageops::crop_imm(picture, start, top, x - start, bottom - top).to_image();
        glyphs.push(imageops::resize(
            &imageops::grayscale(&crop),
            GLYPH.0,
            GLYPH.1,
            imageops::FilterType::Triangle,
        ));
    }
    glyphs
}

fn mean_difference(a: &GrayImage, b: &GrayImage) -> f32 {
    let sum: u32 = a
        .pixels()
        .zip(b.pixels())
        .map(|(p, q)| u32::from(p[0].abs_diff(q[0])))
        .sum();
    sum as f32 / (a.width() * a.height()).max(1) as f32
}

/// Digit glyphs seen in one font, with their digits.
#[derive(Default)]
struct Glyphs {
    known: Vec<(GrayImage, u8)>,
}

impl Glyphs {
    /// The digit a glyph shows, when it is close to one remembered digit and clearly
    /// closer than to any other.
    fn digit(&self, glyph: &GrayImage) -> Option<u8> {
        let mut by_digit = [f32::INFINITY; 10];
        for (known, digit) in &self.known {
            let d = mean_difference(known, glyph);
            let best = &mut by_digit[usize::from(*digit)];
            *best = best.min(d);
        }
        let (digit, &d) = by_digit
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1))?;
        let other = by_digit
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != digit)
            .map(|(_, &d)| d)
            .fold(f32::INFINITY, f32::min);
        (d <= GLYPH_DISTANCE && other - d >= GLYPH_MARGIN).then_some(digit as u8)
    }

    fn learn(&mut self, glyph: GrayImage, digit: u8) {
        if self.digit(&glyph) == Some(digit) {
            return;
        }
        let same: Vec<usize> = (0..self.known.len())
            .filter(|&i| self.known[i].1 == digit)
            .collect();
        if same.len() >= MAX_PER_DIGIT {
            self.known.remove(same[0]);
        }
        self.known.push((glyph, digit));
    }
}

struct Seen {
    thumb: GrayImage,
    value: Option<u32>,
}

/// Reads numbers, sending as few pictures as it can to text recognition.
pub struct NumberReader {
    text: Option<Box<dyn ReadText>>,
    score_glyphs: Glyphs,
    tiles_left_glyphs: Glyphs,
    /// Whole looks of the counters.
    seen: HashMap<Field, Vec<Seen>>,
    recognized_by: HashMap<Field, usize>,
}

impl NumberReader {
    /// Reads with `text`; without it (no text recognition on this platform) only what was
    /// learnt before can be read, which is nothing.
    pub fn new(text: Option<Box<dyn ReadText>>) -> Self {
        Self {
            text,
            score_glyphs: Glyphs::default(),
            tiles_left_glyphs: Glyphs::default(),
            seen: HashMap::new(),
            recognized_by: HashMap::new(),
        }
    }

    /// Forgets everything learnt, e.g. when the window size changes.
    pub fn clear(&mut self) {
        self.score_glyphs = Glyphs::default();
        self.tiles_left_glyphs = Glyphs::default();
        self.seen.clear();
    }

    /// How many pictures went to text recognition so far: the rest were matched.
    pub fn recognized(&self) -> usize {
        self.recognized_by.values().sum()
    }

    /// How many pictures of `field` went to text recognition so far.
    pub fn recognized_by(&self, field: Field) -> usize {
        self.recognized_by.get(&field).copied().unwrap_or(0)
    }

    /// The whole picture through text recognition; `None` (logged) when that fails.
    fn recognize(&mut self, field: Field, picture: &RgbImage) -> Option<Vec<String>> {
        let text = self.text.as_mut()?;
        *self.recognized_by.entry(field).or_default() += 1;
        match text.read_text(picture) {
            Ok(lines) => Some(lines),
            Err(err) => {
                tracing::warn!("reading {field:?}: {err}");
                None
            }
        }
    }

    /// A number written in digits of one font: its glyphs matched one by one, or, while any
    /// of them is new, the whole number recognized and its glyphs learnt.
    pub fn read_digits(&mut self, field: Field, picture: &RgbImage) -> Option<u32> {
        let ink = match field {
            Field::TilesLeft => Ink::Cyan,
            _ => Ink::Orange,
        };
        let glyphs = split_glyphs(picture, ink);
        let set = match ink {
            Ink::Orange => &self.score_glyphs,
            Ink::Cyan => &self.tiles_left_glyphs,
        };
        if !glyphs.is_empty()
            && let Some(digits) = glyphs
                .iter()
                .map(|g| set.digit(g))
                .collect::<Option<Vec<u8>>>()
        {
            return Some(digits.iter().fold(0, |n, &d| n * 10 + u32::from(d)));
        }
        let value = parse_number(&self.recognize(field, picture)?)?;
        tracing::debug!(?field, value, "read a number");
        let digits: Vec<u8> = value.to_string().bytes().map(|b| b - b'0').collect();
        // Learn the glyphs only when the text and the picture agree on how many there are.
        if digits.len() == glyphs.len() {
            let set = match ink {
                Ink::Orange => &mut self.score_glyphs,
                Ink::Cyan => &mut self.tiles_left_glyphs,
            };
            for (glyph, digit) in glyphs.into_iter().zip(digits) {
                set.learn(glyph, digit);
            }
        }
        Some(value)
    }

    /// A counter, remembered by its whole look; `None` when it does not read as a number. A
    /// failed recognition is not remembered, so it is tried again.
    pub fn read_whole(&mut self, field: Field, picture: &RgbImage) -> Option<u32> {
        let thumb = thumbnail(picture);
        if let Some((seen, _)) = self
            .seen
            .get(&field)
            .into_iter()
            .flatten()
            .map(|s| (s, distance(&s.thumb, &thumb)))
            .filter(|&(_, d)| d <= MAX_DISTANCE)
            .min_by(|a, b| a.1.total_cmp(&b.1))
        {
            return seen.value;
        }
        let value = parse_number(&self.recognize(field, picture)?);
        tracing::debug!(?field, ?value, "read a counter");
        let seen = self.seen.entry(field).or_default();
        if seen.len() == MAX_SEEN {
            seen.remove(0);
        }
        seen.push(Seen { thumb, value });
        value
    }
}

/// The picture of a box given in `seat`'s coordinates, upright as that seat sees it.
fn rectify(
    frame: &FrameView<'_>,
    table: &Table,
    seat: Seat,
    [x0, y0, x1, y1]: [f64; 4],
) -> RgbImage {
    table.rectify(frame, seat, [x0, y0], [x1 - x0, y1 - y0], SCALE)
}

/// The picture of a counter under the dora indicators: `place` is `[x, y, width, height]`
/// from the first indicator's top left, in hand pitches.
fn counter(frame: &FrameView<'_>, first: &TileBox, pitch: f32, [x, y, w, h]: [f32; 4]) -> RgbImage {
    let left = (first.left as f32 + x * pitch).max(0.0) as u32;
    let top = (first.top as f32 + y * pitch).max(0.0) as u32;
    frame
        .sub(left, top, (w * pitch) as u32, (h * pitch) as u32)
        .to_rgb()
}

/// Reads the numbers. `first_dora` is the first dora indicator's box, which the counters
/// are found from; `pitch` the hand's tile pitch.
pub fn read(
    frame: &FrameView<'_>,
    table: &Table,
    centre: &CentreLayout,
    dora: &DoraLayout,
    first_dora: Option<&TileBox>,
    pitch: f32,
    reader: &mut NumberReader,
) -> Numbers {
    let byte = |v: Option<u32>| v.and_then(|v| u8::try_from(v).ok());
    let (riichi_sticks, honba) = match first_dora {
        Some(first) => (
            byte(reader.read_whole(
                Field::RiichiSticks,
                &counter(frame, first, pitch, dora.sticks),
            )),
            byte(reader.read_whole(Field::Honba, &counter(frame, first, pitch, dora.honba))),
        ),
        None => (None, None),
    };
    let mut scores = [0u32; 4];
    let mut all = true;
    for seat in Seat::ALL {
        let picture = rectify(frame, table, seat, centre.scores[seat.index()]);
        match reader.read_digits(Field::Score(seat), &picture) {
            Some(score) => scores[seat.index()] = score,
            None => all = false,
        }
    }
    let scores =
        (all && riichi_sticks.is_some_and(|sticks| adds_up(scores, sticks))).then_some(scores);
    let tiles_left = byte(reader.read_digits(
        Field::TilesLeft,
        &rectify(frame, table, Seat::Own, centre.tiles_left),
    ))
    .filter(|&n| n <= 70);
    Numbers {
        scores,
        tiles_left,
        riichi_sticks,
        honba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::TextError;
    use image::Rgb;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|&s| s.to_owned()).collect()
    }

    #[test]
    fn parses_numbers() {
        assert_eq!(parse_number(&lines(&["25000"])), Some(25000));
        assert_eq!(parse_number(&lines(&["x1"])), Some(1));
        assert_eq!(parse_number(&lines(&["余58"])), Some(58));
        assert_eq!(parse_number(&lines(&["×１２"])), Some(12));
        assert_eq!(parse_number(&lines(&["", "余"])), None);
    }

    #[test]
    fn scores_and_sticks_make_the_total() {
        assert!(adds_up([27500, 23500, 26500, 21500], 1));
        assert!(adds_up([25000; 4], 0));
        // A score read as 500 instead of 25000 (a digit under an effect).
        assert!(!adds_up([25000, 500, 25000, 25000], 0));
    }

    /// Says the same text every time.
    struct Says(Vec<String>);

    impl ReadText for Says {
        fn read_text(&mut self, _: &RgbImage) -> Result<Vec<String>, TextError> {
            Ok(self.0.clone())
        }
    }

    /// A number drawn in `colour` on a dark box: each digit a bar with a hole whose place
    /// tells it, plus a glow line under it when asked.
    fn number(digits: &[u8], colour: Rgb<u8>, shift: u32, glow: bool) -> RgbImage {
        let mut image = RgbImage::from_pixel(40 + 30 * digits.len() as u32, 72, Rgb([40, 40, 50]));
        for (i, &d) in digits.iter().enumerate() {
            let x0 = 8 + shift + 30 * i as u32;
            for y in 8..56 {
                for x in x0..x0 + 16 {
                    let hole = (8 + 4 * u32::from(d)..12 + 4 * u32::from(d)).contains(&y)
                        && (x0 + 4..x0 + 12).contains(&x);
                    if !hole {
                        image.put_pixel(x, y, colour);
                    }
                }
            }
        }
        if glow {
            for x in 0..image.width() {
                image.put_pixel(x, 66, Rgb([250, 220, 80]));
            }
        }
        image
    }

    const CYAN: Rgb<u8> = Rgb([90, 190, 200]);
    const ORANGE: Rgb<u8> = Rgb([230, 170, 80]);

    #[test]
    fn digits_are_learnt_once_and_then_matched() {
        let mut reader = NumberReader::new(Some(Box::new(Says(lines(&["58"])))));
        assert_eq!(
            reader.read_digits(Field::TilesLeft, &number(&[5, 8], CYAN, 0, false)),
            Some(58)
        );
        assert_eq!(reader.recognized(), 1);
        // 85 and 55 are made of known glyphs: no recognition needed.
        assert_eq!(
            reader.read_digits(Field::TilesLeft, &number(&[8, 5], CYAN, 0, false)),
            Some(85)
        );
        assert_eq!(
            reader.read_digits(Field::TilesLeft, &number(&[5, 5], CYAN, 0, false)),
            Some(55)
        );
        assert_eq!(reader.recognized(), 1);
    }

    #[test]
    fn scores_survive_a_shift_and_a_glow() {
        let mut reader = NumberReader::new(Some(Box::new(Says(lines(&["25000"])))));
        let score = [2, 5, 0, 0, 0];
        assert_eq!(
            reader.read_digits(Field::Score(Seat::Own), &number(&score, ORANGE, 0, false)),
            Some(25000)
        );
        // The same score a few pixels over, with the turn glow under it, in another seat.
        assert_eq!(
            reader.read_digits(Field::Score(Seat::Across), &number(&score, ORANGE, 3, true)),
            Some(25000)
        );
        assert_eq!(reader.recognized(), 1);
    }

    #[test]
    fn counters_are_remembered_whole() {
        let picture = RgbImage::from_fn(150, 60, |x, _| Rgb([(x * 7 % 251) as u8; 3]));
        let mut reader = NumberReader::new(Some(Box::new(Says(lines(&["x1"])))));
        assert_eq!(reader.read_whole(Field::Honba, &picture), Some(1));
        assert_eq!(reader.read_whole(Field::Honba, &picture), Some(1));
        assert_eq!(reader.recognized_by(Field::Honba), 1);
        reader.read_whole(Field::RiichiSticks, &picture);
        assert_eq!(reader.recognized(), 2);
        reader.clear();
        reader.read_whole(Field::Honba, &picture);
        assert_eq!(reader.recognized(), 3);
    }
}
