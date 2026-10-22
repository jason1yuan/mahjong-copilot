//! The 3D table (#8). Rivers and melds lie flat on it, so one perspective transform maps the
//! table to the screen, and a profile only says where four table points appear
//! ([`TableGeometry`]). Table coordinates are in tile widths from the table centre, with the
//! player's own seat towards +y. Each seat's areas are the own seat's turned by quarter
//! turns, so they are described once, in seat coordinates, for all four seats.
//!
//! [`Table::rectify`] resamples part of a seat's area into an upright picture as seen from
//! that seat: tiles come out as same-sized upright rectangles, whichever seat they are in.

use image::{Rgb, RgbImage};

use crate::capture::FrameView;
use crate::hand::GameArea;
use crate::profile::TableGeometry;

/// The table points whose positions a profile gives, in [`TableGeometry::corners`] order.
pub const CORNERS: [[f64; 2]; 4] = [[-6.0, -6.0], [6.0, -6.0], [6.0, 6.0], [-6.0, 6.0]];

/// A seat relative to the player, in turn order (mjai's relative seats).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Seat {
    Own,
    /// Shimocha, who plays next: on the right.
    Right,
    /// Toimen, across the table.
    Across,
    /// Kamicha, who played before: on the left.
    Left,
}

impl Seat {
    pub const ALL: [Seat; 4] = [Seat::Own, Seat::Right, Seat::Across, Seat::Left];

    /// Position in turn order from the player, 0-3.
    pub fn index(self) -> usize {
        self as usize
    }

    /// Seat coordinates to table coordinates: quarter turns that take the own seat (+y) to
    /// this one.
    pub fn to_table(self, x: f64, y: f64) -> (f64, f64) {
        let (mut x, mut y) = (x, y);
        for _ in 0..self.index() {
            (x, y) = (y, -x);
        }
        (x, y)
    }
}

/// A plane-to-plane perspective transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography([f64; 9]);

impl Homography {
    /// The transform taking each `from` point to the matching `to` point; `None` when three
    /// of the points lie on a line.
    pub fn from_points(from: [[f64; 2]; 4], to: [[f64; 2]; 4]) -> Option<Self> {
        let mut equations = [[0.0; 9]; 8];
        for (i, ([x, y], [u, v])) in from.into_iter().zip(to).enumerate() {
            equations[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -x * u, -y * u, u];
            equations[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -x * v, -y * v, v];
        }
        let h = solve(equations)?;
        Some(Self([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0]))
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.0;
        let w = m[6] * x + m[7] * y + m[8];
        (
            (m[0] * x + m[1] * y + m[2]) / w,
            (m[3] * x + m[4] * y + m[5]) / w,
        )
    }

    /// This transform applied after a quarter-turn rotation `(x, y) -> (y, -x)`.
    fn after_quarter_turn(&self) -> Self {
        let m = &self.0;
        Self([-m[1], m[0], m[2], -m[4], m[3], m[5], -m[7], m[6], m[8]])
    }
}

/// Solves 8 linear equations, each row `[a0..a7, b]`, by Gaussian elimination with partial
/// pivoting; `None` when they have no single solution.
fn solve(mut rows: [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let pivot = (col..8).max_by(|&i, &j| rows[i][col].abs().total_cmp(&rows[j][col].abs()))?;
        if rows[pivot][col].abs() < 1e-12 {
            return None;
        }
        rows.swap(col, pivot);
        let pivot_row = rows[col];
        for (r, row) in rows.iter_mut().enumerate() {
            if r == col {
                continue;
            }
            let f = row[col] / pivot_row[col];
            for (x, p) in row.iter_mut().zip(pivot_row).skip(col) {
                *x -= f * p;
            }
        }
    }
    Some(std::array::from_fn(|i| rows[i][8] / rows[i][i]))
}

/// Where the table is in one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Table {
    /// Table coordinates to frame pixels, one per seat: seat coordinates to frame pixels.
    seats: [Homography; 4],
}

impl Table {
    /// The table in a frame whose game area is `area`; `None` if the profile's corners are
    /// degenerate.
    pub fn new(geometry: &TableGeometry, area: &GameArea) -> Option<Self> {
        let to = geometry.corners.map(|[x, y]| {
            [
                f64::from(area.left + x * area.width),
                f64::from(area.top + y * area.height),
            ]
        });
        let own = Homography::from_points(CORNERS, to)?;
        let right = own.after_quarter_turn();
        let across = right.after_quarter_turn();
        let left = across.after_quarter_turn();
        Some(Self {
            seats: [own, right, across, left],
        })
    }

    /// Frame position in pixels of a point in `seat`'s coordinates.
    pub fn to_frame(&self, seat: Seat, x: f64, y: f64) -> (f64, f64) {
        self.seats[seat.index()].apply(x, y)
    }

    /// Frame pixels per tile width around a point of `seat`'s area: the geometric mean over
    /// both directions, which perspective shortens differently.
    pub fn scale_at(&self, seat: Seat, x: f64, y: f64) -> f64 {
        let span = |(ax, ay): (f64, f64), (bx, by): (f64, f64)| (ax - bx).hypot(ay - by);
        let across = span(
            self.to_frame(seat, x + 0.5, y),
            self.to_frame(seat, x - 0.5, y),
        );
        let along = span(
            self.to_frame(seat, x, y + 0.5),
            self.to_frame(seat, x, y - 0.5),
        );
        (across * along).sqrt()
    }

    /// The rectangle of `seat`'s coordinates from `origin`, `size` tile widths large, as seen
    /// from that seat at `scale` pixels per tile width. Outside the frame is black.
    pub fn rectify(
        &self,
        frame: &FrameView<'_>,
        seat: Seat,
        origin: [f64; 2],
        size: [f64; 2],
        scale: f64,
    ) -> RgbImage {
        let to_frame = &self.seats[seat.index()];
        let width = (size[0] * scale).round().max(1.0) as u32;
        let height = (size[1] * scale).round().max(1.0) as u32;
        RgbImage::from_fn(width, height, |i, j| {
            let x = origin[0] + (f64::from(i) + 0.5) / scale;
            let y = origin[1] + (f64::from(j) + 0.5) / scale;
            let (u, v) = to_frame.apply(x, y);
            sample(frame, u, v)
        })
    }
}

/// Bilinear sample at frame position (`u`, `v`) in pixels, pixel centres at +0.5; black
/// outside the frame.
fn sample(frame: &FrameView<'_>, u: f64, v: f64) -> Rgb<u8> {
    let (x, y) = (u - 0.5, v - 0.5);
    let inside = x >= 0.0
        && y >= 0.0
        && x <= f64::from(frame.width()) - 1.0
        && y <= f64::from(frame.height()) - 1.0;
    if !inside {
        return Rgb([0, 0, 0]);
    }
    let (fx, fy) = (x.fract(), y.fract());
    let (x0, y0) = (x as u32, y as u32);
    let x1 = (x0 + 1).min(frame.width() - 1);
    let y1 = (y0 + 1).min(frame.height() - 1);
    let p = [
        frame.rgb(x0, y0),
        frame.rgb(x1, y0),
        frame.rgb(x0, y1),
        frame.rgb(x1, y1),
    ];
    Rgb(std::array::from_fn(|c| {
        let top = f64::from(p[0][c]) * (1.0 - fx) + f64::from(p[1][c]) * fx;
        let bottom = f64::from(p[2][c]) * (1.0 - fx) + f64::from(p[3][c]) * fx;
        (top * (1.0 - fy) + bottom * fy).round() as u8
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    #[test]
    fn homography_maps_the_four_points() {
        let to = [[100.0, 50.0], [300.0, 60.0], [330.0, 260.0], [80.0, 240.0]];
        let h = Homography::from_points(CORNERS, to).unwrap();
        for (from, to) in CORNERS.iter().zip(to) {
            assert!(close(h.apply(from[0], from[1]), (to[0], to[1])));
        }
        // A perspective transform keeps straight lines straight: the centre is where the
        // diagonals cross.
        let (cx, cy) = h.apply(0.0, 0.0);
        let cross =
            |[ax, ay]: [f64; 2], [bx, by]: [f64; 2]| (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
        assert!(cross(to[0], to[2]).abs() < 1e-6);
        assert!(cross(to[1], to[3]).abs() < 1e-6);
    }

    #[test]
    fn collinear_points_have_no_homography() {
        let to = [[0.0, 0.0], [1.0, 1.0], [2.0, 2.0], [3.0, 3.0]];
        assert!(Homography::from_points(CORNERS, to).is_none());
    }

    #[test]
    fn seats_turn_the_own_seat_around_the_table() {
        // The own seat's area towards +y is on the right for the next player, across for the
        // one after, on the left for the one before.
        assert!(close(Seat::Own.to_table(1.0, 4.0), (1.0, 4.0)));
        assert!(close(Seat::Right.to_table(1.0, 4.0), (4.0, -1.0)));
        assert!(close(Seat::Across.to_table(1.0, 4.0), (-1.0, -4.0)));
        assert!(close(Seat::Left.to_table(1.0, 4.0), (-4.0, 1.0)));
    }

    #[test]
    fn each_seat_has_its_own_transform() {
        let area = GameArea {
            left: 10.0,
            top: 20.0,
            width: 1000.0,
            height: 562.5,
        };
        let geometry = TableGeometry {
            corners: [[0.34, 0.18], [0.66, 0.18], [0.70, 0.62], [0.29, 0.63]],
        };
        let table = Table::new(&geometry, &area).unwrap();
        let own = Homography::from_points(
            CORNERS,
            geometry
                .corners
                .map(|[x, y]| [f64::from(10.0 + x * 1000.0), f64::from(20.0 + y * 562.5)]),
        )
        .unwrap();
        for seat in Seat::ALL {
            let (tx, ty) = seat.to_table(1.5, 4.0);
            assert!(close(table.to_frame(seat, 1.5, 4.0), own.apply(tx, ty)));
        }
    }
}
