//! Change detection and stable-frame gating (#1): which regions of the window changed, and
//! when a change has settled long enough to be worth recognizing.
//!
//! Each region is reduced to a small grid of average brightness. A region is "moving"
//! while its grid keeps changing from frame to frame; once it has not changed for
//! [`SETTLE`] (about two frames at 30 fps) it settles, and is reported if it now differs
//! from what was last reported. So animations (a tile flying in, call effects, character
//! art) are skipped, and a change that undoes itself (a hovered tile dropping back) is not
//! reported at all.
//!
//! A change is judged by the grid cell that changed most, not the average, so one new tile
//! in a river of twenty still counts. On macOS, window capture delivers frames at the full
//! rate even when nothing moves, so [`FrameGate`] drops unchanged frames right on the
//! capture thread before anything is copied.

use std::time::{Duration, Instant};

use crate::capture::{FrameView, RelRect};

/// How long a region must stay unchanged before it counts as settled.
pub const SETTLE: Duration = Duration::from_millis(70);
/// A region changed when the brightness of some grid cell (an average of its samples,
/// 0-255) moved by more than this.
pub const THRESHOLD: u8 = 6;
/// Brightness samples per grid cell along each axis.
const SAMPLES: u32 = 3;

/// A region to watch and the grid it is reduced to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub rect: RelRect,
    pub cols: u32,
    pub rows: u32,
}

struct Tracked {
    region: Region,
    current: Vec<u8>,
    scratch: Vec<u8>,
    reported: Option<Vec<u8>>,
    /// When the region last differed from the frame before; `None` once handled.
    changed_at: Option<Instant>,
}

pub struct ChangeDetector {
    regions: Vec<Tracked>,
}

impl ChangeDetector {
    pub fn new(regions: &[Region]) -> Self {
        Self {
            regions: regions
                .iter()
                .map(|&region| Tracked {
                    region,
                    current: Vec::new(),
                    scratch: Vec::new(),
                    reported: None,
                    changed_at: None,
                })
                .collect(),
        }
    }

    /// Feeds a new frame of the window.
    pub fn observe(&mut self, frame: &FrameView<'_>, now: Instant) {
        for tracked in &mut self.regions {
            grid(frame, tracked.region, &mut tracked.scratch);
            let changed = differs(&tracked.current, &tracked.scratch);
            std::mem::swap(&mut tracked.current, &mut tracked.scratch);
            if changed {
                tracked.changed_at = Some(now);
            }
        }
    }

    /// Indices of the regions that settled since the last call with content that differs
    /// from what they last reported (always true the first time).
    pub fn settled(&mut self, now: Instant) -> Vec<usize> {
        let mut settled = Vec::new();
        for (i, tracked) in self.regions.iter_mut().enumerate() {
            let Some(changed_at) = tracked.changed_at else {
                continue;
            };
            if now.duration_since(changed_at) < SETTLE {
                continue;
            }
            tracked.changed_at = None;
            let new = match &tracked.reported {
                Some(old) => differs(old, &tracked.current),
                None => true,
            };
            if new {
                tracked.reported = Some(tracked.current.clone());
                settled.push(i);
            }
        }
        settled
    }

    /// When to call [`settled`](Self::settled) next if no frame arrives: `None` when no
    /// region is waiting to settle, so the caller can just wait for the next frame.
    pub fn next_check(&self) -> Option<Instant> {
        self.regions
            .iter()
            .filter_map(|t| t.changed_at)
            .min()
            .map(|t| t + SETTLE)
    }
}

/// Mean brightness of each grid cell, sampled at `SAMPLES` x `SAMPLES` points.
fn grid(frame: &FrameView<'_>, region: Region, out: &mut Vec<u8>) {
    out.clear();
    let [left, top, right, bottom] = frame.pixel_rect(region.rect);
    let (width, height) = (right - left, bottom - top);
    if width == 0 || height == 0 {
        return;
    }
    for row in 0..region.rows {
        for col in 0..region.cols {
            let mut total = 0u32;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    // Evenly spread sample points inside the cell.
                    let fx = (col * SAMPLES + sx) * 2 + 1;
                    let fy = (row * SAMPLES + sy) * 2 + 1;
                    let x = left
                        + (u64::from(fx) * u64::from(width) / u64::from(region.cols * SAMPLES * 2))
                            as u32;
                    let y = top
                        + (u64::from(fy) * u64::from(height) / u64::from(region.rows * SAMPLES * 2))
                            as u32;
                    total += u32::from(frame.luma(x.min(right - 1), y.min(bottom - 1)));
                }
            }
            out.push((total / (SAMPLES * SAMPLES)) as u8);
        }
    }
}

/// Whether two grids differ: a different size, or some cell beyond [`THRESHOLD`].
fn differs(a: &[u8], b: &[u8]) -> bool {
    a.len() != b.len() || a.iter().zip(b).any(|(p, q)| p.abs_diff(*q) > THRESHOLD)
}

/// Grid of the whole frame for [`FrameGate`]: cells of about 1.6 % x 2.8 % of the window,
/// smaller than a river tile.
const GATE_GRID: Region = Region {
    rect: RelRect::FULL,
    cols: 64,
    rows: 36,
};

/// Tells whether a frame differs from the one before, cheaply enough to run on every
/// captured frame (about 20 000 samples, whatever the window size).
#[derive(Default)]
pub struct FrameGate {
    size: (u32, u32),
    last: Vec<u8>,
    scratch: Vec<u8>,
}

impl FrameGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// True for the first frame, after a resize, and when anything visibly changed.
    pub fn changed(&mut self, frame: &FrameView<'_>) -> bool {
        grid(frame, GATE_GRID, &mut self.scratch);
        let size = (frame.width(), frame.height());
        let changed = size != self.size || differs(&self.last, &self.scratch);
        self.size = size;
        std::mem::swap(&mut self.last, &mut self.scratch);
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::tests::bgra;

    const W: u32 = 160;
    const H: u32 = 90;

    /// A grey frame with a white square at (`x`, 10); `None` for no square.
    fn frame_with(square: Option<u32>) -> Vec<u8> {
        bgra(W, H, |x, y| match square {
            Some(sx) if (sx..sx + 20).contains(&x) && (10..30).contains(&y) => [250, 250, 250],
            _ => [60, 60, 60],
        })
    }

    fn view(data: &[u8]) -> FrameView<'_> {
        FrameView::new(W, H, W as usize * 4, data, 1.0).unwrap()
    }

    fn detector() -> ChangeDetector {
        ChangeDetector::new(&[
            Region {
                rect: RelRect::new(0.0, 0.0, 1.0, 0.5),
                cols: 16,
                rows: 4,
            },
            Region {
                rect: RelRect::new(0.0, 0.5, 1.0, 0.5),
                cols: 16,
                rows: 4,
            },
        ])
    }

    #[test]
    fn reports_every_region_once_at_the_start() {
        let mut d = detector();
        let t = Instant::now();
        let first = frame_with(Some(10));
        d.observe(&view(&first), t);
        assert!(d.settled(t + SETTLE / 2).is_empty(), "too early");
        assert_eq!(d.next_check(), Some(t + SETTLE));
        assert_eq!(d.settled(t + SETTLE), [0, 1]);
        assert_eq!(
            d.next_check(),
            None,
            "nothing waiting: just wait for frames"
        );
        assert!(d.settled(t + SETTLE * 10).is_empty());
    }

    #[test]
    fn reports_a_change_only_after_it_stops_moving() {
        let mut d = detector();
        let t = Instant::now();
        let start = frame_with(Some(10));
        d.observe(&view(&start), t);
        d.settled(t + SETTLE);
        // An animation: the square moves every frame (33 ms apart), then rests.
        let mut now = t + SETTLE;
        for x in [30, 50, 70, 90] {
            now += Duration::from_millis(33);
            let moving = frame_with(Some(x));
            d.observe(&view(&moving), now);
            assert!(d.settled(now).is_empty(), "still moving at x = {x}");
        }
        let rest = frame_with(Some(90));
        d.observe(&view(&rest), now + Duration::from_millis(33));
        assert_eq!(d.settled(now + SETTLE), [0], "only the top region changed");
    }

    #[test]
    fn a_change_that_undoes_itself_is_not_reported() {
        let mut d = detector();
        let t = Instant::now();
        let start = frame_with(Some(10));
        d.observe(&view(&start), t);
        d.settled(t + SETTLE);
        let raised = frame_with(None);
        d.observe(&view(&raised), t + SETTLE * 2);
        d.observe(&view(&start), t + SETTLE * 2 + Duration::from_millis(33));
        assert!(d.settled(t + SETTLE * 4).is_empty());
        assert_eq!(d.next_check(), None);
    }

    #[test]
    fn one_small_change_in_a_large_region_counts() {
        let mut d = ChangeDetector::new(&[Region {
            rect: RelRect::FULL,
            cols: 32,
            rows: 18,
        }]);
        let t = Instant::now();
        let start = frame_with(None);
        d.observe(&view(&start), t);
        d.settled(t + SETTLE);
        // A 6x6 dot in a 160x90 frame: 0.25 % of it.
        let dot = bgra(W, H, |x, y| {
            if (70..76).contains(&x) && (40..46).contains(&y) {
                [250, 250, 250]
            } else {
                [60, 60, 60]
            }
        });
        d.observe(&view(&dot), t + SETTLE * 2);
        assert_eq!(d.settled(t + SETTLE * 3), [0]);
    }

    #[test]
    fn the_gate_passes_only_changed_frames() {
        let mut gate = FrameGate::new();
        let a = frame_with(Some(10));
        let b = frame_with(Some(40));
        assert!(gate.changed(&view(&a)), "first frame");
        assert!(!gate.changed(&view(&a)));
        assert!(gate.changed(&view(&b)));
        assert!(!gate.changed(&view(&b)));
        let wider = bgra(W * 2, H, |_, _| [60, 60, 60]);
        assert!(
            gate.changed(&FrameView::new(W * 2, H, W as usize * 8, &wider, 1.0).unwrap()),
            "resized"
        );
    }

    #[test]
    fn small_noise_is_not_a_change() {
        let mut d = detector();
        let t = Instant::now();
        let start = frame_with(Some(10));
        d.observe(&view(&start), t);
        d.settled(t + SETTLE);
        let noisy = bgra(W, H, |x, y| {
            let base = if (10..30).contains(&x) && (10..30).contains(&y) {
                250
            } else {
                60
            };
            [base, base + u8::from((x + y) % 2 == 0), base]
        });
        d.observe(&view(&noisy), t + SETTLE * 2);
        assert_eq!(d.next_check(), None);
    }
}
