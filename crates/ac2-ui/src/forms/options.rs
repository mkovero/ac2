//! The option tables the dialogs offer, and the rate, buffer and frequency parsers.

use super::*;

/// Sweep durations offered, shortest first: → is longer, ← shorter.
pub(super) const DURATIONS: [(&str, f64); 4] = [
    ("1 s (quick look)", 1.0),
    ("3 s", 3.0),
    ("6 s (lower floor, longer windows)", 6.0),
    ("12 s", 12.0),
];
/// The CLI's default duration, 3 s.
pub(super) const DEFAULT_DURATION: usize = 1;
pub(super) const REPEATS: [(&str, u8); 4] = [("1", 1), ("2", 2), ("4", 4), ("8", 8)];
/// Silence after each sweep, shortest first: the room parameters need the decay and some
/// noise after it inside it (a hall's 2 s decay needs about 4 s).
pub(super) const TAILS: [(&str, f64); 4] = [
    ("1 s (small rooms)", 1.0),
    ("2 s", 2.0),
    ("4 s (halls)", 4.0),
    ("8 s (large halls, churches)", 8.0),
];
/// Harmonic windows at a sweep's lowest columns, the CLI's `--lf-harmonics` values.
pub(super) const LF_HARMONICS: [(&str, LfHarmonics); 2] = [
    ("standard", LfHarmonics::Standard),
    ("fine", LfHarmonics::Fine),
];
/// Points per octave of a transfer or sweep curve, coarsest first, the CLI's `--resolution`
/// values; index [`DEFAULT_RESOLUTION`] is the default.
pub(super) const RESOLUTIONS: [(&str, Resolution); 4] = [
    ("1/12 oct", Resolution::Twelfth),
    ("1/24 oct", Resolution::TwentyFourth),
    ("1/48 oct", Resolution::FortyEighth),
    ("1/96 oct", Resolution::NinetySixth),
];
pub(super) const DEFAULT_RESOLUTION: usize = 2;
/// Under the Resolution choice: it is the grid, not smoothing, and below some frequency
/// the analysis cannot fill a fine one (the plot marks where).
pub const RESOLUTION_HINT: &str =
    "points per octave, not smoothing; the plot shades where the analysis is coarser";

pub(crate) fn resolution_field(r: Resolution) -> Field {
    Field {
        hint: RESOLUTION_HINT.into(),
        ..Field::choice(
            FieldId::Resolution,
            "Resolution",
            &RESOLUTIONS.map(|r| r.0),
            RESOLUTIONS
                .iter()
                .position(|x| x.1 == r)
                .unwrap_or(DEFAULT_RESOLUTION),
        )
    }
}

/// What `fine` trades, under the LF harmonics choice.
pub const LF_HARMONICS_HINT: &str =
    "fine: finer low-frequency harmonics, higher floor there, longer silence after the sweep";

/// Smoothing choices of the transfer and spectrum dialogs (index 0: none, as `ac2 meas new`
/// without `--smooth`).
pub(super) const SMOOTHING: [(&str, Option<SmoothingFraction>); 6] = [
    ("off", None),
    ("1/3 octave", Some(SmoothingFraction::Third)),
    ("1/6 octave", Some(SmoothingFraction::Sixth)),
    ("1/12 octave", Some(SmoothingFraction::Twelfth)),
    ("1/24 octave", Some(SmoothingFraction::TwentyFourth)),
    ("1/48 octave", Some(SmoothingFraction::FortyEighth)),
];
/// Averaging choices of the spectrum and RTA dialogs, in their list order (index 0: off, as
/// `ac2 meas new` without `--average`). Exponential first, by time constant: a time reads
/// the same for every FFT length and for the RTA, where a FIFO frame does not. The options
/// read as the measurement list says them ([`ac2_scene::format::spec_averaging`]).
pub(super) const AVERAGING: [SpecAveraging; 9] = [
    SpecAveraging::Off,
    SpecAveraging::Exponential {
        time_constant: Seconds(0.5),
    },
    SpecAveraging::Exponential {
        time_constant: Seconds(1.0),
    },
    SpecAveraging::Exponential {
        time_constant: Seconds(2.0),
    },
    SpecAveraging::Exponential {
        time_constant: Seconds(5.0),
    },
    SpecAveraging::Exponential {
        time_constant: Seconds(10.0),
    },
    SpecAveraging::Exponential {
        time_constant: Seconds(30.0),
    },
    SpecAveraging::Fifo { frames: 16 },
    SpecAveraging::Fifo { frames: 64 },
];
/// FFT lengths of the spectrum dialogs: every one the daemon takes, shortest first.
pub(super) const FFT_LENS: [u32; 11] = [
    64, 128, 256, 512, 1024, 2048, 4096, 8192, 16_384, 32_768, 65_536,
];
/// How a spectrum dialog can resolve frequency: one FFT of each length, then the
/// multi-resolution bands on each column grid.
pub(super) fn spec_resolutions() -> Vec<SpecResolution> {
    FFT_LENS
        .iter()
        .map(|&fft_len| SpecResolution::Fixed { fft_len })
        .chain(
            Resolution::ALL
                .iter()
                .map(|&grid| SpecResolution::Multi { grid }),
        )
        .collect()
}
/// Windows of the spectrum dialogs (index 0: Hann, as `ac2 meas new` without `--window`).
pub(super) const WINDOWS: [Window; 4] = [
    Window::Hann,
    Window::BlackmanHarris4,
    Window::FlatTop,
    Window::Rectangular,
];
/// What the level field's choice means, under it.
pub(super) fn spec_level_hint(l: SpecLevel) -> &'static str {
    match l {
        SpecLevel::Tone => "a sine reads its level; noise reads lower the finer the bins",
        SpecLevel::Density => {
            "per hertz: noise and music read alike at any resolution; a sine reads low"
        }
    }
}

/// Levels of the spectrum dialogs, in [`SpecLevel`] order.
pub(super) const SPEC_LEVELS: [(&str, SpecLevel); 2] = [
    ("tone (dBFS)", SpecLevel::Tone),
    ("density (dBFS/Hz)", SpecLevel::Density),
];
pub(super) const DEPTH: [&str; 2] = [
    "equal confidence (every frequency alike)",
    "fast LF (low stages settle within 1 s)",
];
pub(super) const FRACTIONS: [(&str, BandFraction); 5] = [
    ("1/1 octave", BandFraction::Octave),
    ("1/3 octave", BandFraction::Third),
    ("1/6 octave", BandFraction::Sixth),
    ("1/12 octave", BandFraction::Twelfth),
    ("1/24 octave", BandFraction::TwentyFourth),
];
pub(super) const WEIGHTINGS: [(&str, Weighting); 3] = [
    ("Z (flat)", Weighting::Z),
    ("A", Weighting::A),
    ("C", Weighting::C),
];
pub(super) const TIME_WEIGHTINGS: [(&str, TimeWeighting); 3] = [
    ("fast (125 ms)", TimeWeighting::Fast),
    ("slow (1 s)", TimeWeighting::Slow),
    ("impulse", TimeWeighting::Impulse),
];

/// `48000`, `48k`, `48 kHz`, `44.1khz`; empty = the device default.
pub fn parse_rate(text: &str) -> Result<Option<u32>, String> {
    let t = text.trim().to_ascii_lowercase().replace(' ', "");
    if t.is_empty() {
        return Ok(None);
    }
    let (num, scale) = if let Some(n) = t.strip_suffix("khz").or_else(|| t.strip_suffix('k')) {
        (n, 1000.0)
    } else {
        (t.strip_suffix("hz").unwrap_or(&t), 1.0)
    };
    let v = num
        .replace(',', ".")
        .parse::<f64>()
        .map_err(|_| format!("not a sample rate: {:?}", text.trim()))?
        * scale;
    let r = v.round();
    if !(1000.0..=1_000_000.0).contains(&r) || (v - r).abs() > 1e-6 {
        return Err(format!("{} Hz is not a usable sample rate", text.trim()));
    }
    Ok(Some(r as u32))
}

/// Buffer size in frames; empty = the device default.
pub fn parse_buffer(text: &str) -> Result<Option<u32>, String> {
    let t = text.trim().to_ascii_lowercase();
    let t = t
        .strip_suffix("samples")
        .or_else(|| t.strip_suffix("frames"))
        .unwrap_or(&t)
        .trim();
    if t.is_empty() {
        return Ok(None);
    }
    match t.parse::<u32>() {
        Ok(n) if n > 0 => Ok(Some(n)),
        _ => Err(format!("not a buffer size: {:?}", text.trim())),
    }
}
