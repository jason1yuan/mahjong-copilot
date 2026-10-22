//! Runtime template cache: within one game, tile skin, window size and kind of region (hand,
//! river, ...), the same tile looks almost the same every time. So a crop the classifier
//! recognized for sure is kept as a template, and later crops are matched by pixels
//! instead of running the model. This is a cache, not training: nothing is learned and
//! nothing is written to disk. Drop the cache when the game, skin or window size changes.
//!
//! Crops are compared as small thumbnails, allowing a vertical shift of up to about a fifth
//! of the tile (a hovered tile rises by that much) and a pixel sideways. A match counts only
//! when it is close and clearly closer than every template of any other tile.

use image::RgbImage;
use mahjong_core::Tile;

use crate::pillow;

/// Thumbnail size. Big enough to tell 5p from 6p, small enough to compare in microseconds.
pub const THUMB_WIDTH: u32 = 24;
pub const THUMB_HEIGHT: u32 = 32;
const SHIFT_X: usize = 1;
const SHIFT_Y: usize = 6;

/// A match must be at most this far (mean absolute difference per channel, 0-255)...
///
/// Calibrated with `examples/eval_cache.rs` on the 10 real screenshots: the same tile in
/// one screenshot is 2.8 apart at the median and 10.8 at p95; different tiles are never
/// closer than 13.1, and raised (hovered) tiles never closer than 10.4 to another tile.
pub const MAX_DISTANCE: f32 = 12.0;
/// ...and at least this much closer than the nearest template of any other tile.
pub const MIN_MARGIN: f32 = 8.0;
/// A new crop this close to a template of the same tile adds nothing and is not kept.
const DUPLICATE_DISTANCE: f32 = 3.0;
/// Templates kept per tile (e.g. plain, hovered, dora shine); the oldest goes first.
pub const MAX_PER_TILE: usize = 4;

struct Template {
    tile: Tile,
    confidence: f32,
    thumb: Vec<u8>,
}

/// The closest template, for [`TemplateCache::lookup`] and for calibration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Match {
    pub tile: Tile,
    /// Classifier confidence when the template was stored.
    pub confidence: f32,
    pub distance: f32,
    /// Distance to the nearest template of another tile minus `distance`; infinite when no
    /// other tile has a template yet.
    pub margin: f32,
}

/// Doubtful readings remembered, so the same picture is not sent to the classifier again.
const MAX_DOUBTFUL: usize = 64;

#[derive(Default)]
pub struct TemplateCache {
    templates: Vec<Template>,
    /// Crops read with doubt, most recent last. Never templates: only the same picture
    /// again (a tile that stays on the table under a glare) gets their answer.
    doubtful: Vec<Template>,
}

impl TemplateCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    pub fn clear(&mut self) {
        self.templates.clear();
        self.doubtful.clear();
    }

    /// Remembers what the classifier said, with doubt, about a crop.
    pub fn remember_doubtful(&mut self, tile: Tile, confidence: f32, crop: &RgbImage) {
        if self.doubtful.len() >= MAX_DOUBTFUL {
            self.doubtful.remove(0);
        }
        self.doubtful.push(Template {
            tile,
            confidence,
            thumb: thumbnail(crop),
        });
    }

    /// What was said about this very picture, if it was read with doubt before.
    pub fn recall_doubtful(&self, crop: &RgbImage) -> Option<(Tile, f32)> {
        if self.doubtful.is_empty() {
            return None;
        }
        let thumb = thumbnail(crop);
        self.doubtful
            .iter()
            .rev()
            .find(|t| distance(&t.thumb, &thumb) < DUPLICATE_DISTANCE)
            .map(|t| (t.tile, t.confidence))
    }

    /// The cached tile for a crop, when the match is close and unambiguous.
    pub fn lookup(&self, crop: &RgbImage) -> Option<Match> {
        self.closest(&thumbnail(crop))
            .filter(|m| m.distance <= MAX_DISTANCE && m.margin >= MIN_MARGIN)
    }

    /// The closest template whatever its distance, with its margin.
    pub fn closest_to(&self, crop: &RgbImage) -> Option<Match> {
        self.closest(&thumbnail(crop))
    }

    /// Keeps a crop the classifier recognized for sure.
    pub fn insert(&mut self, tile: Tile, confidence: f32, crop: &RgbImage) {
        let thumb = thumbnail(crop);
        let same: Vec<usize> = (0..self.templates.len())
            .filter(|&i| self.templates[i].tile == tile)
            .collect();
        if same
            .iter()
            .any(|&i| distance(&self.templates[i].thumb, &thumb) < DUPLICATE_DISTANCE)
        {
            return;
        }
        if same.len() >= MAX_PER_TILE {
            self.templates.remove(same[0]);
        }
        self.templates.push(Template {
            tile,
            confidence,
            thumb,
        });
    }

    fn closest(&self, thumb: &[u8]) -> Option<Match> {
        let distances: Vec<f32> = self
            .templates
            .iter()
            .map(|t| distance(&t.thumb, thumb))
            .collect();
        let best = (0..distances.len()).min_by(|&a, &b| distances[a].total_cmp(&distances[b]))?;
        let tile = self.templates[best].tile;
        let other = self
            .templates
            .iter()
            .zip(&distances)
            .filter(|(t, _)| t.tile != tile)
            .map(|(_, d)| *d)
            .fold(f32::INFINITY, f32::min);
        Some(Match {
            tile,
            confidence: self.templates[best].confidence,
            distance: distances[best],
            margin: other - distances[best],
        })
    }
}

/// The distance the cache uses between two crops, for calibrating its thresholds.
pub fn crop_distance(a: &RgbImage, b: &RgbImage) -> f32 {
    distance(&thumbnail(a), &thumbnail(b))
}

/// The crop shrunk to the thumbnail size, row-major RGB.
fn thumbnail(crop: &RgbImage) -> Vec<u8> {
    pillow::resize_bilinear(crop, THUMB_WIDTH, THUMB_HEIGHT).into_raw()
}

/// Smallest mean absolute difference per channel between the inner part of `a` and `b`
/// shifted by up to `SHIFT_X` x `SHIFT_Y` pixels.
fn distance(a: &[u8], b: &[u8]) -> f32 {
    let (w, h) = (THUMB_WIDTH as usize, THUMB_HEIGHT as usize);
    let (xs, ys) = (SHIFT_X..w - SHIFT_X, SHIFT_Y..h - SHIFT_Y);
    let samples = (xs.len() * ys.len() * 3) as f32;
    let mut best = u32::MAX;
    for dy in 0..=2 * SHIFT_Y {
        for dx in 0..=2 * SHIFT_X {
            let mut total = 0u32;
            for y in ys.clone() {
                let row_a = &a[(y * w + xs.start) * 3..(y * w + xs.end) * 3];
                let by = y + dy - SHIFT_Y;
                let bx = xs.start + dx - SHIFT_X;
                let row_b = &b[(by * w + bx) * 3..(by * w + bx + xs.len()) * 3];
                total += row_a
                    .iter()
                    .zip(row_b)
                    .map(|(p, q)| u32::from(p.abs_diff(*q)))
                    .sum::<u32>();
            }
            best = best.min(total);
        }
    }
    best as f32 / samples
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    /// A 40x56 tile: light face with a dark bar across the given rows.
    fn tile(bar: std::ops::Range<u32>) -> RgbImage {
        RgbImage::from_fn(40, 56, |x, y| {
            if bar.contains(&y) && (8..32).contains(&x) {
                Rgb([30, 30, 35])
            } else {
                Rgb([240, 240, 235])
            }
        })
    }

    fn t(name: &str) -> Tile {
        name.parse().unwrap()
    }

    #[test]
    fn finds_a_stored_tile_even_when_raised() {
        let mut cache = TemplateCache::new();
        cache.insert(t("3m"), 0.9, &tile(10..16));
        cache.insert(t("7p"), 0.9, &tile(36..48));
        let hit = cache.lookup(&tile(10..16)).unwrap();
        assert_eq!((hit.tile, hit.distance), (t("3m"), 0.0));
        assert_eq!(hit.confidence, 0.9);
        // A hovered tile drawn 8 px (a seventh of its height) higher.
        let flat = tile(10..16);
        let raised = RgbImage::from_fn(40, 56, |x, y| *flat.get_pixel(x, (y + 8).min(55)));
        assert_eq!(cache.lookup(&raised).unwrap().tile, t("3m"));
    }

    #[test]
    fn misses_unknown_or_ambiguous_tiles() {
        let mut cache = TemplateCache::new();
        assert_eq!(cache.lookup(&tile(10..16)), None);
        cache.insert(t("3m"), 0.9, &tile(10..16));
        assert_eq!(
            cache.lookup(&tile(20..50)),
            None,
            "too far from any template"
        );
        // Two tiles that look alike: neither is clearly closer.
        cache.insert(t("4m"), 0.9, &tile(10..17));
        assert_eq!(cache.lookup(&tile(10..16)), None);
    }

    #[test]
    fn keeps_a_few_distinct_templates_per_tile() {
        let mut cache = TemplateCache::new();
        cache.insert(t("3m"), 0.9, &tile(10..16));
        cache.insert(t("3m"), 0.9, &tile(10..16));
        assert_eq!(cache.len(), 1, "duplicates are not kept");
        // Bars of different lengths; a different height alone would match after shifting.
        for extra in 0..6 {
            let bar = RgbImage::from_fn(40, 56, |x, y| {
                if (20..30).contains(&y) && x < 6 + extra * 6 {
                    Rgb([30, 30, 35])
                } else {
                    Rgb([240, 240, 235])
                }
            });
            cache.insert(t("3m"), 0.9, &bar);
        }
        assert_eq!(cache.len(), MAX_PER_TILE);
        cache.clear();
        assert!(cache.is_empty());
    }
}
