//! Björn Ottosson's Oklab and Okhsl, in single precision, operation for operation.
//!
//! The reference ships his published C++ and chooses colors by comparing scores computed from
//! it, so a rounding difference here is a different color there: no reassociation, no fused
//! operations, his constants. The polynomial fits are his too.

#![allow(
    clippy::excessive_precision,
    clippy::unreadable_literal,
    reason = "the published literals, whose single-precision rounding is what is reproduced"
)]
#![allow(
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::float_cmp,
    reason = "the paper's names, `l`, `m`, `s` and their derivatives, and its exact tests"
)]

use std::f32::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Lab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Hsl {
    /// A turn, 0 to 1.
    pub h: f32,
    /// NaN for a grey, whose hue the formulas leave undefined.
    pub s: f32,
    pub l: f32,
}

const TOE_K1: f32 = 0.206;
const TOE_K2: f32 = 0.03;
/// `(1 + k1) / (1 + k2)` and `4 * k2 * k3`, folded as the reference's compiler folded them.
const TOE_K3: f32 = 1.170_873_761_177_063;
const TOE_4K2K3: f32 = 0.140_504_851_937_294;

const TO_RED: [f32; 3] = [4.0767416621, -3.3077115913, 0.2309699292];
const TO_GREEN: [f32; 3] = [-1.2684380046, 2.6097574011, -0.3413193965];
const TO_BLUE: [f32; 3] = [-0.0041960863, -0.7034186147, 1.7076147010];

pub(crate) fn linear_srgb_to_oklab([r, g, b]: [f32; 3]) -> Lab {
    let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
    let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
    let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    Lab {
        l: 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        a: 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        b: 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    }
}

/// `k_l`, `k_m` and `k_s`: how far a unit of chroma in the hue `(a, b)` moves each cone.
fn cone_steps(a: f32, b: f32) -> (f32, f32, f32) {
    (
        0.3963377774 * a + 0.2158037573 * b,
        -0.1055613458 * a - 0.0638541728 * b,
        -0.0894841775 * a - 1.2914855480 * b,
    )
}

fn oklab_to_linear_srgb(c: Lab) -> [f32; 3] {
    let l = c.l + 0.3963377774 * c.a + 0.2158037573 * c.b;
    let m = c.l - 0.1055613458 * c.a - 0.0638541728 * c.b;
    let s = c.l - 0.0894841775 * c.a - 1.2914855480 * c.b;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [TO_RED, TO_GREEN, TO_BLUE].map(|w| w[0] * l + w[1] * m + w[2] * s)
}

fn srgb_transfer(a: f32) -> f32 {
    if 0.0031308 >= a {
        12.92 * a
    } else {
        1.055 * a.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_transfer_inv(a: f32) -> f32 {
    if a > 0.04045 {
        ((a + 0.055) / 1.055).powf(2.4)
    } else {
        a / 12.92
    }
}

fn toe(x: f32) -> f32 {
    let k = TOE_K3 * x - TOE_K1;
    0.5 * (k + (k * k + TOE_4K2K3 * x).sqrt())
}

fn toe_inv(x: f32) -> f32 {
    (x * x + TOE_K1 * x) / (TOE_K3 * (x + TOE_K2))
}

/// The largest saturation `C / L` the sRGB gamut allows in the hue `(a, b)`, a unit vector.
fn max_saturation(a: f32, b: f32) -> f32 {
    let ([k0, k1, k2, k3, k4], w) = if -1.88170328 * a - 0.80936493 * b > 1.0 {
        (
            [1.19086277, 1.76576728, 0.59662641, 0.75515197, 0.56771245],
            TO_RED,
        )
    } else if 1.81444104 * a - 1.19445276 * b > 1.0 {
        (
            [0.73956515, -0.45954404, 0.08285427, 0.12541070, 0.14503204],
            TO_GREEN,
        )
    } else {
        (
            [
                1.35733652,
                -0.00915799,
                -1.15130210,
                -0.50559606,
                0.00692167,
            ],
            TO_BLUE,
        )
    };
    let saturation = k0 + k1 * a + k2 * b + k3 * a * a + k4 * a * b;
    let (k_l, k_m, k_s) = cone_steps(a, b);

    // One Halley step on the polynomial estimate.
    let l_ = 1.0 + saturation * k_l;
    let m_ = 1.0 + saturation * k_m;
    let s_ = 1.0 + saturation * k_s;
    let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let (l_ds, m_ds, s_ds) = (
        3.0 * k_l * l_ * l_,
        3.0 * k_m * m_ * m_,
        3.0 * k_s * s_ * s_,
    );
    let (l_ds2, m_ds2, s_ds2) = (
        6.0 * k_l * k_l * l_,
        6.0 * k_m * k_m * m_,
        6.0 * k_s * k_s * s_,
    );
    let f = w[0] * l + w[1] * m + w[2] * s;
    let f1 = w[0] * l_ds + w[1] * m_ds + w[2] * s_ds;
    let f2 = w[0] * l_ds2 + w[1] * m_ds2 + w[2] * s_ds2;
    saturation - f * f1 / (f1 * f1 - 0.5 * f * f2)
}

/// `std::max` and `std::min`, which keep the first argument on a tie.
fn cpp_max(a: f32, b: f32) -> f32 {
    if a < b {
        b
    } else {
        a
    }
}

fn cpp_min(a: f32, b: f32) -> f32 {
    if b < a {
        b
    } else {
        a
    }
}

/// The lightness and chroma of the most saturated color of a hue.
fn cusp(a: f32, b: f32) -> (f32, f32) {
    let saturation = max_saturation(a, b);
    let [r, g, b] = oklab_to_linear_srgb(Lab {
        l: 1.0,
        a: saturation * a,
        b: saturation * b,
    });
    let lightness = (1.0 / cpp_max(cpp_max(r, g), b)).cbrt();
    (lightness, lightness * saturation)
}

/// Where the line from `(l0, 0)` towards `(l1, c1)` leaves the gamut, as a fraction of it.
fn gamut_intersection(a: f32, b: f32, l1: f32, c1: f32, l0: f32, cusp: (f32, f32)) -> f32 {
    let (cusp_l, cusp_c) = cusp;
    if ((l1 - l0) * cusp_c - (cusp_l - l0) * c1) <= 0.0 {
        return cusp_c * l0 / (c1 * cusp_l + cusp_c * (l0 - l1));
    }
    let t = cusp_c * (l0 - 1.0) / (c1 * (cusp_l - 1.0) + cusp_c * (l0 - l1));

    let (k_l, k_m, k_s) = cone_steps(a, b);
    let d_l = l1 - l0;
    let (l_dt, m_dt, s_dt) = (d_l + c1 * k_l, d_l + c1 * k_m, d_l + c1 * k_s);

    let lightness = l0 * (1.0 - t) + t * l1;
    let chroma = t * c1;
    let l_ = lightness + chroma * k_l;
    let m_ = lightness + chroma * k_m;
    let s_ = lightness + chroma * k_s;
    let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let (ldt, mdt, sdt) = (
        3.0 * l_dt * l_ * l_,
        3.0 * m_dt * m_ * m_,
        3.0 * s_dt * s_ * s_,
    );
    let (ldt2, mdt2, sdt2) = (
        6.0 * l_dt * l_dt * l_,
        6.0 * m_dt * m_dt * m_,
        6.0 * s_dt * s_dt * s_,
    );

    // A Halley step towards each channel's edge; the nearest edge wins.
    let step = |w: [f32; 3]| {
        let value = w[0] * l + w[1] * m + w[2] * s - 1.0;
        let first = w[0] * ldt + w[1] * mdt + w[2] * sdt;
        let second = w[0] * ldt2 + w[1] * mdt2 + w[2] * sdt2;
        let u = first / (first * first - 0.5 * value * second);
        if u >= 0.0 {
            -value * u
        } else {
            f32::MAX
        }
    };
    let (t_r, t_g, t_b) = (step(TO_RED), step(TO_GREEN), step(TO_BLUE));
    t + t_r.min(t_g.min(t_b))
}

/// The chromas Okhsl's saturation scale bends at, for one lightness and hue.
struct Chromas {
    zero: f32,
    mid: f32,
    max: f32,
}

fn chromas(lightness: f32, a: f32, b: f32) -> Chromas {
    let cusp = cusp(a, b);
    let max = gamut_intersection(a, b, lightness, 1.0, lightness, cusp);
    let (cusp_l, cusp_c) = cusp;
    let (st_max_s, st_max_t) = (cusp_c / cusp_l, cusp_c / (1.0 - cusp_l));
    let k = max / cpp_min(lightness * st_max_s, (1.0 - lightness) * st_max_t);

    let st_mid_s = 0.11516993
        + 1.0
            / (7.44778970
                + 4.15901240 * b
                + a * (-2.19557347
                    + 1.75198401 * b
                    + a * (-2.13704948 - 10.02301043 * b
                        + a * (-4.24894561 + 5.38770819 * b + 4.69891013 * a))));
    let st_mid_t = 0.11239642
        + 1.0
            / (1.61320320 - 0.68124379 * b
                + a * (0.40370612
                    + 0.90148123 * b
                    + a * (-0.27087943
                        + 0.61223990 * b
                        + a * (0.00299215 - 0.45399568 * b - 0.14661872 * a))));
    let c_a = lightness * st_mid_s;
    let c_b = (1.0 - lightness) * st_mid_t;
    let mid = 0.9
        * k
        * (1.0 / (1.0 / (c_a * c_a * c_a * c_a) + 1.0 / (c_b * c_b * c_b * c_b)))
            .sqrt()
            .sqrt();

    let c_a = lightness * 0.4;
    let c_b = (1.0 - lightness) * 0.8;
    let zero = (1.0 / (1.0 / (c_a * c_a) + 1.0 / (c_b * c_b))).sqrt();
    Chromas { zero, mid, max }
}

const MID: f32 = 0.8;
const MID_INV: f32 = 1.25;
/// `1 - MID`, which is not quite 0.2 in single precision.
const ONE_MINUS_MID: f32 = 1.0 - MID;

/// `srgb` is gamma-encoded, 0 to 1 per channel.
pub(crate) fn srgb_to_okhsl(srgb: [f32; 3]) -> Hsl {
    let lab = linear_srgb_to_oklab(srgb.map(srgb_transfer_inv));
    let chroma = (lab.b * lab.b + lab.a * lab.a).sqrt();
    let (a, b) = (lab.a / chroma, lab.b / chroma);
    let h = 0.5 * (-lab.b).atan2(-lab.a) / PI + 0.5;

    let cs = chromas(lab.l, a, b);
    let s = if chroma < cs.mid {
        let k1 = MID * cs.zero;
        let k2 = 1.0 - k1 / cs.mid;
        chroma / (k1 + k2 * chroma) * MID
    } else {
        let k1 = ONE_MINUS_MID * cs.mid * cs.mid * MID_INV * MID_INV / cs.zero;
        let k2 = 1.0 - k1 / (cs.max - cs.mid);
        let t = (chroma - cs.mid) / (k1 + k2 * (chroma - cs.mid));
        MID + ONE_MINUS_MID * t
    };
    Hsl {
        h,
        s,
        l: toe(lab.l),
    }
}

/// Gamma-encoded sRGB, unclamped.
pub(crate) fn okhsl_to_srgb(hsl: Hsl) -> [f32; 3] {
    if hsl.l == 1.0 {
        return [1.0; 3];
    }
    if hsl.l == 0.0 {
        return [0.0; 3];
    }
    let a = (2.0 * PI * hsl.h).cos();
    let b = (2.0 * PI * hsl.h).sin();
    let lightness = toe_inv(hsl.l);
    let cs = chromas(lightness, a, b);

    let chroma = if hsl.s < MID {
        let t = MID_INV * hsl.s;
        let k1 = MID * cs.zero;
        let k2 = 1.0 - k1 / cs.mid;
        t * k1 / (1.0 - k2 * t)
    } else {
        let t = (hsl.s - MID) / ONE_MINUS_MID;
        let k1 = ONE_MINUS_MID * cs.mid * cs.mid * MID_INV * MID_INV / cs.zero;
        let k2 = 1.0 - k1 / (cs.max - cs.mid);
        cs.mid + t * k1 / (1.0 - k2 * t)
    };
    oklab_to_linear_srgb(Lab {
        l: lightness,
        a: chroma * a,
        b: chroma * b,
    })
    .map(srgb_transfer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn a_color_survives_the_round_trip() {
        for srgb in [
            [0.8, 0.2, 0.2],
            [0.1, 0.5, 0.9],
            [0.3, 0.3, 0.1],
            [0.95, 0.9, 0.6],
        ] {
            assert!(close(okhsl_to_srgb(srgb_to_okhsl(srgb)), srgb), "{srgb:?}");
        }
    }

    #[test]
    fn white_and_black_are_the_ends_of_lightness() {
        assert!((srgb_to_okhsl([1.0; 3]).l - 1.0).abs() < 1e-4);
        assert!(srgb_to_okhsl([0.0; 3]).l.abs() < 1e-6);
        let black = Hsl {
            h: 0.3,
            s: 0.5,
            l: 0.0,
        };
        assert_eq!(okhsl_to_srgb(black), [0.0; 3]);
    }

    #[test]
    fn a_grey_has_no_saturation() {
        assert!(srgb_to_okhsl([0.5; 3]).s.is_nan() || srgb_to_okhsl([0.5; 3]).s < 1e-3);
    }
}
