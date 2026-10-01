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

/// Milliseconds with `decimals`: `12.34 ms`.
pub fn ms(seconds: f64, decimals: usize) -> String {
    with_unit(fixed(seconds * 1000.0, decimals), " ms")
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

/// Temperature: `20 °C`, `22.5 °C`, `−5 °C`.
pub fn celsius(t: f64) -> String {
    with_unit(fixed(t, needed_decimals(t, 1)), " °C")
}

#[cfg(test)]
mod tests {
    use super::*;

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
