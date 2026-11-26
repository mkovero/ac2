//! Normal equal-loudness-level contours (ISO 226:2023) and the perceived-balance shift
//! between two loudness levels.
//!
//! A pure tone of frequency f is as loud as a 1 kHz tone at loudness level L_N when its
//! level is (ISO 226:2023, Formula (1))
//!
//! ```text
//! L_f = (10 / α_f) · lg[ (p₀/p_a)^(2(α_r − α_f)) · (10^(α_r·L_N/10) − 10^(α_r·T_r/10))
//!                        + 10^(α_f·(T_f + L_U)/10) ] − L_U   dB
//! ```
//!
//! with p₀ = 20 µPa, p_a = 1 Pa, α_f the exponent of loudness perception, L_U the ear's
//! transfer function normalised at 1 kHz and T_f the free-field threshold of hearing
//! (ISO 389-7); α_r = 0.3 and T_r = 2.4 dB are their values at 1 kHz. It is a loudness
//! function: loudness grows as intensity to the power α_f above a threshold-related offset,
//! and two tones are equally loud when those loudnesses match. At L_N = T_r the bracket
//! reduces to its last term, so the contour through the 1 kHz threshold is the threshold of
//! hearing itself; at 1 kHz (α_f = α_r, L_U = 0, T_f = T_r) L_f = L_N exactly.
//!
//! Sources: ISO 226:2023, *Acoustics — Normal equal-loudness-level contours*, Formula (1)
//! and Table 1; the open-access account of the model and its revision, Y. Suzuki,
//! H. Takeshima, K. Kurakata, "Revision of ISO 226 'Normal Equal-Loudness-Level Contours'
//! from 2003 to 2023 edition: The background and results", Acoust. Sci. & Tech. 45(1), 1–8
//! (2024), doi:10.1250/ast.e23.66, section 2.3.
//!
//! The standard specifies the contours at these 29 one-third-octave frequencies only, and
//! only from [`MIN_PHON`] up to [`max_phon`] there; nothing here interpolates them.

/// The nominal one-third-octave frequencies the contours are given at, Hz.
pub const FREQS: [f64; 29] = [
    20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0,
    500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0, 6300.0,
    8000.0, 10000.0, 12500.0,
];

/// α_f: exponent of loudness perception.
const ALPHA: [f64; 29] = [
    0.635, 0.602, 0.569, 0.537, 0.509, 0.482, 0.456, 0.433, 0.412, 0.391, 0.373, 0.357, 0.343,
    0.330, 0.320, 0.311, 0.303, 0.300, 0.295, 0.292, 0.290, 0.290, 0.289, 0.289, 0.289, 0.293,
    0.303, 0.323, 0.354,
];

/// L_U: magnitude of the linear transfer function normalised at 1 kHz, dB.
const LU: [f64; 29] = [
    -31.5, -27.2, -23.1, -19.3, -16.1, -13.1, -10.4, -8.2, -6.3, -4.6, -3.2, -2.1, -1.2, -0.5, 0.0,
    0.4, 0.5, 0.0, -2.7, -4.2, -1.2, 1.4, 2.3, 1.0, -2.3, -7.2, -11.2, -10.9, -3.5,
];

/// T_f: free-field threshold of hearing, dB.
pub const THRESHOLD: [f64; 29] = [
    78.1, 68.7, 59.5, 51.1, 44.0, 37.5, 31.5, 26.5, 22.1, 17.9, 14.4, 11.4, 8.6, 6.2, 4.4, 3.0,
    2.2, 2.4, 3.5, 1.7, -1.3, -4.2, -6.0, -5.4, -1.5, 6.0, 12.6, 13.9, 12.3,
];

/// Index of 1 kHz in [`FREQS`].
pub const REF_INDEX: usize = 17;

/// α_r: the exponent of loudness perception at 1 kHz.
const ALPHA_R: f64 = 0.3;

/// (p₀ / p_a)²: the reference pressure 20 µPa over 1 Pa, squared.
const P0_PA_SQ: f64 = 4e-10;

/// The lowest loudness level the standard specifies, phon: below it there are no
/// measurements between 20 phon and the threshold, and Formula (1) is informative only.
pub const MIN_PHON: f64 = 20.0;

/// The highest loudness level the standard specifies up to 4 kHz, phon.
pub const MAX_PHON: f64 = 90.0;

/// The highest loudness level the standard specifies from 5 kHz up, phon.
pub const MAX_PHON_HF: f64 = 80.0;

/// The highest loudness level the standard specifies at `FREQS[i]`, phon: above it
/// Formula (1) still gives a value, but no measurements (or only one institute's, up to
/// 100 phon below 1 kHz) stand behind it.
pub fn max_phon(i: usize) -> f64 {
    if FREQS[i] <= 4000.0 {
        MAX_PHON
    } else {
        MAX_PHON_HF
    }
}

/// The level of a pure tone at `FREQS[i]` that is as loud as a 1 kHz tone at `phon`, dB
/// SPL (Formula (1)). NaN below the 1 kHz threshold far enough that the bracket is not
/// positive; at or above [`MIN_PHON`] it always is.
pub fn spl(i: usize, phon: f64) -> f64 {
    let (a, lu, tf) = (ALPHA[i], LU[i], THRESHOLD[i]);
    let t_r = THRESHOLD[REF_INDEX];
    let inner = P0_PA_SQ.powf(ALPHA_R - a)
        * (10f64.powf(ALPHA_R * phon / 10.0) - 10f64.powf(ALPHA_R * t_r / 10.0))
        + 10f64.powf(a * (tf + lu) / 10.0);
    10.0 / a * inner.log10() - lu
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
