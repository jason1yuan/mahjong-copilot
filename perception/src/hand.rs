//! Finds the player's hand in a frame, using the tiles themselves as anchors (#3); no
//! reference image of the game is needed.
//!
//! 1. Tile faces are light and grey ([`TileFace`]; darker if the whole picture is), so the
//!    hand shows up as a band of light rows. Bands are tried from the bottom up and the
//!    first one that splits into tiles is the hand, so browser toolbars, side margins and
//!    the empty space around the game in a tall window need no special handling. Rows light
//!    all the way across are bars (a download shelf), not tiles.
//! 2. Columns on a tile join into groups; the widest is the concealed hand. Its edges are
//!    pulled in to where tile margins are clearly light, so text or icons beside the hand
//!    do not join it.
//! 3. The number of tiles is the one whose dividing lines best fall on the dark seams
//!    between tiles, allowing the grid to shift a little (a dark bevel on one side of the
//!    row, a light tile side on the other). A single tile less than a tile away to the
//!    right is the drawn tile; groups further right are melds (#8).
//! 4. Each tile's top is found on its own, scanning up through the face to the dark rim
//!    above it, so a hovered (raised) tile keeps its box and a light background above the
//!    tiles is not mistaken for a face.
//! 5. The game area follows from the hand ([`HandGeometry`]), for the other regions (#8).
//!
//! Measured on the annotated screenshots (`examples/eval_hand.rs`): all 10 real hands, and
//! 96 % of 130 simulated hands at window widths of 960 px or more.

use image::RgbImage;

use crate::capture::FrameView;
use crate::faces;
use crate::profile::{HandGeometry, TileFace};

/// A tile's box in frame pixels; `right` and `bottom` are exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileBox {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl TileBox {
    pub fn width(&self) -> u32 {
        self.right - self.left
    }

    pub fn height(&self) -> u32 {
        self.bottom - self.top
    }

    /// The tile's pixels, for recognition.
    pub fn crop(&self, frame: &FrameView<'_>) -> RgbImage {
        frame
            .sub(self.left, self.top, self.width(), self.height())
            .to_rgb()
    }
}

/// The area the game draws its table in, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GameArea {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

/// The hand as found in one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct HandLayout {
    /// The concealed tiles, left to right.
    pub concealed: Vec<TileBox>,
    /// The tile just drawn, set a little apart on the right.
    pub drawn: Option<TileBox>,
    /// Tile pitch in pixels.
    pub pitch: f32,
    /// Rows of the band of resting tiles.
    pub band: (u32, u32),
}

impl HandLayout {
    /// The concealed tiles, then the drawn tile if there is one.
    pub fn tiles(&self) -> Vec<TileBox> {
        let mut tiles = self.concealed.clone();
        tiles.extend(self.drawn);
        tiles
    }

    /// The game area implied by the hand's size and position.
    pub fn game_area(&self, geometry: &HandGeometry) -> Option<GameArea> {
        let first = self.concealed.first()?;
        let width = self.pitch / geometry.pitch;
        let height = width / geometry.aspect;
        Some(GameArea {
            left: first.left as f32 - geometry.left * width,
            top: self.band.0 as f32 - geometry.top * height,
            width,
            height,
        })
    }
}

/// `[start, end)` runs of `true`, joining runs at most `max_gap` apart, at least `min_len`
/// long.
fn runs(flags: impl Iterator<Item = bool>, min_len: usize, max_gap: usize) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (i, flag) in flags.enumerate() {
        if !flag {
            continue;
        }
        match out.last_mut() {
            Some(last) if i - last.1 <= max_gap => last.1 = i + 1,
            _ => out.push((i, i + 1)),
        }
    }
    out.retain(|(a, b)| b - a >= min_len);
    out
}

/// Tile pitch relative to the band height: tiles are about 0.66 as wide as they are tall.
const MIN_ASPECT: f32 = 0.5;
const MAX_ASPECT: f32 = 0.85;
/// The concealed hand has at least this many tiles (a hand with four melds keeps one or
/// two, but then the melds carry the information; #8).
const MIN_TILES: usize = 3;
/// How much darker than average the seams must be where the tiles are divided.
const MIN_SEAM_SCORE: f32 = 2.0;
/// Below this share of face pixels, a slot at the end of the row is not a tile.
const MIN_TILENESS: f32 = 0.35;

/// The tile size found earlier for this frame size: later frames are read with it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Expect {
    pub pitch: f32,
    pub height: u32,
}

impl Expect {
    fn of(layout: &HandLayout) -> Self {
        Self {
            pitch: layout.pitch,
            height: layout.band.1 - layout.band.0,
        }
    }

    /// Same tile size, within the 4 % rounding and resampling leave.
    fn matches(&self, other: &Expect) -> bool {
        (self.pitch - other.pitch).abs() < 0.04 * self.pitch
    }
}

/// A new tile size seen this many times in a row replaces the locked one.
const RELOCK: u32 = 3;
/// A hand never holds more: 13 concealed tiles and the one drawn. A longer row is something
/// else (a results screen, a wall), or a wrong lock cutting the hand too fine.
const MAX_TILES: usize = 14;

fn plausible(layout: &HandLayout) -> bool {
    layout.concealed.len() + usize::from(layout.drawn.is_some()) <= MAX_TILES
}

/// Finds the hand frame after frame. Once found for a frame size, the tile size is locked,
/// so a hand partly covered (a popup, a dimmed table) still splits into the right tiles
/// instead of being read with a wrong tile size. A changed frame size starts over; a
/// different tile size seen [`RELOCK`] times in a row (the game rescaled) replaces it.
#[derive(Default)]
pub struct HandTracker {
    finder: HandFinder,
    lock: Option<((u32, u32), Expect)>,
    candidate: Option<(Expect, u32)>,
}

impl HandTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// The locked tile size, if any.
    pub fn expect(&self) -> Option<Expect> {
        self.lock.map(|(_, e)| e)
    }

    /// Drops the locked tile size, e.g. when what was found turned out not to be a hand.
    pub fn forget(&mut self) {
        self.lock = None;
        self.candidate = None;
    }

    pub fn find(&mut self, frame: &FrameView<'_>, face: TileFace) -> Option<HandLayout> {
        let size = (frame.width(), frame.height());
        let Some((locked_size, expect)) = self.lock.filter(|(s, _)| *s == size) else {
            let layout = self.finder.find(frame, face).filter(plausible)?;
            self.lock = Some((size, Expect::of(&layout)));
            self.candidate = None;
            return Some(layout);
        };
        match self.finder.find_expecting(frame, face, Some(expect)) {
            Some(layout) if plausible(&layout) => {
                self.candidate = None;
                return Some(layout);
            }
            Some(_) => {
                // The locked size cuts the hand too fine: it was wrong. Start over.
                tracing::debug!("dropping a hand lock that reads too many tiles");
                self.lock = None;
                self.candidate = None;
                return self.find(frame, face);
            }
            None => {}
        }
        // Not there at the locked size: maybe the game rescaled. Believe a new size only
        // once it repeats.
        let layout = self.finder.find(frame, face).filter(plausible)?;
        let seen = Expect::of(&layout);
        let count = match self.candidate {
            Some((c, n)) if c.matches(&seen) => n + 1,
            _ => 1,
        };
        if count >= RELOCK {
            self.lock = Some((locked_size, seen));
            self.candidate = None;
            return Some(layout);
        }
        self.candidate = Some((seen, count));
        None
    }
}

/// Reusable scratch space, so finding the hand again does not allocate frame-sized buffers.
#[derive(Default)]
pub struct HandFinder {
    width: usize,
    height: usize,
    mask: Vec<bool>,
    mean: Vec<f32>,
    expect: Option<Expect>,
}

impl HandFinder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn find(&mut self, frame: &FrameView<'_>, face: TileFace) -> Option<HandLayout> {
        self.find_expecting(frame, face, None)
    }

    /// Like [`find`](Self::find), but with tiles of a known size: the pitch must be within
    /// 4 % of `expect.pitch`, and boxes are `expect.height` tall.
    pub fn find_expecting(
        &mut self,
        frame: &FrameView<'_>,
        face: TileFace,
        expect: Option<Expect>,
    ) -> Option<HandLayout> {
        self.expect = expect;
        let (w, h) = (frame.width() as usize, frame.height() as usize);
        if w < 16 || h < 16 {
            return None;
        }
        self.width = w;
        self.height = h;
        self.build_mask(frame, face);
        let rows: Vec<f32> = (0..h)
            .map(|y| self.mask[y * w..(y + 1) * w].iter().filter(|m| **m).count() as f32 / w as f32)
            .collect();
        let bands = runs(
            rows.iter().map(|d| *d > 0.12 && *d < 0.9),
            (h / 40).max(6),
            (h / 150).max(2),
        );
        bands
            .iter()
            .rev()
            .find_map(|&(y0, y1)| self.segment(y0, y1))
    }

    fn at(&self, x: usize, y: usize) -> bool {
        self.mask[y * self.width + x]
    }

    /// Light grey or tinted pixels. The brightness bar follows the frame's brightest greys when the
    /// picture is darker than usual, but is never stricter than the profile's.
    fn build_mask(&mut self, frame: &FrameView<'_>, face: TileFace) {
        let (w, h) = (self.width, self.height);
        self.mean.clear();
        self.mask.clear();
        let mut histogram = [0u32; 256];
        let mut greys = 0u32;
        let mut grey = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let [r, g, b] = frame.rgb(x as u32, y as u32);
                let mean = (f32::from(r) + f32::from(g) + f32::from(b)) / 3.0;
                let spread = r.max(g).max(b) - r.min(g).min(b);
                let is_grey = spread < face.max_spread;
                if is_grey {
                    histogram[mean as usize] += 1;
                    greys += 1;
                }
                grey.push(is_grey || faces::is_tinted(r, g, b, face));
                self.mean.push(mean);
            }
        }
        let light = if greys == 0 {
            255.0
        } else {
            let mut seen = 0u32;
            let target = (f64::from(greys) * 0.995) as u32;
            histogram
                .iter()
                .position(|count| {
                    seen += count;
                    seen > target
                })
                .unwrap_or(255) as f32
        };
        let threshold = f32::from(face.min_brightness).min((light - 70.0).max(110.0));
        self.mask.extend(
            self.mean
                .iter()
                .zip(&grey)
                .map(|(m, g)| *g && *m > threshold),
        );
    }

    /// Share of face pixels in each column over rows `y0..y1`.
    fn column_density(&self, y0: usize, y1: usize) -> Vec<f32> {
        let n = (y1 - y0).max(1) as f32;
        (0..self.width)
            .map(|x| (y0..y1).filter(|&y| self.at(x, y)).count() as f32 / n)
            .collect()
    }

    /// The concealed hand and drawn tile in the band `y0..y1`, if it is a row of tiles.
    fn segment(&self, y0: usize, y1: usize) -> Option<HandLayout> {
        let (w, h) = (self.width, self.height);
        let band = y1 - y0;
        let margin = (band / 10).max(2);
        let full = self.column_density(y0.saturating_sub(band / 4), y1);
        let top = self.column_density(y0 + band / 30, (y0 + band / 30 + margin).min(h));
        let bottom = self.column_density(y1.saturating_sub(band / 30 + margin), y1 - band / 30);
        // On a tile: face light overall (reaching up for a raised tile), or at its plain
        // top or bottom margin.
        let on_tile: Vec<bool> = (0..w)
            .map(|x| full[x] > 0.3 || top[x] > 0.5 || bottom[x] > 0.5)
            .collect();
        let mut groups = runs(
            on_tile.iter().copied(),
            (band / 3).max(4),
            (band / 12).max(2),
        );
        // Plain light strips (window borders) have no symbols.
        groups.retain(|&(g0, g1)| self.contrast(g0, g1, y0 + band / 10, y1 - band / 10) > 12.0);
        // The concealed hand is the leftmost row of tiles: the melds to its right can be
        // longer once there are a few of them.
        let widest = *groups
            .iter()
            .find(|(a, b)| ((b - a) as f32) >= 2.5 * MIN_ASPECT * band as f32)?;

        let seams = self.seam_profile(y0, y1);
        // Clearly tile: a run of these marks a real tile edge.
        let strong: Vec<bool> = (0..w)
            .map(|x| top[x] > 0.5 || bottom[x] > 0.5 || full[x] > 0.5)
            .collect();
        let run = (band / 6).max(3);
        let solid = |x: usize| x + run <= w && strong[x..x + run].iter().all(|s| *s);
        let solid_before = |x: usize| x >= run && x <= w && strong[x - run..x].iter().all(|s| *s);
        let mut g0 = (widest.0..widest.1.saturating_sub(run)).find(|&x| solid(x))?;
        let mut g1 = (widest.0 + run + 1..=widest.1)
            .rev()
            .find(|&x| solid_before(x))?;
        let (mut n, mut shift);
        let mut rounds = 0;
        loop {
            let (count, score, grid_shift) = tile_count(&seams, g0, g1, band, self.expect)?;
            if count < MIN_TILES || score < MIN_SEAM_SCORE {
                return None;
            }
            (n, shift) = (count, grid_shift);
            rounds += 1;
            if rounds == 3 {
                break;
            }
            // Drop an end slot that is not a tile, moving to the next real tile edge.
            let step = (g1 - g0) as f32 / n as f32;
            let reach = (1.5 * step) as usize;
            let mut moved = false;
            if self.tileness(g0, g0 + step as usize, y0, band) < MIN_TILENESS
                && let Some(x) = (g0 + 1..(g0 + reach).min(w)).find(|&x| solid(x))
            {
                (g0, moved) = (x, true);
            }
            if self.tileness(g1.saturating_sub(step as usize), g1, y0, band) < MIN_TILENESS
                && let Some(x) = (g1.saturating_sub(reach)..g1)
                    .rev()
                    .find(|&x| solid_before(x))
            {
                (g1, moved) = (x, true);
            }
            if !moved {
                break;
            }
        }
        let pitch = (g1 - g0) as f32 / n as f32;
        let start = g0 as f32 + shift;
        let slot = |k: usize| {
            let left = (start + k as f32 * pitch).round().max(0.0) as usize;
            let right = ((start + (k + 1) as f32 * pitch).round() as usize).min(w);
            self.tile_box(left, right, y0, band)
        };
        let concealed: Vec<TileBox> = (0..n).map(slot).collect();
        let end = (start + n as f32 * pitch).round() as usize;
        let drawn = groups
            .iter()
            .find(|g| g.0 >= end)
            .filter(|g| {
                ((g.0 - end) as f32) < pitch && (((g.1 - g.0) as f32 / pitch).round() as usize) == 1
            })
            .map(|&(a, b)| self.tile_box(a, b, y0, band));
        Some(HandLayout {
            concealed,
            drawn,
            pitch,
            band: (y0 as u32, y1 as u32),
        })
    }

    /// Standard deviation of the mean brightness over a rectangle.
    fn contrast(&self, x0: usize, x1: usize, y0: usize, y1: usize) -> f32 {
        let values = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y)));
        let (mut n, mut sum, mut sq) = (0f32, 0f32, 0f32);
        for (x, y) in values {
            let v = self.mean[y * self.width + x];
            n += 1.0;
            sum += v;
            sq += v * v;
        }
        if n == 0.0 {
            return 0.0;
        }
        (sq / n - (sum / n).powi(2)).max(0.0).sqrt()
    }

    /// Per column, how much darker it is than the columns two pixels either side, in the
    /// plain top and bottom margins of the faces, where the seams between tiles stand out.
    fn seam_profile(&self, y0: usize, y1: usize) -> Vec<f32> {
        let (w, h) = (self.width, self.height);
        let band = y1 - y0;
        let margin = (band / 10).max(2);
        let rows: Vec<usize> = (y0 + band / 30..(y0 + band / 30 + margin).min(h))
            .chain(y1.saturating_sub(band / 30 + margin)..y1 - band / 30)
            .collect();
        (0..w)
            .map(|x| {
                let (l, r) = ((x + w - 2) % w, (x + 2) % w);
                let total: f32 = rows
                    .iter()
                    .map(|&y| {
                        let m = |x: usize| self.mean[y * w + x];
                        ((m(l) + m(r)) / 2.0 - m(x)).max(0.0)
                    })
                    .sum();
                total / rows.len().max(1) as f32
            })
            .collect()
    }

    /// Share of face pixels in the middle half of a slot, from its own top down a tile.
    fn tileness(&self, x0: usize, x1: usize, y0: usize, band: usize) -> f32 {
        let top = self.tile_top(x0, x1, y0, band);
        let (ix0, ix1) = (x0 + (x1 - x0) / 4, x1 - (x1 - x0) / 4);
        let rows = top..(top + band).min(self.height);
        let cells = (ix1 - ix0) * rows.len();
        if cells == 0 {
            return 0.0;
        }
        let light = rows
            .flat_map(|y| (ix0..ix1).map(move |x| (x, y)))
            .filter(|&(x, y)| self.at(x, y))
            .count();
        light as f32 / cells as f32
    }

    /// Top of a tile's face: scanning up from just inside the band, a face row is at least
    /// a quarter light even across symbols, while the rim above the face and the table are
    /// dark across the whole width; two dark rows end the scan.
    fn tile_top(&self, x0: usize, x1: usize, y0: usize, band: usize) -> usize {
        let (ix0, ix1) = (x0 + (x1 - x0) / 6, x1 - (x1 - x0) / 6);
        let light = |y: usize| {
            let n = ix1.saturating_sub(ix0);
            n > 0 && (ix0..ix1).filter(|&x| self.at(x, y)).count() as f32 > 0.25 * n as f32
        };
        let mut y = (y0 + band / 10).min(self.height - 1);
        let mut top = y;
        let mut dark = 0;
        let floor = y0.saturating_sub(band / 2);
        while y > floor {
            y -= 1;
            if light(y) {
                (top, dark) = (y, 0);
            } else {
                dark += 1;
                if dark >= 2 {
                    break;
                }
            }
        }
        top
    }

    /// Bottom of a tile's face: scanning up from below the band to the first light row, the
    /// face's plain bottom margin. `None` when the row below the band is light too, so the
    /// bottom is not in sight.
    fn tile_bottom(&self, x0: usize, x1: usize, y1: usize, band: usize) -> Option<usize> {
        let (ix0, ix1) = (x0 + (x1 - x0) / 6, x1 - (x1 - x0) / 6);
        let light = |y: usize| {
            let n = ix1.saturating_sub(ix0);
            n > 0 && (ix0..ix1).filter(|&x| self.at(x, y)).count() as f32 > 0.25 * n as f32
        };
        let start = (y1 + band / 10).min(self.height - 1);
        if light(start) {
            return None;
        }
        (y1.saturating_sub(band / 2)..start)
            .rev()
            .find(|&y| light(y))
            .map(|y| y + 1)
    }

    fn tile_box(&self, left: usize, right: usize, y0: usize, band: usize) -> TileBox {
        let height = self.expect.map_or(band, |e| e.height as usize);
        // A hovered tile rises by up to a third of its height. Scanning up from inside the
        // band then starts among its symbols, where a level stroke (一, the top of 伍) looks
        // like the dark edge above the face; its bottom rises with it, below a plain margin.
        let scanned = self.tile_top(left, right, y0, band);
        let top = match self
            .tile_bottom(left, right, y0 + band, band)
            .map(|bottom| bottom.saturating_sub(height))
        {
            Some(raised) if raised + band / 12 < scanned => raised,
            _ => scanned,
        };
        TileBox {
            left: left as u32,
            top: top as u32,
            right: right as u32,
            bottom: (top + height).min(self.height) as u32,
        }
    }
}

/// The number of tiles in `g0..g1` whose dividing lines best fall on the seams, how
/// clearly (seam darkness over the average), and the shift of the grid in pixels. Tiles
/// are `MIN_ASPECT..MAX_ASPECT` as wide as the band is tall, or within 4 % of an expected
/// pitch.
fn tile_count(
    seams: &[f32],
    g0: usize,
    g1: usize,
    band: usize,
    expect: Option<Expect>,
) -> Option<(usize, f32, f32)> {
    let width = (g1 - g0) as f32;
    let background = seams[g0..g1].iter().sum::<f32>() / width + 1e-3;
    let band = band as f32;
    let (lo, hi) = expect.map_or((MIN_ASPECT * band, MAX_ASPECT * band), |e| {
        (0.96 * e.pitch, 1.04 * e.pitch)
    });
    let mut best: Option<(usize, f32, f32)> = None;
    for n in 1..=(width / lo.max(1.0)) as usize + 1 {
        let step = width / n as f32;
        if !(lo..=hi).contains(&step) {
            continue;
        }
        if n == 1 {
            if best.is_none_or(|b| MIN_SEAM_SCORE > b.1) {
                best = Some((1, MIN_SEAM_SCORE, 0.0));
            }
            continue;
        }
        let mut shift = -0.4 * step;
        while shift <= 0.4 * step + 1.0 {
            let lines: Vec<usize> = (1..n)
                .map(|k| (g0 as f32 + shift + k as f32 * step).round() as usize)
                .filter(|&x| x >= 2 && x + 3 <= seams.len())
                .collect();
            if lines.len() + 2 >= n {
                let darkness: f32 = lines
                    .iter()
                    .map(|&x| seams[x - 2..x + 3].iter().copied().fold(0.0, f32::max))
                    .sum::<f32>()
                    / lines.len() as f32;
                // A shifted grid must earn its shift: prefer no shift on a tie.
                let score = darkness / background - shift.abs() / step * 0.2;
                if best.is_none_or(|b| score > b.1) {
                    best = Some((n, score, shift));
                }
            }
            shift += (step / 40.0).max(1.0);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::tests::bgra;

    const FACE: TileFace = TileFace {
        min_brightness: 175,
        max_spread: 45,
    };
    const TABLE: [u8; 3] = [30, 60, 120];
    const RIM: [u8; 3] = [200, 120, 40];
    const TILE: [u8; 3] = [235, 235, 228];
    const INK: [u8; 3] = [40, 40, 40];

    /// A frame with a dark table, an optional white toolbar on top, and tiles `w` x `h` at
    /// the given left edges and tops: an orange rim on top, a dark symbol in the middle and
    /// a 2 px seam on the right.
    fn table(
        width: u32,
        height: u32,
        toolbar: u32,
        tiles: &[(u32, u32)],
        w: u32,
        h: u32,
    ) -> Vec<u8> {
        bgra(width, height, |x, y| {
            if y < toolbar {
                return [245, 245, 245];
            }
            for &(left, top) in tiles {
                if (left..left + w).contains(&x) && (top.saturating_sub(4)..top + h).contains(&y) {
                    if y < top {
                        return RIM;
                    }
                    if x >= left + w - 2 {
                        return [150, 150, 150];
                    }
                    let symbol = (left + w / 4..left + 3 * w / 4).contains(&x)
                        && (top + h / 4..top + 3 * h / 4).contains(&y)
                        && (y / 3 + x / 5) % 2 == 0;
                    return if symbol { INK } else { TILE };
                }
            }
            TABLE
        })
    }

    fn row(start: u32, count: u32, pitch: u32, top: u32) -> Vec<(u32, u32)> {
        (0..count).map(|i| (start + i * pitch, top)).collect()
    }

    fn find(data: &[u8], width: u32, height: u32) -> Option<HandLayout> {
        let frame = FrameView::new(width, height, width as usize * 4, data, 1.0).unwrap();
        HandFinder::new().find(&frame, FACE)
    }

    fn assert_near(found: &[TileBox], want: &[(u32, u32)], h: u32) {
        assert_eq!(found.len(), want.len(), "{found:?}");
        for (f, &(left, top)) in found.iter().zip(want) {
            assert!(
                f.left.abs_diff(left) <= 3 && f.top.abs_diff(top) <= 2,
                "{f:?} vs ({left}, {top})"
            );
            assert!(f.height().abs_diff(h) <= 2, "{f:?}");
        }
    }

    #[test]
    fn finds_thirteen_tiles_and_the_drawn_one() {
        let (w, h) = (40, 60);
        let mut tiles = row(100, 13, w, 500);
        tiles.push((100 + 13 * w + 14, 500));
        let data = table(800, 580, 0, &tiles, w, h);
        let layout = find(&data, 800, 580).unwrap();
        assert_eq!(layout.concealed.len(), 13);
        assert!(layout.drawn.is_some());
        assert!((layout.pitch - 40.0).abs() < 1.0, "pitch {}", layout.pitch);
        assert_near(&layout.tiles(), &tiles, h);
    }

    #[test]
    fn tiles_tinted_pink_are_part_of_the_hand() {
        let (w, h) = (40, 60);
        let tiles = row(100, 13, w, 500);
        let mut data = table(800, 580, 0, &tiles, w, h);
        // The first two are of the kind under the mouse.
        let [r, g, b] = TILE;
        for (i, px) in data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = i as u32 % 800;
            if x < 100 + 2 * w && px[..3] == [b, g, r] {
                px[..3].copy_from_slice(&[163, 166, 209]);
            }
        }
        let layout = find(&data, 800, 580).unwrap();
        assert_near(&layout.tiles(), &tiles, h);
    }

    #[test]
    fn ignores_a_toolbar_and_keeps_a_raised_tile() {
        let (w, h) = (40, 60);
        let mut tiles = row(100, 13, w, 500);
        tiles[5].1 = 488; // hovered: 12 px higher
        let data = table(800, 580, 90, &tiles, w, h);
        let layout = find(&data, 800, 580).unwrap();
        assert_near(&layout.tiles(), &tiles, h);
    }

    #[test]
    fn melds_further_right_are_not_part_of_the_hand() {
        let (w, h) = (40, 60);
        let mut tiles = row(100, 10, w, 500);
        tiles.extend(row(100 + 13 * w, 3, w, 500));
        let data = table(800, 580, 0, &tiles, w, h);
        let layout = find(&data, 800, 580).unwrap();
        assert_eq!(layout.drawn, None);
        assert_near(&layout.tiles(), &tiles[..10], h);
    }

    #[test]
    fn a_short_hand_left_of_longer_melds_is_the_hand() {
        let (w, h) = (40, 60);
        // Three melds leave four tiles in hand; the melds' row is longer.
        let mut tiles = row(60, 4, w, 500);
        tiles.extend(row(300, 10, w, 500));
        let data = table(800, 580, 0, &tiles, w, h);
        let layout = find(&data, 800, 580).unwrap();
        assert_near(&layout.tiles(), &tiles[..4], h);
    }

    #[test]
    fn a_tile_raised_far_is_found_from_its_bottom() {
        let (w, h) = (40, 60);
        let mut tiles = row(100, 13, w, 500);
        // Hovered a third of its height up, with a level stroke where the scan would start.
        tiles[5].1 = 480;
        let mut data = table(800, 580, 0, &tiles, w, h);
        for y in 484..492 {
            for x in 300..336 {
                let i = (y * 800 + x) * 4;
                data[i..i + 3].copy_from_slice(&[40, 40, 40]);
            }
        }
        let layout = find(&data, 800, 580).unwrap();
        assert_near(&layout.tiles(), &tiles, h);
    }

    #[test]
    fn the_game_area_follows_from_the_hand() {
        let layout = HandLayout {
            concealed: vec![TileBox {
                left: 149,
                top: 620,
                right: 212,
                bottom: 716,
            }],
            drawn: None,
            pitch: 63.1,
            band: (620, 716),
        };
        let geometry = HandGeometry {
            aspect: 16.0 / 9.0,
            pitch: 0.0493,
            left: 0.1163,
            top: 0.86,
        };
        let area = layout.game_area(&geometry).unwrap();
        // The 1280x720 screenshot this layout comes from fills the whole frame.
        assert!((area.width - 1280.0).abs() < 5.0, "{area:?}");
        assert!(area.left.abs() < 5.0 && area.top.abs() < 5.0, "{area:?}");
    }

    #[test]
    fn a_locked_tile_size_reads_a_partly_covered_hand() {
        let (w, h) = (40, 60);
        let tiles = row(100, 13, w, 500);
        let clear = table(800, 580, 0, &tiles, w, h);
        // A dark popup covers the upper 40 % of the hand.
        let covered: Vec<u8> = clear
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .flat_map(|(i, px)| {
                let y = i as u32 / 800;
                if (496..524).contains(&y) {
                    [20, 20, 20, 255]
                } else {
                    *px
                }
            })
            .collect();
        fn view(data: &[u8]) -> FrameView<'_> {
            FrameView::new(800, 580, 3200, data, 1.0).unwrap()
        }
        let mut tracker = HandTracker::new();
        assert_eq!(
            tracker.find(&view(&clear), FACE).unwrap().concealed.len(),
            13
        );
        let expect = tracker.expect().unwrap();
        assert!(
            (expect.pitch - 40.0).abs() < 1.0 && expect.height.abs_diff(h) <= 2,
            "{expect:?}"
        );
        let layout = tracker.find(&view(&covered), FACE).unwrap();
        assert_eq!(layout.concealed.len(), 13);
        assert!((layout.pitch - 40.0).abs() < 1.6);
    }

    #[test]
    fn a_row_too_long_for_a_hand_is_not_locked_onto() {
        // A live session began on a screen with a long row of small tiles. Locked to their
        // size, every later hand was cut into 42 tiles and the lock never let go.
        fn view(data: &[u8]) -> FrameView<'_> {
            FrameView::new(800, 580, 3200, data, 1.0).unwrap()
        }
        let small = table(800, 580, 0, &row(60, 20, 20, 500), 20, 30);
        let hand = table(800, 580, 0, &row(100, 13, 40, 500), 40, 60);
        let mut tracker = HandTracker::new();
        assert!(tracker.find(&view(&small), FACE).is_none());
        assert_eq!(
            tracker.find(&view(&hand), FACE).unwrap().concealed.len(),
            13
        );
    }

    #[test]
    fn a_lock_that_cuts_the_hand_too_fine_is_dropped() {
        fn view(data: &[u8]) -> FrameView<'_> {
            FrameView::new(800, 580, 3200, data, 1.0).unwrap()
        }
        let hand = table(800, 580, 0, &row(100, 13, 40, 500), 40, 60);
        let mut tracker = HandTracker::new();
        // Pretend an earlier screen locked a third of the real size.
        tracker.lock = Some((
            (800, 580),
            Expect {
                pitch: 13.3,
                height: 20,
            },
        ));
        let layout = (0..3)
            .find_map(|_| tracker.find(&view(&hand), FACE))
            .unwrap();
        assert_eq!(layout.concealed.len(), 13);
    }

    #[test]
    fn no_hand_on_an_empty_table() {
        let data = table(400, 300, 40, &[], 40, 60);
        assert_eq!(find(&data, 400, 300), None);
    }
}
