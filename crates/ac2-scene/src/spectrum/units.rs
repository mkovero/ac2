//! Units and captions: what a level value reads in, the axis unit for a bin width, and
//! the labels of a band fraction, weighting and window.

use super::*;

/// The unit a value of `q` in `scale` reads in: `dBFS`, `dB SPL`, `dBFS/Hz`, `dB SPL/Hz`.
pub fn value_unit(scale: LevelScale, q: Quantity) -> &'static str {
    match (scale, q.is_density()) {
        (LevelScale::Dbfs, false) => "dBFS",
        (LevelScale::DbSpl, false) => "dB SPL",
        (LevelScale::Dbfs, true) => "dBFS/Hz",
        (LevelScale::DbSpl, true) => "dB SPL/Hz",
    }
}

/// What a value of `q` is, as the axis unit's parentheses say it: `tone`, `density`,
/// `band`, with the smoothing when there is one. A tone and a density of the same signal
/// differ by the bin width in dB, so a reading without it can't be compared.
pub fn quantity_note(q: Quantity) -> String {
    match q {
        Quantity::Tone => "tone".to_string(),
        Quantity::Density => "density".to_string(),
        Quantity::Band => "band".to_string(),
        Quantity::SmoothedTone(f) => format!("tone, {} smoothed", format::octave_fraction(f)),
        Quantity::SmoothedDensity(f) => {
            format!("density, {} smoothed", format::octave_fraction(f))
        }
    }
}

/// Caption suffix for a calibrated trace (decisions 7a/7b): the calibration's age when it
/// belongs to this device + input + mic, the mismatch warning when it belongs to another mic
/// or input, and the mic-curve note ([`crate::cal::curve_note`]: which curve was
/// subtracted, or why none was). Uncalibrated says nothing: the axis unit is dBFS. The age
/// is `captured − calibrated_at`, both on the daemon clock
/// (`docs/design/q7-calibration.md` §3), so no client clock offset enters it.
pub fn cal_caption(cal: CalStatus, curve: Option<&str>, captured: WallNs) -> String {
    let mut s = String::new();
    if let CalStatus::Verified { calibrated_at, .. }
    | CalStatus::OtherMicOrInput { calibrated_at, .. } = cal
    {
        let age = captured.0.saturating_sub(calibrated_at.0) as f64 / 1e9;
        if let Some(t) = crate::cal::status_text(cal, age) {
            s.push_str(" · ");
            s.push_str(&t);
        }
    }
    if let Some(c) = curve {
        s.push_str(" · ");
        s.push_str(c);
    }
    s
}

/// What a narrowband level is "per": the FFT bin spacing of its grid.
///
/// The width named is the bin spacing `fs / N`, not the window's equivalent noise bandwidth.
/// With tone normalisation a broadband signal of density `S` reads `S · ENBW` in a bin, and
/// ENBW is 1.5 bins for Hann (1.76 dB above `S · fs / N`), 1.0 for rectangular, about 3.8
/// for flat-top: it is a property of the window, while the spacing is what the FFT length
/// sets and what every spectrum grid, live or stored, carries. The label's job is to say
/// which readings are comparable (same spacing and window: same broadband reading) and how
/// a change of FFT length moves broadband sound (3 dB per doubling, whatever the window);
/// a noise density would need the window's ENBW and is not what this axis claims.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BinWidth {
    /// Not an FFT grid (an imported spectrum on another grid).
    Unknown,
    /// Spacing, Hz.
    Hz(f64),
    /// Curves on different spacings share the axis.
    Mixed,
    /// A multi-resolution spectrum: each column's bandwidth is its window's, wider the
    /// higher the column.
    MultiRes,
}

impl BinWidth {
    pub fn of(bin_hz: Option<f64>) -> Self {
        bin_hz.map_or(BinWidth::Unknown, BinWidth::Hz)
    }
}

/// Axis unit, longest form: `dBFS per 1.46 Hz bin (tone)`, `dB SPL (band)`,
/// `dBFS per 11.7 Hz bin (tone, 1/6 oct smoothed)`, `dBFS/Hz (density, multi-res)`.
pub fn level_unit(scale: LevelScale, q: Quantity, bin: BinWidth) -> String {
    level_unit_forms(scale, q, bin).swap_remove(0)
}

/// The axis unit from its longest form to its shortest, for a pane too narrow for the full
/// one: what goes first is what the pane title or the RTA's own axis also says (`tone`,
/// the smoothing), the scale always stays.
pub fn level_unit_forms(scale: LevelScale, q: Quantity, bin: BinWidth) -> Vec<String> {
    let s = scale_unit(scale);
    let smoothed = match q {
        Quantity::SmoothedTone(f) | Quantity::SmoothedDensity(f) => {
            Some(format!("{} smoothed", format::octave_fraction(f)))
        }
        _ => None,
    };
    if q.is_density() {
        // A density is per hertz whatever the bin width: no bin to name.
        let d = value_unit(scale, q);
        let what = match &smoothed {
            Some(sm) => format!("density, {sm}"),
            None => "density".to_string(),
        };
        let mut v = match bin {
            BinWidth::MultiRes => vec![format!("{d} ({what}, multi-res)")],
            _ => vec![],
        };
        v.push(format!("{d} ({what})"));
        if smoothed.is_some() {
            v.push(format!("{d} (density)"));
        }
        v.push(d.to_string());
        v.dedup();
        return v;
    }
    let tone = match &smoothed {
        Some(sm) => format!("tone, {sm}"),
        None => "tone".to_string(),
    };
    let mut v = match (q, bin) {
        (Quantity::Band, _) => vec![format!("{s} (band)")],
        (_, BinWidth::Unknown) => vec![format!("{s} ({tone})")],
        (_, BinWidth::Hz(b)) => {
            let per = format!("{s} per {} bin", format::freq_readout(b));
            let mut v = vec![format!("{per} ({tone})")];
            if let Some(sm) = &smoothed {
                v.push(format!("{per} ({sm})"));
            }
            v.push(per);
            v.push(format!("{s}/bin"));
            v
        }
        (_, BinWidth::MultiRes) => {
            let mut v = vec![format!("{s} ({tone}, multi-res)")];
            if let Some(sm) = &smoothed {
                v.push(format!("{s} ({sm}, multi-res)"));
            }
            v.push(format!("{s} (multi-res)"));
            v
        }
        (_, BinWidth::Mixed) => {
            let per = format!("{s} per bin, mixed widths");
            vec![format!("{per} ({tone})"), per, format!("{s}/bin")]
        }
    };
    v.push(s.to_string());
    v.dedup();
    v
}

/// What the level axis of a narrowband spectrum of quantity `q` means, for its tooltip and
/// the help.
pub fn narrow_unit_help(q: Quantity, bin: BinWidth) -> String {
    if q.is_density() {
        density_unit_help(bin)
    } else {
        tone_unit_help(bin)
    }
}

/// What a density axis means.
pub fn density_unit_help(bin: BinWidth) -> String {
    let source = if bin == BinWidth::MultiRes {
        "from windows of 0.68 s at the lowest frequencies to 21 ms at the highest"
    } else {
        "per hertz"
    };
    format!(
        "Power spectral density {source}: the power each hertz holds, so noise and music \
         read the same on a fixed or a multi-resolution spectrum, at any FFT length (white \
         noise flat, pink falling 3 dB per octave). A sine reads below its level by \
         10·log10 of the bin's noise bandwidth in hertz (Hann, 1.46 Hz bins: 3.4 dB). Use \
         tone level to read a sine, an RTA for band levels in dB SPL."
    )
}

/// What the level axis of a narrowband spectrum means, for its tooltip and the help.
pub fn tone_unit_help(bin: BinWidth) -> String {
    if bin == BinWidth::MultiRes {
        return "Tone level from windows of 0.68 s at the lowest frequencies to 21 ms at the \
                highest: a sine reads its RMS level at any frequency. Each column's \
                bandwidth grows with its frequency, so broadband sound reads as in a \
                constant fraction of an octave (pink noise flat, white rising 3 dB per \
                octave) between the window changes the dashed lines mark. Use an RTA for \
                band levels in dB SPL."
            .to_string();
    }
    let per = match bin {
        BinWidth::Hz(b) => format!("per FFT bin ({} wide)", format::freq_readout(b)),
        BinWidth::Mixed | BinWidth::Unknown | BinWidth::MultiRes => "per FFT bin".to_string(),
    };
    format!(
        "Level {per}: a sine reads its RMS level. Broadband sound reads lower than its band \
         or total level, and lower with finer bins (3 dB per doubling of the FFT length). \
         Use an RTA for band levels in dB SPL."
    )
}

pub(crate) fn scale_unit(scale: LevelScale) -> &'static str {
    match scale {
        LevelScale::Dbfs => "dBFS",
        LevelScale::DbSpl => "dB SPL",
    }
}

/// `1/3 oct`.
pub fn fraction_label(f: BandFraction) -> String {
    format!("1/{} oct", f.b())
}

/// `A-weighted`, `C-weighted`, `Z (unweighted)`.
pub fn weighting_label(w: Weighting) -> &'static str {
    match w {
        Weighting::A => "A-weighted",
        Weighting::C => "C-weighted",
        Weighting::Z => "Z (unweighted)",
    }
}

pub fn window_label(w: Window) -> &'static str {
    match w {
        Window::Hann => "Hann window",
        Window::BlackmanHarris4 => "Blackman-Harris window",
        Window::FlatTop => "flat-top window",
        Window::Rectangular => "rectangular window",
    }
}
