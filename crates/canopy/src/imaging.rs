//! Pixel work on plain BGRA buffers: the glass behind arranged windows, the
//! colour a growing window shows past its picture, and an app's colour.

/// BGRA pixels, 4 bytes each, row after row.
#[derive(Clone, Debug, PartialEq)]
pub struct Pixels {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colour {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Colour {
    pub const fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }

    pub const fn grey(white: f32) -> Self {
        Self::new(white, white, white)
    }

    pub fn lerp(self, other: Colour, t: f32) -> Colour {
        Colour::new(
            self.r + (other.r - self.r) * t,
            self.g + (other.g - self.g) * t,
            self.b + (other.b - self.b) * t,
        )
    }

    pub fn to_rgba(self, a: f32) -> gpui::Rgba {
        gpui::Rgba { r: self.r, g: self.g, b: self.b, a }
    }
}

impl Pixels {
    fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.width + x) * 4;
        [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
    }

    /// The image shrunk by a whole `factor`, averaging each block of pixels.
    pub fn shrunk(&self, factor: usize) -> Pixels {
        let (w, h) = (self.width / factor, self.height / factor);
        let mut data = Vec::with_capacity(w * h * 4);
        let area = (factor * factor) as u32;
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0u32; 4];
                for row in 0..factor {
                    let start = ((y * factor + row) * self.width + x * factor) * 4;
                    for px in self.data[start..start + factor * 4].chunks_exact(4) {
                        for c in 0..4 {
                            sum[c] += u32::from(px[c]);
                        }
                    }
                }
                data.extend(sum.map(|s| (s / area) as u8));
            }
        }
        Pixels { width: w, height: h, data }
    }

    /// Blurred by about `sigma` pixels: three box blurs, which is close to a
    /// Gaussian and doesn't depend on the radius.
    pub fn blurred(&self, sigma: f64) -> Pixels {
        let radius = (((12.0 * sigma * sigma / 3.0 + 1.0).sqrt() - 1.0) / 2.0).round().max(1.0) as usize;
        let mut out = self.clone();
        let mut scratch = vec![0u8; out.data.len()];
        for _ in 0..3 {
            box_blur(&out.data, &mut scratch, out.width, out.height, radius, 4, out.width * 4);
            box_blur(&scratch, &mut out.data, out.height, out.width, radius, out.width * 4, 4);
        }
        out
    }

    /// Multiplies the colour channels, as if black covered it at `1 - amount`.
    pub fn darkened(mut self, amount: f64) -> Pixels {
        let keep = ((1.0 - amount) * 256.0) as u32;
        for px in self.data.chunks_exact_mut(4) {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * keep) >> 8) as u8;
            }
            px[3] = 255;
        }
        self
    }

    /// The average colour along the right and bottom edges: what a window
    /// shows when it grows past its picture.
    pub fn edge_colour(&self) -> Option<Colour> {
        if self.width == 0 || self.height == 0 {
            return None;
        }
        let (cols, rows) = ((self.width / 4).max(1), (self.height / 4).max(1));
        let (step_x, step_y) = ((self.width / 32).max(1), (self.height / 32).max(1));
        let mut sum = [0.0f64; 3];
        let mut n = 0.0;
        for y in (0..self.height).step_by(step_y) {
            for x in (0..self.width).step_by(step_x) {
                if x + cols >= self.width || y + rows >= self.height {
                    let p = self.pixel(x, y);
                    for c in 0..3 {
                        sum[c] += f64::from(p[c]);
                    }
                    n += 1.0;
                }
            }
        }
        (n > 0.0).then(|| {
            Colour::new((sum[2] / n / 255.0) as f32, (sum[1] / n / 255.0) as f32, (sum[0] / n / 255.0) as f32)
        })
    }

    /// The average opaque colour, a little more saturated: an app's colour
    /// from its icon.
    pub fn app_colour(&self) -> Option<Colour> {
        let mut sum = [0.0f64; 3];
        let mut n = 0.0;
        for px in self.data.chunks_exact(4) {
            if px[3] > 127 {
                // Premultiplied by a nearly opaque alpha: close enough to straight.
                for c in 0..3 {
                    sum[c] += f64::from(px[c]);
                }
                n += 1.0;
            }
        }
        if n == 0.0 {
            return None;
        }
        let (b, g, r) = (sum[0] / n / 255.0, sum[1] / n / 255.0, sum[2] / n / 255.0);
        let (h, s, v) = hsv(r, g, b);
        Some(from_hsv(h, (s * 1.3).min(1.0), v.clamp(0.35, 0.85)))
    }
}

/// One pass of a running-sum box blur over `lines` lines of `length` pixels,
/// clamping at the ends. `along` is the byte step along a line and `across`
/// the step between lines, so the same pass does rows and columns.
fn box_blur(src: &[u8], dst: &mut [u8], length: usize, lines: usize, radius: usize, along: usize, across: usize) {
    let window = (2 * radius + 1) as u32;
    let last = length - 1;
    for line in 0..lines {
        let base = line * across;
        for c in 0..4 {
            let at = |i: usize| u32::from(src[base + i.min(last) * along + c]);
            let mut sum = at(0) * (radius as u32 + 1);
            for i in 1..=radius {
                sum += at(i);
            }
            for i in 0..length {
                dst[base + i * along + c] = (sum / window) as u8;
                sum += at(i + radius + 1);
                sum -= at(i.saturating_sub(radius));
            }
        }
    }
}

fn hsv(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let h = if delta == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    (h, if max > 0.0 { delta / max } else { 0.0 }, max)
}

fn from_hsv(h: f64, s: f64, v: f64) -> Colour {
    let sector = (h * 6.0).floor();
    let f = h * 6.0 - sector;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    let (r, g, b) = match sector as i64 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    Colour::new(r as f32, g as f32, b as f32)
}

/// What a window without a picture shows: mostly grey, with a quarter of its
/// app's colour.
pub fn placeholder(app: Colour) -> Colour {
    Colour::grey(0.3).lerp(app, 0.25)
}

/// CSS-style easing: `cubic-bezier(x1, y1, x2, y2)` at time `t`.
pub fn ease(t: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let curve = |a: f64, b: f64, s: f64| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    // Solve x(s) = t by bisection; it is monotonic for these curves.
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if curve(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    curve(y1, y2, (lo + hi) / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: usize, height: usize, bgra: [u8; 4]) -> Pixels {
        Pixels { width, height, data: bgra.repeat(width * height) }
    }

    #[test]
    fn a_blur_leaves_a_flat_image_flat() {
        let image = solid(40, 30, [10, 20, 30, 255]);
        assert_eq!(image.blurred(10.0), image);
    }

    #[test]
    fn a_blur_spreads_a_sharp_edge_without_changing_the_size() {
        let mut image = solid(64, 8, [0, 0, 0, 255]);
        for y in 0..8 {
            for x in 32..64 {
                image.data[(y * 64 + x) * 4 + 2] = 240;
            }
        }
        let b = image.blurred(4.0);
        assert_eq!((b.width, b.height), (64, 8));
        let red = |x: usize| b.pixel(x, 4)[2];
        assert!(red(0) < 5 && red(63) > 235, "the far ends stay put");
        assert!(red(30) > 40 && red(34) < 200, "the edge is soft");
        assert!((0..63).all(|x| red(x) <= red(x + 1)), "and it stays a ramp");
    }

    #[test]
    fn shrinking_averages_each_block() {
        let image = Pixels { width: 2, height: 2, data: vec![0, 0, 0, 255, 100, 0, 0, 255, 0, 100, 0, 255, 100, 100, 0, 255] };
        assert_eq!(image.shrunk(2).data, vec![50, 50, 0, 255]);
    }

    #[test]
    fn darkening_keeps_seventy_percent_and_makes_it_opaque() {
        let d = solid(1, 1, [100, 200, 40, 128]).darkened(0.3);
        assert_eq!(d.data, vec![69, 139, 27, 255]);
    }

    #[test]
    fn the_edge_colour_reads_the_right_and_bottom_of_the_picture() {
        let mut image = solid(64, 64, [0, 0, 255, 255]); // red body
        for y in 0..64 {
            for x in 0..64 {
                if x >= 48 || y >= 48 {
                    image.data[(y * 64 + x) * 4..(y * 64 + x) * 4 + 3].copy_from_slice(&[255, 0, 0]); // blue edges
                }
            }
        }
        let c = image.edge_colour().unwrap();
        assert!(c.b > 0.95 && c.r < 0.05, "{c:?}");
    }

    #[test]
    fn an_apps_colour_ignores_transparent_pixels_and_saturates_a_little() {
        let mut image = solid(4, 4, [0, 0, 0, 0]);
        image.data[..4].copy_from_slice(&[40, 80, 200, 255]);
        let c = image.app_colour().unwrap();
        assert!(c.r > c.g && c.g > c.b);
        assert!((c.r - 200.0 / 255.0).abs() < 0.01);
        assert!(solid(2, 2, [1, 2, 3, 0]).app_colour().is_none());
    }

    #[test]
    fn the_motion_curve_starts_and_ends_in_place_and_eases_out() {
        let at = |t| ease(t, 0.25, 0.1, 0.1, 1.0);
        assert!(at(0.0).abs() < 1e-3 && (at(1.0) - 1.0).abs() < 1e-3);
        assert!(at(0.5) > 0.8, "most of the distance is covered early");
        assert!((0..100).all(|i| at(f64::from(i) / 100.0) <= at(f64::from(i + 1) / 100.0)));
    }
}
