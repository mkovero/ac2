//! Band-log time spans, band-level sources and 1/3-octave band windows.

use std::str::FromStr;

use super::*;

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

/// A band, or a run of them: `63hz`, `20hz..200hz` (both ends included).
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

/// Band windows of one length and weighting: `z:60min@63hz` (one band), or
/// `z:60min@20hz..200hz`, one window per band of the range (Z unless `a:` or `c:`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BandWindowArg {
    pub window: LeqWindowArg,
    pub bands: BandRangeArg,
}

impl BandWindowArg {
    /// Its bands, indices into the 1/3-octave bands, low to high.
    pub fn bands(&self) -> std::ops::RangeInclusive<usize> {
        self.bands.from..=self.bands.to
    }
}

impl FromStr for BandWindowArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let Some((w, b)) = s.split_once('@') else {
            return fail(format!(
                "{s:?}: a band window names its band after @, e.g. z:60min@63hz (or \
                 z:60min@20hz..200hz for one window per band)"
            ));
        };
        Ok(Self {
            window: w.parse()?,
            bands: b.parse()?,
        })
    }
}

/// A band window's limit: `z:60min@63hz=42db` (a range: each band's window);
/// `…=none` removes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandLimitArg {
    pub window: BandWindowArg,
    pub limit: Option<DbSpl>,
}

impl FromStr for BandLimitArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let Some((w, l)) = s.split_once('=') else {
            return fail(format!(
                "{s:?}: expected window=limit, e.g. z:60min@63hz=42db (or …=none)"
            ));
        };
        let limit = if l.trim().eq_ignore_ascii_case("none") {
            None
        } else {
            Some(l.parse::<SplLevel>()?.0)
        };
        Ok(Self {
            window: w.parse()?,
            limit,
        })
    }
}

/// A band window's day offset: `z:60min@63hz=5db` (its limit holds at night, the day's
/// this much higher); `…=none`: one limit day and night. A range: each band's window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DayOffsetArg {
    pub window: BandWindowArg,
    pub offset: Option<Db>,
}

impl FromStr for DayOffsetArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let Some((w, o)) = s.split_once('=') else {
            return fail(format!(
                "{s:?}: expected window=offset, e.g. z:60min@63hz=5db (or …=none)"
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
