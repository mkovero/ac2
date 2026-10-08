//! 1/3-octave band levels as text: what a FOH → dwelling transfer is computed from when the
//! levels were measured with another instrument (`spl.band_transfer` with typed levels,
//! `docs/design/band-leq.md`, *The transfer*).
//!
//! ```text
//! # dwelling, pink noise, 2026-10-08 18:00–18:01, dB SPL unweighted Leq
//! 20      61.2
//! 25      58.0
//! 31.5    55.4
//! ```
//!
//! One band per line: its centre (Hz) and its level (dB SPL), separated by spaces, tabs, a
//! comma or a semicolon; `#` starts a comment line. The centre is the nominal one of a band
//! 20 Hz … 10 kHz or its exact base-ten centre (within 2 %); a band not listed is not
//! measured. A band listed twice, a frequency of no band, or a level that is not a finite
//! number is refused with its line number.

use ac2_proto::model::{BAND_COUNT, BAND_NOMINAL_HZ};
use thiserror::Error;

/// A band level file that does not read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BandLevelsError {
    /// A malformed line.
    #[error("line {line}: {msg}")]
    Line {
        /// 1-based line number.
        line: usize,
        /// What is wrong.
        msg: String,
    },
    /// No band at all.
    #[error("no band levels")]
    Empty,
}

/// The band of `hz`, by its nominal or exact centre within 2 %.
fn band_of(hz: f64) -> Option<usize> {
    (0..BAND_COUNT).find(|&i| {
        let exact = 1000.0 * 10f64.powf((i as f64 - 17.0) / 10.0);
        [BAND_NOMINAL_HZ[i], exact]
            .iter()
            .any(|c| (hz / c - 1.0).abs() <= 0.02)
    })
}

/// Reads levels per band of [`BAND_NOMINAL_HZ`], dB SPL (`None`: not listed).
pub fn parse(text: &str) -> Result<[Option<f64>; BAND_COUNT], BandLevelsError> {
    let mut out = [None; BAND_COUNT];
    let mut any = false;
    for (i, l) in text.lines().enumerate() {
        let line = i + 1;
        let bad = |msg: String| BandLevelsError::Line { line, msg };
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = l
            .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
            .filter(|t| !t.is_empty())
            .collect();
        let [hz, db] = f[..] else {
            return Err(bad("expected a frequency and a level".into()));
        };
        let hz: f64 = hz
            .parse()
            .map_err(|_| bad(format!("{hz:?} is not a frequency")))?;
        let db: f64 = db
            .parse()
            .ok()
            .filter(|v: &f64| v.is_finite())
            .ok_or_else(|| bad(format!("{db:?} is not a level")))?;
        let b = band_of(hz)
            .ok_or_else(|| bad(format!("{hz} Hz is no 1/3-octave band 20 Hz … 10 kHz")))?;
        if out[b].is_some() {
            return Err(bad(format!(
                "the {} Hz band is listed twice",
                BAND_NOMINAL_HZ[b]
            )));
        }
        out[b] = Some(db);
        any = true;
    }
    if !any {
        return Err(BandLevelsError::Empty);
    }
    Ok(out)
}

/// Writes `levels` (the bands that have one) under a `# comment` line.
pub fn format(comment: &str, levels: &[Option<f64>; BAND_COUNT]) -> String {
    let mut s = format!("# {}\n", comment.replace('\n', " "));
    for (i, l) in levels.iter().enumerate() {
        if let Some(l) = l {
            s.push_str(&format!("{}\t{l:.2}\n", BAND_NOMINAL_HZ[i]));
        }
    }
    s
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_exact_centres() {
        let mut l = [None; BAND_COUNT];
        l[0] = Some(61.25);
        l[2] = Some(55.5);
        l[27] = Some(20.0);
        let text = format("dwelling", &l);
        assert_eq!(parse(&text).unwrap(), l);
        // Exact centres and other separators read as the same bands.
        let p = parse("19.95;61.25\n31.62, 55.5\n# x\n\n10000\t20\n").unwrap();
        assert_eq!(p, l);
    }

    #[test]
    fn refusals_name_the_line() {
        assert_eq!(parse("# nothing\n"), Err(BandLevelsError::Empty));
        let e = parse("20 60\n20 61\n").unwrap_err();
        assert!(matches!(e, BandLevelsError::Line { line: 2, .. }), "{e}");
        let e = parse("20 60\n70 61\n").unwrap_err();
        assert!(matches!(e, BandLevelsError::Line { line: 2, .. }), "{e}");
        let e = parse("20 nan\n").unwrap_err();
        assert!(matches!(e, BandLevelsError::Line { line: 1, .. }), "{e}");
        let e = parse("20\n").unwrap_err();
        assert!(matches!(e, BandLevelsError::Line { line: 1, .. }), "{e}");
    }
}
