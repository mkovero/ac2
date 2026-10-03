//! Typed, unit-suffixed command-line values: `20hz`, `1.5khz`, `-12dbfs`, `3db`, `94db`,
//! `1.5ms`, `480samples`, `3.4m`, `20c`, channel lists `1,2,5-6`.
//!
//! Every value must carry its unit (a bare `-20` is refused: is it dBFS or dB?), units are
//! case-insensitive, and parsers never panic: any input yields a value or a message.
//! Channels are 1-based on the command line (as printed on interfaces) and zero-based on the
//! wire.

use std::fmt;
use std::str::FromStr;

use ac2_proto::units::{Db, DbSpl, Dbfs, Hz, Seconds};

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

fn seconds_of(n: f64, u: &str) -> Option<f64> {
    match u {
        "s" | "sec" => Some(n),
        "ms" => Some(n * 1e-3),
        "us" | "µs" | "μs" => Some(n * 1e-6),
        _ => None,
    }
}

/// A non-negative duration: `1.5ms`, `2s`, `250us`. ≤ 1 day.
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
/// `4.3m`. Time may be negative (|t| ≤ 10 s).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DelayAmount {
    /// Seconds.
    Time(Seconds),
    /// Samples at the session rate.
    Samples(i64),
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
        if let Some(r) = samples_of(s, n, &u) {
            return r.map(Self::Samples);
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
                Some(r) if r > 0 => Ok(Seconds(n as f64 / f64::from(r))),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ok<T: FromStr<Err = UnitError>>(s: &str) -> T {
        match s.parse::<T>() {
            Ok(v) => v,
            Err(e) => panic!("{s}: {e}"),
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
