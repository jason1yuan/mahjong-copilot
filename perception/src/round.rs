//! The round and the seat winds (#9), from the box in the middle of the table.
//!
//! - The seat winds need no text reading. The dealer's wind marker is the red 東, so the
//!   corner that is red tells who deals, and every seat's wind follows in turn order.
//! - The round label (東1局) is read with the system's text recognition (`text`). It
//!   changes once a round, so each look of it is read once and kept as a small thumbnail
//!   with its answer; later frames are matched by pixels, as the tile template cache does.
//!   This is a cache, not training, and nothing is written to disk.

use std::fmt;

use image::{GrayImage, RgbImage, imageops};

use crate::capture::FrameView;
use crate::profile::CentreLayout;
use crate::table::{Seat, Table};
use crate::text::ReadText;

/// A wind, in turn order from East.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Wind {
    East,
    South,
    West,
    North,
}

impl Wind {
    pub const ALL: [Wind; 4] = [Wind::East, Wind::South, Wind::West, Wind::North];

    pub fn index(self) -> usize {
        self as usize
    }

    /// The wind of a character as the game writes it, simplified forms included.
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            '東' | '东' => Some(Wind::East),
            '南' => Some(Wind::South),
            '西' => Some(Wind::West),
            '北' => Some(Wind::North),
            _ => None,
        }
    }
}

/// mjai's names: `E`, `S`, `W`, `N`.
impl fmt::Display for Wind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(["E", "S", "W", "N"][self.index()])
    }
}

/// The round, as the label shows it: 東1局 is `East` 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Round {
    pub wind: Wind,
    /// 1-4.
    pub number: u8,
}

/// `E1` for 東1局.
impl fmt::Display for Round {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.wind, self.number)
    }
}

/// The round in recognized text: a wind character then a digit 1-4, half or full width
/// (東1局, 南 4 局). `None` when no line has one.
pub fn parse_round(lines: &[String]) -> Option<Round> {
    lines.iter().find_map(|line| {
        let chars: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
        chars.windows(2).find_map(|pair| {
            let wind = Wind::from_char(pair[0])?;
            let digit = match pair[1] {
                '１'..='４' => u32::from(pair[1]) - u32::from('１') + 1,
                c => c.to_digit(10)?,
            };
            let number = u8::try_from(digit).ok().filter(|n| (1..=4).contains(n))?;
            Some(Round { wind, number })
        })
    })
}

/// `seat`'s wind when `dealer` deals: the dealer is East, the next seat South, and so on.
pub fn seat_wind(dealer: Seat, seat: Seat) -> Wind {
    Wind::ALL[(seat.index() + 4 - dealer.index()) % 4]
}

/// Pixels per tile width the markers are sampled at; they are about 0.75 tile widths wide.
const MARKER_SCALE: f64 = 16.0;
/// A marker is the dealer's when at least this share of it is red...
const MIN_RED: f32 = 0.15;
/// ...and every other marker less than this.
const MAX_OTHER_RED: f32 = 0.05;

/// Whether a pixel is the dealer marker's red.
fn is_red([r, g, b]: [u8; 3]) -> bool {
    r > 150 && g < 90 && b < 90
}

/// The share of each seat's wind marker that is red, in seat order.
pub fn marker_red(frame: &FrameView<'_>, table: &Table, layout: &CentreLayout) -> [f32; 4] {
    layout.markers.map(|[x, y]| {
        let half = layout.marker / 2.0;
        let image = table.rectify(
            frame,
            Seat::Own,
            [x - half, y - half],
            [layout.marker, layout.marker],
            MARKER_SCALE,
        );
        let red = image.pixels().filter(|p| is_red(p.0)).count();
        red as f32 / (image.width() * image.height()).max(1) as f32
    })
}

/// The seat whose marker is red, from [`marker_red`]; `None` unless exactly one clearly is
/// (the box is hidden on a results screen or under a cut-in).
pub fn dealer_of(red: [f32; 4]) -> Option<Seat> {
    let mut seats = Seat::ALL.into_iter().zip(red);
    let (dealer, _) = seats.clone().find(|&(_, r)| r >= MIN_RED)?;
    seats
        .all(|(seat, r)| seat == dealer || r < MAX_OTHER_RED)
        .then_some(dealer)
}

/// The dealer, from the red marker; see [`dealer_of`].
pub fn dealer(frame: &FrameView<'_>, table: &Table, layout: &CentreLayout) -> Option<Seat> {
    dealer_of(marker_red(frame, table, layout))
}

/// Pixels per tile width the round label is sampled at: the label is then about 170 by 90
/// pixels, the same whatever the window size, which suits text recognition.
pub const LABEL_SCALE: f64 = 96.0;

/// The round label, upright and the same size in any window.
pub fn rectify_label(frame: &FrameView<'_>, table: &Table, layout: &CentreLayout) -> RgbImage {
    let [left, top, right, bottom] = layout.label;
    table.rectify(
        frame,
        Seat::Own,
        [left, top],
        [right - left, bottom - top],
        LABEL_SCALE,
    )
}

/// Thumbnail size for matching labels, and the grid it is compared in.
const THUMB: (u32, u32) = (48, 24);
const CELLS: (u32, u32) = (8, 4);
/// Two labels are the same when no cell of their thumbnails differs by more than this
/// (mean absolute difference, 0-255). Calibrated with `examples/eval_table.rs` on 205
/// recorded labels: the nearest earlier label of the same round is 1.0 away at the median
/// and 8.6 at p95; one of another round (東1 against 東2) never closer than 21.1.
pub const MAX_DISTANCE: f32 = 12.0;
/// Labels remembered; a game has at most a few dozen rounds and repeats.
const MAX_SEEN: usize = 32;

/// The thumbnail a label is matched by.
pub fn thumbnail(label: &RgbImage) -> GrayImage {
    let gray = imageops::grayscale(label);
    imageops::resize(&gray, THUMB.0, THUMB.1, imageops::FilterType::Triangle)
}

/// How far apart two label thumbnails are: the largest mean absolute difference over the
/// cells of a grid, so one changed digit counts in full.
pub fn distance(a: &GrayImage, b: &GrayImage) -> f32 {
    let (cw, ch) = (THUMB.0 / CELLS.0, THUMB.1 / CELLS.1);
    let mut worst = 0.0f32;
    for cy in 0..CELLS.1 {
        for cx in 0..CELLS.0 {
            let mut sum = 0u32;
            for y in cy * ch..(cy + 1) * ch {
                for x in cx * cw..(cx + 1) * cw {
                    sum += u32::from(a.get_pixel(x, y)[0].abs_diff(b.get_pixel(x, y)[0]));
                }
            }
            worst = worst.max(sum as f32 / (cw * ch) as f32);
        }
    }
    worst
}

struct Seen {
    thumb: GrayImage,
    round: Option<Round>,
}

/// Reads round labels, each look of a label only once.
pub struct RoundReader {
    text: Option<Box<dyn ReadText>>,
    seen: Vec<Seen>,
    recognized: usize,
}

impl RoundReader {
    /// Reads with `text`; without it (no text recognition on this platform) no round is
    /// read.
    pub fn new(text: Option<Box<dyn ReadText>>) -> Self {
        Self {
            text,
            seen: Vec::new(),
            recognized: 0,
        }
    }

    /// Forgets every label, e.g. when the window size changes.
    pub fn clear(&mut self) {
        self.seen.clear();
    }

    /// How many labels went to text recognition so far: the rest were matched.
    pub fn recognized(&self) -> usize {
        self.recognized
    }

    /// The round on a label from [`rectify_label`]; `None` when it does not read as one
    /// (covered, or no text recognition). A failed recognition is logged and not
    /// remembered, so it is tried again.
    pub fn read(&mut self, label: &RgbImage) -> Option<Round> {
        let thumb = thumbnail(label);
        if let Some(seen) = self
            .seen
            .iter()
            .map(|s| (s, distance(&s.thumb, &thumb)))
            .filter(|&(_, d)| d <= MAX_DISTANCE)
            .min_by(|a, b| a.1.total_cmp(&b.1))
        {
            return seen.0.round;
        }
        let text = self.text.as_mut()?;
        self.recognized += 1;
        let lines = match text.read_text(label) {
            Ok(lines) => lines,
            Err(err) => {
                // At debug: the helper warns once for a run of failures (#49).
                tracing::debug!("reading the round label: {err}");
                return None;
            }
        };
        let round = parse_round(&lines);
        tracing::debug!(?lines, ?round, "read the round label");
        if self.seen.len() == MAX_SEEN {
            self.seen.remove(0);
        }
        self.seen.push(Seen { thumb, round });
        round
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
    fn parses_round_labels() {
        let round = |text: &[&str]| parse_round(&lines(text)).map(|r| r.to_string());
        assert_eq!(round(&["東1局"]).as_deref(), Some("E1"));
        assert_eq!(round(&["余29", "南 4 局"]).as_deref(), Some("S4"));
        assert_eq!(round(&["西２局"]).as_deref(), Some("W2"));
        assert_eq!(round(&["东3局"]).as_deref(), Some("E3"));
        assert_eq!(round(&["北5局"]), None);
        assert_eq!(round(&["25000", "余68"]), None);
        assert_eq!(round(&[]), None);
    }

    #[test]
    fn seat_winds_follow_the_dealer() {
        assert_eq!(seat_wind(Seat::Own, Seat::Own), Wind::East);
        assert_eq!(seat_wind(Seat::Own, Seat::Right), Wind::South);
        // The dealer across: the seat on the left plays next and is South, the player West.
        assert_eq!(seat_wind(Seat::Across, Seat::Left), Wind::South);
        assert_eq!(seat_wind(Seat::Across, Seat::Own), Wind::West);
        assert_eq!(seat_wind(Seat::Across, Seat::Right), Wind::North);
        assert_eq!(seat_wind(Seat::Left, Seat::Own), Wind::South);
        assert_eq!(seat_wind(Seat::Right, Seat::Own), Wind::North);
    }

    #[test]
    fn the_dealer_is_the_one_red_marker() {
        assert_eq!(dealer_of([0.0, 0.0, 0.3, 0.0]), Some(Seat::Across));
        assert_eq!(dealer_of([0.0, 0.01, 0.0, 0.2]), Some(Seat::Left));
        assert_eq!(dealer_of([0.0; 4]), None, "no box on screen");
        assert_eq!(dealer_of([0.3, 0.0, 0.3, 0.0]), None, "two red markers");
        assert_eq!(
            dealer_of([0.3, 0.1, 0.0, 0.0]),
            None,
            "a reddish one besides"
        );
    }

    /// Counts calls and answers with fixed text.
    struct Fake {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        answer: Vec<String>,
    }

    impl ReadText for Fake {
        fn read_text(&mut self, _: &RgbImage) -> Result<Vec<String>, TextError> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(self.answer.clone())
        }
    }

    /// A label-sized picture with a light block at `x`, standing in for a digit.
    fn label(x: u32) -> RgbImage {
        let mut image = RgbImage::from_pixel(172, 91, Rgb([40, 40, 50]));
        for y in 30..60 {
            for dx in 0..20 {
                image.put_pixel(x + dx, y, Rgb([120, 200, 210]));
            }
        }
        image
    }

    #[test]
    fn reads_each_label_once() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = RoundReader::new(Some(Box::new(Fake {
            calls: calls.clone(),
            answer: lines(&["東1局"]),
        })));
        let first = reader.read(&label(70));
        assert_eq!(first.map(|r| r.to_string()).as_deref(), Some("E1"));
        assert_eq!(reader.read(&label(70)), first);
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        // Another label is read anew.
        reader.read(&label(110));
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
        assert_eq!(reader.recognized(), 2);
        reader.clear();
        reader.read(&label(70));
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 3);
    }

    #[test]
    fn without_text_recognition_no_round_is_read() {
        assert_eq!(RoundReader::new(None).read(&label(70)), None);
    }
}
