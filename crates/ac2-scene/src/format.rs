//! Number formatting for every displayed string.
//!
//! Rules (each one is asserted in the tests below):
//! - Negative numbers use the typographic minus `−` (U+2212), which the bundled font has and
//!   which lines up with `+` in tabular columns; a value that rounds to zero carries no sign.
//! - A missing or invalid value is `—` (em dash), never `NaN`, `inf` or a stale number.
//! - Frequencies at or above 1 kHz use a `k` suffix; tick labels use as few decimals as the
//!   value needs (`20`, `31.5`, `1k`, `1.05k`), readouts use three significant digits with
//!   the unit (`63.1 Hz`, `1.00 kHz`).
//! - Units are separated from numbers by a space (`−3.2 dB`, `12.34 ms`), except degrees
//!   (`+45°`).

/// Typographic minus sign.
pub const MINUS: char = '\u{2212}';
/// Shown in place of a value that is missing or invalid.
pub const NO_VALUE: &str = "—";

/// `v` with exactly `decimals` decimals, typographic minus, no sign on a rounded zero.
pub fn fixed(v: f64, decimals: usize) -> String {
    if !v.is_finite() {
        return NO_VALUE.to_string();
    }
    let s = format!("{:.*}", decimals, v.abs());
    if v < 0.0 && !is_zero_text(&s) {
        format!("{MINUS}{s}")
    } else {
        s
    }
}

/// Like [`fixed`] but always signed (`+3.0`, `−3.0`, `0.0`).
pub fn signed(v: f64, decimals: usize) -> String {
    if !v.is_finite() {
        return NO_VALUE.to_string();
    }
    let s = fixed(v, decimals);
    if v > 0.0 && !is_zero_text(&s) {
        format!("+{s}")
    } else {
        s
    }
}

fn is_zero_text(s: &str) -> bool {
    s.chars().all(|c| c == '0' || c == '.')
}

/// Fewest decimals (up to `max`) that represent `v` exactly to within float noise.
fn needed_decimals(v: f64, max: usize) -> usize {
    (0..=max)
        .find(|&d| {
            let scale = 10f64.powi(d as i32);
            let x = v * scale;
            (x - x.round()).abs() < 1e-6 * x.abs().max(1.0)
        })
        .unwrap_or(max)
}

/// Frequency tick label: `20`, `31.5`, `500`, `1k`, `1.05k`, `12.5k`, `20k`.
pub fn freq_tick(hz: f64) -> String {
    if !hz.is_finite() {
        return NO_VALUE.to_string();
    }
    if hz.abs() >= 1000.0 {
        let k = hz / 1000.0;
        format!("{}k", fixed(k, needed_decimals(k, 3)))
    } else {
        fixed(hz, needed_decimals(hz, 3))
    }
}

/// `v` rounded to `sig` significant digits.
fn round_sig(v: f64, sig: i32) -> f64 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    let mag = v.abs().log10().floor() as i32;
    let scale = 10f64.powi(sig - 1 - mag);
    (v * scale).round() / scale
}

/// Decimals that show three significant digits of a value already rounded to three.
fn three_sig_decimals(v: f64) -> usize {
    let a = v.abs();
    if a < 10.0 {
        2
    } else if a < 100.0 {
        1
    } else {
        0
    }
}

/// Frequency readout, three significant digits with unit: `20.0 Hz`, `63.1 Hz`, `125 Hz`,
/// `1.00 kHz`, `12.5 kHz`.
pub fn freq_readout(hz: f64) -> String {
    if !hz.is_finite() || hz <= 0.0 {
        return NO_VALUE.to_string();
    }
    let r = round_sig(hz, 3);
    if r >= 1000.0 {
        let k = r / 1000.0;
        format!("{} kHz", fixed(k, three_sig_decimals(k)))
    } else {
        format!("{} Hz", fixed(r, three_sig_decimals(r)))
    }
}

/// Voltage, four significant digits (a bench meter's resolution) in µV, mV or V:
/// `15.03 mV`, `1.500 V`, `250.0 µV`.
pub fn volts(v: f64) -> String {
    if !v.is_finite() || v <= 0.0 {
        return NO_VALUE.to_string();
    }
    let r = round_sig(v, 4);
    let (x, unit) = if r >= 1.0 {
        (r, "V")
    } else if r >= 1e-3 {
        (r * 1e3, "mV")
    } else {
        (r * 1e6, "µV")
    };
    let decimals = if x < 10.0 {
        3
    } else if x < 100.0 {
        2
    } else if x < 1000.0 {
        1
    } else {
        0
    };
    format!("{} {unit}", fixed(x, decimals))
}

/// Level difference readout: `+3.2 dB`, `−12.0 dB`.
pub fn db_readout(db: f64) -> String {
    with_unit(signed(db, 1), " dB")
}

/// Phase readout, whole degrees: `+45°`, `−170°`, `0°`.
pub fn phase_readout(deg: f64) -> String {
    with_unit(signed(deg, 0), "°")
}

/// Coherence γ² readout: `0.93`.
pub fn coherence_readout(g2: f64) -> String {
    fixed(g2, 2)
}

/// A step of a delay in samples, signed, with as few decimals as it needs (up to 3):
/// `+1 sample`, `−0.1 sample`, `+2 samples`.
pub fn sample_step(samples: f64) -> String {
    if !samples.is_finite() {
        return NO_VALUE.to_string();
    }
    let unit = if samples.abs() > 1.0 {
        "samples"
    } else {
        "sample"
    };
    format!("{} {unit}", signed(samples, needed_decimals(samples, 3)))
}

/// A step in seconds as signed ms with as few decimals as it needs (up to 3): `+0.1 ms`.
fn step_ms(seconds: f64) -> String {
    let ms = seconds * 1000.0;
    with_unit(signed(ms, needed_decimals(ms, 3)), " ms")
}

/// One step of a measurement's delay, in the unit its key steps by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DelayStep {
    /// Whole or fractional samples of the session rate (Ctrl / Alt).
    Samples(f64),
    /// Seconds (the plain keys' 0.1 ms).
    Seconds(f64),
}

/// A measurement's delay from its measured arrival, signed, to the precision of
/// [`delay`] (so that a delay and its offset read to the same digit): `+0.02 ms`,
/// `−0.002 ms`.
pub fn arrival_offset(seconds: f64) -> String {
    let ms_value = seconds * 1000.0;
    with_unit(signed(ms_value, needed_decimals(ms_value, 3).max(2)), " ms")
}

/// `+0.02 ms from arrival`: what the legend tags a live curve with and the delay text puts
/// in brackets; `None` when the delay is the arrival (a float residue of the arithmetic
/// included).
pub fn from_arrival(seconds: f64) -> Option<String> {
    let ms_value = seconds * 1000.0;
    if !ms_value.is_finite() || is_zero_text(&fixed(ms_value.abs(), 3)) {
        return None;
    }
    Some(format!("{} from arrival", arrival_offset(seconds)))
}

/// A measurement's one delay and its offset from the measured arrival, as its list row,
/// its toasts and the CLI say it: `delay 0.94 ms (+0.02 ms from arrival)`, `delay 0.94 ms`.
pub fn meas_delay(applied_s: f64, offset_s: f64) -> String {
    format!("delay {}", delay_and_offset(applied_s, offset_s))
}

/// `0.94 ms (+0.02 ms from arrival)`, `0.94 ms`: [`meas_delay`] where a column already says
/// "delay".
pub fn delay_and_offset(applied_s: f64, offset_s: f64) -> String {
    match from_arrival(offset_s) {
        Some(o) => format!("{} ({o})", delay(applied_s)),
        None => delay(applied_s),
    }
}

/// The toast after a step of a measurement's delay: the step, then the delay it lands on:
/// `TF 2: delay +0.1 ms → 0.94 ms (+0.02 ms from arrival)`, `TF 2: delay −1 sample → 12.50 ms`.
pub fn delay_step_toast(name: &str, step: DelayStep, applied_s: f64, offset_s: f64) -> String {
    let step = match step {
        DelayStep::Samples(n) => sample_step(n),
        DelayStep::Seconds(s) => step_ms(s),
    };
    format!(
        "{name}: delay {step} \u{2192} {}",
        delay_and_offset(applied_s, offset_s)
    )
}

/// The toast after a typed delay: `TF 2: delay 0.94 ms (+0.02 ms from arrival)`.
pub fn typed_delay_toast(name: &str, applied_s: f64, offset_s: f64) -> String {
    format!("{name}: {}", meas_delay(applied_s, offset_s))
}

/// A stored trace's delay from the arrival it was captured at (its nudge), after its state
/// in its list row: `delay +0.30 ms from arrival`; `None` at the arrival. A stored trace
/// keeps the delay it was measured with only as the time base of its phase, not the
/// arrival it had then, so only the offset is said; an imported or averaged trace's
/// arrival is its own alignment.
pub fn trace_delay(offset_s: f64) -> Option<String> {
    from_arrival(offset_s).map(|o| format!("delay {o}"))
}

/// The toast after a step of a stored trace's delay, in the shape of a measurement's:
/// `1083 94cm: delay −0.1 ms → +0.20 ms from arrival`, `… → at arrival`.
pub fn trace_delay_toast(name: &str, step_s: f64, offset_s: f64) -> String {
    let landed = from_arrival(offset_s).unwrap_or_else(|| "at arrival".to_owned());
    format!("{name}: delay {} \u{2192} {landed}", step_ms(step_s))
}

/// An applied delay: milliseconds to 10 µs as everywhere else, with a third decimal when
/// the delay has a finer part, so that a step of a tenth of a sample (2 µs at 48 kHz)
/// shows: `12.50 ms`, `12.502 ms`.
pub fn delay(seconds: f64) -> String {
    let ms_value = seconds * 1000.0;
    ms(seconds, needed_decimals(ms_value, 3).max(2))
}

/// An applied delay as the number a delay prompt starts from, in ms without the unit and
/// with as many decimals as it needs up to 5 (10 ns), so that confirming it unchanged keeps
/// a fractional-sample delay: `12.5`, `12.50208`.
pub fn delay_entry_ms(seconds: f64) -> String {
    let ms = seconds * 1000.0;
    fixed(ms, needed_decimals(ms, 5))
}

/// Milliseconds with `decimals`: `12.34 ms`.
pub fn ms(seconds: f64, decimals: usize) -> String {
    with_unit(fixed(seconds * 1000.0, decimals), " ms")
}

/// Fractional-octave smoothing: `1/6 oct` (magnitude and phase, what front ends set),
/// `1/12 oct mag only` (phase as measured), `off`.
pub fn smoothing(s: Option<ac2_proto::model::Smoothing>) -> String {
    match s {
        None => "off".into(),
        Some(s) => match s.mode {
            ac2_proto::model::SmoothingMode::MagnitudePhase => octave_fraction(s.fraction),
            ac2_proto::model::SmoothingMode::Magnitude => {
                format!("{} mag only", octave_fraction(s.fraction))
            }
        },
    }
}

/// `1/6 oct`.
pub fn octave_fraction(f: ac2_proto::model::SmoothingFraction) -> String {
    format!("1/{} oct", f.b())
}

/// Absolute level with one decimal: `94.0`, `−23.5`.
pub fn level(v: f64) -> String {
    fixed(v, 1)
}

fn with_unit(num: String, unit: &str) -> String {
    if num == NO_VALUE { num } else { num + unit }
}

/// Age of displayed data: `0.4 s`, `9.9 s`, `12 s`, `3 min`, `2 h`, `3 d`.
/// Below 10 s one decimal, because the STALE threshold (1 s) is read against it.
pub fn age(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return NO_VALUE.to_string();
    }
    if seconds < 9.95 {
        format!("{} s", fixed(seconds, 1))
    } else if seconds < 59.5 {
        format!("{} s", fixed(seconds, 0))
    } else if seconds < 3600.0 {
        format!("{} min", (seconds / 60.0).floor().max(1.0))
    } else if seconds < 86_400.0 {
        format!("{} h", (seconds / 3600.0).floor())
    } else {
        format!("{} d", (seconds / 86_400.0).floor())
    }
}

/// Seconds until [`age`] of an age growing from `seconds` reads differently: a counting
/// `STALE · 4.2 s` needs redrawing ten times a second, `STALE · 3 min` once a minute.
pub fn age_changes_in(seconds: f64) -> f64 {
    if seconds.is_nan() || seconds == f64::INFINITY {
        return f64::INFINITY;
    }
    if seconds < 0.0 {
        return -seconds;
    }
    let next = if seconds < 9.95 {
        (((seconds * 10.0).round() + 0.5) / 10.0).min(9.95)
    } else if seconds < 59.5 {
        (seconds.round() + 0.5).min(59.5)
    } else if seconds < 3600.0 {
        (((seconds / 60.0).floor().max(1.0) + 1.0) * 60.0).min(3600.0)
    } else if seconds < 86_400.0 {
        (((seconds / 3600.0).floor() + 1.0) * 3600.0).min(86_400.0)
    } else {
        ((seconds / 86_400.0).floor() + 1.0) * 86_400.0
    };
    (next - seconds).max(0.0)
}

/// The step [`age`] counts in at `seconds`: 0.1 s, 1 s, a minute, an hour, a day.
pub fn age_step(seconds: f64) -> f64 {
    if seconds < 9.95 {
        0.1
    } else if seconds < 59.5 {
        1.0
    } else if seconds < 3600.0 {
        60.0
    } else if seconds < 86_400.0 {
        3600.0
    } else {
        86_400.0
    }
}

/// How long ago something happened, coarse: `just now`, `5 min ago`, `3 h ago`, `2 d ago`.
pub fn ago(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return NO_VALUE.to_string();
    }
    if seconds < 60.0 {
        "just now".to_string()
    } else if seconds < 3600.0 {
        format!("{} min ago", (seconds / 60.0).floor())
    } else if seconds < 86_400.0 {
        format!("{} h ago", (seconds / 3600.0).floor())
    } else {
        format!("{} d ago", (seconds / 86_400.0).floor())
    }
}

/// Duration of an integration interval: `12.0 s`, `1 min 23 s`, `2 h 05 min`.
pub fn duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return NO_VALUE.to_string();
    }
    if seconds < 60.0 {
        format!("{} s", fixed(seconds, 1))
    } else if seconds < 3600.0 {
        let s = seconds.floor() as u64;
        format!("{} min {:02} s", s / 60, s % 60)
    } else {
        let m = (seconds / 60.0).floor() as u64;
        format!("{} h {:02} min", m / 60, m % 60)
    }
}

/// Impulse-response amplitude, signed (polarity is what the linear view shows), three
/// significant digits re full scale: `+0.500 FS`, `−0.0123 FS`, `0 FS`.
pub fn amplitude_readout(v: f64) -> String {
    if !v.is_finite() {
        return NO_VALUE.to_string();
    }
    let r = round_sig(v, 3);
    if r == 0.0 {
        return "0 FS".to_string();
    }
    // Three significant digits whatever the decade, down to a millionth of full scale.
    let decimals = (2 - r.abs().log10().floor() as i32).clamp(0, 8) as usize;
    with_unit(signed(r, decimals), " FS")
}

/// A time on an impulse response, ms with the decimals one sample `dt_ms` apart needs
/// (at 48 kHz, 0.021 ms: two): `1.25 ms`, `−0.23 ms`; three without a spacing.
pub fn ir_time(t_ms: f64, dt_ms: f64) -> String {
    let decimals = if dt_ms > 0.0 && dt_ms.is_finite() {
        (-(dt_ms.log10()).floor()).clamp(0.0, 6.0) as usize
    } else {
        3
    };
    ms(t_ms / 1000.0, decimals)
}

/// Temperature: `20 °C`, `22.5 °C`, `−5 °C`.
pub fn celsius(t: f64) -> String {
    with_unit(fixed(t, needed_decimals(t, 1)), " °C")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ir_readouts() {
        assert_eq!(amplitude_readout(0.5), "+0.500 FS");
        assert_eq!(amplitude_readout(-0.012_34), "−0.0123 FS");
        assert_eq!(amplitude_readout(12.345), "+12.3 FS");
        assert_eq!(amplitude_readout(0.0), "0 FS");
        assert_eq!(amplitude_readout(f64::NAN), "—");
        // 48 kHz: 0.0208 ms a sample, two decimals; 0.1 ms, one; 1 ms, none.
        assert_eq!(ir_time(1.25, 1.0 / 48.0), "1.25 ms");
        assert_eq!(ir_time(-0.229, 1.0 / 48.0), "−0.23 ms");
        assert_eq!(ir_time(1.26, 0.1), "1.3 ms");
        assert_eq!(ir_time(120.4, 1.0), "120 ms");
        assert_eq!(ir_time(1.25, 0.0), "1.250 ms");
    }

    #[test]
    fn minus_and_zero() {
        assert_eq!(fixed(-3.25, 1), "−3.2");
        assert_eq!(fixed(-0.04, 1), "0.0");
        assert_eq!(signed(0.04, 1), "0.0");
        assert_eq!(signed(3.0, 1), "+3.0");
        assert_eq!(signed(-3.0, 1), "−3.0");
        assert_eq!(fixed(f64::NAN, 1), "—");
        assert_eq!(signed(f64::INFINITY, 1), "—");
    }

    #[test]
    fn a_trace_says_its_delay_as_a_measurement_does() {
        assert_eq!(trace_delay(0.0), None);
        assert_eq!(
            trace_delay(0.000_3).as_deref(),
            Some("delay +0.30 ms from arrival")
        );
        // One sample at 48 kHz still reads as a delay.
        assert_eq!(
            trace_delay(1.0 / 48_000.0).as_deref(),
            Some("delay +0.021 ms from arrival")
        );
        assert_eq!(
            trace_delay(-0.000_126).as_deref(),
            Some("delay −0.126 ms from arrival")
        );
        assert_eq!(
            trace_delay_toast("1083 94cm", 0.000_1, 0.000_3),
            "1083 94cm: delay +0.1 ms → +0.30 ms from arrival"
        );
        assert_eq!(
            trace_delay_toast("1083 94cm", -0.000_1, 0.0),
            "1083 94cm: delay −0.1 ms → at arrival"
        );
    }

    #[test]
    fn a_measurement_has_one_delay_and_its_offset_from_the_arrival() {
        assert_eq!(arrival_offset(0.000_02), "+0.02 ms");
        assert_eq!(arrival_offset(-0.000_002), "−0.002 ms");
        assert_eq!(from_arrival(0.0), None);
        // A float residue of the arithmetic is no offset.
        assert_eq!(from_arrival(1e-12), None);
        assert_eq!(from_arrival(-1e-12), None);
        assert_eq!(from_arrival(f64::NAN), None);
        assert_eq!(
            from_arrival(0.000_02).as_deref(),
            Some("+0.02 ms from arrival")
        );
        // A tenth of a sample at 96 kHz still shows.
        assert_eq!(
            from_arrival(0.1 / 96_000.0).as_deref(),
            Some("+0.001 ms from arrival")
        );
        assert_eq!(
            meas_delay(0.000_94, 0.000_02),
            "delay 0.94 ms (+0.02 ms from arrival)"
        );
        assert_eq!(
            meas_delay(0.012_5, -0.001),
            "delay 12.50 ms (−1.00 ms from arrival)"
        );
        assert_eq!(meas_delay(0.000_94, 0.0), "delay 0.94 ms");
        assert_eq!(
            delay_step_toast("TF 2", DelayStep::Seconds(0.000_1), 0.000_94, 0.000_02),
            "TF 2: delay +0.1 ms → 0.94 ms (+0.02 ms from arrival)"
        );
        assert_eq!(
            delay_step_toast("TF 2", DelayStep::Samples(-1.0), 0.012_5, 0.0),
            "TF 2: delay −1 sample → 12.50 ms"
        );
        assert_eq!(
            delay_step_toast(
                "TF 2",
                DelayStep::Samples(0.1),
                0.0125 + 0.1 / 48_000.0,
                0.1 / 48_000.0
            ),
            "TF 2: delay +0.1 sample → 12.502 ms (+0.002 ms from arrival)"
        );
        assert_eq!(
            typed_delay_toast("TF 2", 0.000_94, 0.000_02),
            "TF 2: delay 0.94 ms (+0.02 ms from arrival)"
        );
        assert_eq!(
            typed_delay_toast("TF 2", 0.012_5, -0.001),
            "TF 2: delay 12.50 ms (−1.00 ms from arrival)"
        );
        assert_eq!(
            typed_delay_toast("TF 2", 0.000_94, 1e-12),
            "TF 2: delay 0.94 ms"
        );
    }

    #[test]
    fn delay_steps_and_readout() {
        assert_eq!(sample_step(1.0), "+1 sample");
        assert_eq!(sample_step(-1.0), "−1 sample");
        assert_eq!(sample_step(0.1), "+0.1 sample");
        assert_eq!(sample_step(-0.1), "−0.1 sample");
        assert_eq!(sample_step(2.0), "+2 samples");
        assert_eq!(sample_step(-0.25), "−0.25 sample");
        assert_eq!(sample_step(f64::NAN), "—");
        assert_eq!(delay(0.012_502_083), "12.502 ms");
        assert_eq!(delay(0.0125), "12.50 ms");
        assert_eq!(delay(-0.000_002_083), "−0.002 ms");
        assert_eq!(delay(0.0), "0.00 ms");
        assert_eq!(delay_entry_ms(0.0125), "12.5");
        assert_eq!(delay_entry_ms(600.1 / 48_000.0), "12.50208");
        assert_eq!(delay_entry_ms(0.0), "0");
    }

    #[test]
    fn freq_ticks() {
        let got: Vec<String> = [
            20.0, 31.5, 50.0, 100.0, 500.0, 1000.0, 1050.0, 2000.0, 12500.0, 20000.0, 999.0, 1001.0,
        ]
        .iter()
        .map(|&f| freq_tick(f))
        .collect();
        assert_eq!(
            got,
            [
                "20", "31.5", "50", "100", "500", "1k", "1.05k", "2k", "12.5k", "20k", "999",
                "1.001k"
            ]
        );
    }

    #[test]
    fn freq_readouts() {
        let cases = [
            (5.0, "5.00 Hz"),
            (20.0, "20.0 Hz"),
            (63.0957, "63.1 Hz"),
            (125.0, "125 Hz"),
            (999.6, "1.00 kHz"),
            (1000.0, "1.00 kHz"),
            (1059.25, "1.06 kHz"),
            (12_500.0, "12.5 kHz"),
            (20_000.0, "20.0 kHz"),
            (0.0, "—"),
        ];
        for (f, want) in cases {
            assert_eq!(freq_readout(f), want, "{f}");
        }
    }

    #[test]
    fn readouts() {
        assert_eq!(db_readout(-3.249), "−3.2 dB");
        assert_eq!(db_readout(0.0), "0.0 dB");
        assert_eq!(db_readout(f64::NAN), "—");
        assert_eq!(phase_readout(44.6), "+45°");
        assert_eq!(phase_readout(-170.2), "−170°");
        assert_eq!(phase_readout(-0.4), "0°");
        assert_eq!(coherence_readout(0.934), "0.93");
        assert_eq!(ms(0.012_345, 2), "12.35 ms");
        assert_eq!(ms(-0.0015, 2), "−1.50 ms");
        assert_eq!(level(94.04), "94.0");
        assert_eq!(celsius(20.0), "20 °C");
        assert_eq!(celsius(22.5), "22.5 °C");
        assert_eq!(celsius(-5.0), "−5 °C");
    }

    /// The text stays the same up to the predicted change and differs just after it, over
    /// every format band.
    #[test]
    fn age_changes_when_predicted() {
        let mut s = 0.0;
        while s < 3.0 * 86_400.0 {
            let d = age_changes_in(s);
            assert!(d > 0.0, "{s}");
            assert_eq!(age(s + d - 1e-6), age(s), "{s} + {d}");
            assert_ne!(age(s + d + 1e-6), age(s), "{s} + {d}");
            s += if s < 70.0 { 0.037 } else { 97.3 };
        }
        assert_eq!(age_changes_in(f64::INFINITY), f64::INFINITY);
        assert_eq!(age_step(4.0), 0.1);
        assert_eq!(age_step(30.0), 1.0);
        assert_eq!(age_step(600.0), 60.0);
        assert!((age_changes_in(-0.5) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn ages() {
        let cases = [
            (0.0, "0.0 s"),
            (1.04, "1.0 s"),
            (9.94, "9.9 s"),
            (9.96, "10 s"),
            (59.4, "59 s"),
            (61.0, "1 min"),
            (3599.0, "59 min"),
            (7300.0, "2 h"),
            (200_000.0, "2 d"),
            (-1.0, "—"),
        ];
        for (s, want) in cases {
            assert_eq!(age(s), want, "{s}");
        }
        assert_eq!(ago(30.0), "just now");
        assert_eq!(ago(300.0), "5 min ago");
        assert_eq!(ago(3.0 * 3600.0 + 59.0), "3 h ago");
        assert_eq!(ago(2.5 * 86_400.0), "2 d ago");
        assert_eq!(duration(12.0), "12.0 s");
        assert_eq!(duration(83.9), "1 min 23 s");
        assert_eq!(duration(2.0 * 3600.0 + 5.0 * 60.0 + 30.0), "2 h 05 min");
    }
}
