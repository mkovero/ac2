//! How often the link drains, wakes and keeps frames: meter and display pacing.

use super::*;

/// `old`'s `leq` frames that `new` replaces with a newer one or drops, as a snapshot of
/// their own; `None` when every one of them is still in `new`.
pub(super) fn leq_only(old: &DataSnapshot, new: &DataSnapshot) -> Option<DataSnapshot> {
    let frames: BTreeMap<Arc<str>, ac2_client::TopicFrame> = old
        .latest
        .frames
        .iter()
        .filter(|(k, f)| {
            matches!(
                f.topic,
                Topic::Data {
                    stream: Stream::Leq,
                    ..
                }
            ) && new
                .latest
                .frames
                .get(*k)
                .is_none_or(|n| n.frame.stamp.seq != f.frame.stamp.seq)
        })
        .map(|(k, f)| (k.clone(), f.clone()))
        .collect();
    (!frames.is_empty()).then(|| DataSnapshot {
        latest: Latest {
            frames,
            ..old.latest.clone()
        },
        grids: BTreeMap::new(),
        drained: old.drained,
    })
}

/// Whether two mirror views differ in anything the UI shows, keepalive time aside. The
/// daemon's silence is judged from `responding` flips, which wake the UI on their own.
pub fn mirror_differs(a: &MirrorView, b: &MirrorView) -> bool {
    let state = match (&a.state, &b.state) {
        (Some(x), Some(y)) => !Arc::ptr_eq(x, y),
        (None, None) => false,
        _ => true,
    };
    state
        || a.phase != b.phase
        || a.incarnation != b.incarnation
        || a.session_epoch != b.session_epoch
        || a.rev != b.rev
        || a.generator != b.generator
        || a.timing != b.timing
        || a.clock_offset_ns.is_some() != b.clock_offset_ns.is_some()
        || a.snapshots != b.snapshots
        || a.since_requests != b.since_requests
        || a.incarnation_changes != b.incarnation_changes
        || a.client_id != b.client_id
}

/// Whether two frames of a topic draw the same: equal content, protection, grid and
/// configuration. A meter in steady silence sends such frames; showing them changes no
/// pixel.
pub fn same_picture(a: &ac2_proto::Frame, b: &ac2_proto::Frame) -> bool {
    a.stamp.protection == b.stamp.protection
        && a.stamp.grid_id == b.stamp.grid_id
        && a.stamp.config_rev == b.stamp.config_rev
        && a.data == b.data
}

/// Input meters with a moving bar (an input with signal, or a change of state) redraw at
/// most this often: a bar reads as moving at ten steps a second.
pub(super) const METER_PERIOD: Duration = Duration::from_millis(100);
/// Input meters whose only change is the readout of a silent input (its noise floor)
/// redraw at most this often: a number that changes faster cannot be read anyway.
pub(super) const METER_QUIET_PERIOD: Duration = Duration::from_millis(500);

/// The meter states of an input-levels frame, per channel; `None` for any other frame.
pub(super) fn meter_states(f: &ac2_proto::Frame) -> Option<Vec<ac2_scene::meter::MeterState>> {
    use ac2_proto::FrameData;
    let (peak, rms, clip) = match &f.data {
        FrameData::SessionLevels(l) => (&l.peak, &l.rms, &l.clip),
        FrameData::PreviewLevels(l) => (&l.peak, &l.rms, &l.clip),
        _ => return None,
    };
    Some(
        peak.iter()
            .zip(rms)
            .zip(clip)
            .map(|((p, r), c)| {
                ac2_scene::meter::MeterReading::new(*p, *r, *c != ac2_proto::frame::ClipFlags::NONE)
                    .state
            })
            .collect(),
    )
}

/// How soon a change from `prev` to `new` on an input-meter topic needs drawing; `None`
/// for any other topic, which is drawn at once.
pub(super) fn meter_period(prev: &ac2_proto::Frame, new: &ac2_proto::Frame) -> Option<Duration> {
    use ac2_scene::meter::MeterState;
    let (a, b) = (meter_states(prev)?, meter_states(new)?);
    let quiet = a == b
        && b.iter()
            .all(|s| matches!(s, MeterState::Silent | MeterState::NoData));
    Some(if quiet {
        METER_QUIET_PERIOD
    } else {
        METER_PERIOD
    })
}

/// When the link drains the client's frames next.
///
/// Frames of different topics arrive on their own schedules (meters at 30 Hz, a long
/// spectrum every 85 ms or so). Draining only on a timer of the display period holds each
/// frame for up to a period, so evenly spaced frames reach the screen 32–133 ms apart and
/// the display stutters. Draining on arrival keeps their spacing; half a display period
/// between drains still bounds how often the UI is fed, and gathers frames of several
/// topics landing together into one drain.
#[derive(Debug)]
pub(super) struct PollClock {
    /// When the next drain runs.
    pub(super) next: tokio::time::Instant,
    /// When the last drain ran.
    pub(super) last: tokio::time::Instant,
    /// A frame arriving brings the next drain forward. Off between an arrival and the drain
    /// it brings (the client signals for as long as frames wait), and while the UI is not
    /// drawing, which is polled at [`IDLE_POLL`] only.
    pub(super) watch: bool,
}

impl PollClock {
    pub(super) fn new(now: tokio::time::Instant) -> Self {
        Self {
            next: now,
            last: now,
            watch: false,
        }
    }

    /// A drain ran at `now`; `active`: frames are flowing; `behind`: the UI is not drawing.
    /// While frames flow, a drain every display period still catches what an arrival does
    /// not announce (a meter change held back, a STALE flip).
    pub(super) fn polled(
        &mut self,
        now: tokio::time::Instant,
        active: bool,
        behind: bool,
        period: Duration,
    ) {
        self.last = now;
        self.next = now + if active && !behind { period } else { IDLE_POLL };
        self.watch = !behind;
    }

    /// A frame arrived: drain now, or half a display period after the last drain.
    pub(super) fn arrived(&mut self, period: Duration) {
        self.watch = false;
        self.next = self.next.min(self.last + period / 2);
    }

    /// Drain at once (a new stream's first frame).
    pub(super) fn poll_now(&mut self, now: tokio::time::Instant) {
        self.next = now;
    }
}

/// The data poll: what the UI was last given, per topic.
#[derive(Default)]
pub(super) struct DataPoll {
    pub(super) seen: HashMap<Arc<str>, (u64, bool, Arc<ac2_proto::Frame>)>,
    pub(super) responding: bool,
    /// When a frame with a new `seq` last arrived.
    pub(super) last_new: Option<Instant>,
    /// When the UI was last woken for data.
    pub(super) last_wake: Option<Instant>,
    /// An input-meter change handed over without a wake, and how soon it needs drawing.
    pub(super) meter_due: Option<Duration>,
}

impl DataPoll {
    /// Drains the client's newest frames and hands them to the UI: waking it when the
    /// picture changes (new content, a topic coming or going, a STALE or liveness flip;
    /// input meters at their own rate), quietly when only `seq` and ages moved. `true`
    /// while frames are flowing.
    ///
    /// Polled: the client keeps only the newest frame per topic, so a poll at the display
    /// period loses nothing the screen could show.
    pub(super) async fn poll(&mut self, client: &Client, out: &Out) -> bool {
        let Ok(latest) = client.latest() else {
            return false;
        };
        let mut new_seq = false;
        let mut shows =
            latest.responding != self.responding || latest.frames.len() != self.seen.len();
        for (k, f) in &latest.frames {
            match self.seen.get(k) {
                Some((seq, stale, prev)) => {
                    shows |= *stale != f.stale;
                    if *seq == f.frame.stamp.seq {
                        continue;
                    }
                    new_seq = true;
                    if same_picture(prev, &f.frame) {
                        continue;
                    }
                    match meter_period(prev, &f.frame) {
                        Some(p) => self.meter_due = Some(self.meter_due.map_or(p, |d| d.min(p))),
                        None => shows = true,
                    }
                }
                None => {
                    new_seq = true;
                    shows = true;
                }
            }
        }
        let now = Instant::now();
        if new_seq {
            self.last_new = Some(now);
        }
        let meters = self.meter_due.is_some_and(|due| {
            self.last_wake
                .is_none_or(|t| now.saturating_duration_since(t) >= due)
        });
        let wake = shows || meters;
        if wake {
            self.last_wake = Some(now);
            self.meter_due = None;
        }
        if shows || new_seq {
            self.seen = latest
                .frames
                .iter()
                .map(|(k, f)| (k.clone(), (f.frame.stamp.seq, f.stale, f.frame.clone())))
                .collect();
            self.responding = latest.responding;
            let mut grids = BTreeMap::new();
            for id in latest.grid_ids() {
                if let Ok(g) = client.grid(id).await {
                    grids.insert(id, g);
                }
            }
            let snapshot = DataSnapshot {
                latest,
                grids,
                drained: Instant::now(),
            };
            out.data(Arc::new(snapshot), wake);
        } else if wake {
            out.wake();
        }
        self.last_new
            .is_some_and(|t| now.saturating_duration_since(t) < ACTIVE_HOLD)
    }
}
