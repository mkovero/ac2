//! Colormap lookup tables.
//!
//! Viridis is evaluated from the degree-6 polynomial fit published by Matt Zucker
//! (shadertoy "WlfXRN", CC0), fitted to matplotlib's table in display-encoded sRGB. It is
//! perceptually uniform in lightness and readable with the common colour-vision
//! deficiencies, which is why spectrographs use it.

use crate::scene::Colormap;

/// Entries per lookup table. 256 matches the 8-bit output: finer steps cannot be shown.
pub const LUT_SIZE: usize = 256;

const VIRIDIS: [[f64; 3]; 7] = [
    [
        0.277_727_327_223_417_7,
        0.005_407_344_544_966_578,
        0.334_099_805_335_306_1,
    ],
    [
        0.105_093_043_108_577_4,
        1.404_613_529_898_575,
        1.384_590_162_594_685,
    ],
    [
        -0.330_861_828_725_556_3,
        0.214_847_559_468_213,
        0.095_095_163_028_236_59,
    ],
    [
        -4.634_230_498_983_486,
        -5.799_100_973_351_585,
        -19.332_440_956_279_87,
    ],
    [
        6.228_269_936_347_081,
        14.179_933_366_805_09,
        56.690_552_600_681_05,
    ],
    [
        4.776_384_997_670_288,
        -13.745_145_377_746_01,
        -65.353_032_633_372_34,
    ],
    [
        -5.435_455_855_934_631,
        4.645_852_612_178_535,
        26.312_435_249_583_2,
    ],
];

fn poly(c: &[[f64; 3]; 7], t: f64) -> [f64; 3] {
    let mut out = [0.0; 3];
    for (ch, o) in out.iter_mut().enumerate() {
        // Horner from the highest coefficient.
        *o = c.iter().rev().fold(0.0, |acc, k| acc * t + k[ch]);
    }
    out
}

/// RGBA8 table, entry `i` for normalised value `i / (LUT_SIZE - 1)`.
pub fn lut(map: Colormap) -> [[u8; 4]; LUT_SIZE] {
    let coeffs = match map {
        Colormap::Viridis => &VIRIDIS,
    };
    let mut out = [[0u8; 4]; LUT_SIZE];
    for (i, e) in out.iter_mut().enumerate() {
        let rgb = poly(coeffs, i as f64 / (LUT_SIZE - 1) as f64);
        let q = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        *e = [q(rgb[0]), q(rgb[1]), q(rgb[2]), 255];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Entries 0, 64, 128, 192 and 255 of matplotlib's 256-entry viridis table.
    const MPL: [(usize, [f64; 3]); 5] = [
        (0, [0.267_004, 0.004_874, 0.329_415]),
        (64, [0.229_739, 0.322_361, 0.545_706]),
        (128, [0.127_568, 0.566_949, 0.550_556]),
        (192, [0.369_214, 0.788_888, 0.382_914]),
        (255, [0.993_248, 0.906_157, 0.143_936]),
    ];

    #[test]
    fn viridis_fit_tracks_reference_table() {
        let l = lut(Colormap::Viridis);
        for (i, rgb) in MPL {
            for ch in 0..3 {
                let got = f64::from(l[i][ch]) / 255.0;
                assert!(
                    (got - rgb[ch]).abs() < 0.02,
                    "entry {i} channel {ch}: {got} vs {}",
                    rgb[ch]
                );
            }
        }
    }

    #[test]
    fn viridis_lightness_increases() {
        // Rec. 709 luma of the encoded values as a lightness proxy: a perceptual map must
        // be monotonic so that brighter always means larger.
        let l = lut(Colormap::Viridis);
        let luma = |c: [u8; 4]| {
            0.2126 * f64::from(c[0]) + 0.7152 * f64::from(c[1]) + 0.0722 * f64::from(c[2])
        };
        for w in l.windows(8).step_by(8) {
            assert!(
                luma(w[7]) > luma(w[0]),
                "luma not increasing near {:?}",
                w[0]
            );
        }
    }
}
