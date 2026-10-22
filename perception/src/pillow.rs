//! Two Pillow operations ported bit for bit, so Rust sees exactly the pixels the Python
//! evaluation saw: the 8-bit RGB to HSV conversion (red-five rule) and the bilinear resize
//! (the classifier's preprocessing). `ml/convert/pillow_fixtures.py` writes the fixtures
//! the tests compare against.
//!
//! Ported from Pillow 13.0.0 (`src/libImaging/Convert.c`, `src/libImaging/Resample.c`).
//! Copyright © 1997-2011 by Secret Labs AB, © 1995-2011 by Fredrik Lundh and
//! contributors, © 2010 by Jeffrey 'Alex' Clark and contributors; MIT-CMU License, see
//! THIRD_PARTY_NOTICES.md.

use image::{Rgb, RgbImage};

/// Pillow's RGB to HSV conversion (`rgb2hsv_row` in libImaging/Convert.c), each channel
/// 0-255. The C code keeps `h` and `s` in `float` but computes with `double` literals, so
/// the casts below follow it step by step.
pub(crate) fn hsv([r, g, b]: [u8; 3]) -> [u8; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max == min {
        return [0, 0, max];
    }
    let range = f32::from(max - min);
    let s = range / f32::from(max);
    let rc = f32::from(max - r) / range;
    let gc = f32::from(max - g) / range;
    let bc = f32::from(max - b) / range;
    let h = if r == max {
        f64::from(bc) - f64::from(gc)
    } else if g == max {
        2.0 + f64::from(rc) - f64::from(bc)
    } else {
        4.0 + f64::from(gc) - f64::from(rc)
    } as f32;
    let h = ((f64::from(h) / 6.0 + 1.0) % 1.0) as f32;
    // Truncation as in C's (int) cast; both values are in [0, 255].
    [
        (f64::from(h) * 255.0) as u8,
        (f64::from(s) * 255.0) as u8,
        max,
    ]
}

/// Fixed-point precision of Pillow's 8-bit resampling.
const PRECISION_BITS: u32 = 32 - 8 - 2;

/// Resampling weights for one axis: for each output position, the first input index, how
/// many inputs it reads, and their fixed-point weights.
struct Weights {
    bounds: Vec<(usize, usize)>,
    ksize: usize,
    weights: Vec<i64>,
}

/// `precompute_coeffs` and `normalize_coeffs_8bpc` in libImaging/Resample.c, for the
/// bilinear filter over the whole input.
fn weights(in_size: u32, out_size: u32) -> Weights {
    let scale = f64::from(in_size) / f64::from(out_size);
    let filterscale = scale.max(1.0);
    let support = filterscale; // the bilinear filter's support is 1
    let ss = 1.0 / filterscale;
    let ksize = support.ceil() as usize * 2 + 1;
    let mut bounds = Vec::with_capacity(out_size as usize);
    let mut weights = vec![0; out_size as usize * ksize];
    let mut k = vec![0.0f64; ksize];
    for (xx, row) in weights.chunks_exact_mut(ksize).enumerate() {
        let center = (xx as f64 + 0.5) * scale;
        // C's (int) cast truncates toward zero.
        let xmin = ((center - support + 0.5) as i64).max(0);
        let xmax = ((center + support + 0.5) as i64).min(i64::from(in_size)) - xmin;
        let mut total = 0.0;
        for (x, w) in k.iter_mut().enumerate() {
            *w = if (x as i64) < xmax {
                let t = ((x as i64 + xmin) as f64 - center + 0.5) * ss;
                (1.0 - t.abs()).max(0.0)
            } else {
                0.0
            };
            total += *w;
        }
        for (w, out) in k.iter().zip(row.iter_mut()) {
            let w = if total == 0.0 { *w } else { w / total };
            let fixed = w * f64::from(1u32 << PRECISION_BITS);
            *out = if w < 0.0 {
                (fixed - 0.5) as i64
            } else {
                (fixed + 0.5) as i64
            };
        }
        bounds.push((xmin as usize, xmax.max(0) as usize));
    }
    Weights {
        bounds,
        ksize,
        weights,
    }
}

/// `clip8` in libImaging/Resample.c.
fn clip8(value: i64) -> u8 {
    if value >= 1 << PRECISION_BITS << 8 {
        255
    } else if value <= 0 {
        0
    } else {
        (value >> PRECISION_BITS) as u8
    }
}

/// Weighted sum of up to `ksize` 8-bit samples, rounded like Pillow.
fn sample(weights: &[i64], samples: impl Iterator<Item = [u8; 3]>) -> [u8; 3] {
    let mut sums = [1i64 << (PRECISION_BITS - 1); 3];
    for (w, pixel) in weights.iter().zip(samples) {
        for (sum, value) in sums.iter_mut().zip(pixel) {
            *sum += i64::from(value) * w;
        }
    }
    sums.map(clip8)
}

/// Resamples rows `offset..offset + rows` of `image` to `width` columns.
fn horizontal_pass(
    image: &RgbImage,
    weights: &Weights,
    width: u32,
    offset: usize,
    rows: usize,
) -> RgbImage {
    let mut out = RgbImage::new(width, rows as u32);
    for (y, row) in out.rows_mut().enumerate() {
        for (xx, pixel) in row.enumerate() {
            let (xmin, xmax) = weights.bounds[xx];
            let w = &weights.weights[xx * weights.ksize..][..xmax];
            let samples =
                (xmin..xmin + xmax).map(|x| image.get_pixel(x as u32, (y + offset) as u32).0);
            *pixel = Rgb(sample(w, samples));
        }
    }
    out
}

/// Resamples the columns of `image` to `height` rows.
fn vertical_pass(image: &RgbImage, weights: &Weights, height: u32) -> RgbImage {
    let mut out = RgbImage::new(image.width(), height);
    for (yy, row) in out.rows_mut().enumerate() {
        let (ymin, ymax) = weights.bounds[yy];
        let w = &weights.weights[yy * weights.ksize..][..ymax];
        for (xx, pixel) in row.enumerate() {
            let samples = (ymin..ymin + ymax).map(|y| image.get_pixel(xx as u32, y as u32).0);
            *pixel = Rgb(sample(w, samples));
        }
    }
    out
}

/// `Image.resize((width, height), Image.BILINEAR)` (`ImagingResampleInner`): two passes,
/// each rounded to 8 bits, horizontal first unless the height shrinks more than twice as
/// much as the width.
pub(crate) fn resize_bilinear(image: &RgbImage, width: u32, height: u32) -> RgbImage {
    let (in_width, in_height) = image.dimensions();
    let need_horizontal = width != in_width;
    let need_vertical = height != in_height;
    let shrink_x = i64::from(in_width) - i64::from(width);
    let shrink_y = i64::from(in_height) - i64::from(height);
    let horizontal_first = !(shrink_y > 0 && shrink_y > shrink_x * 2);
    let horizontal = weights(in_width, width);
    let mut vertical = weights(in_height, height);

    if horizontal_first {
        let rows = if need_horizontal {
            // Only the rows the vertical pass reads, with its bounds shifted to match.
            let first = vertical.bounds.first().map_or(0, |b| b.0);
            let last = vertical.bounds.last().map_or(0, |b| b.0 + b.1);
            for bound in &mut vertical.bounds {
                bound.0 -= first;
            }
            Some(horizontal_pass(
                image,
                &horizontal,
                width,
                first,
                last - first,
            ))
        } else {
            None
        };
        let source = rows.as_ref().unwrap_or(image);
        if need_vertical {
            vertical_pass(source, &vertical, height)
        } else {
            source.clone()
        }
    } else {
        // Only reached when the height shrinks, so the vertical pass is always needed.
        let columns = vertical_pass(image, &vertical, height);
        if need_horizontal {
            horizontal_pass(&columns, &horizontal, width, 0, height as usize)
        } else {
            columns
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct HsvFixture {
        rows: Vec<[u8; 6]>,
    }

    #[derive(serde::Deserialize)]
    struct ResizeFixture {
        output: [u32; 2],
        cases: Vec<ResizeCase>,
    }

    #[derive(serde::Deserialize)]
    struct ResizeCase {
        width: u32,
        height: u32,
        fnv1a64: String,
    }

    #[test]
    fn hsv_matches_pillow() {
        let fixture: HsvFixture =
            serde_json::from_str(include_str!("../tests/fixtures/pil_hsv.json")).unwrap();
        assert!(fixture.rows.len() > 8000);
        for [r, g, b, h, s, v] in fixture.rows {
            assert_eq!(hsv([r, g, b]), [h, s, v], "rgb ({r}, {g}, {b})");
        }
    }

    fn pattern(width: u32, height: u32) -> RgbImage {
        RgbImage::from_fn(width, height, |x, y| {
            Rgb([0, 1, 2].map(|c| ((x * 31 + y * 17 + c * 73 + (x * y) % 13) % 256) as u8))
        })
    }

    fn fnv1a64(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }

    #[test]
    fn resize_matches_pillow() {
        let fixture: ResizeFixture =
            serde_json::from_str(include_str!("../tests/fixtures/pil_resize.json")).unwrap();
        let [width, height] = fixture.output;
        for case in fixture.cases {
            let resized = resize_bilinear(&pattern(case.width, case.height), width, height);
            assert_eq!(resized.dimensions(), (width, height));
            let hash = format!("{:016x}", fnv1a64(resized.as_raw()));
            assert_eq!(hash, case.fnv1a64, "{}x{}", case.width, case.height);
        }
    }
}
