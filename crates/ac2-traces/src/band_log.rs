//! The per-second 1/3-octave band log of an SPL meter's band meter as CSV
//! (`docs/design/band-leq.md`, *Stored per second*): kept next to the meter's SPL log
//! ([`ac2_proto::spl_log`]) by the session and the autosave, read back to rebuild the band
//! windows by wall time.
//!
//! ```text
//! # ac2 band log v2
//! # meas: 4
//! # name: FOH SPL
//! # input: 2
//! # mic: M30
//! # zone: Europe/Helsinki
//! # started: 2026-10-03T21:50:00.000Z
//! second,run,start_utc,start_ns,clock,gap,measured_s,unit,utc_offset_s,period,correction_db,sensitivity_db,z20hz,z25hz,…,z10000hz
//! 0,0,2026-10-03T21:57:25.000Z,1790000245000000000,synced,known,1,dB SPL,10800,day,5,120.02,61.25,…,30.10
//! ```
//!
//! `second`, `run`, `clock` and `gap` are the SPL log's ([`ac2_proto::spl_log`]): the same
//! index keys a second in both logs. `utc_offset_s` is the zone's offset in force when the
//! period was judged, so a logged period can be checked afterwards across a daylight-saving
//! change; `period` is empty when the time of day was not known (can't tell).
//!
//! The 28 band levels (20 Hz … 10 kHz) are the unweighted band Leq of the second, in the
//! row's unit (dB SPL when a sensitivity was in force, else dBFS) to 0.01 dB, **without**
//! the §13 correction: the correction in force is its own column, so a correction typed
//! later or wrongly can be reviewed against the raw record. `period` is the limit set of
//! the second's local start (day 07–22, night 22–07). Reading back recovers dBFS by
//! subtracting the sensitivity.

use std::fmt::Write as _;

use ac2_proto::model::{
    BAND_COUNT, BAND_NOMINAL_HZ, BandLogAverage, BandLogSecond, BandPeriod, LogClock, LogGap,
    SplBandLog,
};
use ac2_proto::units::{Db, DbSpl, MeasId, Seconds, WallNs};

use ac2_proto::spl_log::{self, LogRead, LogRow, RowKey, SplLogError, SplLogInfo};

/// The version this build writes and reads.
pub const VERSION: u32 = 2;
/// First line of every band log file.
pub const HEADER_LINE: &str = "# ac2 band log v2";
/// What the first line of a band log of any version starts with.
pub const HEADER_PREFIX: &str = "# ac2 band log v";
const FIXED_COLUMNS: &str = "second,run,start_utc,start_ns,clock,gap,measured_s,unit,\
                             utc_offset_s,period,correction_db,sensitivity_db";
const FIXED: usize = 12;

/// Largest §13 correction, dB: impulse +10 dB with tonal +6 dB
/// ([`ac2_proto::model::BandCorrection::db`]).
pub const MAX_CORRECTION_DB: f64 = 16.0;

/// Checks a row against what the band meter writes: a second with something measured (a
/// row is only written when `measured > 0`), band levels per [`ac2_proto::spl_log::level_ok`],
/// a correction within 0 … [`MAX_CORRECTION_DB`], a finite sensitivity, a start within
/// [`ac2_proto::spl_log::MAX_START_NS`], a zone offset within the ±18 h any zone can have.
pub fn check_row(r: &BandLogRow) -> Result<(), &'static str> {
    if !(r.measured.0 > 0.0 && r.measured.0 <= 1.0) {
        return Err("measured_s outside (0, 1]");
    }
    if r.start.0 > ac2_proto::spl_log::MAX_START_NS {
        return Err("start_ns past the wall clock's range");
    }
    if !r
        .levels
        .iter()
        .all(|l| ac2_proto::spl_log::level_ok(f64::from(*l)))
    {
        return Err("band level not finite or above 771 dBFS");
    }
    if !(0.0..=MAX_CORRECTION_DB).contains(&r.correction.0) {
        return Err("correction_db outside 0 … 16 dB");
    }
    if r.sensitivity.is_some_and(|s| !s.0.is_finite()) {
        return Err("sensitivity_db not finite");
    }
    if r.utc_offset_s.unsigned_abs() > 18 * 3600 {
        return Err("utc_offset_s outside ±18 h");
    }
    Ok(())
}

/// One logged second of the band meter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandLogRow {
    /// The row's key, as the SPL log's.
    pub second: u64,
    /// Its run.
    pub run: u32,
    /// Wall time of the second's start, as read (an annotation).
    pub start: WallNs,
    /// What `start` rests on.
    pub clock: LogClock,
    /// On a run's first row, how the distance to the row before is known.
    pub gap: Option<LogGap>,
    /// Time measured within it (0 … 1 s).
    pub measured: Seconds,
    /// The zone's offset in force when `period` was judged, s east of UTC.
    pub utc_offset_s: i32,
    /// Band Leq per band of [`BAND_NOMINAL_HZ`], dBFS, uncorrected (−∞: no energy).
    pub levels: [f32; BAND_COUNT],
    /// §13 correction in force (dB, ≥ 0).
    pub correction: Db,
    /// Limit set of the second's local start; `None`: can't tell (the time of day was not
    /// known).
    pub period: Option<BandPeriod>,
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

fn period_name(p: Option<BandPeriod>) -> &'static str {
    match p {
        Some(BandPeriod::Day) => "day",
        Some(BandPeriod::Night) => "night",
        None => "",
    }
}

/// The header lines, through the column header.
pub fn header(info: &SplLogInfo) -> String {
    spl_log::header_with(HEADER_LINE, info, &columns())
}

/// Appends one row's line, newline included.
pub fn push_row(s: &mut String, r: &BandLogRow) {
    let (unit, off) = match r.sensitivity {
        Some(Db(o)) => ("dB SPL", o),
        None => ("dBFS", 0.0),
    };
    spl_log::push_key(
        s,
        &RowKey {
            second: r.second,
            run: r.run,
            start: r.start,
            clock: r.clock,
            gap: r.gap,
        },
    );
    let _ = write!(
        s,
        "{},{unit},{},{},{},{}",
        r.measured.0,
        r.utc_offset_s,
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
pub type BandLogRead = LogRead<BandLogRow>;

impl LogRow for BandLogRow {
    fn key(&self) -> (u64, u32, Option<LogGap>) {
        (self.second, self.run, self.gap)
    }
}

/// Reads a band log back ([`spl_log::read_log`]: header, rows, where an append carries on).
pub fn import_csv(bytes: &[u8]) -> Result<BandLogRead, SplLogError> {
    spl_log::read_log(bytes, HEADER_PREFIX, VERSION, &columns(), |l, line| {
        let row = parse_row(l, line)?;
        check_row(&row).map_err(|msg| SplLogError::Line {
            line,
            msg: msg.to_owned(),
        })?;
        Ok(row)
    })
}

fn parse_row(l: &str, line: usize) -> Result<BandLogRow, SplLogError> {
    let bad = |msg: &str| SplLogError::Line {
        line,
        msg: msg.to_owned(),
    };
    let f: Vec<&str> = l.split(',').collect();
    if f.len() != FIXED + BAND_COUNT {
        return Err(bad(&format!("expected {} fields", FIXED + BAND_COUNT)));
    }
    let key = spl_log::parse_key(&f, line)?;
    let measured: f64 = f[6].parse().map_err(|_| bad("bad measured_s"))?;
    if !(0.0..=1.0).contains(&measured) {
        return Err(bad("measured_s outside 0…1"));
    }
    let utc_offset_s: i32 = f[8].parse().map_err(|_| bad("bad utc_offset_s"))?;
    if utc_offset_s.unsigned_abs() > 18 * 3600 {
        return Err(bad("utc_offset_s beyond ±18 h"));
    }
    let period = match f[9] {
        "day" => Some(BandPeriod::Day),
        "night" => Some(BandPeriod::Night),
        "" => None,
        _ => return Err(bad("period is day, night or empty")),
    };
    let correction: f64 = f[10].parse().map_err(|_| bad("bad correction_db"))?;
    if !correction.is_finite() {
        return Err(bad("bad correction_db"));
    }
    let sensitivity = if f[11].is_empty() {
        None
    } else {
        Some(
            f[11]
                .parse::<f64>()
                .map_err(|_| bad("bad sensitivity_db"))?,
        )
    };
    match (f[7], sensitivity) {
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
    let row = BandLogRow {
        second: key.second,
        run: key.run,
        start: key.start,
        clock: key.clock,
        gap: key.gap,
        measured: Seconds(measured),
        utc_offset_s,
        levels,
        correction: Db(correction),
        period,
        sensitivity: sensitivity.map(Db),
    };
    Ok(row)
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
        second: r.second,
        run: r.run,
        start: r.start,
        clock: r.clock,
        gap: r.gap,
        measured: r.measured,
        utc_offset_s: r.utc_offset_s,
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
            zone: "Europe/Helsinki".into(),
            started: WallNs(1_790_000_000_000_000_000),
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
                second: 0,
                run: 0,
                start: WallNs(1_790_000_245_000_000_000),
                clock: LogClock::Synced,
                gap: Some(LogGap::Known),
                measured: Seconds(1.0),
                utc_offset_s: 10_800,
                levels: a,
                correction: Db(5.0),
                period: Some(BandPeriod::Night),
                sensitivity: Some(Db(120.02)),
            },
            BandLogRow {
                second: 1,
                run: 0,
                start: WallNs(1_790_000_246_000_000_000),
                clock: LogClock::Synced,
                gap: None,
                measured: Seconds(0.25),
                utc_offset_s: 7200,
                levels: [-80.5; BAND_COUNT],
                correction: Db(0.0),
                period: None,
                sensitivity: None,
            },
        ]
    }

    #[test]
    fn round_trip_to_a_hundredth() {
        let text = export_csv(&info(), &rows());
        assert!(text.contains(",z31.5hz,"), "{text}");
        assert!(
            text.contains(",dB SPL,10800,night,5,120.0200,60.02,"),
            "{text}"
        );
        // A daylight-saving change between the rows: each keeps the offset it was judged
        // in; the second's period is can't tell.
        assert!(text.contains(",0.25,dBFS,7200,,0,,"), "{text}");
        let back = import_csv(text.as_bytes()).unwrap();
        assert_eq!(back.complete_len, text.len());
        assert_eq!(back.rows.len(), 2);
        for (a, b) in back.rows.iter().zip(rows()) {
            assert_eq!(
                (a.second, a.run, a.start, a.clock, a.gap, a.utc_offset_s),
                (b.second, b.run, b.start, b.clock, b.gap, b.utc_offset_s)
            );
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
        let one = back.complete_len;
        // A torn row then zeros with newlines among them: the rows before, cut there.
        let mut z = cut.to_vec();
        z.extend([0u8; 3000]);
        z.push(b'\n');
        z.extend([0u8; 100]);
        let r = import_csv(&z).unwrap();
        assert_eq!(
            (r.rows.len(), r.complete_len, r.tail),
            (1, one, z.len() - one)
        );
        assert!(import_csv(b"# ac2 spl log v3\n").is_err());
        assert_eq!(
            import_csv(b"# ac2 band log v1\n# meas: 1\n"),
            Err(SplLogError::Version { found: 1, reads: 2 })
        );
        let bad = text.replace(",night,", ",evening,");
        assert!(matches!(
            import_csv(bad.as_bytes()),
            Err(SplLogError::Line { line: 9, .. })
        ));
    }

    #[test]
    fn a_span_averages_energy_over_the_time_measured() {
        let row = |start_s: u64, measured: f64, l: f32, sens: Option<f64>| BandLogRow {
            second: start_s,
            start: WallNs(start_s * 1_000_000_000),
            measured: Seconds(measured),
            levels: [l; BAND_COUNT],
            sensitivity: sens.map(Db),
            ..rows()[0]
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
                second: i,
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
