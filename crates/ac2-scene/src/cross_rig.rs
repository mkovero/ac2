//! The words of a transfer function between two rigs (`docs/design/remote-inputs.md` §5):
//! its pane title naming the remote side, its status lines, the faint-phase verdict of a
//! held result, the top-bar banner while it is stopped, its cue in the measurement list,
//! the basis of its delay readout, and `ac2 status`'s lines. Wherever a cross-rig result
//! appears its clock relation is said next to it, in words.

use ac2_proto::model::{
    CantTellReason, CrossRigTransfer, CrossTfState, Measurement, NotLockedReason, PeerRelation,
    RelockOutcome, RemoteAudio, RemoteInput, RemoteLink, RemoteRig, RemoteSide, RemoteState,
    StopWhy,
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

/// A remote rig's chip in the top bar, beside the rig drop-down: *ketunkolo · clocks
/// agree*, *ketunkolo · measuring clocks…*, or as a warning *ketunkolo · CLOCKS NOT LOCKED
/// +5.6 ppm*. The warning is said in capitals as well as coloured: never only a colour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RigChip {
    pub text: String,
    pub warn: bool,
}

/// The chip's tooltip: where a click leads.
pub const CHIP_TIP: &str = "the clocks and the link between the rigs: Settings › Connection › \
                            Between rigs";

/// The chip of remote rig `rig`; `inputs`: the session's remote inputs. No audio comes
/// first (no relation can be measured without it), then the relation, then a link offset
/// too small for a transfer function on it.
pub fn rig_chip(r: &RemoteState, rig: &RemoteRig, inputs: &[RemoteInput]) -> RigChip {
    let chip = |what: String, warn: bool| RigChip {
        text: format!("{} · {what}", rig.name),
        warn,
    };
    if no_audio(r, rig, inputs) {
        return chip("NO AUDIO".into(), true);
    }
    match rig.clock.relation {
        PeerRelation::NotLocked { reason } => {
            let ppm = match reason {
                NotLockedReason::CrossRate { ppm, .. } | NotLockedReason::OwnClock { ppm, .. } => {
                    ppm
                }
            };
            chip(
                format!("CLOCKS NOT LOCKED {} ppm", format::signed(ppm, 1)),
                true,
            )
        }
        PeerRelation::CantTell { .. } => chip("CLOCKS: CAN'T TELL".into(), true),
        _ if r.transfers.iter().any(|x| x.rig == rig.rig && x.late) => {
            chip("LINK OFFSET TOO SMALL".into(), true)
        }
        PeerRelation::Measuring => chip("measuring clocks…".into(), false),
        PeerRelation::Agree => chip("clocks agree".into(), false),
    }
}

/// No audio comes from `rig`: its link is down or refused, the relation says so, or none of
/// its inputs carries audio this rig can use.
fn no_audio(r: &RemoteState, rig: &RemoteRig, inputs: &[RemoteInput]) -> bool {
    if !matches!(rig.link, RemoteLink::Up | RemoteLink::Connecting) {
        return true;
    }
    if matches!(
        rig.clock.relation,
        PeerRelation::CantTell {
            reason: CantTellReason::NoAudio
        }
    ) {
        return true;
    }
    let audio: Vec<RemoteAudio> = inputs
        .iter()
        .filter(|i| i.rig == rig.rig)
        .filter_map(|i| r.inputs.iter().find(|s| s.slot == i.slot))
        .map(|s| s.audio)
        .collect();
    !audio.is_empty()
        && audio.iter().all(|a| {
            !matches!(
                a,
                RemoteAudio::Streaming | RemoteAudio::Starting | RemoteAudio::Waiting
            )
        })
}

/// The clock line a pane's title carries while the transfer function averages: *clocks
/// agree within 2 ppm · ketunkolo*. Stopped, the pane's banner says it instead.
pub fn caption(x: &CrossRigTransfer, r: &RemoteState) -> Option<String> {
    matches!(x.state, CrossTfState::Running).then(|| status_lines(x, r).remove(0))
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
/// `ours` this rig's name).
pub fn pane_banners(
    label: &str,
    x: &CrossRigTransfer,
    r: &RemoteState,
    ours: &str,
) -> Vec<PaneBanner> {
    let rig = name_of(r, x.rig);
    let mut out = Vec::new();
    if let (Some(v), CrossTfState::Stopped { why, held }) = (stop_verdict(x), x.state) {
        let detail = match (why, banner(label, x, r, ours)) {
            (StopWhy::Measuring, _) | (_, None) => format!(
                "less than a minute of audio from {rig} since the stream started: the clocks                  between the rigs are not known yet"
            ),
            (_, Some(b)) if held => {
                format!("{b} The result from before stands; its phase is faint.")
            }
            (_, Some(b)) => format!("{b} No result was kept from before."),
        };
        out.push(PaneBanner {
            text: v.to_owned(),
            detail: Some(detail),
            warn: why != StopWhy::Measuring,
        });
    }
    if let Some(t) = relock_text(x) {
        let asks = matches!(
            x.relock.map(|r| r.outcome),
            Some(RelockOutcome::FindAgain | RelockOutcome::Ambiguous)
        );
        out.push(PaneBanner {
            text: t,
            detail: asks.then(|| "the phase is faint until the delay is found again".into()),
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
            detail: Some(
                "the delay moved more than 0.05 ms a minute over two minutes: a path or the \
                 clocks are moving"
                    .into(),
            ),
            warn: false,
        });
    }
    if x.late {
        out.push(PaneBanner {
            text: late_text(r),
            detail: Some(format!(
                "audio from {rig} keeps arriving after it is needed: Settings › Connection ›                  {LINK_OFFSET}"
            )),
            warn: true,
        });
    }
    out
}

/// The Connection page's link offset control, as `late_text` names it.
pub const LINK_OFFSET: &str = "Wait for remote inputs";

/// The link offset control's line: *Wait for remote inputs (link offset): 30 ms*, or
/// *…: default (10 ms)*. `set`: the session's own setting; `r`: what the daemon runs.
pub fn link_offset_line(set: Option<ac2_proto::units::Seconds>, r: &RemoteState) -> String {
    let v = match (set, r.link_offset) {
        (Some(s), _) => format::ms(s.0, 0),
        (None, Some(o)) => format!("default ({})", format::ms(o.0, 0)),
        (None, None) => "default".to_owned(),
    };
    format!("{LINK_OFFSET} (link offset): {v}")
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
