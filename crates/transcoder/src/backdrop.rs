//! A backdrop's four corner colors, drawn from an image, and the gradient they make.
//!
//! What Plex calls `UltraBlur`. The colors are the reference's, reproduced from its binary: each
//! quarter of the image gets a palette by median cut ([`palette`]), and the color chosen is
//! the one scoring best on lightness, saturation, its place in the palette and its distance
//! from the corner chosen before it, in Okhsl ([`okhsl`]); it is then darkened to a ceiling and
//! pushed out of the muddy middle saturations. The gradient blends the four over black, each
//! by its distance to its corner. `docs/plex/spec-gaps.md` has the measurements.
//!
//! All of this is CPU work; the caller runs it off the async workers.

mod okhsl;
mod palette;

use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::imageops::FilterType;
use image::{GenericImageView, ImageEncoder, RgbImage};

use self::okhsl::{Hsl, Lab};
use crate::TranscodeError;

/// An sRGB color, 8 bits a channel.
pub type Rgb = [u8; 3];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CornerColors {
    pub top_left: Rgb,
    pub top_right: Rgb,
    pub bottom_right: Rgb,
    pub bottom_left: Rgb,
}

/// The pixels a backdrop may have: 3840×2160, in any shape. A source larger than that is
/// scaled down to it before its colors are drawn.
pub const MAX_PIXELS: u64 = 3840 * 2160;

/// The top-left corner is darker than the others, where a title sits.
const TOP_LEFT_LIGHTNESS: f32 = 0.2;
const LIGHTNESS: f32 = 0.4;

/// The four colors of an encoded image.
///
/// # Errors
///
/// [`TranscodeError::Undecodable`] for what is not an accepted image, and
/// [`TranscodeError::UnusableSize`] for one too small to quarter or so white that a quarter
/// has no color.
pub(crate) fn corner_colors(bytes: &[u8]) -> Result<CornerColors, TranscodeError> {
    let image = crate::convert::decode(bytes)?.pixels;
    let (width, height) = image.dimensions();
    let pixels = u64::from(width) * u64::from(height);
    let image = if pixels > MAX_PIXELS {
        let area = f64::from(width) * f64::from(height);
        #[allow(clippy::cast_precision_loss, reason = "8,294,400 is exact in an f64")]
        let scale = (MAX_PIXELS as f64 / area).sqrt();
        image.resize_exact(
            scaled(width, scale),
            scaled(height, scale),
            FilterType::Triangle,
        )
    } else {
        image
    }
    // Opaque: the reference decodes to three channels, which drops alpha without blending.
    .into_rgb8();

    let (half_width, half_height) = (image.width() / 2, image.height() / 2);
    if half_width == 0 || half_height == 0 {
        return Err(TranscodeError::UnusableSize);
    }
    let quarter = |x: u32, y: u32| -> Vec<Rgb> {
        let mut pixels = Vec::with_capacity((half_width * half_height) as usize);
        for row in y..y + half_height {
            for column in x..x + half_width {
                pixels.push(image.get_pixel(column, row).0);
            }
        }
        pixels
    };
    let chosen = |pixels: Vec<Rgb>, ceiling: f32, previous: Option<&Candidate>| {
        let palette = palette::palette(&pixels).ok_or(TranscodeError::UnusableSize)?;
        Ok::<_, TranscodeError>(choose(&palette, ceiling, previous))
    };

    let top_left = chosen(quarter(0, 0), TOP_LEFT_LIGHTNESS, None)?;
    let top_right = chosen(quarter(half_width, 0), LIGHTNESS, Some(&top_left))?;
    let bottom_left = chosen(quarter(0, half_height), LIGHTNESS, Some(&top_left))?;
    let bottom_right = chosen(
        quarter(half_width, half_height),
        LIGHTNESS,
        Some(&bottom_left),
    )?;
    Ok(CornerColors {
        top_left: top_left.output(),
        top_right: top_right.output(),
        bottom_right: bottom_right.output(),
        bottom_left: bottom_left.output(),
    })
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a scale below one of a side that fits in a u32"
)]
fn scaled(side: u32, scale: f64) -> u32 {
    ((f64::from(side) * scale) as u32).max(1)
}

/// A palette color, with the forms it is judged in.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    hsl: Hsl,
    /// Oklab of the *encoded* values, which the reference feeds it, not of linear light. It
    /// only measures how far apart two corners are.
    lab: Lab,
}

impl Candidate {
    fn new(color: Rgb) -> Self {
        let srgb = color.map(|channel| f32::from(channel) / 255.0);
        Self {
            hsl: okhsl::srgb_to_okhsl(srgb),
            lab: okhsl::linear_srgb_to_oklab(srgb),
        }
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the unit range first, then truncated as the reference does"
    )]
    fn output(&self) -> Rgb {
        let hsl = Hsl {
            s: if self.hsl.s.is_nan() { 0.0 } else { self.hsl.s },
            ..self.hsl
        };
        okhsl::okhsl_to_srgb(hsl).map(|channel| (channel.clamp(0.0, 1.0) * 255.0) as u8)
    }
}

/// The best-scoring color, darkened to `ceiling` and pushed out of the muddy middle
/// saturations.
fn choose(palette: &[Rgb], ceiling: f32, previous: Option<&Candidate>) -> Candidate {
    let candidates: Vec<Candidate> = palette.iter().copied().map(Candidate::new).collect();
    let count = candidates.len();
    let scores: Vec<f32> = candidates
        .iter()
        .enumerate()
        .map(|(position, candidate)| score(candidate, position, count, previous))
        .collect();
    // The reference sorts by score with an insertion sort and takes the first: the earliest
    // of the best, a NaN never displacing anything.
    let best = (1..count).fold(0, |best, position| {
        if scores[position] > scores[best] {
            position
        } else {
            best
        }
    });

    let mut chosen = candidates[best];
    if chosen.hsl.l > ceiling {
        chosen.hsl.l = ceiling;
    }
    if chosen.hsl.s > 0.2 && 0.6 > chosen.hsl.s {
        chosen.hsl.s = 0.6;
    }
    chosen
}

/// 100 inside `[low, high]`, falling linearly to 0 at `low_end` and `high_end`.
fn band(value: f32, low_end: f32, low: f32, high: f32, high_end: f32) -> f32 {
    if value >= low && value <= high {
        return 100.0;
    }
    if low > value {
        let span = low - low_end;
        if span == 0.0 {
            return 0.0;
        }
        return max_ss(0.0, 100.0 - (low - value) / span * 100.0);
    }
    let span = high_end - high;
    if span == 0.0 {
        return 0.0;
    }
    max_ss(0.0, 100.0 - (value - high) / span * 100.0)
}

/// `maxss`: the second operand unless the first is larger, NaN included.
fn max_ss(a: f32, b: f32) -> f32 {
    if a > b {
        a
    } else {
        b
    }
}

/// How well a color suits a corner, as the reference scores it.
#[allow(
    clippy::cast_precision_loss,
    reason = "a palette holds ten colors at most"
)]
fn score(
    candidate: &Candidate,
    position: usize,
    count: usize,
    previous: Option<&Candidate>,
) -> f32 {
    let Hsl { h, s, l } = candidate.hsl;
    let place = band(position as f32, 0.0, 0.0, 0.0, count as f32 - 1.0);

    let dark = if (0.0..=0.4).contains(&l) {
        100.0
    } else if 0.0 > l {
        0.0
    } else {
        max_ss(0.0, 100.0 - (l - 0.4) / 0.299_999_98 * 100.0)
    };

    let s_or_zero = if s.is_nan() { 0.0 } else { s };
    let saturated = if (0.6..=1.0).contains(&s_or_zero) {
        100.0
    } else if 0.6 > s_or_zero {
        max_ss(0.0, 100.0 - (0.6 - s_or_zero) / 0.400_000_04 * 100.0)
    } else {
        0.0
    };

    let apart = previous.map_or(100.0, |previous| {
        let da = candidate.lab.a - previous.lab.a;
        let dl = candidate.lab.l - previous.lab.l;
        let db = candidate.lab.b - previous.lab.b;
        let distance = (da * da + dl * dl + db * db).sqrt();
        band(distance, 0.0, 0.05, 0.1, 1.0)
    });

    let degrees = h * 360.0;
    let green = hue_band(degrees, 80.0, 120.0);
    let yellow = hue_band(degrees, 40.0, 70.0);
    let mid_saturation = if (0.2..=0.6).contains(&s_or_zero) {
        100.0
    } else if 0.2 > s_or_zero {
        max_ss(0.0, 100.0 - (0.2 - s_or_zero) / 0.1 * 100.0)
    } else {
        max_ss(0.0, 100.0 - (s_or_zero - 0.6) / 0.099_999_964 * 100.0)
    };
    let muddy = if yellow == 0.0 || mid_saturation == 0.0 {
        0.0
    } else {
        (mid_saturation + yellow) * 0.5
    };

    let grey = if s.is_nan() || (0.0..=0.2).contains(&s) {
        100.0
    } else if 0.0 > s {
        0.0
    } else {
        max_ss(0.0, 100.0 - (s - 0.2) / 0.2 * 100.0)
    };
    let light = if (0.5..=1.0).contains(&l) {
        100.0
    } else if 0.5 > l {
        max_ss(0.0, 100.0 - (0.5 - l) / 0.199_999_99 * 100.0)
    } else {
        0.0
    };
    let washed = if light == 0.0 || grey == 0.0 {
        0.0
    } else {
        (light + grey) * 0.5
    };

    dark * 1.5 + place + saturated * 3.0 + apart * 3.0
        - (green + green)
        - (muddy + muddy)
        - (washed + washed)
}

/// 100 between `low` and `high` degrees, falling to 0 ten degrees outside.
fn hue_band(degrees: f32, low: f32, high: f32) -> f32 {
    if degrees >= low && degrees <= high {
        100.0
    } else if low > degrees {
        max_ss(0.0, 100.0 - (low - degrees) / 10.0 * 100.0)
    } else {
        max_ss(0.0, 100.0 - (degrees - high) / 10.0 * 100.0)
    }
}

/// The corners' gradient as a PNG, `width` × `height`, at most [`MAX_PIXELS`].
///
/// Each pixel starts black and is blended towards each corner in turn — bottom left, bottom
/// right, top right, top left — by one minus its distance to that corner, the axes measured in
/// 95% of the image's sides. A weight past one pushes away from the color. `noise` shifts each
/// pixel's position along the diagonal by up to a 48th of the width, which breaks the banding
/// of so smooth a gradient; it is drawn from a fixed seed, so an image is the same every time.
///
/// # Errors
///
/// [`TranscodeError::UnusableSize`] outside the bounds, [`TranscodeError::Encode`] when the PNG
/// cannot be written.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "sides are bounded by MAX_PIXELS, far inside f32's exact integers, and each \
              ratio is narrowed to f32 as the reference narrows it"
)]
pub(crate) fn gradient(
    colors: CornerColors,
    width: u32,
    height: u32,
    noise: bool,
) -> Result<Vec<u8>, TranscodeError> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(TranscodeError::UnusableSize);
    }
    let (w, h) = (width as f32, height as f32);
    let (scale_x, scale_y) = (f64::from(w) * 0.95, f64::from(h) * 0.95);
    let spread = i64::from(width / 48);
    let mut random = SplitMix64(0x9e37_79b9_7f4a_7c15);

    let mut image = RgbImage::new(width, height);
    for (column, row, pixel) in image.enumerate_pixels_mut() {
        let shift = if noise {
            random.between(-spread, spread) as f32
        } else {
            0.0
        };
        let x = column as f32 + shift;
        let y = row as f32 + shift;
        let squared = |offset: f32, scale: f64| {
            let ratio = (f64::from(offset) / scale) as f32;
            ratio * ratio
        };
        let left = squared(x, scale_x);
        let top = squared(y, scale_y);
        let right = squared(x - w, scale_x);
        let bottom = squared(y - h, scale_y);

        let mut color = [0u8; 3];
        for (corner, distance) in [
            (colors.bottom_left, (bottom + left).sqrt()),
            (colors.bottom_right, (bottom + right).sqrt()),
            (colors.top_right, (right + top).sqrt()),
            (colors.top_left, (left + top).sqrt()),
        ] {
            color = blend(color, corner, 1.0 - distance);
        }
        pixel.0 = color;
    }

    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, PngFilter::Adaptive)
        .write_image(
            image.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(TranscodeError::Encode)?;
    Ok(png)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to a byte's range first, then truncated as the reference does"
)]
fn blend(from: Rgb, to: Rgb, weight: f32) -> Rgb {
    [0, 1, 2].map(|channel| {
        let from = f32::from(from[channel]);
        let value = (f32::from(to[channel]) - from) * weight + from;
        value.clamp(0.0, 255.0) as u8
    })
}

/// Vigna's `SplitMix64`: enough to scatter dither, and no dependency.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `[low, high]`, near enough: the modulo bias is far below what dither shows.
    #[allow(
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss,
        reason = "the span is a few hundred at most"
    )]
    fn between(&mut self, low: i64, high: i64) -> i64 {
        let span = (high - low + 1) as u64;
        low + (self.next() % span) as i64
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use image::codecs::png::PngEncoder;
    use image::{DynamicImage, ImageEncoder, RgbImage};

    use super::*;

    fn png(image: &RgbImage) -> Vec<u8> {
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        bytes
    }

    fn solid(color: Rgb) -> Vec<u8> {
        png(&RgbImage::from_pixel(64, 64, image::Rgb(color)))
    }

    fn hex(color: Rgb) -> String {
        format!("{:02x}{:02x}{:02x}", color[0], color[1], color[2])
    }

    fn corners(bytes: &[u8]) -> [String; 4] {
        let colors = corner_colors(bytes).unwrap();
        [
            colors.top_left,
            colors.top_right,
            colors.bottom_right,
            colors.bottom_left,
        ]
        .map(hex)
    }

    /// Captured from the reference for flat 64×64 PNGs: a flat image still gets four colors,
    /// the near-copies of its color that the cuts leave behind.
    #[test]
    fn reproduces_the_reference_on_flat_images() {
        for (color, expected) in [
            ([0, 0, 0], ["0d0202", "030403", "0d0202", "030403"]),
            ([128, 128, 128], ["2e2e2e", "605c5c", "5d5d5d", "605c5c"]),
            ([255, 0, 0], ["5d0000", "b20101", "b20101", "b20101"]),
            ([0, 0, 255], ["010193", "0703fc", "0304fc", "0703fc"]),
            ([100, 50, 25], ["482210", "64331b", "68331c", "64331b"]),
            ([200, 100, 50], ["4b1f09", "94421c", "93431d", "94421c"]),
            ([10, 20, 30], ["051422", "081229", "051422", "081229"]),
        ] {
            assert_eq!(corners(&solid(color)), expected, "{color:?}");
        }
    }

    #[test]
    fn each_corner_reads_its_own_quarter() {
        let mut image = RgbImage::from_pixel(64, 64, image::Rgb([200, 40, 40]));
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            pixel.0 = match (x < 32, y < 32) {
                (true, true) => [200, 40, 40],
                (false, true) => [40, 200, 40],
                (false, false) => [40, 40, 200],
                (true, false) => [200, 200, 40],
            };
        }
        assert_eq!(
            corners(&png(&image)),
            ["580b0c", "197118", "2c2ccb", "626211"]
        );
    }

    #[allow(
        clippy::cast_possible_truncation,
        reason = "patterns taken modulo a byte"
    )]
    fn pattern(color: impl Fn(u32, u32) -> [u32; 3]) -> Vec<u8> {
        png(&RgbImage::from_fn(96, 80, |x, y| {
            image::Rgb(color(x, y).map(|channel| channel as u8))
        }))
    }

    /// Busy images whose quarters hold full palettes, answered by the reference.
    #[test]
    fn reproduces_the_reference_on_busy_images() {
        let busy = pattern(|x, y| {
            [
                (x * 37 + y * 91) % 256,
                (x * x + y * 13) % 256,
                (x * y) % 256,
            ]
        });
        assert_eq!(corners(&busy), ["0d3147", "4a1881", "1f518c", "962386"]);
        let stripes =
            pattern(|x, y| [(x * 3 + y) % 200 + 20, (y * 2) % 180 + 30, (x + y * 5) % 90]);
        assert_eq!(corners(&stripes), ["4b1f09", "a52930", "486a13", "237022"]);
    }

    #[test]
    fn a_white_image_has_no_colors() {
        assert!(matches!(
            corner_colors(&solid([255, 255, 255])),
            Err(TranscodeError::UnusableSize)
        ));
        let speck = png(&RgbImage::from_pixel(1, 9, image::Rgb([9, 9, 9])));
        assert!(matches!(
            corner_colors(&speck),
            Err(TranscodeError::UnusableSize)
        ));
    }

    #[test]
    fn transparency_is_dropped_not_blended() {
        let clear = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            64,
            64,
            image::Rgba([200, 100, 50, 0]),
        ));
        let mut bytes = Vec::new();
        clear
            .write_with_encoder(PngEncoder::new(&mut bytes))
            .unwrap();
        assert_eq!(corners(&bytes), corners(&solid([200, 100, 50])));
    }

    fn sample() -> CornerColors {
        CornerColors {
            top_left: [0x3f, 0x28, 0x0a],
            top_right: [0x6b, 0x47, 0x13],
            bottom_right: [0x0f, 0x2a, 0x43],
            bottom_left: [0x1c, 0x42, 0x5d],
        }
    }

    /// The reference's `/services/ultrablur/image` for the spec's example colors at 97×61
    /// with `noise=0`.
    const REFERENCE_97X61: &[u8] = include_bytes!("../fixtures/backdrop_97x61.png");

    #[test]
    fn draws_the_reference_gradient_pixel_for_pixel() {
        let ours = gradient(sample(), 97, 61, false).unwrap();
        let ours = image::load_from_memory(&ours).unwrap().into_rgb8();
        let reference = image::load_from_memory(REFERENCE_97X61)
            .unwrap()
            .into_rgb8();
        assert_eq!(ours.dimensions(), reference.dimensions());
        assert!(ours.pixels().eq(reference.pixels()));
    }

    #[test]
    fn refuses_a_size_out_of_bounds() {
        for (width, height) in [(0, 10), (10, 0), (3841, 2160), (8_294_401, 1)] {
            assert!(matches!(
                gradient(sample(), width, height, false),
                Err(TranscodeError::UnusableSize)
            ));
        }
        assert!(gradient(sample(), 4000, 100, false).is_ok());
    }

    #[test]
    fn noise_is_the_same_every_time() {
        let once = gradient(sample(), 96, 64, true).unwrap();
        assert_eq!(once, gradient(sample(), 96, 64, true).unwrap());
        assert_ne!(once, gradient(sample(), 96, 64, false).unwrap());
    }
}
