//! The per-second SPL log as CSV (`docs/protocol.md` §7.4): what a session stores for each
//! SPL meter and what `ac2 spl leq export` writes for compliance records.
//!
//! ```text
//! # ac2 spl log v1
//! # meas: 4
//! # name: FOH SPL
//! # input: 2
//! # mic: M30
//! start_utc,start_ns,measured_s,unit,laeq_1s,lceq_1s,lzeq_1s,sensitivity_db
//! 2026-10-03T14:57:25.000Z,1790000245000000000,1,dB SPL,95.1234,98.0000,99.5000,120.0200
//! ```
//!
//! Levels are written in the unit of their row (dB SPL when a sensitivity was in force,
//! else dBFS) with four decimals: 10⁻⁴ dB is far below anything a one-second level is read
//! to, and keeps a day's log near 6 MB. Reading back recovers dBFS by subtracting the
//! sensitivity.

use ac2_proto::model::SplLogRow;
use ac2_proto::units::{Db, Dbfs, MeasId, Seconds, WallNs};
use thiserror::Error;

/// First line of every SPL log file.
pub const HEADER_LINE: &str = "# ac2 spl log v1";
const COLUMNS: &str = "start_utc,start_ns,measured_s,unit,laeq_1s,lceq_1s,lzeq_1s,sensitivity_db";

/// What the header names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplLogInfo {
    /// Measurement.
    pub meas: MeasId,
    /// Its name.
    pub name: String,
    /// Its input channel.
    pub input: u16,
    /// The input's mic name, if any.
    pub mic: Option<String>,
}

/// An unreadable SPL log file.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SplLogError {
    /// Not an ac2 SPL log of this version.
    #[error("not an ac2 SPL log (first line must be {HEADER_LINE:?})")]
    Header,
    /// A malformed line.
    #[error("line {line}: {msg}")]
    Line {
        /// 1-based line number.
        line: usize,
        /// What is wrong.
        msg: String,
    },
}

/// `YYYY-MM-DDThh:mm:ss.mmmZ` of Unix nanoseconds (civil-from-days, proleptic Gregorian).
pub fn utc_iso(ns: u64) -> String {
    let secs = ns / 1_000_000_000;
    let ms = (ns % 1_000_000_000) / 1_000_000;
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn level(v: f64) -> String {
    if v == f64::NEG_INFINITY {
        "-inf".into()
    } else if v.is_finite() {
        format!("{v:.4}")
    } else {
        "nan".into()
    }
}

/// The header lines, through the column header, for `info`.
pub fn header(info: &SplLogInfo) -> String {
    let mut s = String::with_capacity(160);
    s.push_str(HEADER_LINE);
    s.push('\n');
    s.push_str(&format!("# meas: {}\n", info.meas.0));
    s.push_str(&format!("# name: {}\n", info.name.replace('\n', " ")));
    // Inputs as printed on the interface: 1-based.
    s.push_str(&format!("# input: {}\n", u32::from(info.input) + 1));
    if let Some(m) = &info.mic {
        s.push_str(&format!("# mic: {}\n", m.replace('\n', " ")));
    }
    s.push_str(COLUMNS);
    s.push('\n');
    s
}

/// Appends one row's line, newline included, to `s`.
pub fn push_row(s: &mut String, r: &SplLogRow) {
    use std::fmt::Write as _;
    let (unit, off) = match r.sensitivity {
        Some(Db(o)) => ("dB SPL", o),
        None => ("dBFS", 0.0),
    };
    let _ = writeln!(
        s,
        "{},{},{},{unit},{},{},{},{}",
        utc_iso(r.start.0),
        r.start.0,
        r.measured.0,
        level(r.laeq.0 + off),
        level(r.lceq.0 + off),
        level(r.lzeq.0 + off),
        r.sensitivity
            .map_or_else(String::new, |d| format!("{:.4}", d.0)),
    );
}

/// One header and one row per second.
pub fn export_csv(info: &SplLogInfo, rows: &[SplLogRow]) -> String {
    let mut s = header(info);
    s.reserve(rows.len() * 96);
    for r in rows {
        push_row(&mut s, r);
    }
    s
}

fn parse_level(s: &str) -> Option<f64> {
    match s {
        "-inf" => Some(f64::NEG_INFINITY),
        "nan" => Some(f64::NAN),
        _ => s.parse().ok(),
    }
}

/// A log read back.
#[derive(Debug, Clone, PartialEq)]
pub struct SplLogRead {
    /// Rows, oldest first.
    pub rows: Vec<SplLogRow>,
    /// Bytes up to the end of the last whole line: where an append carries on.
    pub complete_len: usize,
}

/// Reads rows back (the header lines are checked, not returned). A log grows a line at a
/// time and a power cut may keep only part of the last append, so an unterminated last line
/// (and whatever follows the last newline, such as the zeros some file systems leave) is
/// neither a row nor an error.
pub fn import_csv(bytes: &[u8]) -> Result<SplLogRead, SplLogError> {
    let complete_len = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let text = std::str::from_utf8(&bytes[..complete_len]).map_err(|_| SplLogError::Header)?;
    let mut lines = text.lines().enumerate();
    match lines.next() {
        Some((_, l)) if l.trim_end() == HEADER_LINE => {}
        _ => return Err(SplLogError::Header),
    }
    let mut rows = Vec::new();
    let mut seen_columns = false;
    for (i, l) in lines {
        let line = i + 1;
        let bad = |msg: &str| SplLogError::Line {
            line,
            msg: msg.to_owned(),
        };
        let l = l.trim_end();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if !seen_columns {
            if l != COLUMNS {
                return Err(bad("expected the column header"));
            }
            seen_columns = true;
            continue;
        }
        let f: Vec<&str> = l.split(',').collect();
        if f.len() != 8 {
            return Err(bad("expected 8 fields"));
        }
        let start: u64 = f[1].parse().map_err(|_| bad("bad start_ns"))?;
        let measured: f64 = f[2].parse().map_err(|_| bad("bad measured_s"))?;
        if !(0.0..=1.0).contains(&measured) {
            return Err(bad("measured_s outside 0…1"));
        }
        let sensitivity = if f[7].is_empty() {
            None
        } else {
            Some(f[7].parse::<f64>().map_err(|_| bad("bad sensitivity_db"))?)
        };
        match (f[3], sensitivity) {
            ("dB SPL", Some(_)) | ("dBFS", None) => {}
            _ => return Err(bad("unit and sensitivity disagree")),
        }
        let off = sensitivity.unwrap_or(0.0);
        let lv = |s: &str| {
            parse_level(s)
                .map(|v| Dbfs(v - off))
                .ok_or_else(|| bad("bad level"))
        };
        rows.push(SplLogRow {
            start: WallNs(start),
            measured: Seconds(measured),
            laeq: lv(f[4])?,
            lceq: lv(f[5])?,
            lzeq: lv(f[6])?,
            sensitivity: sensitivity.map(Db),
        });
    }
    if !seen_columns {
        return Err(SplLogError::Line {
            line: 1,
            msg: "no column header".into(),
        });
    }
    Ok(SplLogRead { rows, complete_len })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<SplLogRow> {
        vec![
            SplLogRow {
                start: WallNs(1_790_000_245_000_000_000),
                measured: Seconds(1.0),
                laeq: Dbfs(-24.9),
                lceq: Dbfs(-22.0),
                lzeq: Dbfs(-20.5),
                sensitivity: Some(Db(120.02)),
            },
            SplLogRow {
                start: WallNs(1_790_000_246_000_000_000),
                measured: Seconds(0.25),
                laeq: Dbfs(f64::NEG_INFINITY),
                lceq: Dbfs(-90.125),
                lzeq: Dbfs(-80.0),
                sensitivity: None,
            },
        ]
    }

    #[test]
    fn utc_text() {
        assert_eq!(utc_iso(0), "1970-01-01T00:00:00.000Z");
        // 2026-10-03 14:57:25.5 UTC.
        assert_eq!(
            utc_iso(1_791_039_445_500_000_000),
            "2026-10-03T14:57:25.500Z"
        );
        assert_eq!(utc_iso(951_782_400_000_000_000), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn round_trip() {
        let info = SplLogInfo {
            meas: MeasId(4),
            name: "FOH SPL".into(),
            input: 1,
            mic: Some("M30".into()),
        };
        let csv = export_csv(&info, &rows());
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], HEADER_LINE);
        assert_eq!(lines[4], "# mic: M30");
        assert_eq!(lines[5], COLUMNS);
        assert_eq!(
            lines[6],
            "2026-09-21T14:17:25.000Z,1790000245000000000,1,dB SPL,95.1200,98.0200,99.5200,120.0200"
        );
        assert_eq!(
            lines[7],
            "2026-09-21T14:17:26.000Z,1790000246000000000,0.25,dBFS,-inf,-90.1250,-80.0000,"
        );
        let back = import_csv(csv.as_bytes()).expect("reads back").rows;
        assert_eq!(back.len(), 2);
        for (a, b) in back.iter().zip(rows()) {
            assert_eq!(a.start, b.start);
            assert_eq!(a.measured, b.measured);
            assert_eq!(a.sensitivity, b.sensitivity);
            for (x, y) in [(a.laeq, b.laeq), (a.lceq, b.lceq), (a.lzeq, b.lzeq)] {
                assert!(x.0 == y.0 || (x.0 - y.0).abs() < 1e-9, "{x:?} {y:?}");
            }
        }
    }

    /// A log cut short by a power cut reads up to its last whole line and says where that
    /// ends, so an append carries on from there.
    #[test]
    fn an_unterminated_last_line_is_left_out() {
        let info = SplLogInfo {
            meas: MeasId(1),
            name: "m".into(),
            input: 0,
            mic: None,
        };
        let csv = export_csv(&info, &rows());
        let whole = import_csv(csv.as_bytes()).expect("whole");
        assert_eq!((whole.rows.len(), whole.complete_len), (2, csv.len()));
        let one = csv.len() - csv.lines().last().expect("row").len() - 1;
        for cut in [csv.len() - 1, csv.len() - 30, one + 1] {
            let r = import_csv(&csv.as_bytes()[..cut]).expect("cut");
            assert_eq!((r.rows.len(), r.complete_len), (1, one), "cut at {cut}");
        }
        let mut zeros = csv.clone().into_bytes();
        zeros.truncate(one + 7);
        zeros.extend([0u8; 300]);
        let r = import_csv(&zeros).expect("zeros");
        assert_eq!((r.rows.len(), r.complete_len), (1, one));
        // A malformed whole line is damage, not a cut.
        let bad = format!("{csv}x,1\n");
        assert!(import_csv(bad.as_bytes()).is_err());
    }

    #[test]
    fn refuses_what_it_did_not_write() {
        assert_eq!(import_csv(b"start,leq\n"), Err(SplLogError::Header));
        assert_eq!(import_csv(HEADER_LINE.as_bytes()), Err(SplLogError::Header));
        let bad = format!("{HEADER_LINE}\n{COLUMNS}\n1,2,3\n");
        assert!(matches!(
            import_csv(bad.as_bytes()),
            Err(SplLogError::Line { line: 3, .. })
        ));
        let bad = format!("{HEADER_LINE}\n{COLUMNS}\nx,1,1,dBFS,1,1,1,120\n");
        assert!(matches!(
            import_csv(bad.as_bytes()),
            Err(SplLogError::Line { line: 3, .. })
        ));
    }
}
