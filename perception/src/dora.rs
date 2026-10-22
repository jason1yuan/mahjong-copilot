//! Reads the dora indicators (#8) from the box in the top left corner. The box is part of
//! the game's interface, not the 3D table, and in a wide window it can stick to the window's
//! edge rather than the game area, so it is searched for: the leftmost face of the profile's
//! size ([`DoraLayout`], relative to the hand's tile pitch) in the top left corner is the
//! first indicator, and the next slots follow at the profile's pitch. A slot is revealed
//! while it shows a grey face; unrevealed slots show the tile back, whose colour depends on
//! the skin.
//!
//! On 77 recorded frames the faces are 0.545 × 0.807 of the hand's pitch, 0.594 apart, and
//! sit in the same place every frame.

use image::RgbImage;

use crate::capture::FrameView;
use crate::classifier::ClassifierError;
use crate::faces;
use crate::hand::{GameArea, TileBox};
use crate::profile::{DoraLayout, TileFace};
use crate::recognizer::{Classify, Recognition, TileRecognizer, Zone};

/// The corner searched, as fractions of the game area: up to this far right of its left
/// edge (or the window's, whichever is further left) and down from its top.
const SEARCH_WIDTH: f32 = 0.3;
const SEARCH_HEIGHT: f32 = 0.3;
/// How far above the game area the box may start, as a fraction of its height.
const SEARCH_ABOVE: f32 = 0.05;
/// How far a face's size may be off the profile's.
const SIZE_TOLERANCE: f32 = 0.25;
/// How far a slot may sit from where the pitch puts it, as a fraction of the pitch.
const SLOT_TOLERANCE: f32 = 0.2;
/// Share of a slot that must be plain grey for a revealed face.
const MIN_GREY: f64 = 0.5;

/// The revealed indicators' boxes in frame pixels, left to right.
pub fn find(
    frame: &FrameView<'_>,
    area: &GameArea,
    hand_pitch: f32,
    layout: &DoraLayout,
    face: TileFace,
) -> Vec<TileBox> {
    let x1 = (area.left + SEARCH_WIDTH * area.width).clamp(0.0, frame.width() as f32) as u32;
    let y0 = (area.top - SEARCH_ABOVE * area.height).clamp(0.0, frame.height() as f32) as u32;
    let y1 = (area.top + SEARCH_HEIGHT * area.height).clamp(0.0, frame.height() as f32) as u32;
    if x1 == 0 || y1 <= y0 {
        return Vec::new();
    }
    let corner = frame.sub(0, y0, x1, y1 - y0).to_rgb();
    let (w, h, pitch) = (
        layout.width * hand_pitch,
        layout.height * hand_pitch,
        layout.pitch * hand_pitch,
    );
    let near = |value: u32, target: f32| (value as f32 - target).abs() <= SIZE_TOLERANCE * target;
    let mut candidates: Vec<faces::Face> = faces::find(&corner, face, f64::from(w))
        .faces
        .into_iter()
        .filter(|f| {
            near(f.width(), w) && near(f.height(), h) && grey_share(&corner, f, face) >= MIN_GREY
        })
        .collect();
    candidates.sort_by_key(|f| f.left);
    let Some(first) = candidates.first().copied() else {
        return Vec::new();
    };
    let mut boxes = Vec::new();
    for slot in 0..layout.slots {
        let expected = first.left as f32 + slot as f32 * pitch;
        let Some(found) = candidates.iter().find(|f| {
            (f.left as f32 - expected).abs() <= SLOT_TOLERANCE * pitch
                && (f.top as f32 - first.top as f32).abs() <= SLOT_TOLERANCE * h
        }) else {
            break;
        };
        boxes.push(TileBox {
            left: found.left,
            top: found.top + y0,
            right: found.right,
            bottom: found.bottom + y0,
        });
    }
    boxes
}

/// Share of the face's box that is plain grey by the profile's own test.
fn grey_share(image: &RgbImage, f: &faces::Face, face: TileFace) -> f64 {
    let mut grey = 0u32;
    for y in f.top..f.bottom {
        for x in f.left..f.right {
            let [r, g, b] = image.get_pixel(x, y).0;
            let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
            if hi >= face.min_brightness && hi - lo <= face.max_spread {
                grey += 1;
            }
        }
    }
    f64::from(grey) / f64::from((f.width() * f.height()).max(1))
}

/// Reads the revealed dora indicators, with their own template cache.
pub fn read<C: Classify>(
    frame: &FrameView<'_>,
    area: &GameArea,
    hand_pitch: f32,
    layout: &DoraLayout,
    face: TileFace,
    recognizer: &mut TileRecognizer<C>,
) -> Result<Vec<Recognition>, ClassifierError> {
    let crops: Vec<RgbImage> = find(frame, area, hand_pitch, layout, face)
        .iter()
        .map(|b| b.crop(frame))
        .collect();
    recognizer.recognize_in(Zone::Dora, &crops)
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    const FACE: [u8; 3] = [232, 230, 224];
    const BACK: [u8; 3] = [150, 190, 100];
    const PANEL: [u8; 3] = [60, 40, 30];

    fn frame_with(revealed: usize) -> Vec<u8> {
        // A 400x300 frame: the box's five 22x32 slots, 24 apart, from (20, 30).
        let mut image = RgbImage::from_pixel(400, 300, Rgb(PANEL));
        for slot in 0..5u32 {
            let colour = if (slot as usize) < revealed {
                FACE
            } else {
                BACK
            };
            for y in 30..62 {
                for x in 20 + 24 * slot..42 + 24 * slot {
                    image.put_pixel(x, y, Rgb(colour));
                }
            }
        }
        image
            .pixels()
            .flat_map(|p| [p[2], p[1], p[0], 255])
            .collect()
    }

    #[test]
    fn finds_the_revealed_indicators_only() {
        // Hand pitch 40 px: faces 21.8 x 32.3, 23.8 apart.
        let layout = DoraLayout {
            width: 0.545,
            height: 0.807,
            pitch: 0.594,
            slots: 5,
        };
        let face = TileFace {
            min_brightness: 175,
            max_spread: 45,
        };
        let area = GameArea {
            left: 0.0,
            top: 20.0,
            width: 400.0,
            height: 225.0,
        };
        for revealed in [1, 2, 4] {
            let bgra = frame_with(revealed);
            let frame = FrameView::new(400, 300, 1600, &bgra, 1.0).unwrap();
            let boxes = find(&frame, &area, 40.0, &layout, face);
            assert_eq!(boxes.len(), revealed);
            assert_eq!((boxes[0].left, boxes[0].top), (20, 30));
        }
    }
}
