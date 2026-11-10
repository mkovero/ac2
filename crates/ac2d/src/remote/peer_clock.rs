//! The relation between this rig's clocks and a remote rig's (`docs/design/remote-inputs.md`
//! §3.1), from what both rigs state and the cross-rig rate the joined feed measures.
//!
//! Stamps resolve about 2 ppm a minute (0.34–0.40 ppm sd per 60 s window measured), so ±2
//! ppm over a minute is 5σ of that noise; ±3 ppm over 40 s catches a fallback to a free
//! crystal within 40 s without tripping on a locked pair's short windows. A relation that
//! went *not locked* holds until a minute clean, so a rate wandering about the bound reads
//! as the problem it is rather than flickering.

use std::time::Duration;

use ac2_proto::model::{
    CantTellReason, ClockBasis, Clocks, NotLockedReason, PeerClock, PeerRelation, SampleClock,
    SystemClock,
};
use ac2_proto::units::{RigId, Seconds, WallNs};

/// Each rig's own clock line and the cross-rig rate over a minute.
const BOUND_PPM: f64 = 2.0;
/// The cross-rig rate over the short window.
const SHORT_BOUND_PPM: f64 = 3.0;
const LONG_S: f64 = 60.0;
const SHORT_S: f64 = 40.0;
/// Stream needed before the windows can say anything, and clean time to leave not locked.
const SETTLE: Duration = Duration::from_secs(60);

/// What the relation is judged from.
pub(crate) struct Evidence<'a> {
    pub(crate) ours: (RigId, &'a Clocks),
    pub(crate) theirs: (RigId, Option<&'a Clocks>),
    /// The source stamps capture times.
    pub(crate) capture_stamps: bool,
    /// The cross-rig rate over the long and the short window: (ppm, span s).
    pub(crate) long: Option<(f64, f64)>,
    pub(crate) short: Option<(f64, f64)>,
    /// Audio has streamed this long since it started or re-anchored.
    pub(crate) streaming_for: Option<Duration>,
    /// The system-time anchor fell outside the link offset: the clocks disagree by this, s.
    pub(crate) disagree: Option<f64>,
}

/// The relation as it stands, with what it needs to hold *not locked*.
#[derive(Debug, Clone)]
pub(crate) struct PeerClockWatch {
    pub(crate) clock: PeerClock,
    /// Since when the evidence has been clean while held not locked.
    clean_since: Option<Duration>,
}

impl Default for PeerClockWatch {
    fn default() -> Self {
        Self {
            clock: PeerClock {
                relation: PeerRelation::CantTell {
                    reason: CantTellReason::NoAudio,
                },
                ppm: None,
                span: None,
                last_change: None,
            },
            clean_since: None,
        }
    }
}

fn system_reason(rig: RigId, c: &Clocks) -> Option<CantTellReason> {
    match c.system {
        SystemClock::Synced { .. } => None,
        SystemClock::Unsynced { .. } => Some(CantTellReason::SystemClockUnsynced { rig }),
        SystemClock::NotStated => Some(CantTellReason::SystemClockNotStated { rig }),
    }
}

/// A rig's sample clock against its system clock, ppm; `Err` when it is measured against
/// the computer's own crystal instead; `Ok(None)` while measuring.
fn own_line(rig: RigId, c: &Clocks) -> Result<Option<f64>, CantTellReason> {
    match c.capture.as_ref().map(|d| d.rate) {
        Some(SampleClock::Measured {
            ppm,
            against: ClockBasis::SystemClock,
            ..
        }) => Ok(Some(ppm)),
        Some(SampleClock::Measured {
            against: ClockBasis::HostCrystal,
            ..
        }) => Err(CantTellReason::NotAgainstSystemClock { rig }),
        _ => Ok(None),
    }
}

/// The relation the evidence alone shows, before any hold.
fn judge(e: &Evidence<'_>) -> PeerRelation {
    let cant = |reason| PeerRelation::CantTell { reason };
    let Some(theirs) = e.theirs.1 else {
        return cant(CantTellReason::NoAudio);
    };
    let Some(streaming) = e.streaming_for else {
        return cant(CantTellReason::NoAudio);
    };
    for (rig, c) in [(e.ours.0, e.ours.1), (e.theirs.0, theirs)] {
        if let Some(r) = system_reason(rig, c) {
            return cant(r);
        }
    }
    let mut lines = Vec::new();
    for (rig, c) in [(e.ours.0, e.ours.1), (e.theirs.0, theirs)] {
        match own_line(rig, c) {
            Ok(l) => lines.push((rig, l)),
            Err(r) => return cant(r),
        }
    }
    if !e.capture_stamps {
        return cant(CantTellReason::NoCaptureStamps);
    }
    if let Some(by) = e.disagree {
        return cant(CantTellReason::ClocksDisagree { by: Seconds(by) });
    }
    for &(rig, l) in &lines {
        if let Some(ppm) = l
            && ppm.abs() > BOUND_PPM
        {
            return PeerRelation::NotLocked {
                reason: NotLockedReason::OwnClock { rig, ppm },
            };
        }
    }
    if let Some((ppm, _)) = e.short
        && ppm.abs() > SHORT_BOUND_PPM
    {
        return PeerRelation::NotLocked {
            reason: NotLockedReason::CrossRate {
                ppm,
                over: Seconds(SHORT_S),
            },
        };
    }
    if let Some((ppm, _)) = e.long
        && ppm.abs() > BOUND_PPM
    {
        return PeerRelation::NotLocked {
            reason: NotLockedReason::CrossRate {
                ppm,
                over: Seconds(LONG_S),
            },
        };
    }
    if streaming < SETTLE || e.long.is_none() || lines.iter().any(|(_, l)| l.is_none()) {
        return PeerRelation::Measuring;
    }
    PeerRelation::Agree
}

fn same_kind(a: &PeerRelation, b: &PeerRelation) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
        && match (a, b) {
            (PeerRelation::CantTell { reason: x }, PeerRelation::CantTell { reason: y }) => {
                std::mem::discriminant(x) == std::mem::discriminant(y)
            }
            _ => true,
        }
}

impl PeerClockWatch {
    /// Judges the evidence at `now` (time since the daemon's start, for the hold) and wall
    /// time `wall`; whether the relation changed.
    pub(crate) fn update(&mut self, e: &Evidence<'_>, now: Duration, wall: WallNs) -> bool {
        let judged = judge(e);
        let held = matches!(self.clock.relation, PeerRelation::NotLocked { .. });
        let relation = match (&judged, held) {
            // Held not locked until a minute of clean evidence; measuring is clean too, as it
            // only says the windows are short.
            (PeerRelation::Agree | PeerRelation::Measuring, true) => {
                let since = *self.clean_since.get_or_insert(now);
                if now.saturating_sub(since) >= SETTLE {
                    self.clean_since = None;
                    judged
                } else {
                    self.clock.relation
                }
            }
            _ => {
                self.clean_since = None;
                judged
            }
        };
        let (ppm, span) = match e.long.or(e.short) {
            Some((p, s)) => (Some(p), Some(Seconds(s))),
            None => (None, None),
        };
        let changed = !same_kind(&relation, &self.clock.relation);
        let before = self.clock.clone();
        self.clock = PeerClock {
            relation,
            ppm,
            span,
            last_change: if changed {
                Some(wall)
            } else {
                self.clock.last_change
            },
        };
        before != self.clock
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::DeviceClock;

    fn clocks(system: SystemClock, ppm: Option<f64>) -> Clocks {
        Clocks {
            system,
            capture: Some(DeviceClock {
                device: ac2_proto::model::DeviceId("fake".into()),
                rate: ppm.map_or(SampleClock::Measuring, |ppm| SampleClock::Measured {
                    ppm,
                    span: Seconds(60.0),
                    against: ClockBasis::SystemClock,
                }),
            }),
            stamp_map: None,
        }
    }

    const SYNCED: SystemClock = SystemClock::Synced { maxerror_us: 100 };

    fn ev<'a>(a: &'a Clocks, b: &'a Clocks, long: Option<f64>, secs: u64) -> Evidence<'a> {
        Evidence {
            ours: (RigId(1), a),
            theirs: (RigId(2), Some(b)),
            capture_stamps: true,
            long: long.map(|p| (p, 60.0)),
            short: long.map(|p| (p, 40.0)),
            streaming_for: Some(Duration::from_secs(secs)),
            disagree: None,
        }
    }

    #[test]
    fn locked_rigs_agree_after_a_minute_and_measure_before() {
        let (a, b) = (clocks(SYNCED, Some(0.1)), clocks(SYNCED, Some(-0.2)));
        assert_eq!(judge(&ev(&a, &b, None, 10)), PeerRelation::Measuring);
        assert_eq!(judge(&ev(&a, &b, Some(0.3), 61)), PeerRelation::Agree);
    }

    #[test]
    fn a_cross_rate_beyond_the_bounds_is_not_locked() {
        let (a, b) = (clocks(SYNCED, Some(0.1)), clocks(SYNCED, Some(0.0)));
        let r = judge(&ev(&a, &b, Some(5.0), 61));
        assert!(matches!(
            r,
            PeerRelation::NotLocked {
                reason: NotLockedReason::CrossRate { .. }
            }
        ));
        // A short window beyond 3 ppm trips before the minute is up.
        assert!(matches!(
            judge(&ev(&a, &b, Some(3.5), 41)),
            PeerRelation::NotLocked { .. }
        ));
    }

    #[test]
    fn an_interface_off_its_own_host_is_not_locked() {
        let (a, b) = (clocks(SYNCED, Some(0.1)), clocks(SYNCED, Some(5.6)));
        assert_eq!(
            judge(&ev(&a, &b, Some(0.0), 61)),
            PeerRelation::NotLocked {
                reason: NotLockedReason::OwnClock {
                    rig: RigId(2),
                    ppm: 5.6
                }
            }
        );
    }

    #[test]
    fn unsynced_or_unstated_system_clocks_cannot_tell() {
        let a = clocks(SystemClock::Unsynced { maxerror_us: 1 }, Some(0.0));
        let b = clocks(SystemClock::NotStated, Some(0.0));
        assert_eq!(
            judge(&ev(&a, &clocks(SYNCED, None), Some(0.0), 61)),
            PeerRelation::CantTell {
                reason: CantTellReason::SystemClockUnsynced { rig: RigId(1) }
            }
        );
        assert_eq!(
            judge(&ev(&clocks(SYNCED, None), &b, Some(0.0), 61)),
            PeerRelation::CantTell {
                reason: CantTellReason::SystemClockNotStated { rig: RigId(2) }
            }
        );
    }

    #[test]
    fn not_locked_holds_until_a_minute_clean() {
        let (a, b) = (clocks(SYNCED, Some(0.0)), clocks(SYNCED, Some(0.0)));
        let mut w = PeerClockWatch::default();
        let t = |s: u64| Duration::from_secs(s);
        w.update(&ev(&a, &b, Some(5.0), 100), t(100), WallNs(1));
        assert!(matches!(w.clock.relation, PeerRelation::NotLocked { .. }));
        w.update(&ev(&a, &b, Some(0.0), 130), t(130), WallNs(2));
        assert!(matches!(w.clock.relation, PeerRelation::NotLocked { .. }));
        w.update(&ev(&a, &b, Some(0.0), 180), t(180), WallNs(3));
        assert!(matches!(w.clock.relation, PeerRelation::NotLocked { .. }));
        w.update(&ev(&a, &b, Some(0.0), 191), t(191), WallNs(4));
        assert_eq!(w.clock.relation, PeerRelation::Agree);
        assert_eq!(w.clock.last_change, Some(WallNs(4)));
    }
}
