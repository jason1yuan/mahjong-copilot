//! Tile faces in a rectified view of the table (#8), where tiles are upright and about
//! `scale` pixels wide: the rivers and melds are both read this way.
//!
//! - A face is light and grey, or tinted pink or light blue: the game tints tiles of the
//!   kind under the mouse. The right and across seats get less light, the whole table dims
//!   at times, and the game greys some discards, so "light" reaches well below each view's
//!   lightest greys.
//! - Light patches inside a face (the middle of a ring of circles) and specks smaller than a
//!   tile are not faces.
//! - A face wider and lower than the upright ones lies sideways (riichi, or the called tile
//!   of a meld). Lower is measured against the view's upright faces, since a tile's light
//!   side can join its face; it is as tall on both.

use image::RgbImage;

use crate::profile::TileFace;

/// How much more the pink tint spreads a face's channels than the profile allows...
const PINK_SPREAD: u8 = 15;
/// ...and the blue one.
const BLUE_SPREAD: u8 = 25;
/// Most a pink-tinted face's green and blue differ.
const PINK_GREEN_BLUE: u8 = 12;
/// In a darker view, faces are the greys within this of the lightest one...
const LIGHT_MARGIN: u8 = 60;
/// ...but never darker than this, so an empty view does not make the table a face.
const MIN_LIGHT: u8 = 130;
/// Share of grey pixels lighter than "the lightest grey", so stray specks do not count.
const LIGHTEST_SHARE: f64 = 0.005;
/// A face's long side spans at least this many tile widths, its short side at least
/// [`MIN_FACE_SHORT`].
const MIN_FACE_LONG: f64 = 0.9;
const MIN_FACE_SHORT: f64 = 0.5;
/// A face wider than this many tile widths (an upright one is about 0.95), and lower than
/// [`SIDEWAYS_MAX_HEIGHT`] of the view's upright faces, lies sideways: it is 1.36 wide and
/// 0.36 lower.
pub const SIDEWAYS_MIN_WIDTH: f64 = 1.15;
const SIDEWAYS_MAX_HEIGHT: f64 = 0.95;
/// Height of an upright face in tile widths, when the view has none to measure.
const UPRIGHT_HEIGHT: f64 = 1.3;

/// A face's box in the view's pixels; `right` and `bottom` are exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Face {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl Face {
    pub fn width(&self) -> u32 {
        self.right - self.left
    }

    pub fn height(&self) -> u32 {
        self.bottom - self.top
    }

    /// Whether `other` lies inside this one's box (and is not this one).
    fn encloses(&self, other: &Face) -> bool {
        self != other
            && self.left <= other.left
            && self.top <= other.top
            && self.right >= other.right
            && self.bottom >= other.bottom
    }
}

/// The faces in a view, with how tall its upright faces are.
pub struct Faces {
    pub faces: Vec<Face>,
    /// Median height of the faces not wider than an upright one, in pixels.
    pub upright_height: f64,
    /// Pixels per tile width.
    pub scale: f64,
}

impl Faces {
    /// Whether `face` lies sideways, measured against the whole view's upright faces.
    pub fn is_sideways(&self, face: &Face) -> bool {
        is_sideways(face, self.scale, self.upright_height)
    }
}

/// Whether `face` lies sideways in a view of `scale` pixels per tile width, next to upright
/// faces `upright_height` pixels high.
pub fn is_sideways(face: &Face, scale: f64, upright_height: f64) -> bool {
    f64::from(face.width()) > SIDEWAYS_MIN_WIDTH * scale
        && f64::from(face.height()) < SIDEWAYS_MAX_HEIGHT * upright_height
}

/// The tile faces in `image`, a view where tiles are about `scale` pixels wide.
pub fn find(image: &RgbImage, face: TileFace, scale: f64) -> Faces {
    let (width, height) = image.dimensions();
    let light = face_light(image, face);
    let mask: Vec<bool> = image
        .pixels()
        .map(|p| {
            let [r, g, b] = p.0;
            let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
            hi >= light && (hi - lo <= face.max_spread || is_tinted(r, g, b, face))
        })
        .collect();
    let sized: Vec<Face> = components(&mask, width as usize, height as usize)
        .into_iter()
        .filter(|c| {
            let (w, h) = (f64::from(c.width()), f64::from(c.height()));
            w.max(h) >= MIN_FACE_LONG * scale && w.min(h) >= MIN_FACE_SHORT * scale
        })
        .collect();
    let faces: Vec<Face> = sized
        .iter()
        .filter(|c| !sized.iter().any(|o| o.encloses(c)))
        .copied()
        .collect();
    let mut upright: Vec<u32> = faces
        .iter()
        .filter(|c| f64::from(c.width()) <= SIDEWAYS_MIN_WIDTH * scale)
        .map(Face::height)
        .collect();
    upright.sort_unstable();
    let upright_height = upright
        .get(upright.len() / 2)
        .map_or(UPRIGHT_HEIGHT * scale, |h| f64::from(*h));
    Faces {
        faces,
        upright_height,
        scale,
    }
}

/// Whether a pixel is of a tinted face. The game tints tiles pink (red leads, green and blue
/// stay together) or light blue (blue leads, then green); the green glow around a tile that
/// can be called and the table's wooden rim (red, then green well above blue) are neither.
pub(crate) fn is_tinted(r: u8, g: u8, b: u8, face: TileFace) -> bool {
    let pink = r >= g
        && r >= b
        && g.abs_diff(b) <= PINK_GREEN_BLUE
        && r - g.min(b) <= face.max_spread.saturating_add(PINK_SPREAD);
    let blue = b >= g && g >= r && b - r <= face.max_spread.saturating_add(BLUE_SPREAD);
    pink || blue
}

/// How light a face must be in this view: the profile's bar, or a little below the view's
/// lightest greys when it is darker.
fn face_light(image: &RgbImage, face: TileFace) -> u8 {
    let mut histogram = [0u32; 256];
    let mut greys = 0u32;
    for p in image.pixels() {
        let [r, g, b] = p.0;
        let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
        if hi - lo <= face.max_spread {
            histogram[usize::from(hi)] += 1;
            greys += 1;
        }
    }
    let lighter_allowed = (f64::from(greys) * LIGHTEST_SHARE) as u32;
    let mut lighter = 0u32;
    let mut lightest = 0u8;
    for (value, count) in histogram.iter().enumerate().rev() {
        lighter += count;
        if lighter > lighter_allowed {
            lightest = value as u8;
            break;
        }
    }
    face.min_brightness
        .min(lightest.saturating_sub(LIGHT_MARGIN))
        .max(MIN_LIGHT)
}

/// 4-connected groups of mask pixels, as boxes.
fn components(mask: &[bool], width: usize, height: usize) -> Vec<Face> {
    let mut seen = vec![false; mask.len()];
    let mut stack = Vec::new();
    let mut out = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % width, i / width);
            left = left.min(x);
            right = right.max(x + 1);
            top = top.min(y);
            bottom = bottom.max(y + 1);
            let neighbours = [
                (x > 0).then(|| i - 1),
                (x + 1 < width).then_some(i + 1),
                (y > 0).then(|| i - width),
                (y + 1 < height).then_some(i + width),
            ];
            for j in neighbours.into_iter().flatten() {
                if mask[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        out.push(Face {
            left: left as u32,
            top: top as u32,
            right: right as u32,
            bottom: bottom as u32,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_are_four_connected() {
        #[rustfmt::skip]
        let mask = [
            true,  true,  false, false,
            false, true,  false, true,
            false, false, true,  true,
        ];
        let found = components(&mask, 4, 3);
        assert_eq!(
            found,
            [
                Face {
                    left: 0,
                    top: 0,
                    right: 2,
                    bottom: 2
                },
                Face {
                    left: 2,
                    top: 1,
                    right: 4,
                    bottom: 3
                },
            ]
        );
    }

    const FACE: TileFace = TileFace {
        min_brightness: 175,
        max_spread: 45,
    };

    /// A row of 20 x 26 tiles on the table, two pixels apart, in the given colours.
    fn row(colours: &[[u8; 3]]) -> RgbImage {
        let mut image = RgbImage::from_pixel(200, 40, image::Rgb([54, 73, 115]));
        for (i, colour) in colours.iter().enumerate() {
            let left = 5 + 22 * i as u32;
            for y in 7..33 {
                for x in left..left + 20 {
                    image.put_pixel(x, y, image::Rgb(*colour));
                }
            }
        }
        image
    }

    #[test]
    fn greyed_and_tinted_tiles_are_faces() {
        let image = row(&[
            [218, 218, 216],
            [169, 169, 168],
            [209, 166, 163],
            [147, 183, 213],
        ]);
        assert_eq!(find(&image, FACE, 20.0).faces.len(), 4);
    }

    #[test]
    fn the_glow_round_a_tile_to_call_is_not_a_face() {
        let image = row(&[[218, 218, 216], [150, 210, 170]]);
        assert_eq!(find(&image, FACE, 20.0).faces.len(), 1);
        assert!(!is_tinted(150, 210, 170, FACE));
        assert!(!is_tinted(200, 120, 40, FACE));
    }
}
