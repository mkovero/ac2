//! The per-second SPL log as CSV (`docs/protocol.md` §7.4): what a session stores for each
//! SPL meter and what `ac2 spl leq export` writes for compliance records.
//!
//! ```text
//! # ac2 spl log v3
//! # meas: 4
//! # name: FOH SPL
//! # input: 2
//! # mic: M30
//! # zone: Europe/Helsinki
//! # started: 2026-10-03T14:50:00.000Z
//! second,run,start_utc,start_ns,clock,gap,measured_s,unit,laeq_1s,lceq_1s,lzeq_1s,lcpeak_1s,lafmax_1s,sensitivity_db,position_db,position_peak_db
//! 0,0,2026-10-03T14:57:25.000Z,1790000245000000000,synced,known,1,dB SPL,95.1234,98.0000,99.5000,121.2500,101.0000,120.0200,3.0000,2.0000
//! 1,0,2026-10-03T14:57:26.000Z,1790000246000000000,synced,,1,dB SPL,95.0000,98.0000,99.5000,121.2500,101.0000,120.0200,3.0000,2.0000
//! ```
//!
//! A row's key is `second`, an index into the log that only rises: within a run (one
//! stretch of the meter on one open stream) it advances by the seconds the audio clock
//! counted, so a wall-clock step changes nothing but the annotation `start_ns`, and a
//! capture gap shows as missing indices. `gap` (on a run's first row only) says whether the
//! seconds since the row before are known (`docs/design/protocol-next.md` §7).
//!
//! Levels are written in the unit of their row (dB SPL when a sensitivity was in force,
//! else dBFS) with four decimals: 10⁻⁴ dB is far below anything a one-second level is read
//! to, and keeps a day's log near 9 MB. Reading back recovers dBFS by subtracting the
//! sensitivity. The levels are what was measured: a measuring-position correction in force
//! is recorded beside them (energy, peak), never added in.

use crate::model::{LogClock, LogGap, PositionCorrection, SplLogRow};
use crate::units::{Db, Dbfs, MeasId, Seconds, WallNs};
use thiserror::Error;

mod read;
#[cfg(test)]
mod tests;

pub use read::{LogRead, LogRow, read_log};

/// The version this build writes and reads.
pub const VERSION: u32 = 3;
/// First line of every SPL log file.
pub const HEADER_LINE: &str = "# ac2 spl log v3";
/// What the first line of an SPL log of any version starts with.
pub const HEADER_PREFIX: &str = "# ac2 spl log v";
const COLUMNS: &str = "second,run,start_utc,start_ns,clock,gap,measured_s,unit,laeq_1s,lceq_1s,\
                       lzeq_1s,lcpeak_1s,lafmax_1s,sensitivity_db,position_db,position_peak_db";

/// Highest level a row can hold, dBFS. The meter squares f32 samples, so no mean square or
/// peak exceeds `f32::MAX²`: 20·log10(f32::MAX) ≈ 770.6 dB. A higher level is not a
/// measurement, and its energy (10^(L/10)) summed over a log's seconds would overflow.
pub const MAX_LEVEL_DBFS: f64 = 771.0;

/// Latest row start, Unix ns: the wall clock's signed 64-bit range (year 2262). Leaves room
/// for the second a row spans (`start + 1 s`) in u64 arithmetic.
pub const MAX_START_NS: u64 = i64::MAX as u64;

/// Whether `v` is a level the meter writes: finite and at most [`MAX_LEVEL_DBFS`], or −∞
/// (a second without energy or without a peak). NaN and +∞ never come from a measured
/// second.
pub fn level_ok(v: f64) -> bool {
    v == f64::NEG_INFINITY || (v.is_finite() && v <= MAX_LEVEL_DBFS)
}

/// Checks a row against what the meter writes: a second with something measured (a row is
/// only written when `measured > 0`), levels per [`level_ok`], a finite sensitivity, a
/// valid measuring-position correction, a start within [`MAX_START_NS`].
pub fn check_row(r: &SplLogRow) -> Result<(), &'static str> {
    if !(r.measured.0 > 0.0 && r.measured.0 <= 1.0) {
        return Err("measured_s outside (0, 1]");
    }
    if r.start.0 > MAX_START_NS {
        return Err("start_ns past the wall clock's range");
    }
    if ![r.laeq, r.lceq, r.lzeq, r.lcpeak, r.lafmax]
        .iter()
        .all(|l| level_ok(l.0))
    {
        return Err("level not finite or above 771 dBFS");
    }
    if r.sensitivity.is_some_and(|s| !s.0.is_finite()) {
        return Err("sensitivity_db not finite");
    }
    if r.position.is_some_and(|p| !p.is_valid()) {
        return Err("position correction not finite or beyond ±30 dB");
    }
    Ok(())
}

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
    /// The host's time zone when the header was written: its IANA name, else `±hh:mm`
    /// ([`crate::model::TimeZone::label`]).
    pub zone: String,
    /// When the log started (ms resolution, as the header holds it): the log's identity,
    /// which a client compares to tell a log it cached from a new one.
    pub started: WallNs,
}

/// An unreadable SPL log file.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SplLogError {
    /// Not an ac2 log of this kind.
    #[error("not an ac2 log of this kind (first line must be {HEADER_LINE:?} or its band log's)")]
    Header,
    /// An ac2 log of this kind, of another version: read by nothing in this build.
    #[error("a log of version {found}; this build reads version {reads}")]
    Version {
        /// The file's version.
        found: u32,
        /// The version this build reads.
        reads: u32,
    },
    /// A malformed line.
    #[error("line {line}: {msg}")]
    Line {
        /// 1-based line number.
        line: usize,
        /// What is wrong.
        msg: String,
    },
}

/// `y`, `m` (1–12), `d` of a day count from 1970-01-01 (civil-from-days, proleptic
/// Gregorian; negative counts are days before 1970).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Days from 1970-01-01 of civil date `y`-`m`-`d` (the inverse of [`civil_from_days`]).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `YYYY-MM-DDThh:mm:ss.mmmZ` of Unix nanoseconds.
pub fn utc_iso(ns: u64) -> String {
    let secs = ns / 1_000_000_000;
    let ms = (ns % 1_000_000_000) / 1_000_000;
    let (y, m, d) = civil_from_days(i64::try_from(secs / 86_400).unwrap_or(0));
    let rem = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Unix nanoseconds of `YYYY-MM-DDThh:mm:ss.mmmZ` (what [`utc_iso`] writes), on or after
/// 1970; `None` for anything else.
pub fn parse_utc_iso(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() != 24 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' {
        return None;
    }
    if b[16] != b':' || b[19] != b'.' || b[23] != b'Z' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| -> Option<u32> {
        let t = s.get(r)?;
        if t.bytes().all(|c| c.is_ascii_digit()) {
            t.parse().ok()
        } else {
            None
        }
    };
    let (y, mo, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (h, mi, se, ms) = (n(11..13)?, n(14..16)?, n(17..19)?, n(20..23)?);
    if !(1..=12).contains(&mo) || d == 0 || h > 23 || mi > 59 || se > 59 {
        return None;
    }
    let days = days_from_civil(i64::from(y), mo, d);
    if civil_from_days(days) != (i64::from(y), mo, d) {
        return None;
    }
    let secs = u64::try_from(days * 86_400 + i64::from(h * 3600 + mi * 60 + se)).ok()?;
    secs.checked_mul(1_000_000_000)?
        .checked_add(u64::from(ms) * 1_000_000)
}

/// A clock column's text.
pub fn clock_name(c: LogClock) -> &'static str {
    match c {
        LogClock::Synced => "synced",
        LogClock::Unsynced => "unsynced",
        LogClock::Derived => "derived",
        LogClock::NotStated => "not_stated",
    }
}

/// The clock of a clock column's text.
pub fn parse_clock(s: &str) -> Option<LogClock> {
    Some(match s {
        "synced" => LogClock::Synced,
        "unsynced" => LogClock::Unsynced,
        "derived" => LogClock::Derived,
        "not_stated" => LogClock::NotStated,
        _ => return None,
    })
}

/// A gap column's text (empty on a row that is not its run's first).
pub fn gap_name(g: Option<LogGap>) -> &'static str {
    match g {
        Some(LogGap::Known) => "known",
        Some(LogGap::Unknown) => "unknown",
        None => "",
    }
}

/// The gap of a gap column's text.
pub fn parse_gap(s: &str) -> Option<Option<LogGap>> {
    Some(match s {
        "known" => Some(LogGap::Known),
        "unknown" => Some(LogGap::Unknown),
        "" => None,
        _ => return None,
    })
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

/// The header lines of either log kind (first line `first`), through `columns`.
pub fn header_with(first: &str, info: &SplLogInfo, columns: &str) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(240 + columns.len());
    s.push_str(first);
    s.push('\n');
    let _ = writeln!(s, "# meas: {}", info.meas.0);
    let _ = writeln!(s, "# name: {}", info.name.replace(['\n', '\r'], " "));
    // Inputs as printed on the interface: 1-based.
    let _ = writeln!(s, "# input: {}", u32::from(info.input) + 1);
    if let Some(m) = &info.mic {
        let _ = writeln!(s, "# mic: {}", m.replace(['\n', '\r'], " "));
    }
    let _ = writeln!(s, "# zone: {}", info.zone.replace(['\n', '\r'], " "));
    let _ = writeln!(s, "# started: {}", utc_iso(info.started.0));
    s.push_str(columns);
    s.push('\n');
    s
}

/// The header lines, through the column header, for `info`.
pub fn header(info: &SplLogInfo) -> String {
    header_with(HEADER_LINE, info, COLUMNS)
}

/// Appends one row's line, newline included, to `s`.
pub fn push_row(s: &mut String, r: &SplLogRow) {
    use std::fmt::Write as _;
    let (unit, off) = match r.sensitivity {
        Some(Db(o)) => ("dB SPL", o),
        None => ("dBFS", 0.0),
    };
    let db = |d: Option<f64>| d.map_or_else(String::new, |d| format!("{d:.4}"));
    let _ = writeln!(
        s,
        "{},{},{},{},{},{},{},{unit},{},{},{},{},{},{},{},{}",
        r.second,
        r.run,
        utc_iso(r.start.0),
        r.start.0,
        clock_name(r.clock),
        gap_name(r.gap),
        r.measured.0,
        level(r.laeq.0 + off),
        level(r.lceq.0 + off),
        level(r.lzeq.0 + off),
        level(r.lcpeak.0 + off),
        level(r.lafmax.0 + off),
        db(r.sensitivity.map(|d| d.0)),
        db(r.position.map(|p| p.level.0)),
        db(r.position.map(|p| p.peak.0)),
    );
}

/// One header and one row per second.
pub fn export_csv(info: &SplLogInfo, rows: &[SplLogRow]) -> String {
    let mut s = header(info);
    s.reserve(rows.len() * 110);
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
pub type SplLogRead = LogRead<SplLogRow>;

impl LogRow for SplLogRow {
    fn key(&self) -> (u64, u32, Option<LogGap>) {
        (self.second, self.run, self.gap)
    }
}

/// Reads an SPL log back ([`read_log`]: header, rows, where an append carries on).
pub fn import_csv(bytes: &[u8]) -> Result<SplLogRead, SplLogError> {
    read_log(bytes, HEADER_PREFIX, VERSION, COLUMNS, |l, line| {
        let row = parse_row(l, line)?;
        check_row(&row).map_err(|msg| SplLogError::Line {
            line,
            msg: msg.to_owned(),
        })?;
        Ok(row)
    })
}

/// The leading `second,run,start_utc,start_ns,clock,gap` fields of a row of either log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowKey {
    /// Second index.
    pub second: u64,
    /// Run.
    pub run: u32,
    /// Wall start, ns.
    pub start: WallNs,
    /// What `start` rests on.
    pub clock: LogClock,
    /// On a run's first row.
    pub gap: Option<LogGap>,
}

/// Parses the key fields `f[0..6]`; `f` holds at least six fields.
pub fn parse_key(f: &[&str], line: usize) -> Result<RowKey, SplLogError> {
    let bad = |msg: &str| SplLogError::Line {
        line,
        msg: msg.to_owned(),
    };
    Ok(RowKey {
        second: f[0].parse().map_err(|_| bad("bad second"))?,
        run: f[1].parse().map_err(|_| bad("bad run"))?,
        start: WallNs(f[3].parse().map_err(|_| bad("bad start_ns"))?),
        clock: parse_clock(f[4]).ok_or_else(|| bad("bad clock"))?,
        gap: parse_gap(f[5]).ok_or_else(|| bad("bad gap"))?,
    })
}

/// Writes the key fields, comma-terminated.
pub fn push_key(s: &mut String, k: &RowKey) {
    use std::fmt::Write as _;
    let _ = write!(
        s,
        "{},{},{},{},{},{},",
        k.second,
        k.run,
        utc_iso(k.start.0),
        k.start.0,
        clock_name(k.clock),
        gap_name(k.gap)
    );
}

fn parse_row(l: &str, line: usize) -> Result<SplLogRow, SplLogError> {
    let bad = |msg: &str| SplLogError::Line {
        line,
        msg: msg.to_owned(),
    };
    let f: Vec<&str> = l.split(',').collect();
    if f.len() != 16 {
        return Err(bad("expected 16 fields"));
    }
    let key = parse_key(&f, line)?;
    let measured: f64 = f[6].parse().map_err(|_| bad("bad measured_s"))?;
    let opt_db = |t: &str, what: &str| {
        if t.is_empty() {
            Ok(None)
        } else {
            t.parse::<f64>()
                .map(Some)
                .map_err(|_| bad(&format!("bad {what}")))
        }
    };
    let sensitivity = opt_db(f[13], "sensitivity_db")?;
    let position = match (
        opt_db(f[14], "position_db")?,
        opt_db(f[15], "position_peak_db")?,
    ) {
        (None, None) => None,
        (Some(level), Some(peak)) => Some(PositionCorrection {
            level: Db(level),
            peak: Db(peak),
        }),
        _ => return Err(bad("position_db and position_peak_db go together")),
    };
    match (f[7], sensitivity) {
        ("dB SPL", Some(_)) | ("dBFS", None) => {}
        _ => return Err(bad("unit and sensitivity disagree")),
    }
    let off = sensitivity.unwrap_or(0.0);
    let lv = |s: &str| {
        parse_level(s)
            .map(|v| Dbfs(v - off))
            .ok_or_else(|| bad("bad level"))
    };
    Ok(SplLogRow {
        second: key.second,
        run: key.run,
        start: key.start,
        clock: key.clock,
        gap: key.gap,
        measured: Seconds(measured),
        laeq: lv(f[8])?,
        lceq: lv(f[9])?,
        lzeq: lv(f[10])?,
        lcpeak: lv(f[11])?,
        lafmax: lv(f[12])?,
        sensitivity: sensitivity.map(Db),
        position,
    })
}
