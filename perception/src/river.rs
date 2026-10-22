//! Reads the four rivers (#8). Each seat's river is rectified into an upright view from that
//! seat (see [`crate::table`]), where every river looks like the player's own: rows of
//! `columns` tiles from the left, the last row running on. In that view the tiles are found
//! by their faces and read with the seat's own template cache.
//!
//! - A face is light and grey, or tinted pink: the game tints tiles of the kind under the
//!   mouse. The right and across rivers get less light, and the whole table dims at times,
//!   so "light" follows each view's lightest greys.
//! - Light patches inside a face (the middle of a ring of circles) and specks smaller than a
//!   tile are not faces.
//! - A face wider and lower than the upright ones lies sideways: the riichi declaration. It
//!   is turned upright before it is read. Lower is measured against the view's upright
//!   faces, since a tile's light side can join its face; it is as tall on both.
//! - Faces that touch are split by width.
//! - A row goes on only while each face starts where the last one ended, and a row starts
//!   only after a full one, so avatars and labels beside a short river are left out.
//!
//! On 76 frames recorded from live games (3024×1900 px) the row lengths grow one discard at
//! a time in turn order, called tiles disappear and a riichi tile stays sideways, and the
//! same layout rectifies a 1493×912 screenshot of another client.

use image::RgbImage;
use image::imageops;

use crate::capture::FrameView;
use crate::classifier::ClassifierError;
use crate::profile::{RiverLayout, TileFace};
use crate::recognizer::{Classify, Recognition, TileRecognizer, Zone};
use crate::table::{Seat, Table};

/// Rows a river is read to; the last runs on.
pub const ROWS: usize = 3;
/// Tile widths read across: room for the last row to run on.
const READ_WIDTH: f64 = 13.0;
/// Tile widths kept above the first row and below the last.
const MARGIN: f64 = 0.3;
/// How much more the pink tint spreads a face's channels than the profile allows.
const TINT_SPREAD: u8 = 15;
/// In a darker view, faces are the greys within this of the lightest one...
const LIGHT_MARGIN: u8 = 25;
/// ...but never darker than this, so an empty river does not make the table a face.
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
const SIDEWAYS_MIN_WIDTH: f64 = 1.15;
const SIDEWAYS_MAX_HEIGHT: f64 = 0.95;
/// Height of an upright face in tile widths, when the view has none to measure.
const UPRIGHT_HEIGHT: f64 = 1.3;
/// How far a face may start from where the last one ended and still continue the row.
const MAX_GAP: f64 = 0.45;
/// Least pixels per tile width a river is rectified at.
const MIN_SCALE: f64 = 8.0;

/// A river rectified into an upright view from its seat.
pub struct RiverView {
    pub image: RgbImage,
    /// Pixels per tile width.
    pub scale: f64,
}

/// A tile found in a [`RiverView`], in its pixels; `right` and `bottom` are exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RiverSlot {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub row: usize,
    pub sideways: bool,
}

/// One tile of a river as read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RiverTile {
    pub tile: Recognition,
    /// The riichi declaration.
    pub sideways: bool,
}

/// The four rivers in turn order from the player ([`Seat::index`]), each in discard order.
pub type Rivers = [Vec<RiverTile>; 4];

/// `seat`'s river as seen from that seat, at about the frame's own resolution there.
pub fn rectify(
    frame: &FrameView<'_>,
    table: &Table,
    layout: &RiverLayout,
    seat: Seat,
) -> RiverView {
    let middle = [
        layout.left + layout.columns as f64 / 2.0,
        layout.near + layout.row_pitch,
    ];
    let scale = table.scale_at(seat, middle[0], middle[1]).max(MIN_SCALE);
    // Half a tile to the left of the first column, so the first face starts at 0.5 tiles.
    let origin = [layout.left - 0.5, layout.near - MARGIN];
    let size = [READ_WIDTH, ROWS as f64 * layout.row_pitch + 2.0 * MARGIN];
    RiverView {
        image: table.rectify(frame, seat, origin, size, scale),
        scale,
    }
}

/// The river's tiles in discard order.
pub fn find_tiles(view: &RiverView, layout: &RiverLayout, face: TileFace) -> Vec<RiverSlot> {
    let s = view.scale;
    let (width, height) = view.image.dimensions();
    let max_spread = face.max_spread.saturating_add(TINT_SPREAD);
    let light = face_light(&view.image, face);
    let mask: Vec<bool> = view
        .image
        .pixels()
        .map(|p| {
            let [r, g, b] = p.0;
            let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
            hi >= light && hi - lo <= max_spread
        })
        .collect();

    let faces: Vec<Component> = components(&mask, width as usize, height as usize)
        .into_iter()
        .filter(|c| {
            let (w, h) = (f64::from(c.right - c.left), f64::from(c.bottom - c.top));
            w.max(h) >= MIN_FACE_LONG * s && w.min(h) >= MIN_FACE_SHORT * s
        })
        .collect();
    let faces: Vec<&Component> = faces
        .iter()
        .filter(|c| !faces.iter().any(|o| o.encloses(c)))
        .collect();
    let mut upright: Vec<u32> = faces
        .iter()
        .filter(|c| f64::from(c.right - c.left) <= SIDEWAYS_MIN_WIDTH * s)
        .map(|c| c.bottom - c.top)
        .collect();
    upright.sort_unstable();
    let upright_height = upright
        .get(upright.len() / 2)
        .map_or(UPRIGHT_HEIGHT * s, |h| f64::from(*h));

    let mut found = Vec::new();
    for c in faces {
        let (w, h) = (f64::from(c.right - c.left), f64::from(c.bottom - c.top));
        let row = ((f64::from(c.top + c.bottom) / 2.0 / s - MARGIN) / layout.row_pitch).floor();
        if !(0.0..ROWS as f64).contains(&row) {
            continue;
        }
        let sideways = w > SIDEWAYS_MIN_WIDTH * s && h < SIDEWAYS_MAX_HEIGHT * upright_height;
        let parts = if sideways {
            1
        } else {
            (w / s).round().max(1.0) as u32
        };
        for k in 0..parts {
            found.push(RiverSlot {
                left: c.left + (c.right - c.left) * k / parts,
                top: c.top,
                right: c.left + (c.right - c.left) * (k + 1) / parts,
                bottom: c.bottom,
                row: row as usize,
                sideways,
            });
        }
    }

    let mut river = Vec::new();
    for row in 0..ROWS {
        let mut slots: Vec<RiverSlot> = found.iter().filter(|t| t.row == row).copied().collect();
        slots.sort_by_key(|t| t.left);
        let mut edge = 0.5 * s;
        let mut count = 0;
        for slot in slots {
            if (f64::from(slot.left) - edge).abs() > MAX_GAP * s {
                break;
            }
            edge = f64::from(slot.right);
            river.push(slot);
            count += 1;
        }
        if count < layout.columns {
            break;
        }
    }
    river
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

/// The tile's picture, upright.
pub fn crop(view: &RiverView, slot: &RiverSlot) -> RgbImage {
    let tile = imageops::crop_imm(
        &view.image,
        slot.left,
        slot.top,
        slot.right - slot.left,
        slot.bottom - slot.top,
    )
    .to_image();
    if slot.sideways {
        // The game turns the riichi tile a quarter turn clockwise.
        imageops::rotate270(&tile)
    } else {
        tile
    }
}

/// Reads all four rivers, each with its seat's template cache.
pub fn read<C: Classify>(
    frame: &FrameView<'_>,
    table: &Table,
    layout: &RiverLayout,
    face: TileFace,
    recognizer: &mut TileRecognizer<C>,
) -> Result<Rivers, ClassifierError> {
    let mut rivers = Rivers::default();
    for seat in Seat::ALL {
        let view = rectify(frame, table, layout, seat);
        let slots = find_tiles(&view, layout, face);
        let crops: Vec<RgbImage> = slots.iter().map(|slot| crop(&view, slot)).collect();
        let tiles = recognizer.recognize_in(Zone::River(seat), &crops)?;
        rivers[seat.index()] = slots
            .iter()
            .zip(tiles)
            .map(|(slot, tile)| RiverTile {
                tile,
                sideways: slot.sideways,
            })
            .collect();
    }
    Ok(rivers)
}

/// A 4-connected group of mask pixels: its bounding box, `right` and `bottom` exclusive.
#[derive(Debug, PartialEq, Eq)]
struct Component {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

impl Component {
    /// Whether `other` lies inside this one's box (and is not this one).
    fn encloses(&self, other: &Component) -> bool {
        self != other
            && self.left <= other.left
            && self.top <= other.top
            && self.right >= other.right
            && self.bottom >= other.bottom
    }
}

fn components(mask: &[bool], width: usize, height: usize) -> Vec<Component> {
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
        out.push(Component {
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
    use image::Rgb;

    use super::*;

    const TABLE: Rgb<u8> = Rgb([54, 73, 115]);
    const FACE: Rgb<u8> = Rgb([232, 230, 224]);
    const PINK: Rgb<u8> = Rgb([215, 176, 173]);
    const S: f64 = 20.0;

    fn layout() -> RiverLayout {
        RiverLayout {
            left: -3.0,
            near: 3.2,
            row_pitch: 1.364,
            columns: 6,
        }
    }

    fn face() -> TileFace {
        TileFace {
            min_brightness: 175,
            max_spread: 45,
        }
    }

    fn fill(image: &mut RgbImage, x: u32, y: u32, w: u32, h: u32, colour: Rgb<u8>) {
        for yy in y..y + h {
            for xx in x..x + w {
                image.put_pixel(xx, yy, colour);
            }
        }
    }

    /// Top of row `row`'s faces in a view at [`S`] pixels per tile width.
    fn row_top(row: usize) -> u32 {
        ((MARGIN + row as f64 * 1.364) * S) as u32 + 2
    }

    #[test]
    fn finds_rows_sideways_tiles_and_touching_faces() {
        let mut image = RgbImage::from_pixel(260, 94, TABLE);
        // A full first row; the last two faces touch, and one is tinted pink.
        for k in 0..6 {
            let colour = if k == 2 { PINK } else { FACE };
            let w = if k == 4 { 20 } else { 18 };
            fill(&mut image, 10 + 20 * k, row_top(0), w, 25, colour);
        }
        // The second row: a tile, the riichi tile lying sideways, a tile after it.
        let top = row_top(1);
        fill(&mut image, 10, top, 18, 25, FACE);
        fill(&mut image, 30, top + 7, 26, 18, FACE);
        fill(&mut image, 57, top, 18, 25, FACE);
        // An avatar further along the row is not a tile.
        fill(&mut image, 200, top, 30, 25, FACE);
        let view = RiverView { image, scale: S };

        let slots = find_tiles(&view, &layout(), face());
        let rows: Vec<usize> = slots.iter().map(|s| s.row).collect();
        assert_eq!(rows, [0, 0, 0, 0, 0, 0, 1, 1, 1]);
        let sideways: Vec<bool> = slots.iter().map(|s| s.sideways).collect();
        assert_eq!(sideways.iter().filter(|s| **s).count(), 1);
        assert!(sideways[7]);
        // The touching faces are split in the middle.
        assert_eq!((slots[4].left, slots[4].right), (90, 109));
        assert_eq!((slots[5].left, slots[5].right), (109, 128));
        // Turned upright, the sideways tile is taller than wide.
        let upright = crop(&view, &slots[7]);
        assert!(upright.height() > upright.width());
    }

    /// Draws a dark ring just inside a face, leaving a light patch in its middle, as the
    /// circles of a 5p do.
    fn ring(image: &mut RgbImage, x: u32, y: u32) {
        let dark = Rgb([40, 40, 40]);
        fill(image, x + 2, y + 2, 14, 1, dark);
        fill(image, x + 2, y + 22, 14, 1, dark);
        fill(image, x + 2, y + 2, 1, 21, dark);
        fill(image, x + 15, y + 2, 1, 21, dark);
    }

    #[test]
    fn a_light_patch_inside_a_face_does_not_end_the_row() {
        // A recorded frame lost the end of a river this way: the patch inside a 5p looked
        // like a tile that did not start where the 5p ended.
        let mut image = RgbImage::from_pixel(260, 94, TABLE);
        for k in 0..6 {
            fill(&mut image, 10 + 20 * k, row_top(0), 18, 25, FACE);
        }
        ring(&mut image, 10 + 20 * 4, row_top(0));
        fill(&mut image, 10, row_top(1), 18, 25, FACE);
        let view = RiverView { image, scale: S };
        assert_eq!(find_tiles(&view, &layout(), face()).len(), 7);
    }

    #[test]
    fn faces_in_a_darker_view_are_found() {
        // The right and across rivers are lit less; their faces can fall below the profile's
        // bar.
        let dim = Rgb([168, 168, 163]);
        let mut image = RgbImage::from_pixel(260, 94, TABLE);
        for k in 0..6 {
            fill(&mut image, 10 + 20 * k, row_top(0), 18, 25, dim);
        }
        let view = RiverView { image, scale: S };
        assert_eq!(find_tiles(&view, &layout(), face()).len(), 6);
    }

    #[test]
    fn a_sideways_tile_is_lower_than_its_neighbours_even_with_their_sides() {
        // In a client with white tile sides each face runs into its side, so every face is
        // taller than a tile (1.5 here) and the sideways one too (1.25); it is still lower
        // than its neighbours by the same amount.
        let mut image = RgbImage::from_pixel(260, 94, TABLE);
        let top = row_top(0);
        fill(&mut image, 10, top, 18, 30, FACE);
        fill(&mut image, 30, top + 5, 26, 25, FACE);
        for k in 0..4 {
            fill(&mut image, 58 + 20 * k, top, 18, 30, FACE);
        }
        let view = RiverView { image, scale: S };
        let slots = find_tiles(&view, &layout(), face());
        let sideways: Vec<bool> = slots.iter().map(|s| s.sideways).collect();
        assert_eq!(sideways, [false, true, false, false, false, false]);
    }

    #[test]
    fn a_short_row_ends_the_river() {
        let mut image = RgbImage::from_pixel(260, 94, TABLE);
        for k in 0..4 {
            fill(&mut image, 10 + 20 * k, row_top(0), 18, 25, FACE);
        }
        // Something light where the second row would be is not part of the river.
        fill(&mut image, 10, row_top(1), 18, 25, FACE);
        let view = RiverView { image, scale: S };
        assert_eq!(find_tiles(&view, &layout(), face()).len(), 4);
    }

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
                Component {
                    left: 0,
                    top: 0,
                    right: 2,
                    bottom: 2
                },
                Component {
                    left: 2,
                    top: 1,
                    right: 4,
                    bottom: 3
                },
            ]
        );
    }
}
