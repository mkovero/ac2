//! Normal equal-loudness-level contours (ISO 226:2003 model) and the perceived-balance
//! shift between two loudness levels.
//!
//! The model is Suzuki & Takeshima's loudness function: a pure tone of frequency f is as
//! loud as a 1 kHz tone at loudness level L_N when its level is
//!
//! ```text
//! L_p = (10 / α_f) · lg A_f − L_U + 94 dB
//! A_f = 4.47·10⁻³ · (10^(0.025·L_N) − 1.15) + (0.4 · 10^((T_f + L_U)/10 − 9))^α_f
//! ```
//!
//! with α_f the exponent of loudness perception, L_U the ear's transfer function normalised
//! at 1 kHz and T_f the threshold of hearing. Sources, all openly published:
//! - the formula as the 2003 edition writes it: Y. Suzuki, H. Takeshima, K. Kurakata,
//!   "Revision of ISO 226 'Normal Equal-Loudness-Level Contours' from 2003 to 2023
//!   edition: The background and results", Acoust. Sci. & Tech. 45(1), 1–8 (2024),
//!   doi:10.1250/ast.e23.66 (open access), section 2.3;
//! - the per-frequency parameters: J. Parmanen, "Some Reasons to Revise the International
//!   Standard ISO 226:2003", Open Journal of Acoustics 2, 143–149 (2012),
//!   doi:10.4236/oja.2012.24016 (open access), Table 1, which reproduces the standard's;
//! - the derivation: Y. Suzuki et al., "Precise and Full-range Determination of
//!   Two-dimensional Equal Loudness Contours" (NEDO project report IS-01, 2003), and
//!   Suzuki & Takeshima, J. Acoust. Soc. Am. 116(2), 918–933 (2004).
//!
//! The standard defines the contours at these 29 one-third-octave frequencies only; nothing
//! here interpolates them.

/// The nominal one-third-octave frequencies the contours are given at, Hz.
pub const FREQS: [f64; 29] = [
    20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0,
    500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0, 6300.0,
    8000.0, 10000.0, 12500.0,
];

/// α_f: exponent of loudness perception.
const ALPHA: [f64; 29] = [
    0.532, 0.506, 0.480, 0.455, 0.432, 0.409, 0.387, 0.367, 0.349, 0.330, 0.315, 0.301, 0.288,
    0.276, 0.267, 0.259, 0.253, 0.250, 0.246, 0.244, 0.243, 0.243, 0.243, 0.242, 0.242, 0.245,
    0.254, 0.271, 0.301,
];

/// L_U: magnitude of the linear transfer function normalised at 1 kHz, dB.
const LU: [f64; 29] = [
    -31.6, -27.2, -23.0, -19.1, -15.9, -13.0, -10.3, -8.1, -6.2, -4.5, -3.1, -2.0, -1.1, -0.4, 0.0,
    0.3, 0.5, 0.0, -2.7, -4.1, -1.0, 1.7, 2.5, 1.2, -2.1, -7.1, -11.2, -10.7, -3.1,
];

/// T_f: threshold of hearing, dB.
const TF: [f64; 29] = [
    78.5, 68.7, 59.5, 51.1, 44.0, 37.5, 31.5, 26.5, 22.1, 17.9, 14.4, 11.4, 8.6, 6.2, 4.4, 3.0,
    2.2, 2.4, 3.5, 1.7, -1.3, -4.2, -6.0, -5.4, -1.5, 6.0, 12.6, 13.9, 12.3,
];

/// Index of 1 kHz in [`FREQS`].
pub const REF_INDEX: usize = 17;

/// The lowest loudness level the contours rest on measurements for, phon (below it the
/// report draws them dotted, between the threshold and 20 phon).
pub const MIN_PHON: f64 = 20.0;

/// The level of a pure tone at `FREQS[i]` that is as loud as a 1 kHz tone at `phon`, dB
/// SPL; NaN for a level so far below the threshold that `A_f` is not positive.
pub fn spl(i: usize, phon: f64) -> f64 {
    let (a, lu, tf) = (ALPHA[i], LU[i], TF[i]);
    let af = 4.47e-3 * (10f64.powf(0.025 * phon) - 1.15)
        + (0.4 * 10f64.powf((tf + lu) / 10.0 - 9.0)).powf(a);
    10.0 / a * af.log10() - lu + 94.0
}

/// The contour at `phon` at every [`FREQS`] point, dB SPL.
pub fn contour(phon: f64) -> [f64; 29] {
    std::array::from_fn(|i| spl(i, phon))
}

/// How the perceived tonal balance shifts when a programme heard at `from_phon` is played
/// at `to_phon` instead, dB at every [`FREQS`] point, 0 at 1 kHz.
///
/// A level change adds the same dB at every frequency, but loudness grows faster with
/// level where the contours are crowded (the low end, and the top octave): played louder,
/// the lows and highs seem louder relative to 1 kHz. In SPL terms that is the difference of
/// the two contours each referred to its own 1 kHz value, `C_from(f) − C_to(f)`: positive
/// where the louder playback seems to have more. It describes perception, not the system:
/// the ear applies it by itself, so equalising a system to it would apply it twice.
pub fn loudness_shift(from_phon: f64, to_phon: f64) -> [f64; 29] {
    let a = contour(from_phon);
    let b = contour(to_phon);
    std::array::from_fn(|i| (a[i] - a[REF_INDEX]) - (b[i] - b[REF_INDEX]))
}
