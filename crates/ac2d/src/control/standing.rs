//! The standing evidence on the control thread (`docs/design/remote-auto.md` §5.1): every
//! paired rig's levels feed judged into a relation beside the audio-fed one, which it does
//! not replace; it is logged when it changes.

use std::collections::BTreeMap;

use ac2_proto::model::{
    CantTellReason, PeerClock, PeerRelation, RemoteLink, StandingRig, StandingSession,
};
use ac2_proto::units::{RigId, Seconds, WallNs};

use crate::remote::peer_clock::{Evidence, same_kind};
use crate::remote::standing::Absent;
use crate::remote::standing_verdict::{Shown, StandingWatch, late};
use crate::util::wall_ns;

use super::Control;
use super::remote::capture_stamps;

impl Control {
    /// `remote.standing`: each paired rig as its levels feed shows it. `network`: this rig
    /// has a server key to link with.
    pub(super) fn standing_entries(&mut self, network: bool) -> Vec<StandingRig> {
        let st = self.store.state();
        let ours = st.clocks.clone();
        let peers = st.remote.peers.clone();
        let our_stamps = self
            .session
            .as_ref()
            .is_none_or(|rt| capture_stamps(rt.open.backend));
        let set_offset = self
            .session
            .as_ref()
            .and_then(|rt| rt.open.config.link_offset)
            .map(|s| s.0);
        let now = self.remote.started.elapsed();
        let wall = WallNs(wall_ns());
        let mut out = Vec::with_capacity(peers.len());
        for p in &peers {
            let link = if network {
                self.remote
                    .link_state
                    .get(&p.rig)
                    .map_or(RemoteLink::Connecting, |(l, ..)| l.clone())
            } else {
                RemoteLink::NoServerKey
            };
            let up = link == RemoteLink::Up;
            let report = self.remote.standing.get(&p.rig);
            let src = self.remote.sources.get(&p.rig);
            // Linked but neither its state nor its feed heard yet: nothing is missing, it
            // is only early.
            let early = up && (src.is_none() || report.is_none());
            let their_session = src.is_some_and(|s| s.session.is_some());
            let absent = match report.map(|r| r.absent) {
                _ if !up => Some(CantTellReason::LinkDown),
                None | Some(Some(Absent::Marks)) => Some(CantTellReason::NoMarks),
                Some(None) => None,
                Some(Some(Absent::OurSession)) => {
                    Some(CantTellReason::NoSession { rig: self.s.rig_id })
                }
                // Its state shows a session yet no frame comes: its levels, not its session.
                Some(Some(Absent::TheirFrames)) if their_session => Some(CantTellReason::NoMarks),
                Some(Some(Absent::TheirFrames)) => Some(CantTellReason::NoSession { rig: p.rig }),
            };
            let fed = report.filter(|_| absent.is_none());
            let evidence = Evidence {
                ours: (self.s.rig_id, &ours),
                theirs: (p.rig, src.map(|s| &s.clocks)),
                capture_stamps: our_stamps
                    && src
                        .and_then(|s| s.session.as_ref())
                        .is_none_or(|s| s.capture_stamps),
                long: fed.and_then(|r| r.long),
                short: fed.and_then(|r| r.short),
                streaming_for: fed.and_then(|r| r.marks_for),
                disagree: fed.and_then(|r| r.ahead),
                late: fed.and_then(|r| late(r, set_offset)),
                absent: absent.unwrap_or(CantTellReason::NoMarks),
                other_rate: report.and_then(|r| r.other_rate).filter(|_| up),
            };
            let shown = self
                .remote
                .standing_watches
                .entry(p.rig)
                .or_insert_with(StandingWatch::new)
                .update(evidence, fed.and_then(|r| r.minutes), now, wall);
            let relation = if early {
                PeerRelation::Measuring
            } else {
                shown.relation
            };
            let figures = (shown.ppm, shown.span);
            let name = src.map_or_else(|| p.name.clone(), |s| s.name.clone());
            log_raw(&name, &shown);
            let clock = standing_clock(
                &mut self.remote.standing_shown,
                p.rig,
                &name,
                relation,
                figures,
                wall,
            );
            let session = report
                .filter(|_| up)
                .and_then(|r| r.levels.as_deref())
                .map(|f| StandingSession {
                    sample_rate_hz: f.meta.sample_rate_hz,
                    channels: f.meta.channels.clone(),
                });
            out.push(StandingRig {
                rig: p.rig,
                name,
                link,
                session,
                clock,
                marks_since: shown.marks_since.filter(|_| fed.is_some()),
                counting: shown.counting.filter(|_| !early),
            });
        }
        let kept = |rig: &RigId| peers.iter().any(|p| p.rig == *rig);
        self.remote.standing_watches.retain(|rig, _| kept(rig));
        self.remote.standing_shown.retain(|rig, _| kept(rig));
        out
    }
}

/// What the hold was given, logged as it happens: each minute as it closes, and the
/// judge's relation as it changes (bounded by the watch). With the shown relation's own line
/// it tells a verdict the hold kept back from one the evidence withheld.
fn log_raw(name: &str, shown: &Shown) {
    if let Some((raw, unlogged)) = &shown.raw_changed {
        let held = if *unlogged > 0 {
            format!(" ({unlogged} changes not logged before it)")
        } else {
            String::new()
        };
        tracing::info!("clocks with {name} from levels, judged before the hold: {raw:?}{held}");
    }
    if let Some(c) = &shown.closed {
        let cross = c
            .cross
            .map_or("too thin to fit".into(), |p| format!("{p:+.2} ppm"));
        let lines: Vec<String> = c
            .lines
            .iter()
            .map(|l| {
                format!(
                    "rig {} {:+.2} ppm{}",
                    l.rig,
                    l.ppm,
                    if l.young { " (young)" } else { "" }
                )
            })
            .collect();
        let lines = if lines.is_empty() {
            "none".into()
        } else {
            lines.join(", ")
        };
        tracing::info!(
            "clocks with {name} from levels: minute closed at {cross}, worst own lines {lines}, \
             {}, judged {:?}",
            if c.classed { "classed" } else { "not classed" },
            c.raw
        );
    }
}

/// The relation shown for `rig` with the watch's figures (ppm, span), its time of change
/// kept from the one shown before (`shown`); logged when it changes.
fn standing_clock(
    shown: &mut BTreeMap<RigId, PeerClock>,
    rig: RigId,
    name: &str,
    relation: PeerRelation,
    (ppm, span): (Option<f64>, Option<Seconds>),
    wall: WallNs,
) -> PeerClock {
    let before = shown.get(&rig);
    let changed = before.is_none_or(|b| !same_kind(&b.relation, &relation));
    let clock = PeerClock {
        relation,
        ppm,
        span,
        last_change: if changed {
            Some(wall)
        } else {
            before.and_then(|b| b.last_change)
        },
    };
    if changed {
        match ppm {
            Some(p) => {
                tracing::info!("clocks with {name} from levels: {relation:?}, {p:+.2} ppm")
            }
            None => tracing::info!("clocks with {name} from levels: {relation:?}"),
        }
    }
    shown.insert(rig, clock.clone());
    clock
}
