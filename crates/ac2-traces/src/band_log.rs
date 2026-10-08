//! The per-second 1/3-octave band log of an SPL meter's band meter as CSV
//! (`docs/design/band-leq.md`, *Stored per second*): kept next to the meter's SPL log
//! ([`crate::spl_log`]) by the session and the autosave, read back to rebuild the band
//! windows by wall time.
//!
//! ```text
//! # ac2 band log v1
//! # meas: 4
//! # name: FOH SPL
//! # input: 2
//! # mic: M30
//! start_utc,start_ns,measured_s,unit,period,correction_db,sensitivity_db,z20hz,z25hz,…,z10000hz
//! 2026-10-03T21:57:25.000Z,1790000245000000000,1,dB SPL,day,5,120.02,61.25,…,30.10
//! ```
//!
//! The 28 band levels (20 Hz … 10 kHz) are the unweighted band Leq of the second, in the
//! row's unit (dB SPL when a sensitivity was in force, else dBFS) to 0.01 dB, **without**
//! the §13 correction: the correction in force is its own column, so a correction typed
//! later or wrongly can be reviewed against the raw record. `period` is the limit set of
//! the second's local start (day 07–22, night 22–07). Reading back recovers dBFS by
//! subtracting the sensitivity.

use std::fmt::Write as _;

use ac2_proto::model::{
    BAND_COUNT, BAND_NOMINAL_HZ, BandLogAverage, BandLogSecond, BandPeriod, SplBandLog,
};
use ac2_proto::units::{Db, DbSpl, MeasId, Seconds, WallNs};

use crate::spl_log::{SplLogError, SplLogInfo, utc_iso};

/// First line of every band log file.
pub const HEADER_LINE: &str = "# ac2 band log v1";
const FIXED_COLUMNS: &str =
    "start_utc,start_ns,measured_s,unit,period,correction_db,sensitivity_db";
const FIXED: usize = 7;

/// One logged second of the band meter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandLogRow {
    /// Wall time of the second's start.
    pub start: WallNs,
    /// Time measured within it (0 … 1 s).
    pub measured: Seconds,
    /// Band Leq per band of [`BAND_NOMINAL_HZ`], dBFS, uncorrected (−∞: no energy).
    pub levels: [f32; BAND_COUNT],
    /// §13 correction in force (dB, ≥ 0).
    pub correction: Db,
    /// Limit set of the second's local start.
    pub period: BandPeriod,
    /// dB SPL of 0 dBFS in force, if calibrated.
    pub sensitivity: Option<Db>,
}

/// Column name of band `i`: `z<nominal>hz` (`z31.5hz`).
fn band_column(i: usize) -> String {
    format!("z{}hz", BAND_NOMINAL_HZ[i])
}

fn columns() -> String {
    let mut s = FIXED_COLUMNS.to_owned();
    for i in 0..BAND_COUNT {
        s.push(',');
        s.push_str(&band_column(i));
    }
    s
}

fn level(v: f64) -> String {
    if v == f64::NEG_INFINITY {
        "-inf".into()
    } else if v.is_finite() {
        format!("{v:.2}")
    } else {
        "nan".into()
    }
}

fn period_name(p: BandPeriod) -> &'static str {
    match p {
        BandPeriod::Day => "day",
        BandPeriod::Night => "night",
    }
}

/// The header lines, through the column header.
pub fn header(info: &SplLogInfo) -> String {
    let mut s = String::with_capacity(400);
    s.push_str(HEADER_LINE);
    s.push('\n');
    let _ = writeln!(s, "# meas: {}", info.meas.0);
    let _ = writeln!(s, "# name: {}", info.name.replace('\n', " "));
    let _ = writeln!(s, "# input: {}", u32::from(info.input) + 1);
    if let Some(m) = &info.mic {
        let _ = writeln!(s, "# mic: {}", m.replace('\n', " "));
    }
    s.push_str(&columns());
    s.push('\n');
    s
}

/// Appends one row's line, newline included.
pub fn push_row(s: &mut String, r: &BandLogRow) {
    let (unit, off) = match r.sensitivity {
        Some(Db(o)) => ("dB SPL", o),
        None => ("dBFS", 0.0),
    };
    let _ = write!(
        s,
        "{},{},{},{unit},{},{},{}",
        utc_iso(r.start.0),
        r.start.0,
        r.measured.0,
        period_name(r.period),
        r.correction.0,
        r.sensitivity
            .map_or_else(String::new, |d| format!("{:.4}", d.0)),
    );
    for &l in &r.levels {
        s.push(',');
        s.push_str(&level(f64::from(l) + off));
    }
    s.push('\n');
}

/// One header and one row per second.
pub fn export_csv(info: &SplLogInfo, rows: &[BandLogRow]) -> String {
    let mut s = header(info);
    s.reserve(rows.len() * 260);
    for r in rows {
        push_row(&mut s, r);
    }
    s
}

/// A band log read back.
#[derive(Debug, Clone, PartialEq)]
pub struct BandLogRead {
    /// Rows, oldest first.
    pub rows: Vec<BandLogRow>,
    /// Bytes up to the end of the last whole line: where an append carries on.
    pub complete_len: usize,
}

/// Reads rows back (the header lines are checked, not returned); an unterminated last line
/// is neither a row nor an error, as for [`crate::spl_log::import_csv`].
pub fn import_csv(bytes: &[u8]) -> Result<BandLogRead, SplLogError> {
    let complete_len = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let text = std::str::from_utf8(&bytes[..complete_len]).map_err(|_| SplLogError::Header)?;
    let mut lines = text.lines().enumerate();
    match lines.next() {
        Some((_, l)) if l.trim_end() == HEADER_LINE => {}
        _ => return Err(SplLogError::Header),
    }
    let cols = columns();
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
            if l != cols {
                return Err(bad("expected the column header"));
            }
            seen_columns = true;
            continue;
        }
        let f: Vec<&str> = l.split(',').collect();
        if f.len() != FIXED + BAND_COUNT {
            return Err(bad(&format!("expected {} fields", FIXED + BAND_COUNT)));
        }
        let start: u64 = f[1].parse().map_err(|_| bad("bad start_ns"))?;
        let measured: f64 = f[2].parse().map_err(|_| bad("bad measured_s"))?;
        if !(0.0..=1.0).contains(&measured) {
            return Err(bad("measured_s outside 0…1"));
        }
        let period = match f[4] {
            "day" => BandPeriod::Day,
            "night" => BandPeriod::Night,
            _ => return Err(bad("period is day or night")),
        };
        let correction: f64 = f[5].parse().map_err(|_| bad("bad correction_db"))?;
        if !correction.is_finite() {
            return Err(bad("bad correction_db"));
        }
        let sensitivity = if f[6].is_empty() {
            None
        } else {
            Some(f[6].parse::<f64>().map_err(|_| bad("bad sensitivity_db"))?)
        };
        match (f[3], sensitivity) {
            ("dB SPL", Some(_)) | ("dBFS", None) => {}
            _ => return Err(bad("unit and sensitivity disagree")),
        }
        let off = sensitivity.unwrap_or(0.0);
        let mut levels = [0f32; BAND_COUNT];
        for (o, t) in levels.iter_mut().zip(&f[FIXED..]) {
            let v = match *t {
                "-inf" => f64::NEG_INFINITY,
                "nan" => f64::NAN,
                t => t.parse::<f64>().map_err(|_| bad("bad level"))?,
            };
            *o = (v - off) as f32;
        }
        rows.push(BandLogRow {
            start: WallNs(start),
            measured: Seconds(measured),
            levels,
            correction: Db(correction),
            period,
            sensitivity: sensitivity.map(Db),
        });
    }
    if !seen_columns {
        return Err(SplLogError::Line {
            line: 1,
            msg: "no column header".into(),
        });
    }
    Ok(BandLogRead { rows, complete_len })
}

/// The energy average of band seconds over the time they measured
/// ([`span_average`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpanAverage {
    /// Seconds averaged.
    pub seconds: u32,
    /// Time they measured, s.
    pub measured: f64,
    /// Of `seconds`, those without a sensitivity.
    pub uncalibrated: u32,
    /// dB SPL per band (−∞: no energy).
    levels: [f64; BAND_COUNT],
}

/// Why a span gives no band levels in dB SPL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanGap {
    /// No second was logged in it.
    Empty,
    /// A second in it had no sensitivity: the levels would mix dBFS and dB SPL.
    Uncalibrated,
    /// Its seconds measured no time.
    NothingMeasured,
}

impl std::fmt::Display for SpanGap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SpanGap::Empty => "no band seconds logged in that span",
            SpanGap::Uncalibrated => "not calibrated for all of that span: a transfer needs dB SPL",
            SpanGap::NothingMeasured => "nothing measured in that span",
        })
    }
}

impl SpanAverage {
    /// The levels a transfer takes, dB SPL per band (−∞: no energy), or why there are none.
    pub fn levels(&self) -> Result<[f64; BAND_COUNT], SpanGap> {
        if self.seconds == 0 {
            Err(SpanGap::Empty)
        } else if self.uncalibrated > 0 {
            Err(SpanGap::Uncalibrated)
        } else if self.measured <= 0.0 {
            Err(SpanGap::NothingMeasured)
        } else {
            Ok(self.levels)
        }
    }

    /// As `spl.band_log_get` carries it.
    pub fn wire(&self) -> BandLogAverage {
        BandLogAverage {
            seconds: self.seconds,
            measured: Seconds(self.measured),
            uncalibrated: self.uncalibrated,
            levels: self
                .levels()
                .ok()
                .map(|l| l.map(|l| l.is_finite().then_some(DbSpl(l)))),
        }
    }
}

/// The energy average of `rows` per band over the time they measured, each second on the
/// sensitivity it was logged with: a level held for half a second weighs half. The §13
/// correction stays out: it belongs to the music's character, not to the building.
pub fn span_average(rows: &[BandLogRow]) -> SpanAverage {
    let mut e = [0.0; BAND_COUNT];
    let mut measured = 0.0;
    let mut uncalibrated = 0;
    for r in rows {
        let Some(s) = r.sensitivity else {
            uncalibrated += 1;
            continue;
        };
        for (e, &l) in e.iter_mut().zip(&r.levels) {
            let p = 10f64.powf((f64::from(l) + s.0) / 10.0);
            if p.is_finite() {
                *e += p * r.measured.0;
            }
        }
        measured += r.measured.0;
    }
    SpanAverage {
        seconds: u32::try_from(rows.len()).unwrap_or(u32::MAX),
        measured,
        uncalibrated,
        levels: e.map(|e| 10.0 * (e / measured).log10()),
    }
}

/// A logged second as `spl.band_log_get` carries it: levels in dB SPL when calibrated.
pub fn wire_row(r: &BandLogRow) -> BandLogSecond {
    let o = r.sensitivity.map_or(0.0, |s| s.0);
    BandLogSecond {
        start: r.start,
        measured: r.measured,
        period: r.period,
        correction: r.correction,
        sensitivity: r.sensitivity,
        levels: r.levels.map(|l| {
            let l = f64::from(l) + o;
            l.is_finite().then_some(l)
        }),
    }
}

/// The reply to `spl.band_log_get` from the band rows of `meas` starting in
/// `[from, until)`: the average over all of them and every `step`-th as a row; refused
/// when the span is empty or backwards, `step` is 0, or the rows would pass
/// [`SplBandLog::MAX_ROWS`].
pub fn span_reply(
    meas: MeasId,
    from: WallNs,
    until: WallNs,
    step: Option<u32>,
    rows: &[BandLogRow],
) -> Result<SplBandLog, String> {
    if until <= from {
        return Err("the span ends before it starts".into());
    }
    let out = match step {
        None => Vec::new(),
        Some(0) => return Err("the step is 1 or more seconds".into()),
        Some(n) => {
            let n = n as usize;
            let count = rows.len().div_ceil(n);
            if count > SplBandLog::MAX_ROWS as usize {
                let fits = rows.len().div_ceil(SplBandLog::MAX_ROWS as usize);
                return Err(format!(
                    "{count} rows at step {n}: at most {} per reply; ask with step {fits} or \
                     more, or a shorter span",
                    SplBandLog::MAX_ROWS
                ));
            }
            rows.iter().step_by(n).map(wire_row).collect()
        }
    };
    Ok(SplBandLog {
        meas,
        from,
        until,
        step,
        average: span_average(rows).wire(),
        rows: out,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ac2_proto::units::MeasId;

    fn info() -> SplLogInfo {
        SplLogInfo {
            meas: MeasId(4),
            name: "FOH SPL".into(),
            input: 1,
            mic: Some("M30".into()),
        }
    }

    fn rows() -> Vec<BandLogRow> {
        let mut a = [0f32; BAND_COUNT];
        for (i, l) in a.iter_mut().enumerate() {
            *l = -60.0 + i as f32 * 1.25;
        }
        a[27] = f32::NEG_INFINITY;
        vec![
            BandLogRow {
                start: WallNs(1_790_000_245_000_000_000),
                measured: Seconds(1.0),
                levels: a,
                correction: Db(5.0),
                period: BandPeriod::Night,
                sensitivity: Some(Db(120.02)),
            },
            BandLogRow {
                start: WallNs(1_790_000_246_000_000_000),
                measured: Seconds(0.25),
                levels: [-80.5; BAND_COUNT],
                correction: Db(0.0),
                period: BandPeriod::Day,
                sensitivity: None,
            },
        ]
    }

    #[test]
    fn round_trip_to_a_hundredth() {
        let text = export_csv(&info(), &rows());
        assert!(text.contains(",z31.5hz,"), "{text}");
        assert!(text.contains(",dB SPL,night,5,120.0200,60.02,"), "{text}");
        let back = import_csv(text.as_bytes()).unwrap();
        assert_eq!(back.complete_len, text.len());
        assert_eq!(back.rows.len(), 2);
        for (a, b) in back.rows.iter().zip(rows()) {
            assert_eq!(a.start, b.start);
            assert_eq!(a.measured, b.measured);
            assert_eq!(a.correction, b.correction);
            assert_eq!(a.period, b.period);
            assert_eq!(a.sensitivity, b.sensitivity);
            for (x, y) in a.levels.iter().zip(b.levels) {
                assert!((x == &y) || (x - y).abs() <= 0.0051, "{x} {y}");
            }
        }
    }

    #[test]
    fn a_torn_last_line_is_left_out_and_bad_lines_refused() {
        let text = export_csv(&info(), &rows());
        let cut = &text.as_bytes()[..text.len() - 7];
        let back = import_csv(cut).unwrap();
        assert_eq!(back.rows.len(), 1);
        assert!(import_csv(b"# ac2 spl log v2\n").is_err());
        let bad = text.replace(",night,", ",evening,");
        assert!(matches!(
            import_csv(bad.as_bytes()),
            Err(SplLogError::Line { line: 7, .. })
        ));
    }

    #[test]
    fn a_span_averages_energy_over_the_time_measured() {
        let row = |start_s: u64, measured: f64, l: f32, sens: Option<f64>| BandLogRow {
            start: WallNs(start_s * 1_000_000_000),
            measured: Seconds(measured),
            levels: [l; BAND_COUNT],
            correction: Db(5.0),
            period: BandPeriod::Night,
            sensitivity: sens.map(Db),
        };
        // 1 s at 70 dB SPL and 0.5 s at 80: (10^7 + 0.5·10^8) / 1.5.
        let mut rows = vec![
            row(100, 1.0, -50.0, Some(120.0)),
            row(101, 0.5, -40.0, Some(120.0)),
        ];
        rows[0].levels[3] = f32::NEG_INFINITY;
        rows[1].levels[3] = f32::NEG_INFINITY;
        let a = span_average(&rows);
        let want = 10.0 * ((1e7 + 0.5e8) / 1.5f64).log10();
        let l = a.levels().unwrap();
        assert!((l[0] - want).abs() < 1e-9, "{}", l[0]);
        assert_eq!(l[3], f64::NEG_INFINITY);
        let w = a.wire();
        assert_eq!(
            (w.seconds, w.measured, w.uncalibrated),
            (2, Seconds(1.5), 0)
        );
        let wl = w.levels.unwrap();
        assert!((wl[0].unwrap().0 - want).abs() < 1e-9);
        assert_eq!(wl[3], None, "no energy in the band");
        // A second without a sensitivity: no dB SPL average.
        rows.push(row(102, 1.0, -40.0, None));
        let a = span_average(&rows);
        assert_eq!(a.levels(), Err(SpanGap::Uncalibrated));
        assert_eq!((a.wire().uncalibrated, a.wire().levels), (1, None));
        assert_eq!(span_average(&[]).levels(), Err(SpanGap::Empty));
        assert_eq!(
            span_average(&[row(1, 0.0, -40.0, Some(120.0))]).levels(),
            Err(SpanGap::NothingMeasured)
        );
    }

    #[test]
    fn a_span_reply_steps_and_bounds_its_rows() {
        let rows: Vec<BandLogRow> = (0..7200u64)
            .map(|i| BandLogRow {
                start: WallNs(i * 1_000_000_000),
                ..rows()[0]
            })
            .collect();
        let (from, until) = (WallNs(0), WallNs(7200 * 1_000_000_000));
        let r = span_reply(MeasId(4), from, until, None, &rows).unwrap();
        assert!(r.rows.is_empty());
        assert_eq!(r.average.seconds, 7200);
        let e = span_reply(MeasId(4), from, until, Some(1), &rows).unwrap_err();
        assert!(e.contains("ask with step 2"), "{e}");
        let r = span_reply(MeasId(4), from, until, Some(2), &rows).unwrap();
        assert_eq!(r.rows.len(), 3600);
        assert_eq!(r.rows[1].start, WallNs(2_000_000_000));
        // Levels in dB SPL when calibrated; no energy is none.
        assert!((r.rows[0].levels[0].unwrap() - (-60.0 + 120.02)).abs() < 1e-4);
        assert_eq!(r.rows[0].levels[27], None);
        assert!(span_reply(MeasId(4), from, until, Some(0), &rows).is_err());
        assert!(span_reply(MeasId(4), until, from, None, &rows).is_err());
        // Uncalibrated rows read in dBFS.
        assert_eq!(wire_row(&self::rows()[1]).levels[0], Some(-80.5));
    }
}
