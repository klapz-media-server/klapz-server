//! A palette by modified median cut, the algorithm of Leptonica and `color-thief`.
//!
//! As the reference's own port of `quantize.js` runs it, departures included: every fifth
//! pixel, colors cut to five bits, ten colors asked for, and two quirks of the port kept
//! because they decide which boxes exist — the second pass counts its colors from one rather
//! than from the boxes it starts with, and a cut landing past the last pixel walks back
//! without looking at what lies ahead. A box that a cut leaves empty stays in the palette as
//! its center color: those near-copies of a flat image's one color are what the corners
//! choose between.

use std::collections::HashMap;

const SIGNIFICANT_BITS: u32 = 5;
const SHIFT: u32 = 8 - SIGNIFICANT_BITS;
const SIDE: usize = 1 << SIGNIFICANT_BITS;
const MAX_COLORS: usize = 10;
/// The first pass splits by population until three quarters of the colors, rounded up, exist.
const BY_POPULATION: usize = 8;
const MAX_ITERATIONS: u32 = 1000;
const SAMPLE_STEP: usize = 5;
/// A pixel this bright on all three channels is left out, as white.
const WHITE: u8 = 250;

type Histogram = Vec<u32>;

fn bin(r: usize, g: usize, b: usize) -> usize {
    (r << (2 * SIGNIFICANT_BITS)) | (g << SIGNIFICANT_BITS) | b
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Red,
    Green,
    Blue,
}

/// A box of the color cube, bounds inclusive, in five-bit units. A cut may leave one with
/// its lower bound past its upper: empty, with a center all the same.
#[derive(Debug, Clone, Copy)]
struct ColorBox {
    low: [i32; 3],
    high: [i32; 3],
}

impl ColorBox {
    fn bins(self) -> impl Iterator<Item = (i32, i32, i32)> {
        (self.low[0]..=self.high[0]).flat_map(move |r| {
            (self.low[1]..=self.high[1])
                .flat_map(move |g| (self.low[2]..=self.high[2]).map(move |b| (r, g, b)))
        })
    }

    fn count(self, histogram: &Histogram) -> u32 {
        self.bins().map(|(r, g, b)| histogram[index(r, g, b)]).sum()
    }

    fn volume(self) -> i64 {
        (0..3)
            .map(|axis| i64::from(self.high[axis] - self.low[axis] + 1))
            .product()
    }

    /// The pixels' mean, or the box's center when it holds none.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "truncated into a byte as the reference stores it; an empty box past the \
                  top wraps to 0 there too"
    )]
    fn average(self, histogram: &Histogram) -> [u8; 3] {
        let mult = f64::from(1u32 << SHIFT);
        let mut total = 0u32;
        let mut sums = [0f64; 3];
        for (r, g, b) in self.bins() {
            let count = histogram[index(r, g, b)];
            total += count;
            let count = f64::from(count);
            for (sum, position) in sums.iter_mut().zip([r, g, b]) {
                *sum += count * (f64::from(position) + 0.5) * mult;
            }
        }
        if total > 0 {
            sums.map(|sum| (sum / f64::from(total)) as i32 as u8)
        } else {
            [0, 1, 2].map(|axis| {
                (f64::from((self.low[axis] + self.high[axis]) * 8 + 8) * 0.5) as i32 as u8
            })
        }
    }
}

#[allow(
    clippy::cast_sign_loss,
    reason = "bins are inside the cube: boxes are cut from the pixels' bounds"
)]
fn index(r: i32, g: i32, b: i32) -> usize {
    bin(r as usize, g as usize, b as usize)
}

/// The palette of `pixels`, RGB, largest boxes first; `None` when every sampled pixel was
/// white.
pub(crate) fn palette(pixels: &[[u8; 3]]) -> Option<Vec<[u8; 3]>> {
    let mut histogram = vec![0u32; SIDE * SIDE * SIDE];
    let mut low = [i32::MAX; 3];
    let mut high = [i32::MIN; 3];
    for pixel in pixels.iter().step_by(SAMPLE_STEP) {
        if pixel.iter().all(|&channel| channel > WHITE) {
            continue;
        }
        let bins = pixel.map(|channel| i32::from(channel >> SHIFT));
        for axis in 0..3 {
            low[axis] = low[axis].min(bins[axis]);
            high[axis] = high[axis].max(bins[axis]);
        }
        histogram[index(bins[0], bins[1], bins[2])] += 1;
    }
    if low[0] > high[0] {
        return None;
    }

    let mut boxes = vec![ColorBox { low, high }];
    let by_population = |colors: ColorBox| u64::from(colors.count(&histogram));
    split(&mut boxes, &histogram, BY_POPULATION, by_population);
    // The second queue is filled by popping the first, sorted: the largest first.
    boxes.sort_by_key(|colors| by_population(*colors));
    boxes.reverse();
    let target = MAX_COLORS.saturating_sub(boxes.len());
    let by_size =
        |colors: ColorBox| u64::from(colors.count(&histogram)) * colors.volume().unsigned_abs();
    split(&mut boxes, &histogram, target, by_size);
    boxes.sort_by_key(|colors| by_size(*colors));
    Some(
        boxes
            .iter()
            .rev()
            .map(|colors| colors.average(&histogram))
            .collect(),
    )
}

/// Splits the box with the largest `priority` until `target` colors exist, counting from
/// one whatever `boxes` holds, as the reference does.
///
/// The queue is sorted ascending and popped from the back; every turn pushes, so every turn
/// sorts again. The sort is stable, as the reference's insertion sort of so few boxes is.
fn split(
    boxes: &mut Vec<ColorBox>,
    histogram: &Histogram,
    target: usize,
    priority: impl Fn(ColorBox) -> u64,
) {
    let mut colors = 1;
    for _ in 0..MAX_ITERATIONS {
        boxes.sort_by_key(|colors| priority(*colors));
        let Some(largest) = boxes.pop() else {
            return;
        };
        if largest.count(histogram) == 0 {
            boxes.push(largest);
            continue;
        }
        let (lower, upper) = median_cut(histogram, largest);
        boxes.push(lower);
        if let Some(upper) = upper {
            boxes.push(upper);
            colors += 1;
        }
        if colors >= target {
            return;
        }
    }
}

/// Cuts a box across its longest side at its median, leaning into the larger half.
#[allow(
    clippy::cast_possible_truncation,
    reason = "truncated toward zero as the reference does, inside the box's bounds"
)]
fn median_cut(histogram: &Histogram, colors: ColorBox) -> (ColorBox, Option<ColorBox>) {
    if colors.count(histogram) == 1 {
        return (colors, None);
    }
    let widths = [0, 1, 2].map(|axis| colors.high[axis] - colors.low[axis] + 1);
    let longest = widths[0].max(widths[1].max(widths[2]));
    let axis = if widths[0] == longest {
        Axis::Red
    } else if widths[1] == longest {
        Axis::Green
    } else {
        Axis::Blue
    };
    let along = axis as usize;
    let (first, last) = (colors.low[along], colors.high[along]);

    // Cumulative counts up to each slice, keyed as the reference keys its map.
    let mut partial = HashMap::new();
    let mut total = 0u32;
    for slice in first..=last {
        let mut layer = colors;
        layer.low[along] = slice;
        layer.high[along] = slice;
        total += layer.count(histogram);
        partial.insert(slice, total);
    }
    let positive = |slice: i32| partial.get(&slice).is_some_and(|&sum| sum > 0);
    let Some(median) = (first..=last).find(|slice| partial[slice] > total / 2) else {
        return (colors, None);
    };

    let left = median - first;
    let right = last - median;
    let mut cut = if left <= right {
        (last - 1).min((f64::from(median) + f64::from(right) * 0.5) as i32)
    } else {
        first.max((f64::from(median - 1) - f64::from(left) * 0.5) as i32)
    };
    while !positive(cut) {
        cut += 1;
    }
    if total - partial[&cut] == 0 {
        while positive(cut - 1) {
            cut -= 1;
        }
    }

    let mut lower = colors;
    let mut upper = colors;
    lower.high[along] = cut;
    upper.low[along] = cut + 1;
    (lower, Some(upper))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_white_image_has_no_palette() {
        assert_eq!(palette(&[[255, 255, 255]; 40]), None);
        assert_eq!(palette(&[]), None);
    }

    #[test]
    fn samples_every_fifth_pixel() {
        let mut pixels = vec![[255, 255, 255]; 10];
        pixels[5] = [10, 10, 10];
        assert!(palette(&pixels).is_some());
        pixels[5] = [255; 3];
        pixels[4] = [10, 10, 10];
        assert_eq!(palette(&pixels), None);
    }

    #[test]
    fn a_flat_image_keeps_its_color_first_and_the_empty_boxes_after() {
        let colors = palette(&[[132, 132, 132]; 64]).unwrap();
        assert_eq!(colors[0], [132, 132, 132]);
        assert!(colors.len() > 1);
        assert!(colors[1..].iter().all(|color| *color != [132, 132, 132]));
    }

    #[test]
    fn a_mean_lands_in_the_middle_of_its_bin() {
        assert_eq!(palette(&[[0, 0, 0]; 5]).unwrap()[0], [4, 4, 4]);
        assert_eq!(palette(&[[204, 44, 44]; 5]).unwrap()[0], [204, 44, 44]);
    }
}
