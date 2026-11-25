//! Reading either per-second log back (the SPL log here, the band log in `ac2-traces`): the
//! header lines, the rows in key order, and where a cut write left off.

use super::{SplLogError, SplLogInfo, parse_utc_iso};
use crate::model::LogGap;
use crate::units::{MeasId, WallNs};

/// A row of a per-second log: what its order is checked by.
pub trait LogRow {
    /// `(second, run, gap)`.
    fn key(&self) -> (u64, u32, Option<LogGap>);
}

/// A log read back.
#[derive(Debug, Clone, PartialEq)]
pub struct LogRead<R> {
    /// What the header names.
    pub info: SplLogInfo,
    /// Rows, oldest first.
    pub rows: Vec<R>,
    /// Bytes up to the end of the last whole row: where an append carries on.
    pub complete_len: usize,
    /// Bytes after it (a torn row, zeros, garbage), cut off before the next append.
    pub tail: usize,
}

#[derive(Default)]
struct Head {
    meas: Option<MeasId>,
    name: Option<String>,
    input: Option<u16>,
    mic: Option<String>,
    zone: Option<String>,
    started: Option<WallNs>,
}

impl Head {
    fn take(&mut self, l: &str, line: usize) -> Result<(), SplLogError> {
        let bad = |msg: &str| SplLogError::Line {
            line,
            msg: msg.to_owned(),
        };
        let Some((k, v)) = l.trim_start_matches('#').trim_start().split_once(": ") else {
            return Ok(());
        };
        match k {
            "meas" => self.meas = Some(MeasId(v.parse().map_err(|_| bad("bad meas"))?)),
            "name" => self.name = Some(v.to_owned()),
            "input" => {
                let n: u32 = v.parse().map_err(|_| bad("bad input"))?;
                let i = n
                    .checked_sub(1)
                    .and_then(|i| u16::try_from(i).ok())
                    .ok_or_else(|| bad("bad input"))?;
                self.input = Some(i);
            }
            "mic" => self.mic = Some(v.to_owned()),
            "zone" => self.zone = Some(v.to_owned()),
            "started" => {
                self.started = Some(WallNs(parse_utc_iso(v).ok_or_else(|| bad("bad started"))?));
            }
            _ => {}
        }
        Ok(())
    }

    fn info(self, line: usize) -> Result<SplLogInfo, SplLogError> {
        let missing = |what: &str| SplLogError::Line {
            line,
            msg: format!("the header names no {what}"),
        };
        Ok(SplLogInfo {
            meas: self.meas.ok_or_else(|| missing("meas"))?,
            name: self.name.ok_or_else(|| missing("name"))?,
            input: self.input.ok_or_else(|| missing("input"))?,
            mic: self.mic,
            zone: self.zone.ok_or_else(|| missing("zone"))?,
            started: self.started.ok_or_else(|| missing("started"))?,
        })
    }
}

/// Reads a log back: its first line `<prefix><version>` (another version is
/// [`SplLogError::Version`]), its header lines, its column header `columns`, its rows by
/// `parse`. A log grows a line at a time and a power cut keeps an unpredictable part of the
/// last appends: a torn last row, zeros a file system extended the file with (possibly
/// after a torn row, possibly with newlines among them), bytes that are not text. The rows
/// before the first such line are the log; that line and everything after it are the tail,
/// cut off before the next append ([`LogRead::complete_len`]). A whole line of text that
/// is not a row, or a row out of key order (seconds rise; a run's first row and only it
/// carries a gap), was written so and is an error.
pub fn read_log<R: LogRow>(
    bytes: &[u8],
    prefix: &str,
    version: u32,
    columns: &str,
    parse: impl Fn(&str, usize) -> Result<R, SplLogError>,
) -> Result<LogRead<R>, SplLogError> {
    let whole = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let mut rows: Vec<R> = Vec::new();
    let mut header = false;
    let mut head = Head::default();
    let mut info = None;
    let mut complete_len = 0;
    let mut off = 0;
    for (i, raw) in bytes[..whole].split_inclusive(|&b| b == b'\n').enumerate() {
        let line = i + 1;
        let start = off;
        off += raw.len();
        let text = std::str::from_utf8(raw).map(str::trim_end);
        if !header {
            let found = text
                .ok()
                .and_then(|l| l.strip_prefix(prefix))
                .and_then(|v| v.parse::<u32>().ok())
                .ok_or(SplLogError::Header)?;
            if found != version {
                return Err(SplLogError::Version {
                    found,
                    reads: version,
                });
            }
            header = true;
            complete_len = off;
            continue;
        }
        if info.is_none() {
            match text {
                Ok(l) if l.starts_with('#') => head.take(l, line)?,
                Ok("") => {}
                Ok(l) if l == columns => info = Some(std::mem::take(&mut head).info(line)?),
                _ => {
                    return Err(SplLogError::Line {
                        line,
                        msg: "expected the column header".to_owned(),
                    });
                }
            }
            complete_len = off;
            continue;
        }
        // What a cut leaves past the last whole row: zeros (a file system that committed the
        // length before the data) or bytes that are not text.
        let junk = raw.contains(&0) || text.is_err();
        let parsed = match text {
            Ok(l) => parse(l, line),
            Err(_) => Err(SplLogError::Header),
        };
        match parsed {
            Ok(row) => {
                if let Some(prev) = rows.last() {
                    in_order(prev.key(), row.key()).map_err(|msg| SplLogError::Line {
                        line,
                        msg: msg.to_owned(),
                    })?;
                }
                rows.push(row);
                complete_len = off;
            }
            Err(_) if junk => {
                complete_len = start;
                break;
            }
            Err(e) => return Err(e),
        }
    }
    if !header {
        return Err(SplLogError::Header);
    }
    let info = info.ok_or(SplLogError::Line {
        line: 1,
        msg: "no column header".into(),
    })?;
    Ok(LogRead {
        info,
        rows,
        complete_len,
        tail: bytes.len() - complete_len,
    })
}

/// Whether a row keyed `b` may follow one keyed `a`.
fn in_order(
    (s0, r0, _): (u64, u32, Option<LogGap>),
    (s1, r1, g1): (u64, u32, Option<LogGap>),
) -> Result<(), &'static str> {
    if s1 <= s0 {
        return Err("second does not rise");
    }
    match (r1.cmp(&r0), g1) {
        (std::cmp::Ordering::Less, _) => Err("run falls"),
        (std::cmp::Ordering::Greater, None) => Err("a run's first row without its gap"),
        (std::cmp::Ordering::Equal, Some(_)) => Err("a gap inside a run"),
        _ => Ok(()),
    }
}
