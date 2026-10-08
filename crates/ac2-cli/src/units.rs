//! Typed, unit-suffixed command-line values: `20hz`, `1.5khz`, `-12dbfs`, `3db`, `94db`,
//! `15.0mv`, `15mv/pa`, `1.5ms`, `480samples`, `3.4m`, `20c`, channel lists `1,2,5-6`.
//!
//! Every value must carry its unit (a bare `-20` is refused: is it dBFS or dB?), units are
//! case-insensitive, and parsers never panic: any input yields a value or a message.
//! Channels are 1-based on the command line (as printed on interfaces) and zero-based on the
//! wire.

use std::fmt;
use std::str::FromStr;

use ac2_proto::units::{Db, DbSpl, Dbfs, Hz, MvPerPa, Seconds, Volts};

/// A rejected value, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitError(pub String);

impl fmt::Display for UnitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UnitError {}

fn fail<T>(msg: impl Into<String>) -> Result<T, UnitError> {
    Err(UnitError(msg.into()))
}

/// Splits `s` into a finite number and a lowercase unit suffix. The number is
/// `[+-]digits[.digits][e[+-]digits]` — no `inf`, `nan`, hex or locale forms.
fn split(s: &str) -> Result<(f64, String), UnitError> {
    let t = s.trim();
    let b = t.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let digits_start = i;
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
        i += 1;
    }
    let mantissa = &t[digits_start..i];
    if mantissa.is_empty() || mantissa == "." || mantissa.matches('.').count() > 1 {
        return fail(format!("{s:?}: expected a number followed by a unit"));
    }
    // Exponent only when followed by digits, so `1e` stays a unit error rather than a
    // half-parsed number.
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let exp_digits = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_digits {
            i = j;
        }
    }
    let num: f64 = t[..i]
        .parse()
        .map_err(|_| UnitError(format!("{s:?}: not a number")))?;
    if !num.is_finite() {
        return fail(format!("{s:?}: out of range"));
    }
    let unit = t[i..].trim().to_lowercase();
    Ok((num, unit))
}

fn need_unit<T>(s: &str, unit: &str, expected: &str) -> Result<T, UnitError> {
    if unit.is_empty() {
        fail(format!("{s:?}: missing unit (expected {expected})"))
    } else {
        fail(format!(
            "{s:?}: unknown unit {unit:?} (expected {expected})"
        ))
    }
}

fn in_range(s: &str, v: f64, lo: f64, hi: f64, what: &str) -> Result<f64, UnitError> {
    if (lo..=hi).contains(&v) {
        Ok(v)
    } else {
        fail(format!("{s:?}: {what} must be within {lo}…{hi}"))
    }
}

/// A frequency: `20hz`, `1.5khz`. 0 < f ≤ 1 MHz.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Freq(pub Hz);

impl FromStr for Freq {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let (n, u) = split(s)?;
        let hz = match u.as_str() {
            "hz" => n,
            "khz" | "k" => n * 1e3,
            _ => return need_unit(s, &u, "hz or khz"),
        };
        if hz <= 0.0 {
            return fail(format!("{s:?}: frequency must be positive"));
        }
        Ok(Self(Hz(in_range(s, hz, 0.0, 1e6, "frequency")?)))
    }
}

/// A generator level in dBFS: `-20dbfs`. −150 ≤ x ≤ 0 (0 dBFS = RMS of a full-scale sine).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelDbfs(pub Dbfs);

impl FromStr for LevelDbfs {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let (n, u) = split(s)?;
        if u != "dbfs" {
            return need_unit(s, &u, "dbfs");
        }
        Ok(Self(Dbfs(in_range(s, n, -150.0, 0.0, "level")?)))
    }
}

/// A level ratio in dB: `3db`, `-6db`. |x| ≤ 200.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gain(pub Db);

impl FromStr for Gain {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let (n, u) = split(s)?;
        if u != "db" {
            return need_unit(s, &u, "db");
        }
        Ok(Self(Db(in_range(s, n, -200.0, 200.0, "gain")?)))
    }
}

/// A sound pressure level: `94db` or `94dbspl`. 0 ≤ x ≤ 200.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplLevel(pub DbSpl);

impl FromStr for SplLevel {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let (n, u) = split(s)?;
        if u != "db" && u != "dbspl" {
            return need_unit(s, &u, "db or dbspl");
        }
        Ok(Self(DbSpl(in_range(s, n, 0.0, 200.0, "SPL")?)))
    }
}

/// An RMS voltage as a meter shows it: `15.03mv`, `0.01503v`, `250uv`. 0 < x ≤ 1000 V.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoltsArg(pub Volts);

impl FromStr for VoltsArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let (n, u) = split(s)?;
        let v = match u.as_str() {
            "v" => n,
            "mv" => n * 1e-3,
            "uv" | "µv" | "μv" => n * 1e-6,
            _ => return need_unit(s, &u, "v, mv or uv"),
        };
        if !(v > 0.0 && v <= 1000.0) {
            return fail(format!("{s:?}: voltage must be above 0 and at most 1000 V"));
        }
        Ok(Self(Volts(v)))
    }
}

/// A mic sensitivity as data sheets state it: `15.0mv/pa`, `0.015v/pa`, `-36.5dbv/pa`.
/// 0.01 … 10 000 mV/Pa.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MicSensitivityArg(pub MvPerPa);

impl FromStr for MicSensitivityArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let (n, u) = split(s)?;
        let mv = match u.replace(' ', "").as_str() {
            "mv/pa" => n,
            "v/pa" => n * 1e3,
            "dbv/pa" | "dbv" => 1e3 * 10f64.powf(n / 20.0),
            _ => return need_unit(s, &u, "mv/pa, v/pa or dbv/pa"),
        };
        if !(0.01..=10_000.0).contains(&mv) {
            return fail(format!("{s:?}: mic sensitivity must be 0.01 … 10000 mV/Pa"));
        }
        Ok(Self(MvPerPa(mv)))
    }
}

fn seconds_of(n: f64, u: &str) -> Option<f64> {
    match u {
        "h" => Some(n * 3600.0),
        "min" => Some(n * 60.0),
        "s" | "sec" => Some(n),
        "ms" => Some(n * 1e-3),
        "us" | "µs" | "μs" => Some(n * 1e-6),
        _ => None,
    }
}

/// A non-negative duration: `1.5ms`, `2s`, `250us`, `10min`, `1h`. ≤ 1 day.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Time(pub Seconds);

impl FromStr for Time {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let (n, u) = split(s)?;
        let Some(sec) = seconds_of(n, &u) else {
            return need_unit(s, &u, "s, ms or us");
        };
        Ok(Self(Seconds(in_range(s, sec, 0.0, 86_400.0, "duration")?)))
    }
}

/// A file size: `500MB`, `2GB`, `1.5GiB` (decimal kB/MB/GB/TB, binary KiB/MiB/GiB/TiB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteSize(pub u64);

impl FromStr for ByteSize {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let (n, u) = split(s)?;
        let k: f64 = match u.as_str() {
            "b" => 1.0,
            "kb" => 1e3,
            "mb" => 1e6,
            "gb" => 1e9,
            "tb" => 1e12,
            "kib" => 1024.0,
            "mib" => 1_048_576.0,
            "gib" => 1_073_741_824.0,
            "tib" => 1_099_511_627_776.0,
            _ => return need_unit(s, &u, "kB, MB, GB, TB or KiB, MiB, GiB, TiB"),
        };
        let v = in_range(s, n * k, 1.0, 1e15, "size")?;
        Ok(Self(v.round() as u64))
    }
}

/// A whole sample count: `480samples`, `480smp`. |n| ≤ 2³¹.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleCount(pub i64);

fn samples_of(s: &str, n: f64, u: &str) -> Option<Result<i64, UnitError>> {
    if !matches!(u, "samples" | "sample" | "smp") {
        return None;
    }
    Some(if n.fract() != 0.0 {
        fail(format!("{s:?}: sample counts are whole numbers"))
    } else if n.abs() > 2_147_483_648.0 {
        fail(format!("{s:?}: sample count out of range"))
    } else {
        Ok(n as i64)
    })
}

impl FromStr for SampleCount {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let (n, u) = split(s)?;
        match samples_of(s, n, &u) {
            Some(r) => r.map(Self),
            None => need_unit(s, &u, "samples"),
        }
    }
}

/// A distance in metres: `3.4m`, `120cm`, `850mm`. 0 ≤ d ≤ 1000 m.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Distance(pub f64);

fn metres_of(n: f64, u: &str) -> Option<f64> {
    match u {
        "m" => Some(n),
        "cm" => Some(n * 1e-2),
        "mm" => Some(n * 1e-3),
        _ => None,
    }
}

impl FromStr for Distance {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let (n, u) = split(s)?;
        let Some(m) = metres_of(n, &u) else {
            return need_unit(s, &u, "m, cm or mm");
        };
        Ok(Self(in_range(s, m, 0.0, 1000.0, "distance")?))
    }
}

/// Air temperature: `20c`, `22.5°c`, `-5degc`. −50 … 60 °C.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Celsius(pub f64);

impl FromStr for Celsius {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let (n, u) = split(s)?;
        if !matches!(u.as_str(), "c" | "°c" | "degc") {
            return need_unit(s, &u, "c");
        }
        Ok(Self(in_range(s, n, -50.0, 60.0, "temperature")?))
    }
}

impl Celsius {
    /// Speed of sound in dry air, m/s: c = 331.3 · √(1 + T / 273.15).
    pub fn speed_of_sound(self) -> f64 {
        331.3 * (1.0 + self.0 / 273.15).sqrt()
    }
}

/// A delay given as time, samples or the distance sound travels: `12.5ms`, `600samples`,
/// `600.25samples`, `4.3m`. Time and samples may be negative (|t| ≤ 10 s); samples may have
/// a fraction (the analyzer aligns to fractions of a sample).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DelayAmount {
    /// Seconds.
    Time(Seconds),
    /// Samples at the session rate, fractions allowed.
    Samples(f64),
    /// Metres of sound travel.
    Distance(f64),
}

impl FromStr for DelayAmount {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let (n, u) = split(s)?;
        if let Some(sec) = seconds_of(n, &u) {
            return Ok(Self::Time(Seconds(in_range(s, sec, -10.0, 10.0, "delay")?)));
        }
        if matches!(u.as_str(), "samples" | "sample" | "smp") {
            return Ok(Self::Samples(in_range(
                s,
                n,
                -2_147_483_648.0,
                2_147_483_648.0,
                "sample count",
            )?));
        }
        if let Some(m) = metres_of(n, &u) {
            return Ok(Self::Distance(in_range(s, m, 0.0, 1000.0, "distance")?));
        }
        need_unit(s, &u, "ms, s, us, samples, m, cm or mm")
    }
}

impl DelayAmount {
    /// The delay in seconds, given the session rate (for samples) and air temperature (for
    /// distance).
    pub fn seconds(self, rate_hz: Option<u32>, temp: Celsius) -> Result<Seconds, UnitError> {
        match self {
            Self::Time(t) => Ok(t),
            Self::Samples(n) => match rate_hz {
                Some(r) if r > 0 => Ok(Seconds(n / f64::from(r))),
                _ => fail("a delay in samples needs an open session (sample rate)"),
            },
            Self::Distance(m) => Ok(Seconds(m / temp.speed_of_sound())),
        }
    }
}

/// Highest channel number accepted.
pub const MAX_CHANNEL: u16 = 256;

/// One 1-based channel number, stored zero-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel(pub u16);

fn channel_number(s: &str, tok: &str) -> Result<u16, UnitError> {
    match tok.trim().parse::<u16>() {
        Ok(0) => fail(format!("{s:?}: channels are numbered from 1")),
        Ok(n) if n <= MAX_CHANNEL => Ok(n - 1),
        Ok(_) => fail(format!("{s:?}: channel above {MAX_CHANNEL}")),
        Err(_) => fail(format!("{s:?}: expected a channel number like 1")),
    }
}

impl FromStr for Channel {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        channel_number(s, s).map(Self)
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", u32::from(self.0) + 1)
    }
}

/// A list of 1-based channels: `1,2`, `3-6`, `1,4-5`. Stored zero-based, unique, in order
/// given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channels(pub Vec<u16>);

impl FromStr for Channels {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let mut out: Vec<u16> = Vec::new();
        for part in s.split(',') {
            let part = part.trim();
            if part.is_empty() {
                return fail(format!("{s:?}: empty channel in list"));
            }
            let (a, b) = match part.split_once('-') {
                Some((a, b)) => (channel_number(s, a)?, channel_number(s, b)?),
                None => {
                    let c = channel_number(s, part)?;
                    (c, c)
                }
            };
            if b < a {
                return fail(format!("{s:?}: range {part} runs backwards"));
            }
            for c in a..=b {
                if out.contains(&c) {
                    return fail(format!("{s:?}: channel {} listed twice", c + 1));
                }
                out.push(c);
            }
        }
        Ok(Self(out))
    }
}

/// 1-based text of zero-based channels: `1,2`.
pub fn channels_text(ch: &[u16]) -> String {
    ch.iter()
        .map(|c| (u32::from(*c) + 1).to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// A Leq window: an optional weighting and a length in whole seconds, 1 s … 24 h: `30min`,
/// `1h`, `10s`, `c:30s` (A-weighted unless a weighting is given).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeqWindowArg {
    /// `a:`, `c:` or `z:`; `None`: A.
    pub weighting: Option<ac2_proto::model::Weighting>,
    /// Length, s.
    pub seconds: u32,
}

impl LeqWindowArg {
    /// The weighting meant: A unless given.
    pub fn weighting(&self) -> ac2_proto::model::Weighting {
        self.weighting.unwrap_or(ac2_proto::model::Weighting::A)
    }
}

impl FromStr for LeqWindowArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        use ac2_proto::model::Weighting;
        let t = s.trim().to_ascii_lowercase();
        let (weighting, len) = match t.split_once(':') {
            Some(("a", l)) => (Some(Weighting::A), l),
            Some(("c", l)) => (Some(Weighting::C), l),
            Some(("z", l)) => (Some(Weighting::Z), l),
            Some((w, _)) => return fail(format!("{s:?}: weighting {w:?} (expected a, c or z)")),
            None => (None, t.as_str()),
        };
        let (n, u) = split(len)?;
        let sec = match u.as_str() {
            "s" | "sec" => n,
            "min" => n * 60.0,
            "h" => n * 3600.0,
            _ => return need_unit(s, &u, "s, min or h"),
        };
        let sec = in_range(s, sec, 1.0, 86_400.0, "a window")?;
        if sec.fract() != 0.0 {
            return fail(format!("{s:?}: a window is whole seconds"));
        }
        Ok(Self {
            weighting,
            seconds: sec as u32,
        })
    }
}

/// A limit on a window: `30min=99db`, `c:10min=110db`; `30min=none` removes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeqLimitArg {
    pub window: LeqWindowArg,
    pub limit: Option<DbSpl>,
}

impl FromStr for LeqLimitArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let Some((w, l)) = s.split_once('=') else {
            return fail(format!(
                "{s:?}: expected window=limit, e.g. 30min=99db (or 30min=none)"
            ));
        };
        let window = w.parse()?;
        let limit = if l.trim().eq_ignore_ascii_case("none") {
            None
        } else {
            Some(l.parse::<SplLevel>()?.0)
        };
        Ok(Self { window, limit })
    }
}

/// A peak limit: `lcpeak=135db`, `lafmax=125db`; `lcpeak=none` removes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeakLimitArg {
    pub quantity: ac2_proto::model::PeakQuantity,
    pub limit: Option<DbSpl>,
}

impl FromStr for PeakLimitArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        use ac2_proto::model::PeakQuantity;
        let Some((q, l)) = s.split_once('=') else {
            return fail(format!(
                "{s:?}: expected lcpeak=LIMIT or lafmax=LIMIT, e.g. lcpeak=135db (or =none)"
            ));
        };
        let quantity = match q.trim().to_lowercase().as_str() {
            "lcpeak" => PeakQuantity::LcPeak,
            "lafmax" => PeakQuantity::LafMax,
            other => return fail(format!("{other:?}: a peak limit is on lcpeak or lafmax")),
        };
        let limit = if l.trim().eq_ignore_ascii_case("none") {
            None
        } else {
            Some(l.parse::<SplLevel>()?.0)
        };
        Ok(Self { quantity, limit })
    }
}

/// A measuring-position correction: `4db`, `-2.5db`, or `none`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionArg(pub Option<Db>);

impl FromStr for PositionArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        if s.trim().eq_ignore_ascii_case("none") {
            return Ok(Self(None));
        }
        let (n, u) = split(s)?;
        if u != "db" {
            return need_unit(s, &u, "db");
        }
        let max = ac2_proto::model::PositionCorrection::MAX_DB;
        Ok(Self(Some(Db(in_range(
            s,
            n,
            -max,
            max,
            "position correction",
        )?))))
    }
}

/// A point in time of a band-log span: `now`, `-30s` (before now), `21:00` / `21:00:30`
/// (local time today), `2026-10-08T21:00:30` (local), `2026-10-08T21:00:30Z` (UTC).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimeRef {
    Now,
    /// Seconds before now.
    Ago(f64),
    /// Local wall time: the date (today when `None`) and seconds of the day.
    Local {
        date: Option<(i32, u32, u32)>,
        seconds: u32,
    },
    /// UTC.
    Utc {
        date: (i32, u32, u32),
        seconds: u32,
    },
}

fn time_of_day(s: &str) -> Option<u32> {
    let f: Vec<&str> = s.split(':').collect();
    if !(2..=3).contains(&f.len()) {
        return None;
    }
    let n: Vec<u32> = f.iter().map(|t| t.parse().ok()).collect::<Option<_>>()?;
    let (h, m, sec) = (n[0], n[1], n.get(2).copied().unwrap_or(0));
    (h < 24 && m < 60 && sec < 60).then_some(h * 3600 + m * 60 + sec)
}

fn date(s: &str) -> Option<(i32, u32, u32)> {
    let f: Vec<&str> = s.split('-').collect();
    let [y, m, d] = f.as_slice() else {
        return None;
    };
    let (y, m, d) = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    chrono::NaiveDate::from_ymd_opt(y, m, d).map(|_| (y, m, d))
}

impl FromStr for TimeRef {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let t = s.trim();
        if t.eq_ignore_ascii_case("now") {
            return Ok(Self::Now);
        }
        if let Some(rest) = t.strip_prefix('-') {
            let (n, u) = split(rest)?;
            let Some(sec) = seconds_of(n, &u) else {
                return need_unit(s, &u, "s, min or h");
            };
            return Ok(Self::Ago(in_range(
                s,
                sec,
                0.0,
                7.0 * 86_400.0,
                "a time ago",
            )?));
        }
        let bad = || {
            fail(format!(
                "{s:?}: expected now, -30s, 21:00:30, 2026-10-08T21:00:30 or …Z"
            ))
        };
        let (utc, body) = match t.strip_suffix(['Z', 'z']) {
            Some(b) => (true, b),
            None => (false, t),
        };
        match body.split_once(['T', ' ']) {
            Some((d, tod)) => match (date(d), time_of_day(tod)) {
                (Some(date), Some(seconds)) if utc => Ok(Self::Utc { date, seconds }),
                (Some(date), Some(seconds)) => Ok(Self::Local {
                    date: Some(date),
                    seconds,
                }),
                _ => bad(),
            },
            None if !utc => match time_of_day(body) {
                Some(seconds) => Ok(Self::Local {
                    date: None,
                    seconds,
                }),
                None => bad(),
            },
            None => bad(),
        }
    }
}

impl TimeRef {
    /// Unix ns, given now (Unix ns).
    pub fn resolve(&self, now_ns: u64) -> Result<u64, UnitError> {
        use chrono::TimeZone;
        let at = |d: (i32, u32, u32), sec: u32| {
            chrono::NaiveDate::from_ymd_opt(d.0, d.1, d.2)
                .and_then(|d| d.and_hms_opt(sec / 3600, (sec % 3600) / 60, sec % 60))
        };
        let ns = |ts: i64| u64::try_from(ts).map(|t| t * 1_000_000_000).ok();
        let out = match *self {
            Self::Now => Some(now_ns),
            Self::Ago(s) => Some(now_ns.saturating_sub((s * 1e9).round() as u64)),
            Self::Utc { date, seconds } => {
                at(date, seconds).and_then(|t| ns(t.and_utc().timestamp()))
            }
            Self::Local { date, seconds } => {
                let date = date.unwrap_or_else(|| {
                    let today = chrono::Local
                        .timestamp_nanos(i64::try_from(now_ns).unwrap_or(i64::MAX))
                        .date_naive();
                    use chrono::Datelike;
                    (today.year(), today.month(), today.day())
                });
                at(date, seconds)
                    .and_then(|t| chrono::Local.from_local_datetime(&t).earliest())
                    .and_then(|t| ns(t.timestamp()))
            }
        };
        out.ok_or_else(|| UnitError(format!("{self:?}: not a time this host can place")))
    }
}

/// Where band levels for a transfer come from: a file of `<Hz> <dB>` lines, or a span of
/// an SPL meter's band log, `METER@FROM..UNTIL`.
#[derive(Debug, Clone, PartialEq)]
pub enum BandSourceArg {
    File(std::path::PathBuf),
    Span {
        /// Name or id.
        meter: String,
        from: TimeRef,
        until: TimeRef,
    },
}

impl FromStr for BandSourceArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        if let Some((meter, span)) = s.rsplit_once('@')
            && let Some((from, until)) = span.split_once("..")
        {
            if meter.trim().is_empty() {
                return fail(format!("{s:?}: name the meter before @"));
            }
            return Ok(Self::Span {
                meter: meter.trim().to_owned(),
                from: from.parse()?,
                until: until.parse()?,
            });
        }
        if s.trim().is_empty() {
            return fail("a band-level source: a file, or METER@FROM..UNTIL");
        }
        Ok(Self::File(s.into()))
    }
}

/// A 1/3-octave band by its nominal mid-band frequency: `63hz`, `31.5hz`, `1khz`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BandNominalArg(pub usize);

impl FromStr for BandNominalArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let f: Freq = s.parse()?;
        ac2_proto::model::band_index(f.0.0)
            .map(Self)
            .ok_or_else(|| {
                UnitError(format!(
                    "{s:?}: not a 1/3-octave band (nominal 20hz, 25hz, 31.5hz … 8khz, 10khz)"
                ))
            })
    }
}

/// Bands shown: `20hz..200hz` (a range, both ends shown) or `1khz` (one band).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BandRangeArg {
    pub from: usize,
    pub to: usize,
}

impl FromStr for BandRangeArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let (from, to) = match s.split_once("..") {
            Some((a, b)) => (
                a.parse::<BandNominalArg>()?.0,
                b.parse::<BandNominalArg>()?.0,
            ),
            None => {
                let b = s.parse::<BandNominalArg>()?.0;
                (b, b)
            }
        };
        if to < from {
            return fail(format!("{s:?}: a range runs low to high"));
        }
        Ok(Self { from, to })
    }
}

/// A band window's limit for one band: `z:60min:63hz=42db` (`60min:63hz=42db`: Z);
/// `…=none` removes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandLimitArg {
    pub window: LeqWindowArg,
    pub band: usize,
    pub limit: Option<DbSpl>,
}

impl FromStr for BandLimitArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let usage =
            || format!("{s:?}: expected window:band=limit, e.g. z:60min:63hz=42db (or …=none)");
        let parts = s
            .split_once('=')
            .and_then(|(lhs, l)| lhs.rsplit_once(':').map(|(w, b)| (w, b, l)));
        let Some((w, b, l)) = parts else {
            return fail(usage());
        };
        let Ok(band) = b.parse::<BandNominalArg>() else {
            return fail(usage());
        };
        let limit = if l.trim().eq_ignore_ascii_case("none") {
            None
        } else {
            Some(l.parse::<SplLevel>()?.0)
        };
        Ok(Self {
            window: w.parse()?,
            band: band.0,
            limit,
        })
    }
}

/// A band window's day offset: `z:60min=5db` (its limits hold at night, the day's this much
/// higher); `z:60min=none`: one set day and night.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DayOffsetArg {
    pub window: LeqWindowArg,
    pub offset: Option<Db>,
}

impl FromStr for DayOffsetArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let Some((w, o)) = s.split_once('=') else {
            return fail(format!(
                "{s:?}: expected window=offset, e.g. z:60min=5db (or z:60min=none)"
            ));
        };
        let offset = if o.trim().eq_ignore_ascii_case("none") {
            None
        } else {
            let g: Gain = o.parse()?;
            Some(Db(in_range(o, g.0.0, -30.0, 30.0, "a day offset")?))
        };
        Ok(Self {
            window: w.parse()?,
            offset,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_and_band_limits() {
        assert_eq!(ok::<BandNominalArg>("63hz"), BandNominalArg(5));
        assert_eq!(ok::<BandNominalArg>("1khz"), BandNominalArg(17));
        assert_eq!(ok::<BandNominalArg>("31.5hz"), BandNominalArg(2));
        assert!(bad::<BandNominalArg>("64hz").contains("not a 1/3-octave band"));
        assert_eq!(
            ok::<BandRangeArg>("20hz..200hz"),
            BandRangeArg { from: 0, to: 10 }
        );
        assert_eq!(
            ok::<BandRangeArg>("1khz"),
            BandRangeArg { from: 17, to: 17 }
        );
        assert!(bad::<BandRangeArg>("200hz..20hz").contains("low to high"));
        let l: BandLimitArg = ok("z:60min:63hz=42db");
        assert_eq!(
            (l.window.weighting, l.window.seconds, l.band, l.limit),
            (
                Some(ac2_proto::model::Weighting::Z),
                3600,
                5,
                Some(DbSpl(42.0))
            )
        );
        let l: BandLimitArg = ok("15min:1khz=none");
        assert_eq!((l.window.weighting, l.band, l.limit), (None, 17, None));
        assert!(bad::<BandLimitArg>("z:60min=42db").contains("window:band=limit"));
        let d: DayOffsetArg = ok("z:60min=5db");
        assert_eq!(d.offset, Some(Db(5.0)));
        assert_eq!(ok::<DayOffsetArg>("60min=none").offset, None);
        assert!(bad::<DayOffsetArg>("60min=40db").contains("day offset"));
    }

    #[test]
    fn peak_limits_and_positions() {
        use ac2_proto::model::PeakQuantity;
        let p: PeakLimitArg = ok("lcpeak=135db");
        assert_eq!(
            (p.quantity, p.limit),
            (PeakQuantity::LcPeak, Some(DbSpl(135.0)))
        );
        let p: PeakLimitArg = ok("LAFmax=none");
        assert_eq!((p.quantity, p.limit), (PeakQuantity::LafMax, None));
        for bad in ["lzpeak=130db", "lcpeak", "lcpeak=135"] {
            assert!(bad.parse::<PeakLimitArg>().is_err(), "{bad}");
        }
        assert_eq!(ok::<PositionArg>("4db").0, Some(Db(4.0)));
        assert_eq!(ok::<PositionArg>("-2.5 dB").0, Some(Db(-2.5)));
        assert_eq!(ok::<PositionArg>("none").0, None);
        for bad in ["4", "31db", "4dbfs"] {
            assert!(bad.parse::<PositionArg>().is_err(), "{bad}");
        }
    }

    fn ok<T: FromStr<Err = UnitError>>(s: &str) -> T {
        match s.parse::<T>() {
            Ok(v) => v,
            Err(e) => panic!("{s}: {e}"),
        }
    }

    fn bad<T: FromStr<Err = UnitError> + std::fmt::Debug>(s: &str) -> String {
        match s.parse::<T>() {
            Ok(v) => panic!("{s}: parsed as {v:?}"),
            Err(e) => e.0,
        }
    }

    #[test]
    fn frequencies() {
        assert_eq!(ok::<Freq>("20hz").0, Hz(20.0));
        assert_eq!(ok::<Freq>("1.5kHz").0, Hz(1500.0));
        assert_eq!(ok::<Freq>("2k").0, Hz(2000.0));
        assert_eq!(ok::<Freq>(" 31.5 Hz ").0, Hz(31.5));
        assert_eq!(ok::<Freq>("1e3hz").0, Hz(1000.0));
        for bad in [
            "20", "hz", "0hz", "-1hz", "2mhz", "infhz", "nanhz", "1.2.3hz", "", "2e9hz",
        ] {
            assert!(bad.parse::<Freq>().is_err(), "{bad}");
        }
    }

    #[test]
    fn volts_and_sensitivities() {
        assert_eq!(ok::<VoltsArg>("15.0mV").0, Volts(0.015));
        assert_eq!(ok::<VoltsArg>("0.0150v").0, Volts(0.015));
        assert!((ok::<VoltsArg>("250uV").0.0 - 250e-6).abs() < 1e-15);
        assert_eq!(ok::<VoltsArg>("1.228 V").0, Volts(1.228));
        for bad in ["0.015", "15", "0mv", "-1mv", "2000v", "15ma"] {
            assert!(bad.parse::<VoltsArg>().is_err(), "{bad}");
        }
        assert_eq!(ok::<MicSensitivityArg>("15.0mV/Pa").0, MvPerPa(15.0));
        assert_eq!(ok::<MicSensitivityArg>("0.05V/Pa").0, MvPerPa(50.0));
        let db = ok::<MicSensitivityArg>("-36.5dBV/Pa").0.0;
        assert!((db - 14.962).abs() < 1e-3, "{db}");
        for bad in ["15", "15mv", "0mv/pa", "15pa"] {
            assert!(bad.parse::<MicSensitivityArg>().is_err(), "{bad}");
        }
    }

    #[test]
    fn levels() {
        assert_eq!(ok::<LevelDbfs>("-20dbfs").0, Dbfs(-20.0));
        assert_eq!(ok::<LevelDbfs>("-12.5dBFS").0, Dbfs(-12.5));
        assert_eq!(ok::<LevelDbfs>("0dbfs").0, Dbfs(0.0));
        for bad in ["-20", "-20db", "3dbfs", "-200dbfs", "dbfs", "--3dbfs"] {
            assert!(bad.parse::<LevelDbfs>().is_err(), "{bad}");
        }
        assert_eq!(ok::<Gain>("-6db").0, Db(-6.0));
        assert_eq!(ok::<Gain>("+3dB").0, Db(3.0));
        assert!("3dbfs".parse::<Gain>().is_err());
        assert_eq!(ok::<SplLevel>("94db").0, DbSpl(94.0));
        assert_eq!(ok::<SplLevel>("114dBSPL").0, DbSpl(114.0));
        assert!("-3db".parse::<SplLevel>().is_err());
    }

    #[test]
    fn times_samples_distances() {
        assert_eq!(ok::<Time>("1.5ms").0, Seconds(0.0015));
        assert_eq!(ok::<Time>("2s").0, Seconds(2.0));
        assert_eq!(ok::<Time>("250us").0, Seconds(0.000_25));
        assert!("-1s".parse::<Time>().is_err());
        assert!("1".parse::<Time>().is_err());
        assert_eq!(ok::<SampleCount>("480samples").0, 480);
        assert_eq!(ok::<SampleCount>("-12smp").0, -12);
        assert!("1.5samples".parse::<SampleCount>().is_err());
        assert!("9e99samples".parse::<SampleCount>().is_err());
        assert_eq!(ok::<Distance>("120cm").0, 1.2);
        assert!("-1m".parse::<Distance>().is_err());
        assert_eq!(ok::<Celsius>("20c").0, 20.0);
        assert_eq!(ok::<Celsius>("-5°C").0, -5.0);
        assert!("300c".parse::<Celsius>().is_err());
        assert!("20f".parse::<Celsius>().is_err());
    }

    #[test]
    fn delay_amounts() {
        let c20 = Celsius(20.0);
        assert_eq!(
            ok::<DelayAmount>("12.5ms").seconds(None, c20),
            Ok(Seconds(0.0125))
        );
        assert_eq!(
            ok::<DelayAmount>("480samples").seconds(Some(48_000), c20),
            Ok(Seconds(0.01))
        );
        assert!(ok::<DelayAmount>("480samples").seconds(None, c20).is_err());
        assert_eq!(
            ok::<DelayAmount>("-0.25samples").seconds(Some(48_000), c20),
            Ok(Seconds(-0.25 / 48_000.0))
        );
        let d = ok::<DelayAmount>("3.432m")
            .seconds(None, c20)
            .unwrap_or(Seconds(0.0));
        // 343.2 m/s at 20 °C.
        assert!((d.0 - 0.01).abs() < 2e-5, "{}", d.0);
        assert_eq!(
            ok::<DelayAmount>("-2ms"),
            DelayAmount::Time(Seconds(-0.002))
        );
        assert!("11s".parse::<DelayAmount>().is_err());
        assert!("3".parse::<DelayAmount>().is_err());
    }

    #[test]
    fn channel_lists() {
        assert_eq!(ok::<Channels>("1,2").0, vec![0, 1]);
        assert_eq!(ok::<Channels>("1,4-6").0, vec![0, 3, 4, 5]);
        assert_eq!(ok::<Channel>("3").0, 2);
        assert_eq!(Channel(2).to_string(), "3");
        assert_eq!(channels_text(&[0, 3]), "1,4");
        for bad in ["0", "1,,2", "3-1", "1,1", "a", "", "257", "1-", "-2"] {
            assert!(bad.parse::<Channels>().is_err(), "{bad}");
        }
    }

    #[test]
    fn leq_windows_and_limits() {
        use ac2_proto::model::Weighting;
        let w: LeqWindowArg = ok("30min");
        assert_eq!((w.weighting(), w.seconds), (Weighting::A, 1800));
        let w: LeqWindowArg = ok("C:10s");
        assert_eq!((w.weighting, w.seconds), (Some(Weighting::C), 10));
        assert_eq!(ok::<LeqWindowArg>("1h").seconds, 3600);
        for bad in ["30", "0.5s", "2d", "x:1min", "25h", "1.5s"] {
            assert!(bad.parse::<LeqWindowArg>().is_err(), "{bad}");
        }
        let l: LeqLimitArg = ok("30min=99db");
        assert_eq!(l.limit, Some(DbSpl(99.0)));
        assert_eq!(l.window.seconds, 1800);
        let l: LeqLimitArg = ok("c:60min=None");
        assert_eq!(l.limit, None);
        for bad in ["30min", "30min=99", "=99db"] {
            assert!(bad.parse::<LeqLimitArg>().is_err(), "{bad}");
        }
    }
}
