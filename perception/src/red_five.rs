//! Tells red fives from plain fives by the colour of their strokes; a port of
//! `ml/vlm_eval/red_five.py`, which explains how the threshold was chosen and checked.
//!
//! A plain five has non-red strokes (the black 伍 of 5m, the dark circles of 5p, the green
//! sticks of 5s) while a red five is drawn in red only, so the share of non-red ink among
//! all ink separates them. The top of the crop (the orange rim) and the edges (tile
//! borders) are ignored. Colours go through Pillow's 8-bit HSV conversion, exactly as in
//! the Python version, so both make the same call on the same crop.

use image::RgbImage;
use mahjong_core::Tile;

use crate::pillow;

/// Below this share of non-red ink a five is called red.
pub const NON_RED_THRESHOLD: f64 = 0.25;

/// Shares of red, green and dark pixels among ink pixels in the tile's face, and the share
/// of ink among all face pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InkShares {
    pub red: f64,
    pub green: f64,
    pub dark: f64,
    pub ink: f64,
}

pub fn ink_shares(crop: &RgbImage) -> InkShares {
    let (w, h) = crop.dimensions();
    let at = |size: u32, share: f64| ((f64::from(size) * share).round_ties_even() as u32).min(size);
    let (left, top, right, bottom) = (at(w, 0.12), at(h, 0.18), at(w, 0.88), at(h, 0.92));
    let (mut red, mut green, mut dark, mut pixels) = (0u32, 0u32, 0u32, 0u32);
    for y in top..bottom {
        for x in left..right {
            let [h8, s8, v8] = pillow::hsv(crop.get_pixel(x, y).0);
            let (hue, sat, val) = (
                f64::from(h8) / 255.0 * 360.0,
                f64::from(s8) / 255.0,
                f64::from(v8) / 255.0,
            );
            let is_red = (!(15.0..=330.0).contains(&hue)) && sat > 0.35 && val > 0.2;
            let is_green = hue > 75.0 && hue < 175.0 && sat > 0.25 && val > 0.15;
            // Black ink is unsaturated; this keeps the blue table out of small river crops.
            let is_dark = val < 0.4 && sat < 0.45 && !is_red && !is_green;
            red += u32::from(is_red);
            green += u32::from(is_green);
            dark += u32::from(is_dark);
            pixels += 1;
        }
    }
    let ink = red + green + dark;
    let total = f64::from(ink.max(1));
    InkShares {
        red: f64::from(red) / total,
        green: f64::from(green) / total,
        dark: f64::from(dark) / total,
        ink: if pixels == 0 {
            0.0
        } else {
            f64::from(ink) / f64::from(pixels)
        },
    }
}

pub fn non_red_share(crop: &RgbImage) -> f64 {
    let shares = ink_shares(crop);
    shares.green + shares.dark
}

/// Turns a predicted plain five into a red five when its ink is (almost) all red.
pub fn refine(tile: Tile, crop: &RgbImage) -> Tile {
    match tile.to_red() {
        Some(red) if non_red_share(crop) < NON_RED_THRESHOLD => red,
        _ => tile,
    }
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    /// A 40x56 white tile with a stroke of each given colour across its face.
    fn tile(strokes: &[[u8; 3]]) -> RgbImage {
        let mut crop = RgbImage::from_pixel(40, 56, Rgb([245, 245, 240]));
        for (i, colour) in strokes.iter().enumerate() {
            let y0 = 16 + 10 * i as u32;
            for y in y0..y0 + 6 {
                for x in 8..32 {
                    crop.put_pixel(x, y, Rgb(*colour));
                }
            }
        }
        crop
    }

    const RED: [u8; 3] = [200, 30, 40];
    const BLACK: [u8; 3] = [30, 30, 35];
    const GREEN: [u8; 3] = [40, 140, 70];

    #[test]
    fn all_red_fives_are_red() {
        for five in ["5m", "5p", "5s"] {
            let tile_kind = five.parse::<Tile>().unwrap();
            let red = refine(tile_kind, &tile(&[RED, RED, RED]));
            assert_eq!(red.to_string(), format!("{five}r"));
        }
    }

    #[test]
    fn fives_with_black_or_green_strokes_stay_plain() {
        let five = "5m".parse::<Tile>().unwrap();
        assert_eq!(refine(five, &tile(&[BLACK, RED])), five);
        let five = "5s".parse::<Tile>().unwrap();
        assert_eq!(refine(five, &tile(&[GREEN, GREEN, RED])), five);
    }

    #[test]
    fn only_fives_are_refined() {
        let three = "3m".parse::<Tile>().unwrap();
        assert_eq!(refine(three, &tile(&[RED, RED])), three);
    }

    #[test]
    fn a_crop_without_ink_has_no_shares() {
        let shares = ink_shares(&tile(&[]));
        assert_eq!(shares, InkShares::default());
    }
}
