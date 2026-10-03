//! The log as a whole in words: how long the meter has logged, since when, its total Leq
//! and its gaps (`docs/design/leq.md`, "Run clock and total"), from the `leq` frame's
//! `run`, and the confirmation before a new log ends it.

use ac2_proto::frame::LeqRun;
use ac2_proto::model::{LeqConfig, Weighting};
use ac2_proto::units::WallNs;

use super::{clock, w_letter};
use crate::format;

/// Hours a full log holds (`SplLogPage::RETAINED_ROWS`).
const RETAINED_H: usize = ac2_proto::model::SplLogPage::RETAINED_ROWS / 3600;

/// Every string of the run, and the caption's wordings of it from longest to shortest.
#[derive(Clone, Debug, PartialEq)]
pub struct LeqRunText {
    /// Time from the log's oldest kept second to its newest: `2:14:05` (always hours, so
    /// it never reads as a time of day).
    pub clock: String,
    /// Local time of the log's start: `19:02`, with the date when it is not the newest
    /// second's day (`2 Oct 19:02`).
    pub since: String,
    /// The totals shown: `("LAeq", "97.8")`, then C and Z when a window uses them.
    pub totals: Vec<(String, String)>,
    /// Every window is A-weighted, so a bare "total" can only mean LAeq.
    pub plain_total: bool,
    /// Time not measured, when there was any: `0:12`.
    pub gaps: Option<String>,
    /// The log is at its retention: the clock and the total cover the last 48 h kept.
    pub trimmed: bool,
}

/// `y`, `m` (1–12), `d` of a day count from 1970-01-01 (civil-from-days, proleptic
/// Gregorian).
fn civil(days: i64) -> (i64, u32, u32) {
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

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Local seconds since the epoch of `t` with the UTC offset in force then.
fn local_s(t: WallNs, offset_s: i32) -> i64 {
    (t.0 / 1_000_000_000) as i64 + i64::from(offset_s)
}

/// The run of an SPL meter's log in words. `offset_s` gives the local UTC offset (s) in
/// force at a wall time: the start is shown in local time, and its date when that is not
/// the newest second's local day. Totals follow the windows' weightings: LAeq always,
/// LCeq and LZeq when a window uses them.
pub fn run_text(run: &LeqRun, cfg: &LeqConfig, offset_s: impl Fn(WallNs) -> i32) -> LeqRunText {
    let span = run.until.0.saturating_sub(run.started_at.0) as f64 / 1e9;
    let s = span.floor() as u64;
    let clock_text = format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60);
    let start = local_s(run.started_at, offset_s(run.started_at));
    let end = local_s(run.until, offset_s(run.until));
    let (sd, ed) = (start.div_euclid(86_400), end.div_euclid(86_400));
    let tod = start.rem_euclid(86_400);
    let hm = format!("{}:{:02}", tod / 3600, (tod % 3600) / 60);
    let since = if sd == ed {
        hm
    } else {
        let (_, m, d) = civil(sd);
        format!("{d} {} {hm}", MONTHS[(m - 1) as usize])
    };
    let used = |w: Weighting| cfg.windows.iter().any(|x| x.weighting == w);
    let mut totals = vec![("LAeq".to_owned(), format::level(run.laeq))];
    for (w, v) in [(Weighting::C, run.lceq), (Weighting::Z, run.lzeq)] {
        if used(w) {
            totals.push((format!("L{}eq", w_letter(w)), format::level(v)));
        }
    }
    let gaps = run.gaps.0.max(0.0);
    LeqRunText {
        clock: clock_text,
        since,
        totals,
        plain_total: !used(Weighting::C) && !used(Weighting::Z),
        gaps: (gaps >= 1.0).then(|| clock(gaps)),
        trimmed: run.trimmed,
    }
}

impl LeqRunText {
    /// `LAeq total 97.8`, each shown total.
    fn all_totals(&self) -> String {
        self.totals
            .iter()
            .map(|(n, v)| format!("{n} total {v}"))
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// The first total, named.
    fn first_total(&self) -> String {
        self.totals
            .first()
            .map(|(n, v)| format!("{n} total {v}"))
            .unwrap_or_default()
    }

    /// The first total, short: `total 97.8` when only LAeq can be meant.
    fn short_total(&self) -> String {
        match self.totals.first() {
            Some((_, v)) if self.plain_total => format!("total {v}"),
            _ => self.first_total(),
        }
    }

    fn with_gaps(&self, s: String) -> String {
        match &self.gaps {
            Some(g) => format!("{s} · gaps {g}"),
            None => s,
        }
    }

    /// The caption's wordings, longest first; each drops or shortens a part of the one
    /// before. The last is what is kept at any width: the clock (or "last 48 h").
    pub fn variants(&self) -> Vec<String> {
        let head = if self.trimmed {
            format!("last {RETAINED_H} h: {} since {}", self.clock, self.since)
        } else {
            format!("running {} since {}", self.clock, self.since)
        };
        let lead = if self.trimmed {
            format!("last {RETAINED_H} h")
        } else {
            self.clock.clone()
        };
        let mut v = vec![
            self.with_gaps(format!("{head} · {}", self.all_totals())),
            self.with_gaps(format!("{head} · {}", self.first_total())),
            self.with_gaps(format!(
                "{} since {} · {}",
                if self.trimmed {
                    lead.clone()
                } else {
                    self.clock.clone()
                },
                self.since,
                self.short_total()
            )),
            self.with_gaps(format!("{lead} · {}", self.short_total())),
            format!("{lead} · {}", self.short_total()),
            lead,
        ];
        v.dedup();
        v
    }

    /// The longest wording: `running 2:14:05 since 19:02 · LAeq total 97.8`.
    pub fn line(&self) -> String {
        self.variants().swap_remove(0)
    }
}

/// What the confirmation before a new log says.
#[derive(Clone, Debug, PartialEq)]
pub struct NewLogConfirm {
    /// `Start a new SPL log for FOH SPL?`
    pub title: String,
    /// What ends, what starts over, what is kept, where the old log goes.
    pub lines: Vec<String>,
    /// The keys.
    pub hint: String,
}

/// The confirmation before `spl.log_new` on `meter`: it discards show data, so it names
/// the run that ends and everything that starts over.
pub fn new_log_confirm(meter: &str, cfg: &LeqConfig, run: Option<&LeqRunText>) -> NewLogConfirm {
    let n = cfg.windows.len();
    let ends = match run {
        Some(r) => format!("The current log ends: {}.", r.line()),
        None => "The current log ends (no seconds logged yet).".to_owned(),
    };
    let windows = match n {
        0 => "no Leq windows".to_owned(),
        1 => "the Leq window".to_owned(),
        _ => format!("the {n} Leq windows"),
    };
    NewLogConfirm {
        title: format!("Start a new SPL log for {meter}?"),
        lines: vec![
            ends,
            format!(
                "Starts over: {windows} and their states, the alarms, the run clock and the \
                 total."
            ),
            "Kept: the windows, limits and headroom horizon.".to_owned(),
            "The ended log stays exportable until the next new log or a daemon restart: \
             ac2 spl leq export --previous"
                .to_owned(),
        ],
        hint: "Enter starts the new log · N or Esc keeps the current one".to_owned(),
    }
}
