//! The action buttons (#9): chi, pon, kan, ron, riichi and tsumo, shown in a row ending in
//! the grey skip button whenever the player may act.
//!
//! - The skip button is always there with the others and keeps its place, so a grey panel
//!   in its box is what says the buttons are up. Cut-ins, win effects and results screens
//!   put colour where the buttons go, but never the skip button.
//! - The others fill slots leftwards from it, one per slot, and each kind has its own
//!   colour, so a slot is read by the hue of its coloured pixels. No text is read, so the
//!   button's language does not matter, and no picture of a button is kept.
//! - Positions are fractions of the game area (see [`GameArea`]), colours hues in degrees;
//!   both come from the profile.

use crate::capture::FrameView;
use crate::hand::GameArea;
use crate::profile::ButtonLayout;

/// An action button other than skip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Chi,
    Pon,
    Kan,
    Ron,
    Riichi,
    Tsumo,
}

impl Button {
    pub const ALL: [Button; 6] = [
        Button::Chi,
        Button::Pon,
        Button::Kan,
        Button::Ron,
        Button::Riichi,
        Button::Tsumo,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Button::Chi => "chi",
            Button::Pon => "pon",
            Button::Kan => "kan",
            Button::Ron => "ron",
            Button::Riichi => "riichi",
            Button::Tsumo => "tsumo",
        }
    }
}

/// Share of the skip box that must be grey for the buttons to be up. Measured: about half
/// with the buttons up, under 0.1 otherwise.
const MIN_SKIP_GREY: f64 = 0.3;
/// Share of a slot that must be coloured for a button to be in it. Measured: half or more
/// with a button, under 0.1 without.
const MIN_SLOT_COLOUR: f64 = 0.3;
/// Most a slot's hue may differ from a button's, in degrees. The kinds are at least 46
/// apart (chi 157, pon 203) and each varied by about 1 across frames.
const MAX_HUE_DIFFERENCE: f64 = 15.0;
/// Hues of the table cloth, in degrees, left out of a slot's colour: pon (203) lies close.
const TABLE_HUES: std::ops::RangeInclusive<f64> = 212.0..=250.0;
/// Every this many pixels is looked at, both ways.
const STEP: usize = 3;

/// The buttons shown, left to right; empty when there are none.
pub fn read(frame: &FrameView<'_>, area: &GameArea, layout: &ButtonLayout) -> Vec<Button> {
    let Some(skip) = pixels(frame, area, layout.skip) else {
        return Vec::new();
    };
    let grey = share(frame, skip, |_, s, v| s < 0.12 && (0.25..0.85).contains(&v));
    if grey < MIN_SKIP_GREY {
        return Vec::new();
    }
    let mut buttons = Vec::new();
    for k in 0..layout.slots {
        let shift = k as f32 * layout.pitch;
        let [left, top, right, bottom] = layout.first;
        let Some(slot) = pixels(frame, area, [left - shift, top, right - shift, bottom]) else {
            break;
        };
        // Slots fill from the right, so the first empty one ends the row.
        let Some(button) = slot_button(frame, slot, layout) else {
            break;
        };
        buttons.push(button);
    }
    buttons.reverse();
    buttons
}

/// The button in a slot, by the median hue of its coloured pixels.
fn slot_button(frame: &FrameView<'_>, slot: [u32; 4], layout: &ButtonLayout) -> Option<Button> {
    let coloured = |h: f64, s: f64, v: f64| s > 0.35 && v > 0.35 && !TABLE_HUES.contains(&h);
    if share(frame, slot, coloured) < MIN_SLOT_COLOUR {
        return None;
    }
    let mut hues = Vec::new();
    for_each(frame, slot, |h, s, v| {
        if coloured(h, s, v) {
            hues.push(h);
        }
    });
    hues.sort_by(f64::total_cmp);
    let hue = *hues.get(hues.len() / 2)?;
    Button::ALL
        .into_iter()
        .map(|button| (button, hue_difference(hue, layout.hues.of(button))))
        .filter(|(_, difference)| *difference <= MAX_HUE_DIFFERENCE)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(button, _)| button)
}

/// `[left, top, right, bottom]` fractions of the game area as pixels inside the frame;
/// `None` when the box falls outside it.
fn pixels(frame: &FrameView<'_>, area: &GameArea, box_: [f32; 4]) -> Option<[u32; 4]> {
    let x = |f: f32| area.left + f * area.width;
    let y = |f: f32| area.top + f * area.height;
    let (left, top, right, bottom) = (x(box_[0]), y(box_[1]), x(box_[2]), y(box_[3]));
    let inside = left >= 0.0
        && top >= 0.0
        && right <= frame.width() as f32
        && bottom <= frame.height() as f32
        && right > left
        && bottom > top;
    inside.then_some([left as u32, top as u32, right as u32, bottom as u32])
}

/// Share of the sampled pixels in `box_` for which `test(hue, saturation, value)` holds.
fn share(frame: &FrameView<'_>, box_: [u32; 4], test: impl Fn(f64, f64, f64) -> bool) -> f64 {
    let (mut yes, mut all) = (0u32, 0u32);
    for_each(frame, box_, |h, s, v| {
        all += 1;
        yes += u32::from(test(h, s, v));
    });
    if all == 0 {
        0.0
    } else {
        f64::from(yes) / f64::from(all)
    }
}

fn for_each(
    frame: &FrameView<'_>,
    [left, top, right, bottom]: [u32; 4],
    mut f: impl FnMut(f64, f64, f64),
) {
    for y in (top..bottom).step_by(STEP) {
        for x in (left..right).step_by(STEP) {
            let (h, s, v) = hsv(frame.rgb(x, y));
            f(h, s, v);
        }
    }
}

/// Hue in degrees, saturation and value in 0-1.
fn hsv([r, g, b]: [u8; 3]) -> (f64, f64, f64) {
    let (r, g, b) = (
        f64::from(r) / 255.0,
        f64::from(g) / 255.0,
        f64::from(b) / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let saturation = if max == 0.0 { 0.0 } else { delta / max };
    (hue, saturation, max)
}

/// Difference between two hues in degrees, round the circle.
fn hue_difference(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::tests::bgra;
    use crate::profile::ButtonHues;

    const TABLE: [u8; 3] = [54, 73, 115];
    const SKIP: [u8; 3] = [150, 150, 146];
    const CHI: [u8; 3] = [70, 150, 110];
    const PON: [u8; 3] = [69, 153, 202];
    const KAN: [u8; 3] = [163, 80, 161];
    const RON: [u8; 3] = [190, 50, 45];
    const RIICHI: [u8; 3] = [200, 110, 50];
    const TSUMO: [u8; 3] = [190, 45, 85];

    fn layout() -> ButtonLayout {
        ButtonLayout {
            skip: [0.63, 0.74, 0.71, 0.80],
            first: [0.485, 0.74, 0.57, 0.80],
            pitch: 0.1537,
            slots: 3,
            hues: ButtonHues {
                chi: 157.0,
                pon: 203.0,
                kan: 305.0,
                ron: 3.0,
                riichi: 24.0,
                tsumo: 342.0,
            },
        }
    }

    fn area() -> GameArea {
        GameArea {
            left: 0.0,
            top: 0.0,
            width: 1000.0,
            height: 500.0,
        }
    }

    /// A table with the skip button if `skip`, and `buttons` in the slots from the right.
    fn frame(skip: bool, buttons: &[[u8; 3]]) -> Vec<u8> {
        let layout = layout();
        bgra(1000, 500, |x, y| {
            let (fx, fy) = (x as f32 / 1000.0, y as f32 / 500.0);
            if !(0.73..0.81).contains(&fy) {
                return TABLE;
            }
            if skip && (0.62..0.72).contains(&fx) {
                return SKIP;
            }
            for (k, colour) in buttons.iter().enumerate() {
                let shift = k as f32 * layout.pitch;
                if (layout.first[0] - shift - 0.01..layout.first[2] - shift + 0.01).contains(&fx) {
                    return *colour;
                }
            }
            TABLE
        })
    }

    fn read_frame(data: &[u8]) -> Vec<Button> {
        let view = FrameView::new(1000, 500, 4000, data, 1.0).unwrap();
        read(&view, &area(), &layout())
    }

    #[test]
    fn reads_each_button_by_its_colour() {
        let cases = [
            (CHI, Button::Chi),
            (PON, Button::Pon),
            (KAN, Button::Kan),
            (RON, Button::Ron),
            (RIICHI, Button::Riichi),
            (TSUMO, Button::Tsumo),
        ];
        for (colour, button) in cases {
            assert_eq!(read_frame(&frame(true, &[colour])), [button], "{button:?}");
        }
    }

    #[test]
    fn reads_two_buttons_left_to_right() {
        // The slot next to skip holds the first; the second is to its left.
        assert_eq!(
            read_frame(&frame(true, &[KAN, PON])),
            [Button::Pon, Button::Kan]
        );
        assert_eq!(
            read_frame(&frame(true, &[RON, CHI])),
            [Button::Chi, Button::Ron]
        );
    }

    #[test]
    fn colour_without_the_skip_button_is_not_buttons() {
        // A cut-in or a win effect.
        assert!(read_frame(&frame(false, &[TSUMO, KAN])).is_empty());
        assert!(read_frame(&frame(false, &[])).is_empty());
    }

    #[test]
    fn an_unknown_colour_is_not_read_as_a_button() {
        assert!(read_frame(&frame(true, &[[150, 60, 200]])).is_empty());
    }

    #[test]
    fn hues_wrap_round() {
        assert_eq!(hue_difference(355.0, 3.0), 8.0);
        assert_eq!(hsv([255, 0, 0]).0, 0.0);
        assert_eq!(hsv([0, 0, 255]).0, 240.0);
    }
}
