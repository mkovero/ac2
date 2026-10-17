//! Colormap lookup tables.
//!
//! Viridis, magma and inferno are evaluated from the degree-6 polynomial fits published by
//! Matt Zucker (shadertoy "WlfXRN", CC0), fitted to matplotlib's tables in display-encoded
//! sRGB. Cividis is the same degree-6 least-squares fit, made here to matplotlib's cividis
//! table (`lib/matplotlib/_cm_listed.py`, CC0; Nuñez, Anderton & Renslow 2018). All four
//! are perceptually uniform in lightness and readable with the common colour-vision
//! deficiencies, which is why spectrographs use them. Grey is computed directly.

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

const MAGMA: [[f64; 3]; 7] = [
    [
        -0.002_136_485_053_939_582,
        -0.000_749_655_052_795_221,
        -0.005_386_127_855_323_933,
    ],
    [
        0.251_660_540_737_164_2,
        0.677_523_243_683_766_8,
        2.494_026_599_312_351,
    ],
    [
        8.353_717_279_216_625,
        -3.577_719_514_958_484,
        0.314_467_903_013_257_3,
    ],
    [
        -27.668_733_085_768_66,
        14.264_730_780_965_33,
        -13.649_213_188_139_22,
    ],
    [
        52.176_139_812_340_68,
        -27.943_606_071_683_51,
        12.944_169_442_383_94,
    ],
    [
        -50.768_525_364_735_88,
        29.046_582_821_272_91,
        4.234_152_993_845_98,
    ],
    [
        18.655_705_065_918_83,
        -11.489_773_519_977_11,
        -5.601_961_508_734_096,
    ],
];

const INFERNO: [[f64; 3]; 7] = [
    [
        0.000_218_940_369_119_226_5,
        0.001_651_004_631_001_012,
        -0.019_480_898_437_091_84,
    ],
    [
        0.106_513_419_485_611_6,
        0.563_956_436_788_409_1,
        3.932_712_388_889_277,
    ],
    [
        11.602_493_082_471_87,
        -3.972_853_965_665_698,
        -15.942_394_106_291_4,
    ],
    [
        -41.703_996_131_394_59,
        17.436_398_882_053_13,
        44.354_145_198_728_13,
    ],
    [
        77.162_935_699_427,
        -33.402_358_942_100_92,
        -81.807_309_257_389_93,
    ],
    [
        -71.319_428_244_992_14,
        32.626_064_263_977_23,
        73.209_519_858_032_02,
    ],
    [
        25.131_126_224_773_41,
        -12.242_668_952_385_67,
        -23.070_325_002_871_72,
    ],
];

const CIVIDIS: [[f64; 3]; 7] = [
    [
        -0.008_973_442_521_551_932,
        0.136_755_893_585_938_6,
        0.294_170_187_093_989_57,
    ],
    [
        -0.384_689_046_301_142_4,
        0.639_493_702_419_393_9,
        2.982_653_925_809_012_6,
    ],
    [
        15.429_210_348_539_81,
        0.385_561_650_718_251_27,
        -22.363_760_372_237_945,
    ],
    [
        -58.977_031_461_128_334,
        -1.404_197_119_381_229,
        74.863_561_097_975_43,
    ],
    [
        102.370_491_510_385_9,
        2.600_914_296_487_346_6,
        -121.303_164_220_606_45,
    ],
    [
        -83.187_238_960_837_85,
        -2.140_750_389_310_415_6,
        93.974_216_395_721_91,
    ],
    [
        25.776_070_143_613_875,
        0.688_122_315_033_055_7,
        -28.262_533_018_389_608,
    ],
];

/// Grey's top: light grey, not white, whose glare in a dark room is what the dark maps
/// exist to avoid.
const GREY_TOP: f64 = 0.8;

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
    let rgb_at = |t: f64| match map {
        Colormap::Viridis => poly(&VIRIDIS, t),
        Colormap::Magma => poly(&MAGMA, t),
        Colormap::Inferno => poly(&INFERNO, t),
        Colormap::Cividis => poly(&CIVIDIS, t),
        // Linear in the encoded value: sRGB's encoding is close to perceived lightness, so
        // equal steps in level look like equal steps in grey.
        Colormap::Grey => [GREY_TOP * t; 3],
    };
    let mut out = [[0u8; 4]; LUT_SIZE];
    for (i, e) in out.iter_mut().enumerate() {
        let rgb = rgb_at(i as f64 / (LUT_SIZE - 1) as f64);
        let q = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        *e = [q(rgb[0]), q(rgb[1]), q(rgb[2]), 255];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Entries 0, 64, 128, 192 and 255 of matplotlib's 256-entry tables.
    type Reference = [(usize, [f64; 3]); 5];

    const MPL_VIRIDIS: Reference = [
        (0, [0.267_004, 0.004_874, 0.329_415]),
        (64, [0.229_739, 0.322_361, 0.545_706]),
        (128, [0.127_568, 0.566_949, 0.550_556]),
        (192, [0.369_214, 0.788_888, 0.382_914]),
        (255, [0.993_248, 0.906_157, 0.143_936]),
    ];
    const MPL_MAGMA: Reference = [
        (0, [0.001_462, 0.000_466, 0.013_866]),
        (64, [0.316_654, 0.071_69, 0.485_38]),
        (128, [0.716_387, 0.214_982, 0.475_29]),
        (192, [0.986_7, 0.535_582, 0.382_21]),
        (255, [0.987_053, 0.991_438, 0.749_504]),
    ];
    const MPL_INFERNO: Reference = [
        (0, [0.001_462, 0.000_466, 0.013_866]),
        (64, [0.341_5, 0.062_325, 0.429_425]),
        (128, [0.735_683, 0.215_906, 0.330_245]),
        (192, [0.978_422, 0.557_937, 0.034_931]),
        (255, [0.988_362, 0.998_364, 0.644_924]),
    ];
    const MPL_CIVIDIS: Reference = [
        (0, [0.0, 0.135_112, 0.304_751]),
        (64, [0.263_738, 0.307_831, 0.422_789]),
        (128, [0.488_697, 0.485_318, 0.471_008]),
        (192, [0.736_488, 0.680_629, 0.424_028]),
        (255, [0.995_737, 0.909_344, 0.217_772]),
    ];

    fn tracks(map: Colormap, reference: &Reference, tol: f64) {
        let l = lut(map);
        for (i, rgb) in reference {
            for ch in 0..3 {
                let got = f64::from(l[*i][ch]) / 255.0;
                assert!(
                    (got - rgb[ch]).abs() < tol,
                    "{map:?} entry {i} channel {ch}: {got} vs {}",
                    rgb[ch]
                );
            }
        }
    }

    #[test]
    fn fits_track_reference_tables() {
        tracks(Colormap::Viridis, &MPL_VIRIDIS, 0.02);
        tracks(Colormap::Magma, &MPL_MAGMA, 0.025);
        tracks(Colormap::Inferno, &MPL_INFERNO, 0.035);
        tracks(Colormap::Cividis, &MPL_CIVIDIS, 0.045);
    }

    #[test]
    fn grey_runs_from_black_to_light_grey() {
        let l = lut(Colormap::Grey);
        assert_eq!(l[0], [0, 0, 0, 255]);
        assert_eq!(l[LUT_SIZE - 1], [204, 204, 204, 255]);
        assert!(l.iter().all(|c| c[0] == c[1] && c[1] == c[2]));
    }

    #[test]
    fn dark_maps_start_black() {
        for map in [Colormap::Magma, Colormap::Inferno] {
            let c = lut(map)[0];
            assert!(c[..3].iter().all(|v| *v <= 1), "{map:?}: {c:?}");
        }
    }

    #[test]
    fn lightness_increases_in_every_map() {
        // Rec. 709 luma of the encoded values as a lightness proxy: a perceptual map must
        // be monotonic so that brighter always means larger.
        let luma = |c: [u8; 4]| {
            0.2126 * f64::from(c[0]) + 0.7152 * f64::from(c[1]) + 0.0722 * f64::from(c[2])
        };
        for map in Colormap::ALL {
            let l = lut(map);
            for w in l.windows(8).step_by(8) {
                assert!(
                    luma(w[7]) > luma(w[0]),
                    "{map:?}: luma not increasing near {:?}",
                    w[0]
                );
            }
        }
    }
}
