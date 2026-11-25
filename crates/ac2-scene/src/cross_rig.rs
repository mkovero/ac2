//! The words of a transfer function between two rigs (`docs/design/remote-inputs.md` §5):
//! its pane title naming the remote side, its status lines, the faint-phase verdict of a
//! held result, its pane banner and why while it is stopped, its cue in the measurement list,
//! the basis of its delay readout, and `ac2 status`'s lines. Wherever a cross-rig result
//! appears its clock relation is said next to it, in words.

use ac2_proto::model::{
    CrossRigTransfer, CrossTfState, NotLockedReason, PeerRelation, RelockOutcome, RemoteRig,
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
/// with the plot chrome hidden. `None` while it averages. A result held from before a
/// re-anchor is said even while the clocks are measured again: the curve on screen is not
/// what the clocks now measure.
pub fn stop_verdict(x: &CrossRigTransfer) -> Option<&'static str> {
    match x.state {
        CrossTfState::Running => None,
        CrossTfState::Stopped { why, held } => Some(match (why, held) {
            (StopWhy::Measuring, false) => "Measuring clocks…: averaging waits",
            (StopWhy::Measuring, true) => "Measuring clocks…: averaging waits · result from before",
            (StopWhy::NotLocked, _) => "Clocks not locked: averaging stopped",
            (StopWhy::CantTell, _) => "Clocks: can't tell: averaging stopped",
        }),
    }
}

/// The phase is drawn faint (the magnitude stands, as for an independent-epoch trace) under
/// these words: any held result (its time base is not known to hold across the stop,
/// whatever the clocks say now), or a delay that may have moved and waits for the operator.
pub fn faint_phase(x: &CrossRigTransfer) -> Option<&'static str> {
    if let CrossTfState::Stopped { why, held: true } = x.state {
        return Some(match why {
            StopWhy::NotLocked => "clocks not locked",
            StopWhy::CantTell => "clocks: can't tell",
            StopWhy::Measuring => "measuring clocks: result from before",
        });
    }
    match x.relock.map(|r| r.outcome) {
        Some(RelockOutcome::FindAgain) => Some("delay may have moved: find it again"),
        Some(RelockOutcome::Ambiguous) => Some("delay ambiguous: pick one"),
        _ => None,
    }
}

/// What a passage through a stop did to the delay: *clocks were not locked for 3 min 10 s ·
/// delay tracked again: 13.41 ms (was 12.38 ms)*. `find`: how the operator runs the delay
/// finder where the words are read (the app's key, `ac2 delay find` on the command line).
pub fn relock_text(x: &CrossRigTransfer, find: &str) -> Option<String> {
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
        RelockOutcome::Ambiguous => format!("delay ambiguous: {find} to pick one"),
        RelockOutcome::FindAgain => format!("delay may have moved: {find} to find it again"),
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

/// The pane's status lines: the clock line first (*clocks agree · ketunkolo*,
/// or why it stopped), then a relock for a minute, a slip, the drift and late cues.
/// `find`: as for [`relock_text`].
pub fn status_lines(x: &CrossRigTransfer, r: &RemoteState, find: &str) -> Vec<String> {
    let rig = name_of(r, x.rig);
    let mut out = vec![match stop_verdict(x) {
        None => format!("{} · {rig}", running_clock_line(x, r)),
        Some(v) => format!("{v} · {rig}"),
    }];
    out.extend(relock_text(x, find));
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

/// The clock line of a transfer function that averages. It still averages for a moment
/// after the relation leaves Agree (the gate acts on the audio once it is past the link
/// offset), and then the relation is said, not the agreement it no longer has.
fn running_clock_line(x: &CrossRigTransfer, r: &RemoteState) -> String {
    match r
        .rigs
        .iter()
        .find(|g| g.rig == x.rig)
        .map(|g| &g.clock.relation)
    {
        Some(rel) => crate::between_rigs::relation_text(r, &name_of(r, x.rig), rel),
        // The remote state holds no relation for the rig: that much is known, not why.
        None => format!(
            "CLOCKS: CAN'T TELL (no clock reading for {})",
            name_of(r, x.rig)
        ),
    }
}

/// *link offset too small: raise Wait for remote inputs (now 10 ms)*.
fn late_text(r: &RemoteState) -> String {
    let now = r
        .link_offset
        .map(|o| format!(" (now {})", format::ms(o.0, 0)))
        .unwrap_or_default();
    format!("link offset too small: raise Wait for remote inputs{now}")
}

/// Why the clocks stop it, and the fix where one helps: the detail of its pane banner and a
/// line of `ac2 status -v`. `ours`: this rig's name. `None` while it averages or only waits
/// for the clocks to be measured.
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
                "{label} stopped: clocks {} ppm apart · lock both to one reference",
                format::signed(ppm, 1)
            ),
            // One rig's interface against its own host: its crystal, or a host whose
            // discipline wanders, not the two rigs against each other.
            NotLockedReason::OwnClock { rig: who, ppm } => {
                let who = if who == x.rig { theirs.as_str() } else { ours };
                format!(
                    "{label} stopped: {who}'s interface runs {} ppm off",
                    format::signed(ppm, 1)
                )
            }
            NotLockedReason::OtherRate { theirs: t, ours: o } => format!(
                "{label} stopped: {}.",
                crate::between_rigs::other_rate(theirs, t, o)
            ),
        }),
        (StopWhy::NotLocked, _) => Some(format!(
            "{label} stopped: clocks not locked · lock both to one reference"
        )),
        (StopWhy::CantTell, PeerRelation::CantTell { reason }) => Some(format!(
            "{label} stopped: can't tell the clocks: {}",
            cant_tell_why(r, theirs, &reason)
        )),
        (StopWhy::CantTell, _) => Some(format!("{label} stopped: can't tell the clocks")),
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

/// [`delay_basis`] in short, for a line that names the rigs elsewhere: *12.38 ms · incl.
/// the rigs' clock offset (not measured)*.
pub fn delay_basis_short(delay_s: f64) -> String {
    format!(
        "{} · incl. the rigs' clock offset (not measured)",
        format::delay(delay_s)
    )
}

/// The clocks between this rig and remote rig `rig`, for that rig's heading in the
/// measurement list, and whether it is a warning: *clocks agree*, *measuring clocks…*, or
/// as a warning *CLOCKS NOT LOCKED +5.6 ppm*, *CLOCKS: CAN'T TELL*, and a link offset too
/// small for a transfer function on it. A warning is said in capitals: a heading has one
/// colour.
pub fn clock_words(r: &RemoteState, rig: &RemoteRig) -> (String, bool) {
    match rig.clock.relation {
        PeerRelation::NotLocked { reason } => match reason {
            NotLockedReason::CrossRate { ppm, .. } | NotLockedReason::OwnClock { ppm, .. } => (
                format!("CLOCKS NOT LOCKED {} ppm", format::signed(ppm, 1)),
                true,
            ),
            NotLockedReason::OtherRate { theirs, ours } => (
                format!(
                    "CLOCKS NOT LOCKED: {}",
                    crate::between_rigs::other_rate(&rig.name, theirs, ours)
                ),
                true,
            ),
        },
        rel @ PeerRelation::CantTell { .. } => {
            (crate::between_rigs::relation_text(r, &rig.name, &rel), true)
        }
        _ if r.transfers.iter().any(|x| x.rig == rig.rig && x.late) => {
            ("LINK OFFSET TOO SMALL".into(), true)
        }
        PeerRelation::Measuring => ("measuring clocks…".into(), false),
        PeerRelation::Agree => ("clocks agree".into(), false),
    }
}

/// The clock line a pane's title carries while the transfer function averages and the
/// clocks agree: *clocks agree · ketunkolo*. Otherwise the pane's banner and
/// the rig's heading in the list say it, and the title says nothing it would contradict.
pub fn caption(x: &CrossRigTransfer, r: &RemoteState) -> Option<String> {
    let agree = r
        .rigs
        .iter()
        .any(|g| g.rig == x.rig && g.clock.relation == PeerRelation::Agree);
    (matches!(x.state, CrossTfState::Running) && agree)
        .then(|| format!("clocks agree · {}", name_of(r, x.rig)))
}

/// A line of a cross-rig transfer function's banner strip on its pane: its stop verdict
/// (which stays with the plot chrome hidden), the relock, slip, drift and late cues.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneBanner {
    pub text: String,
    /// Why, and what to do.
    pub detail: Option<String>,
    /// A warning (the operator has something to do or the result is held); else a note.
    pub warn: bool,
}

/// The banner lines of the pane showing cross-rig transfer function `x` (`label` its name,
/// `ours` this rig's name, `find` as for [`relock_text`]).
pub fn pane_banners(
    label: &str,
    x: &CrossRigTransfer,
    r: &RemoteState,
    ours: &str,
    find: &str,
) -> Vec<PaneBanner> {
    let rig = name_of(r, x.rig);
    let mut out = Vec::new();
    if let (Some(v), CrossTfState::Stopped { why, .. }) = (stop_verdict(x), x.state) {
        let why_text = match why {
            StopWhy::Measuring => Some(format!("under a minute of audio from {rig} yet")),
            _ => banner(label, x, r, ours),
        };
        // Whether a result from before stands shows on the plot: its phase drawn faint.
        out.push(PaneBanner {
            text: v.to_owned(),
            detail: why_text,
            warn: why != StopWhy::Measuring,
        });
    }
    if let Some(t) = relock_text(x, find) {
        let asks = matches!(
            x.relock.map(|r| r.outcome),
            Some(RelockOutcome::FindAgain | RelockOutcome::Ambiguous)
        );
        out.push(PaneBanner {
            text: t,
            detail: None,
            warn: asks,
        });
    }
    if let Some(s) = x.slip {
        out.push(PaneBanner {
            text: format!(
                "{rig}'s audio slipped {} · delay moved with it",
                format::delay(s.by.0.abs())
            ),
            detail: None,
            warn: false,
        });
    }
    if let Some(d) = drift_text(x) {
        out.push(PaneBanner {
            text: format!("{d}: path or clocks"),
            detail: Some("over 0.05 ms a minute for two minutes".into()),
            warn: false,
        });
    }
    if x.late {
        out.push(PaneBanner {
            text: late_text(r),
            detail: Some(format!(
                "audio from {rig} arrives late · Settings › Connection"
            )),
            warn: true,
        });
    }
    out
}

/// The Connection page's link offset control, as `late_text` names it.
pub const LINK_OFFSET: &str = "Wait for remote inputs";

/// The link offset control's line: *Wait for remote inputs (link offset): 30 ms*, or
/// *…: default (10 ms)*. `set`: the session's own setting; `r`: what the daemon runs. A set
/// value below the least that works is raised to it by the daemon (below it every remote
/// sample is late), and then the line says both: *1 ms set · 17 ms in force, the least
/// that works*.
pub fn link_offset_line(set: Option<ac2_proto::units::Seconds>, r: &RemoteState) -> String {
    let v = match (set, r.link_offset) {
        (Some(s), Some(o)) if o.0 > s.0 && format::ms(o.0, 0) != format::ms(s.0, 0) => format!(
            "{} set · {} in force, the least that works",
            format::ms(s.0, 0),
            format::ms(o.0, 0)
        ),
        (Some(s), _) => format::ms(s.0, 0),
        (None, Some(o)) => format!("default ({})", format::ms(o.0, 0)),
        (None, None) => "default".to_owned(),
    };
    format!("{LINK_OFFSET} (link offset): {v}")
}

/// `ac2 status`'s short form: `link offset 15 ms`, whether set or the daemon's default.
pub fn link_offset_short(set: Option<ac2_proto::units::Seconds>, r: &RemoteState) -> String {
    let v = match (set, r.link_offset) {
        (Some(s), Some(o)) if o.0 > s.0 && format::ms(o.0, 0) != format::ms(s.0, 0) => format!(
            "{} set, {} in force",
            format::ms(s.0, 0),
            format::ms(o.0, 0)
        ),
        (Some(s), _) => format::ms(s.0, 0),
        (None, Some(o)) => format::ms(o.0, 0),
        (None, None) => "default".to_owned(),
    };
    format!("link offset {v}")
}

/// What the link offset control asks while it is typed.
pub const LINK_OFFSET_PROMPT: &str =
    "Wait for remote inputs (a time up to 500 ms, e.g. 20ms, or default):";

/// The longest link offset the daemon takes: audio meeting a remote input waits this long
/// at most.
pub const LINK_OFFSET_MAX_S: f64 = 0.5;

/// A typed link offset: `20ms`, `20` (a bare number reads as ms, as the delay field reads
/// it), or `default` (`None`: the least that works).
pub fn parse_link_offset(s: &str) -> Result<Option<f64>, String> {
    use crate::unit_text::{Time, UnitText};
    let t = s.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("default") {
        return Ok(None);
    }
    let Time(sec) = Time::parse_text(t, Some("ms")).map_err(|e| e.0)?;
    if sec.0 <= 0.0 || sec.0 > LINK_OFFSET_MAX_S {
        return Err(format!(
            "{t}: the link offset is above 0 and at most 500 ms (or default)"
        ));
    }
    Ok(Some(sec.0))
}

/// The toast after the link offset is set: the session reopens, its remote inputs anchor
/// again.
pub fn link_offset_toast(offset_s: Option<f64>) -> String {
    let v = offset_s.map_or_else(|| "default".to_owned(), |s| format::ms(s, 0));
    format!("link offset {v}: the remote inputs anchor again, their averages restart")
}

#[cfg(test)]
#[path = "cross_rig_tests.rs"]
mod tests;
