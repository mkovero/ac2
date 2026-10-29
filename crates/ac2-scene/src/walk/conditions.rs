//! The walk pane's warning when its captures were taken under different conditions
//! (`docs/design/spatial-walk.md` §5, *Captures that differ*): a walk resumed another day,
//! or a change in the chain between captures. It warns, never refuses: the average of such
//! captures is still an average, the operator decides whether it is the one wanted.
//!
//! Every claim is a fact the captures recorded ([`CaptureContext`]): a condition the daemon
//! does not record is never said to have changed or stayed.

use ac2_proto::model::{CaptureChain, CaptureContext, PointState, WalkState};
use ac2_proto::spl_log::civil_from_days;
use ac2_proto::units::WallNs;

use crate::time::MONTHS;

/// A recorded condition that differs between a walk's captures, in the order the line names
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Changed {
    /// Another mic stood on a walk measurement's input.
    Mic,
    /// The same mic, another correction curve (or one on and off).
    MicCurve,
    /// The EQ marked "in the console" on a walk measurement.
    Eq,
}

impl Changed {
    pub fn text(self) -> &'static str {
        match self {
            Self::Mic => "mic changed",
            Self::MicCurve => "mic curve changed",
            Self::Eq => "EQ changed",
        }
    }
}

/// The local day of `at`: days since 1970-01-01 at the UTC offset `offset_s` gives for it.
fn local_day(at: WallNs, offset_s: &impl Fn(WallNs) -> i32) -> i64 {
    ((at.0 / 1_000_000_000) as i64 + i64::from(offset_s(at))).div_euclid(86_400)
}

/// `today`, `yesterday`, otherwise the date (`27 Oct`, with the year when not `today`'s).
fn day_word(day: i64, today: i64) -> String {
    match today - day {
        0 => "today".to_owned(),
        1 => "yesterday".to_owned(),
        _ => {
            let (y, m, d) = civil_from_days(day);
            let (ty, _, _) = civil_from_days(today);
            let month = MONTHS[(m - 1) as usize];
            if y == ty {
                format!("{d} {month}")
            } else {
                format!("{d} {month} {y}")
            }
        }
    }
}

/// What differs between the chains of two captures.
fn differ(a: &CaptureChain, b: &CaptureChain, out: &mut Vec<Changed>) {
    match (&a.mic, &b.mic) {
        (Some(x), Some(y)) if x.name != y.name => out.push(Changed::Mic),
        (Some(x), Some(y)) => {
            // The curve's file contents are what the columns had subtracted; a re-import of
            // the same file is no change.
            let hash = |c: &Option<ac2_proto::model::MicCurveRef>| {
                c.as_ref().map(|c| c.content_hash.clone())
            };
            if hash(&x.curve) != hash(&y.curve) {
                out.push(Changed::MicCurve);
            }
        }
        (None, None) => {}
        _ => out.push(Changed::Mic),
    }
    let marks = |c: &CaptureChain| {
        let mut m = c.eq_in.clone();
        m.sort();
        m
    };
    if marks(a) != marks(b) {
        out.push(Changed::Eq);
    }
}

/// The recorded conditions that differ between any two of `contexts`, each named once.
pub fn changed(contexts: &[&CaptureContext]) -> Vec<Changed> {
    let mut out = Vec::new();
    if let Some((first, rest)) = contexts.split_first() {
        for c in rest {
            // Chains pair by walk measurement; a walk keeps its measurements, so both
            // captures hold the same number of them.
            for (a, b) in first.chains.iter().zip(&c.chains) {
                differ(a, b, &mut out);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `P1–P4 yesterday, P5–P8 today · EQ changed`: the captured points grouped by the local
/// day they were captured, oldest day first, then each recorded condition that differs.
/// Empty when every capture was taken on one day under the same conditions. A run of a
/// day's points goes on across points with no capture (pending or skipped: they belong to
/// no day), so `P1–P4` says nothing of P3 when it was skipped. `today` is on the captures'
/// (the daemon's) clock; `offset_s` gives the local UTC offset in force at a wall time.
pub fn conditions_text(w: &WalkState, today: WallNs, offset_s: impl Fn(WallNs) -> i32) -> String {
    let captured: Vec<(&str, &CaptureContext)> = w
        .points
        .iter()
        .filter_map(|p| match &p.state {
            PointState::Captured { context, .. } => Some((p.label.as_str(), context)),
            PointState::Pending | PointState::Skipped => None,
        })
        .collect();
    let mut parts: Vec<String> = Vec::new();
    // Runs of consecutive captures on one day: (day, first label, last label).
    let mut runs: Vec<(i64, &str, &str)> = Vec::new();
    for &(label, c) in &captured {
        let day = local_day(c.at, &offset_s);
        match runs.last_mut() {
            Some(r) if r.0 == day => r.2 = label,
            _ => runs.push((day, label, label)),
        }
    }
    let mut days: Vec<i64> = runs.iter().map(|r| r.0).collect();
    days.sort_unstable();
    days.dedup();
    if days.len() > 1 {
        let today = local_day(today, &offset_s);
        let groups: Vec<String> = days
            .iter()
            .map(|&d| {
                let ranges: Vec<String> = runs
                    .iter()
                    .filter(|r| r.0 == d)
                    .map(|&(_, a, b)| {
                        if a == b {
                            a.to_owned()
                        } else {
                            format!("{a}–{b}")
                        }
                    })
                    .collect();
                format!("{} {}", ranges.join(" + "), day_word(d, today))
            })
            .collect();
        parts.push(groups.join(", "));
    }
    let contexts: Vec<&CaptureContext> = captured.iter().map(|&(_, c)| c).collect();
    let changes = changed(&contexts);
    if !changes.is_empty() && parts.is_empty() {
        // One day: the line still says what it is about.
        parts.push("captures differ".to_owned());
    }
    parts.extend(changes.iter().map(|c| c.text().to_owned()));
    parts.join(" · ")
}
