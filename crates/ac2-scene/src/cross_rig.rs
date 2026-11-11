//! The words of a transfer function between two rigs (`docs/design/remote-inputs.md` §5):
//! its pane title naming the remote side, its status lines, the faint-phase verdict of a
//! held result, the top-bar banner while it is stopped, its cue in the measurement list,
//! the basis of its delay readout, and `ac2 status`'s lines. Wherever a cross-rig result
//! appears its clock relation is said next to it, in words.

use ac2_proto::model::{
    CrossRigTransfer, CrossTfState, Measurement, NotLockedReason, PeerRelation, RelockOutcome,
    RemoteSide, RemoteState, StopWhy,
};

use crate::between_rigs::{cant_tell_why, name_of};
use crate::format;

/// The cross-rig entry of measurement `meas`, if it is one.
pub fn entry(r: &RemoteState, meas: ac2_proto::units::MeasId) -> Option<&CrossRigTransfer> {
    r.transfers.iter().find(|x| x.meas == meas)
}

/// The pane title: *TF 3 · meas from ketunkolo*, *TF 3 · ref from ketunkolo*.
pub fn pane_title(label: &str, x: &CrossRigTransfer, r: &RemoteState) -> String {
    let side = match x.side {
        RemoteSide::Reference => "ref",
        RemoteSide::Measurement => "meas",
    };
    format!("{label} · {side} from {}", name_of(r, x.rig))
}

/// Why it does not average, as the pane says it over the held curve; the verdict stays
/// with the plot chrome hidden. `None` while it averages.
pub fn stop_verdict(x: &CrossRigTransfer) -> Option<&'static str> {
    match x.state {
        CrossTfState::Running => None,
        CrossTfState::Stopped { why, .. } => Some(match why {
            StopWhy::Measuring => "Measuring clocks…: averaging waits",
            StopWhy::NotLocked => "Clocks not locked: averaging stopped",
            StopWhy::CantTell => "Clocks: can't tell: averaging stopped",
        }),
    }
}

/// The phase is drawn faint (the magnitude stands, as for an independent-epoch trace) under
/// these words: a held result, or a delay that may have moved and waits for the operator.
pub fn faint_phase(x: &CrossRigTransfer) -> Option<&'static str> {
    match x.state {
        CrossTfState::Stopped {
            why: StopWhy::NotLocked,
            held: true,
        } => return Some("clocks not locked"),
        CrossTfState::Stopped {
            why: StopWhy::CantTell,
            held: true,
        } => return Some("clocks: can't tell"),
        _ => {}
    }
    match x.relock.map(|r| r.outcome) {
        Some(RelockOutcome::FindAgain) => Some("delay may have moved: find it again"),
        Some(RelockOutcome::Ambiguous) => Some("delay ambiguous: pick one"),
        _ => None,
    }
}

/// What a passage through a stop did to the delay: *clocks were not locked for 3 min 10 s ·
/// delay tracked again: 13.41 ms (was 12.38 ms)*.
pub fn relock_text(x: &CrossRigTransfer) -> Option<String> {
    let r = x.relock?;
    let span = format::duration(r.after.0);
    let passage = match r.from {
        StopWhy::NotLocked => format!("clocks were not locked for {span}"),
        StopWhy::CantTell => format!("clocks could not be told for {span}"),
        StopWhy::Measuring => format!("clocks were measured again for {span}"),
    };
    let delay = match r.outcome {
        RelockOutcome::Tracking => "tracking the delay again…".to_owned(),
        RelockOutcome::Tracked { delay, was } => format!(
            "delay tracked again: {} (was {})",
            format::delay(delay.0),
            format::delay(was.0)
        ),
        RelockOutcome::Ambiguous => "delay ambiguous: pick one".to_owned(),
        RelockOutcome::FindAgain => "delay may have moved: find it again".to_owned(),
    };
    Some(format!("{passage} · {delay}"))
}

/// The drift cue: *delay drifting 0.30 ms/min*.
fn drift_text(x: &CrossRigTransfer) -> Option<String> {
    x.delay_drift.map(|d| {
        format!(
            "delay drifting {} ms/min",
            format::fixed(d.0.abs() * 1e3, 2)
        )
    })
}

/// The pane's status lines: the clock line first (*clocks agree within 2 ppm · ketunkolo*,
/// or why it stopped), then a relock for a minute, a slip, the drift and late cues.
pub fn status_lines(x: &CrossRigTransfer, r: &RemoteState) -> Vec<String> {
    let rig = name_of(r, x.rig);
    let mut out = vec![match stop_verdict(x) {
        None => format!("clocks agree within 2 ppm · {rig}"),
        Some(v) => format!("{v} · {rig}"),
    }];
    out.extend(relock_text(x));
    if let Some(s) = x.slip {
        out.push(format!(
            "{rig}'s audio slipped {} · delay moved with it",
            format::delay(s.by.0.abs())
        ));
    }
    if let Some(d) = drift_text(x) {
        out.push(format!("{d}: path or clocks"));
    }
    if x.late {
        out.push(late_text(r));
    }
    out
}

/// *link offset too small: raise Wait for remote inputs (now 10 ms)*.
fn late_text(r: &RemoteState) -> String {
    let now = r
        .link_offset
        .map(|o| format!(" (now {})", format::ms(o.0, 0)))
        .unwrap_or_default();
    format!("link offset too small: raise Wait for remote inputs{now}")
}

/// The top-bar banner while it is stopped by the clocks: why and what to do. `ours`: this
/// rig's name. `None` while it averages or only waits for the clocks to be measured.
pub fn banner(label: &str, x: &CrossRigTransfer, r: &RemoteState, ours: &str) -> Option<String> {
    let rig = r.rigs.iter().find(|g| g.rig == x.rig)?;
    let theirs = &rig.name;
    let CrossTfState::Stopped { why, .. } = x.state else {
        return None;
    };
    match (why, rig.clock.relation) {
        (StopWhy::Measuring, _) => None,
        (StopWhy::NotLocked, PeerRelation::NotLocked { reason }) => Some(match reason {
            NotLockedReason::CrossRate { ppm, .. } => format!(
                "{label} stopped: {ours} and {theirs} are not on one clock ({} ppm). Lock both \
                 interfaces to one reference.",
                format::signed(ppm, 1)
            ),
            // One rig's interface against its own host: its crystal, or a host whose
            // discipline wanders, not the two rigs against each other.
            NotLockedReason::OwnClock { rig: who, ppm } => {
                let who = if who == x.rig { theirs.as_str() } else { ours };
                format!(
                    "{label} stopped: {who}'s interface runs {} ppm against its host. Lock it \
                     to the reference its host is disciplined from.",
                    format::signed(ppm, 1)
                )
            }
        }),
        (StopWhy::NotLocked, _) => Some(format!(
            "{label} stopped: {ours} and {theirs} are not on one clock. Lock both interfaces \
             to one reference."
        )),
        (StopWhy::CantTell, PeerRelation::CantTell { reason }) => Some(format!(
            "{label} stopped: can't tell whether {ours} and {theirs} share a clock: {}.",
            cant_tell_why(r, &reason)
        )),
        (StopWhy::CantTell, _) => Some(format!(
            "{label} stopped: can't tell whether {ours} and {theirs} share a clock."
        )),
    }
}

/// Its short cue in the measurement list, so it shows with its pane closed: *clocks* while
/// the clocks stop it, else the slip, drift or late cue.
pub fn list_cue(x: &CrossRigTransfer) -> Option<String> {
    match x.state {
        CrossTfState::Stopped {
            why: StopWhy::NotLocked | StopWhy::CantTell,
            ..
        } => return Some("clocks".into()),
        CrossTfState::Stopped {
            why: StopWhy::Measuring,
            ..
        } => return Some("measuring clocks…".into()),
        CrossTfState::Running => {}
    }
    if matches!(
        x.relock.map(|r| r.outcome),
        Some(RelockOutcome::FindAgain | RelockOutcome::Ambiguous)
    ) {
        return Some("delay may have moved".into());
    }
    if let Some(s) = x.slip {
        return Some(format!("audio slipped {}", format::delay(s.by.0.abs())));
    }
    if let Some(d) = drift_text(x) {
        return Some(d);
    }
    x.late.then(|| "link offset too small".into())
}

/// The delay readout of a cross-rig transfer function, with its basis: *12.38 ms ·
/// includes the clock offset between pupu and ketunkolo (not measured)*. The network and
/// the link offset are not in it: remote audio is placed by its capture time.
pub fn delay_basis(delay_s: f64, ours: &str, theirs: &str) -> String {
    format!(
        "{} · includes the clock offset between {ours} and {theirs} (not measured)",
        format::delay(delay_s)
    )
}

/// `ac2 status` lines, one block per cross-rig transfer function of `measurements`: its
/// title, then its status lines and delay with its basis, indented. `ours`: this rig's name.
pub fn status_cli(r: &RemoteState, measurements: &[Measurement], ours: &str) -> Vec<String> {
    let mut out = Vec::new();
    for x in &r.transfers {
        let Some(m) = measurements.iter().find(|m| m.id == x.meas) else {
            continue;
        };
        out.push(format!("between rigs {}", pane_title(&m.config.name, x, r)));
        for l in status_lines(x, r) {
            out.push(format!("  {l}"));
        }
        if let Some(b) = banner(&m.config.name, x, r, ours) {
            out.push(format!("  {b}"));
        }
        if let Some(d) = &m.delay {
            out.push(format!(
                "  delay {}",
                delay_basis(d.applied.0, ours, &name_of(r, x.rig))
            ));
        }
    }
    out
}

#[cfg(test)]
#[path = "cross_rig_tests.rs"]
mod tests;
