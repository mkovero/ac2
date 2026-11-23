//! The standing evidence on the control thread (`docs/design/remote-auto.md` §5.1): every
//! paired rig's levels feed judged into a relation beside the audio-fed one, which it does
//! not replace; it is logged when it changes.

use ac2_proto::model::{CantTellReason, PeerRelation, RemoteLink, StandingRig, StandingSession};
use ac2_proto::units::{Seconds, WallNs};

use crate::remote::peer_clock::{Evidence, PeerClockWatch};
use crate::remote::standing::Absent;
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
            let absent = match (up, report.map(|r| r.absent)) {
                (false, _) => Some(CantTellReason::LinkDown),
                (true, None) => Some(CantTellReason::NoMarks),
                (true, Some(None)) => None,
                (true, Some(Some(Absent::OurSession))) => {
                    Some(CantTellReason::NoSession { rig: self.s.rig_id })
                }
                (true, Some(Some(Absent::TheirSession))) => {
                    Some(CantTellReason::NoSession { rig: p.rig })
                }
                (true, Some(Some(Absent::Marks))) => Some(CantTellReason::NoMarks),
            };
            let evidence = Evidence {
                ours: (self.s.rig_id, &ours),
                theirs: (p.rig, src.map(|s| &s.clocks)),
                capture_stamps: our_stamps
                    && src
                        .and_then(|s| s.session.as_ref())
                        .is_none_or(|s| s.capture_stamps),
                long: report.and_then(|r| r.long),
                short: report.and_then(|r| r.short),
                streaming_for: report
                    .filter(|_| absent.is_none())
                    .and_then(|r| r.marks_for),
                disagree: report.and_then(|r| r.disagree),
                absent: absent.unwrap_or(CantTellReason::NoMarks),
                other_rate: report.and_then(|r| r.other_rate).filter(|_| up),
            };
            let watch = self
                .remote
                .standing_watches
                .entry(p.rig)
                .or_insert_with(|| PeerClockWatch::absent(CantTellReason::LinkDown));
            let before = watch.clock.last_change;
            watch.update(&evidence, now, wall);
            let name = src.map_or_else(|| p.name.clone(), |s| s.name.clone());
            if watch.clock.last_change != before {
                log_change(&name, &watch.clock.relation, watch.clock.ppm);
            }
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
                clock: watch.clock.clone(),
                marks_for: report
                    .and_then(|r| r.marks_for)
                    .map(|d| Seconds(d.as_secs_f64())),
            });
        }
        self.remote
            .standing_watches
            .retain(|rig, _| peers.iter().any(|p| p.rig == *rig));
        out
    }
}

/// One line per change of the standing relation, beside the audio-fed one's.
fn log_change(name: &str, relation: &PeerRelation, ppm: Option<f64>) {
    match ppm {
        Some(p) => tracing::info!("clocks with {name} from levels: {relation:?}, {p:+.2} ppm"),
        None => tracing::info!("clocks with {name} from levels: {relation:?}"),
    }
}
