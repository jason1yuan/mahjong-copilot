//! Reads the four seats' melds (#8). Each seat's melds lie on the table at its right-hand
//! side, in a band given in seat coordinates (see [`crate::table`]), so one layout serves
//! all four seats. In the band rectified into an upright view from the seat:
//!
//! - Melds run leftwards from the rightmost face; the strip goes on while each tile starts
//!   where the last one ended. Where a face is missing but something tile-coloured that is
//!   not a face fills the place, it is a tile lying face down (a concealed kan).
//! - A sideways face is the called tile; two sideways faces one above the other are an added
//!   kan, the farther one the added tile.
//! - Melds touch, so the strip is split by what its tiles are: the split in which every
//!   group is a meld (three tiles with one called: a pon or a run; one called tile with an
//!   added one; four alike with one called or some face down).
//!
//! The seat a tile was called from follows from where the sideways tile sits; that, and
//! which kind of meld it is, is left to the state engine.

use image::RgbImage;
use image::imageops;

use crate::capture::FrameView;
use crate::classifier::ClassifierError;
use crate::faces::{self, Face};
use crate::profile::{MeldLayout, TileFace};
use crate::recognizer::{Classify, Recognition, TileRecognizer, Zone};
use crate::table::{Seat, Table};

/// How far a tile may start from where the last one ended and still continue the strip, in
/// tile widths.
const MAX_GAP: f64 = 0.45;
/// Share of a place that must be tile-coloured, not table, for a face-down tile.
const MIN_BACK: f64 = 0.6;
/// Least share of a face's height within the strip's row for it to continue the strip.
const MIN_ROW_SHARE: f64 = 0.5;
/// Most space, in tile widths, between a tile added to a kan and the sideways tile under it.
const MAX_STACK_GAP: f64 = 0.3;
/// Longest side a face may have, in tile widths, to be a tile: the perspective stretches
/// tiles to about 1.8.
const MAX_SIZE: f64 = 2.5;
/// Width of a tile lying face down, in tile widths.
const BACK_WIDTH: f64 = 0.95;
/// Least pixels per tile width the band is rectified at.
const MIN_SCALE: f64 = 8.0;
/// Most pixels per tile width: the classifier and the cache shrink tiles further anyway,
/// and the near seats would otherwise cost the most.
const MAX_SCALE: f64 = 72.0;

/// A seat's meld band rectified into an upright view from that seat.
pub struct MeldView {
    pub image: RgbImage,
    /// Pixels per tile width.
    pub scale: f64,
}

/// A tile found in a [`MeldView`], in its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeldSlot {
    pub face: Face,
    /// Lying face down: there is no face to read.
    pub back: bool,
    pub sideways: bool,
    /// The tile added on top of this sideways one (an added kan).
    pub added: Option<Face>,
}

/// One tile of a meld as read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeldTile {
    /// `None` for a tile lying face down.
    pub tile: Option<Recognition>,
    /// The called tile.
    pub sideways: bool,
    /// The tile added to a called pon, making it a kan.
    pub added: Option<Recognition>,
}

/// A meld's tiles from left to right as its owner sees them.
pub type Meld = Vec<MeldTile>;

/// Each seat's melds in turn order from the player ([`Seat::index`]), in the order they were
/// made (right to left on the table).
pub type Melds = [Vec<Meld>; 4];

/// `seat`'s meld band as seen from that seat, at about the frame's own resolution there.
pub fn rectify(frame: &FrameView<'_>, table: &Table, layout: &MeldLayout, seat: Seat) -> MeldView {
    let middle = [layout.right - 3.0, (layout.near + layout.far) / 2.0];
    let scale = table
        .scale_at(seat, middle[0], middle[1])
        .clamp(MIN_SCALE, MAX_SCALE);
    let origin = [layout.left, layout.near];
    let size = [layout.right - layout.left, layout.far - layout.near];
    MeldView {
        image: table.rectify(frame, seat, origin, size, scale),
        scale,
    }
}

/// The band's tiles from right to left, the order the melds were made in.
pub fn find_strip(view: &MeldView, layout: &MeldLayout, face: TileFace) -> Vec<MeldSlot> {
    let s = view.scale;
    let found = faces::find(&view.image, face, s);
    let wide = |f: &Face| f64::from(f.width()) > faces::SIDEWAYS_MIN_WIDTH * s;

    // A wide face lying right on another wide face is the tile added to a kan; it goes with
    // the one under it and is not a slot of its own. A face further up (a river tile at the
    // band's edge) is not.
    let stacked = |upper: &Face, lower: &Face| {
        upper != lower
            && wide(upper)
            && wide(lower)
            && overlap(upper, lower) > 0.5
            && upper.top < lower.top
            && f64::from(lower.top) - f64::from(upper.bottom) <= MAX_STACK_GAP * s
    };
    let mut slots: Vec<MeldSlot> = found
        .faces
        .iter()
        .filter(|f| {
            // Something far bigger than a tile (an arm in a cut-in) is not one.
            f64::from(f.width().max(f.height())) <= MAX_SIZE * s
                && !found.faces.iter().any(|o| stacked(f, o))
        })
        .map(|f| MeldSlot {
            face: *f,
            back: false,
            sideways: false,
            added: found.faces.iter().find(|o| stacked(o, f)).copied(),
        })
        .collect();
    slots.sort_by_key(|slot| std::cmp::Reverse(slot.face.right));

    // Walk leftwards from the rightmost tile near the band's right end.
    let start_from = (layout.start_after - layout.left) * s;
    let Some(first) = slots
        .iter()
        .position(|slot| f64::from(slot.face.right) >= start_from)
    else {
        return Vec::new();
    };
    let (top, bottom) = (slots[first].face.top, slots[first].face.bottom);
    let width = BACK_WIDTH * s;
    let mut strip = Vec::new();
    // A concealed kan can end in a tile face down on the right too.
    let right = f64::from(slots[first].face.right);
    let beyond = Face {
        left: right as u32,
        top,
        right: (right + width) as u32,
        bottom,
    };
    if f64::from(beyond.right) <= f64::from(view.image.width())
        && back_share(&view.image, &beyond) >= MIN_BACK
    {
        strip.push(MeldSlot {
            face: beyond,
            back: true,
            sideways: false,
            added: None,
        });
    }
    strip.push(slots[first]);
    let mut rest: Vec<MeldSlot> = slots[first + 1..].to_vec();
    loop {
        let edge = f64::from(strip[strip.len() - 1].face.left);
        // The next tile adjoins the last one and lies in the same row: a face of something
        // else in the band (a name plate, a river tile) can end at the same place.
        let in_row = |slot: &MeldSlot| {
            let shared = slot
                .face
                .bottom
                .min(bottom)
                .saturating_sub(slot.face.top.max(top));
            let shorter = slot.face.height().min(bottom - top).max(1);
            f64::from(shared) / f64::from(shorter)
        };
        if let Some(i) = rest
            .iter()
            .enumerate()
            .filter(|(_, slot)| {
                (f64::from(slot.face.right) - edge).abs() <= MAX_GAP * s
                    && in_row(slot) >= MIN_ROW_SHARE
            })
            .max_by(|(_, a), (_, b)| in_row(a).total_cmp(&in_row(b)))
            .map(|(i, _)| i)
        {
            strip.push(rest.remove(i));
            continue;
        }
        // No face adjoins: a tile lying face down, or the end of the melds.
        let left = edge - width;
        if left < 0.0 {
            break;
        }
        let back = Face {
            left: left as u32,
            top,
            right: edge as u32,
            bottom,
        };
        if back_share(&view.image, &back) < MIN_BACK {
            break;
        }
        strip.push(MeldSlot {
            face: back,
            back: true,
            sideways: false,
            added: None,
        });
    }

    // Sideways is judged against the strip's own tallest tile, which stands upright: the band
    // also holds other faces (river tiles, the hand's backs) whose heights would skew the
    // view's, and with the perspective upright tiles can come out as wide as sideways ones.
    let upright_height = strip
        .iter()
        .filter(|slot| !slot.back)
        .map(|slot| f64::from(slot.face.height()))
        .fold(0.0, f64::max);
    for slot in strip.iter_mut().filter(|slot| !slot.back) {
        slot.sideways = faces::is_sideways(&slot.face, s, upright_height);
        if !slot.sideways {
            slot.added = None;
        }
    }
    strip
}

/// Splits a strip of tiles (right to left) into melds, each left to right, choosing the
/// split with the fewest groups that are not melds, and keeps only the melds.
pub fn group(strip: &[MeldTile]) -> Vec<Meld> {
    // best[i]: (groups that are not melds, sizes) for the strip from i on.
    let n = strip.len();
    let mut best: Vec<Option<(usize, Vec<usize>)>> = vec![None; n + 1];
    best[n] = Some((0, Vec::new()));
    for i in (0..n).rev() {
        let mut choice: Option<(usize, Vec<usize>)> = None;
        for size in [3, 4, 2, 1] {
            let Some((bad, sizes)) = (i + size <= n).then(|| best[i + size].clone()).flatten()
            else {
                continue;
            };
            let bad = bad + usize::from(!is_meld(&strip[i..i + size]));
            if choice.as_ref().is_none_or(|(b, _)| bad < *b) {
                let mut sizes = sizes;
                sizes.insert(0, size);
                choice = Some((bad, sizes));
            }
        }
        best[i] = choice;
    }
    let sizes = best[0].clone().map_or_else(Vec::new, |(_, sizes)| sizes);
    let mut melds = Vec::new();
    let mut start = 0;
    for size in sizes {
        let group = &strip[start..start + size];
        start += size;
        // Something in the band that is not a meld (an avatar's light patch, a hand lying
        // face down beside it) is left out.
        if is_meld(group) {
            let mut meld = group.to_vec();
            meld.reverse();
            melds.push(meld);
        }
    }
    melds
}

/// Whether the tiles (in any order) make one meld.
fn is_meld(tiles: &[MeldTile]) -> bool {
    let kinds: Vec<u8> = tiles
        .iter()
        .filter_map(|t| t.tile)
        .map(|r| r.tile.kind())
        .collect();
    let backs = tiles.len() - kinds.len();
    let called = tiles.iter().filter(|t| t.sideways).count();
    let added = tiles.iter().filter_map(|t| t.added).map(|r| r.tile.kind());
    let alike = kinds.iter().all(|k| Some(*k) == kinds.first().copied())
        && added.clone().all(|k| Some(k) == kinds.first().copied());
    match (tiles.len(), backs, called) {
        (3, 0, 1) if added.count() > 0 => alike,
        (3, 0, 1) => alike || is_run(&kinds),
        (4, 0, 1) => alike,
        (4, 1.., 0) => alike,
        _ => false,
    }
}

/// Three consecutive numbers of one suit.
fn is_run(kinds: &[u8]) -> bool {
    let mut k = kinds.to_vec();
    k.sort_unstable();
    k.len() == 3 && k[2] < 27 && k[0] / 9 == k[2] / 9 && k[1] == k[0] + 1 && k[2] == k[1] + 1
}

/// Share of the narrower face's width that two faces share.
fn overlap(a: &Face, b: &Face) -> f64 {
    let shared = a.right.min(b.right).saturating_sub(a.left.max(b.left));
    f64::from(shared) / f64::from(a.width().min(b.width()).max(1))
}

/// Share of the box that is tile-coloured but not a face: not table blue, not dark, and
/// not grey.
fn back_share(image: &RgbImage, area: &Face) -> f64 {
    let (w, h) = image.dimensions();
    let (right, bottom) = (area.right.min(w), area.bottom.min(h));
    let mut back = 0u32;
    let mut all = 0u32;
    for y in area.top..bottom {
        for x in area.left..right {
            let [r, g, b] = image.get_pixel(x, y).0;
            let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
            all += 1;
            if hi >= 100 && hi - lo >= 40 && b < hi {
                back += 1;
            }
        }
    }
    f64::from(back) / f64::from(all.max(1))
}

/// The face's picture, upright.
fn crop(view: &MeldView, face: &Face, sideways: bool) -> RgbImage {
    let tile = imageops::crop_imm(
        &view.image,
        face.left,
        face.top,
        face.width(),
        face.height(),
    )
    .to_image();
    if sideways {
        // Called tiles lie a quarter turn clockwise, like the riichi tile in a river.
        imageops::rotate270(&tile)
    } else {
        tile
    }
}

/// Reads all four seats' melds, each with its seat's template cache.
pub fn read<C: Classify>(
    frame: &FrameView<'_>,
    table: &Table,
    layout: &MeldLayout,
    face: TileFace,
    recognizer: &mut TileRecognizer<C>,
) -> Result<Melds, ClassifierError> {
    let mut all = Melds::default();
    for seat in Seat::ALL {
        let view = rectify(frame, table, layout, seat);
        let strip = find_strip(&view, layout, face);
        // One batch per seat: every face, then every added tile.
        let mut crops = Vec::new();
        for slot in strip.iter().filter(|slot| !slot.back) {
            crops.push(crop(&view, &slot.face, slot.sideways));
        }
        for added in strip.iter().filter_map(|slot| slot.added) {
            crops.push(crop(&view, &added, true));
        }
        let mut tiles = recognizer
            .recognize_in(Zone::Meld(seat), &crops)?
            .into_iter();
        let mut read: Vec<MeldTile> = strip
            .iter()
            .map(|slot| MeldTile {
                tile: if slot.back { None } else { tiles.next() },
                sideways: slot.sideways,
                added: None,
            })
            .collect();
        for (tile, slot) in read.iter_mut().zip(&strip) {
            if slot.added.is_some() {
                tile.added = tiles.next();
            }
        }
        all[seat.index()] = group(&read);
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    const TABLE: Rgb<u8> = Rgb([54, 73, 115]);
    const FACE: Rgb<u8> = Rgb([232, 230, 224]);
    const BACK: Rgb<u8> = Rgb([150, 190, 100]);
    const S: f64 = 20.0;

    fn layout() -> MeldLayout {
        MeldLayout {
            left: 0.0,
            right: 13.0,
            near: 9.3,
            far: 12.7,
            start_after: 9.0,
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

    fn band() -> RgbImage {
        RgbImage::from_pixel(260, 68, TABLE)
    }

    #[test]
    fn finds_the_strip_and_the_called_tiles() {
        let mut image = band();
        // From the right: a pon called from across (sideways in the middle)...
        fill(&mut image, 232, 20, 18, 26, FACE);
        fill(&mut image, 205, 27, 26, 19, FACE);
        fill(&mut image, 186, 20, 18, 26, FACE);
        // ...then a chi called from the left (sideways first).
        fill(&mut image, 165, 20, 18, 26, FACE);
        fill(&mut image, 146, 20, 18, 26, FACE);
        fill(&mut image, 119, 27, 26, 19, FACE);
        // Something light much further left is not a meld.
        fill(&mut image, 20, 20, 18, 26, FACE);
        let view = MeldView { image, scale: S };
        let strip = find_strip(&view, &layout(), face());
        let sideways: Vec<bool> = strip.iter().map(|t| t.sideways).collect();
        assert_eq!(sideways, [false, true, false, false, false, true]);
    }

    #[test]
    fn an_added_kan_is_one_slot_with_the_added_tile() {
        let mut image = band();
        fill(&mut image, 232, 20, 18, 26, FACE);
        fill(&mut image, 205, 27, 26, 19, FACE);
        fill(&mut image, 205, 6, 26, 19, FACE);
        fill(&mut image, 186, 20, 18, 26, FACE);
        let view = MeldView { image, scale: S };
        let strip = find_strip(&view, &layout(), face());
        assert_eq!(strip.len(), 3);
        let called = strip[1];
        assert!(called.sideways);
        assert_eq!(called.added.map(|a| a.top), Some(6));
    }

    #[test]
    fn faces_off_the_strip_change_nothing() {
        let mut image = band();
        // A pon called from across...
        fill(&mut image, 232, 20, 18, 26, FACE);
        fill(&mut image, 205, 27, 26, 19, FACE);
        fill(&mut image, 186, 20, 18, 26, FACE);
        // ...a light face at the band's edge above the called tile, not added to it...
        fill(&mut image, 205, 0, 26, 19, FACE);
        // ...and low faces far to the left, more of them than upright tiles in the strip.
        for x in [10, 45, 80] {
            fill(&mut image, x, 40, 26, 19, FACE);
        }
        let view = MeldView { image, scale: S };
        let strip = find_strip(&view, &layout(), face());
        let sideways: Vec<bool> = strip.iter().map(|t| t.sideways).collect();
        assert_eq!(sideways, [false, true, false]);
        assert_eq!(strip[1].added, None);
    }

    #[test]
    fn upright_tiles_as_wide_as_sideways_ones() {
        let mut image = band();
        // With the perspective, upright tiles near the band's end come out wide too...
        fill(&mut image, 226, 20, 24, 26, FACE);
        fill(&mut image, 201, 20, 24, 26, FACE);
        fill(&mut image, 170, 28, 30, 18, FACE);
        // ...and the band's other faces are mostly low ones (the hand's backs).
        for x in [10, 40, 70, 100] {
            fill(&mut image, x, 50, 20, 11, FACE);
        }
        let view = MeldView { image, scale: S };
        let strip = find_strip(&view, &layout(), face());
        let sideways: Vec<bool> = strip.iter().map(|t| t.sideways).collect();
        assert_eq!(sideways, [false, false, true]);
    }

    #[test]
    fn something_far_bigger_than_a_tile_is_not_one() {
        let mut image = band();
        fill(&mut image, 232, 20, 18, 26, FACE);
        fill(&mut image, 205, 27, 26, 19, FACE);
        fill(&mut image, 186, 20, 18, 26, FACE);
        // An arm reaching in from a cut-in, touching the meld.
        fill(&mut image, 100, 0, 85, 68, FACE);
        let view = MeldView { image, scale: S };
        let strip = find_strip(&view, &layout(), face());
        let sideways: Vec<bool> = strip.iter().map(|t| t.sideways).collect();
        assert_eq!(sideways, [false, true, false]);
    }

    #[test]
    fn a_face_above_the_row_does_not_continue_the_strip() {
        let mut image = band();
        // A pon called from across...
        fill(&mut image, 232, 20, 18, 26, FACE);
        fill(&mut image, 205, 27, 26, 19, FACE);
        fill(&mut image, 186, 20, 18, 26, FACE);
        // ...and something light above the row ending where its last tile does.
        fill(&mut image, 180, 0, 25, 12, FACE);
        let view = MeldView { image, scale: S };
        let strip = find_strip(&view, &layout(), face());
        let tops: Vec<u32> = strip.iter().map(|t| t.face.top).collect();
        assert_eq!(tops, [20, 27, 20]);
    }

    #[test]
    fn tiles_face_down_continue_a_concealed_kan() {
        let mut image = band();
        fill(&mut image, 232, 20, 18, 26, BACK);
        fill(&mut image, 213, 20, 18, 26, FACE);
        fill(&mut image, 194, 20, 18, 26, FACE);
        fill(&mut image, 175, 20, 18, 26, BACK);
        let view = MeldView { image, scale: S };
        let strip = find_strip(&view, &layout(), face());
        let backs: Vec<bool> = strip.iter().map(|t| t.back).collect();
        assert_eq!(backs, [true, false, false, true]);
    }

    fn tile(name: &str, sideways: bool) -> MeldTile {
        MeldTile {
            tile: Some(Recognition {
                tile: name.parse().unwrap(),
                confidence: 0.99,
                source: crate::recognizer::Source::Classifier,
                doubt: None,
            }),
            sideways,
            added: None,
        }
    }

    fn names(melds: &[Meld]) -> Vec<String> {
        melds
            .iter()
            .map(|m| {
                m.iter()
                    .map(|t| {
                        let name = t.tile.map_or("x".to_owned(), |r| r.tile.to_string());
                        format!("{name}{}", if t.sideways { "*" } else { "" })
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }

    #[test]
    fn groups_touching_melds_by_their_tiles() {
        // Right to left: a pon of 5p, then a run 3p-5p whose 5p sits next to the pon. Four 5p
        // in a row must not make a kan here.
        let strip = [
            tile("5p", false),
            tile("5p", true),
            tile("5p", false),
            tile("5p", false),
            tile("4p", false),
            tile("3p", true),
        ];
        assert_eq!(names(&group(&strip)), ["5p 5p* 5p", "3p* 4p 5p"]);
    }

    #[test]
    fn what_is_not_a_meld_is_left_out() {
        // A seat without melds: a light patch read as a sideways 3p, and the hand lying face
        // down beside it.
        let back = MeldTile {
            tile: None,
            sideways: false,
            added: None,
        };
        assert!(group(&[tile("3p", true), back]).is_empty());
        // A real pon is kept even with junk after it.
        assert_eq!(
            names(&group(&[
                tile("E", false),
                tile("E", true),
                tile("E", false),
                back
            ])),
            ["E E* E"]
        );
    }

    #[test]
    fn groups_kans_of_four() {
        // An open kan, then a concealed kan with its ends face down.
        let back = MeldTile {
            tile: None,
            sideways: false,
            added: None,
        };
        let strip = [
            tile("E", false),
            tile("E", false),
            tile("E", true),
            tile("E", false),
            back,
            tile("9s", false),
            tile("9s", false),
            back,
        ];
        assert_eq!(names(&group(&strip)), ["E E* E E", "x 9s 9s x"]);
    }
}
