//! Loopback timing monitor job (Q3): correlates the generator history with the loopback
//! input every hop, publishes `timing` frames and reports state changes (lock, jump, loss,
//! drift warning) to control, which commits them as `timing` events.

use std::collections::VecDeque;
use std::sync::mpsc::Sender;

use ac2_audio::{HistoryError, HistoryReader};
use ac2_core::timing::{LoopbackTiming, Outcome, TimingConfig, TimingEvent};
use ac2_proto::frame::{FrameData, ProtectionFlags, TimingMeta, TimingWindow};
use ac2_proto::model::{Drift, TimingStatus};
use ac2_proto::units::{Db, Dbfs, Rev, SampleIndex, Samples, Seconds, SessionEpoch, WallNs};

use super::{Analysis, Emitter, JobCmd, StampArgs};
use crate::control::ControlMsg;
use crate::conv;
use crate::fanout::Block;

pub(crate) struct Timing {
    mon: LoopbackTiming,
    history: HistoryReader,
    idx: usize,
    window: usize,
    hop: usize,
    /// Loopback samples from capture index `ring_start`.
    ring: VecDeque<f32>,
    ring_start: u64,
    next_start: Option<u64>,
    end: Option<u64>,
    wall: u64,
    capture: Vec<f32>,
    reference: Vec<f32>,
    status: TimingStatus,
    reported: Option<TimingStatus>,
    last_window: Option<TimingWindow>,
    last_lock_wall: u64,
    to_control: Sender<ControlMsg>,
    epoch: SessionEpoch,
    dirty: bool,
}

impl Timing {
    pub(crate) fn new(
        sample_rate: u32,
        idx: usize,
        history: HistoryReader,
        to_control: Sender<ControlMsg>,
        epoch: SessionEpoch,
        initial: TimingStatus,
    ) -> Self {
        let cfg = TimingConfig::for_rate(f64::from(sample_rate));
        Self {
            window: cfg.window,
            hop: cfg.hop.max(1),
            mon: LoopbackTiming::new(cfg),
            history,
            idx,
            ring: VecDeque::new(),
            ring_start: 0,
            next_start: None,
            end: None,
            wall: 0,
            capture: Vec::new(),
            reference: Vec::new(),
            status: initial,
            reported: Some(initial),
            last_window: None,
            last_lock_wall: initial.last_lock.map_or(0, |l| l.at.0),
            to_control,
            epoch,
            dirty: false,
        }
    }

    fn restart(&mut self, at: u64) {
        self.mon.new_epoch();
        self.ring.clear();
        self.ring_start = at;
        self.next_start = Some(at);
    }

    /// Reads output indices `[start, start + out.len())`; indices before the stream began
    /// are silence.
    fn read_history(&mut self, start: i64, len: usize) -> Result<(), HistoryError> {
        self.reference.clear();
        self.reference.resize(len, 0.0);
        let skip = usize::try_from(-start.min(0)).unwrap_or(len).min(len);
        let first = start.max(0) as u64;
        self.history.read(first, &mut self.reference[skip..])
    }

    fn run_windows(&mut self) {
        while let Some(start) = self.next_start {
            let need_end = start + self.window as u64;
            if self.ring_start + (self.ring.len() as u64) < need_end || start < self.ring_start {
                break;
            }
            let range = self.mon.search_range();
            let ref_len = range.reference_len(self.window);
            match self.read_history(range.reference_start(start), ref_len) {
                Ok(()) => {}
                Err(HistoryError::NotYetWritten { .. }) => break,
                Err(e @ HistoryError::Overwritten { .. }) => {
                    tracing::debug!("timing window skipped: {e}");
                    self.advance(start);
                    continue;
                }
            }
            let off = (start - self.ring_start) as usize;
            self.capture.clear();
            self.capture
                .extend(self.ring.iter().skip(off).take(self.window).copied());
            match self
                .mon
                .process_window(start, &self.capture, &self.reference, range)
            {
                Ok((m, events)) => {
                    for ev in events.iter() {
                        log_event(ev);
                    }
                    let (offset, psr) = match m.outcome {
                        Outcome::Offset(p) => (Some(Samples(p.offset)), Some(Db(p.psr_db))),
                        _ => (None, None),
                    };
                    self.last_window = Some(TimingWindow {
                        capture_start: SampleIndex(start),
                        offset,
                        psr,
                        loopback: Dbfs(m.loopback_dbfs),
                        stimulus: Dbfs(m.stimulus_dbfs),
                    });
                    self.update_status();
                    self.dirty = true;
                }
                Err(e) => tracing::error!("timing window: {e:?}"),
            }
            self.advance(start);
        }
    }

    fn advance(&mut self, start: u64) {
        let next = start + self.hop as u64;
        self.next_start = Some(next);
        while self.ring_start < next && !self.ring.is_empty() {
            self.ring.pop_front();
            self.ring_start += 1;
        }
    }

    fn update_status(&mut self) {
        let t = self.mon.tracker();
        if let Some(l) = t.last_lock()
            && self
                .status
                .last_lock
                .is_none_or(|p| p.at_sample.0 != l.at_capture_sample)
        {
            self.last_lock_wall = self.wall;
        }
        self.status = TimingStatus {
            epoch: t.epoch(),
            state: conv::timing_state(t.state()),
            last_lock: t
                .last_lock()
                .map(|l| conv::last_lock(l, WallNs(self.last_lock_wall))),
            drift: t.drift().map(|d| Drift {
                ppm: d.ppm,
                span: Seconds(d.span_s),
                warning: d.warning,
            }),
            internal_reference: t.internal_reference_allowed(),
        };
        // Control commits only changes an operator must see: state, lock offset / epoch,
        // drift warning and internal-reference availability; not every refreshed age.
        let key = |s: &TimingStatus| {
            (
                s.epoch,
                s.state,
                s.last_lock.map(|l| (l.epoch, l.offset)),
                s.drift.map(|d| d.warning),
                s.internal_reference,
            )
        };
        if self
            .reported
            .as_ref()
            .is_none_or(|r| key(r) != key(&self.status))
        {
            self.reported = Some(self.status);
            let _ = self.to_control.send(ControlMsg::Timing {
                epoch: self.epoch,
                status: self.status,
            });
        }
    }
}

fn log_event(ev: &TimingEvent) {
    match ev {
        TimingEvent::Jump {
            from,
            to,
            at_capture_sample,
            ..
        } => tracing::warn!(
            "output timing jump {from} → {to} samples (Δ {}) at capture sample {at_capture_sample}",
            to - from
        ),
        TimingEvent::DriftWarning { ppm, .. } => tracing::warn!(
            "input and output clocks drift by {ppm:.1} ppm; internal reference refused"
        ),
        TimingEvent::Locked { offset, .. } => tracing::info!("loopback locked at {offset} samples"),
        TimingEvent::Lost { .. } => tracing::warn!("loopback timing lost"),
        TimingEvent::StimulusOff { .. } => tracing::debug!("loopback: stimulus off"),
    }
}

impl Analysis for Timing {
    fn push(&mut self, b: &Block) {
        let contiguous = self.end == Some(b.start_sample);
        if self.end.is_none() || !contiguous || b.flags.breaks_continuity() {
            if self.end.is_some() {
                tracing::info!("timing: new offset epoch at sample {}", b.start_sample);
            }
            self.restart(b.start_sample);
        }
        let n = usize::from(b.channels).max(1);
        self.ring
            .extend(b.data.iter().skip(self.idx).step_by(n).copied());
        self.end = Some(b.end_sample());
        self.wall = b.wall_ns;
        self.run_windows();
    }

    fn command(&mut self, _c: JobCmd) {}

    fn emit(&mut self, e: &Emitter) {
        let Some(end) = self.end else {
            return;
        };
        if !self.dirty {
            return;
        }
        self.dirty = false;
        e.send(
            StampArgs {
                audio_sample: end.saturating_sub(1),
                config_rev: Rev(0),
                applied_at: 0,
                wall_ns: self.wall,
                grid_id: None,
                protection: ProtectionFlags::NONE,
            },
            FrameData::Timing(TimingMeta {
                status: self.status,
                window: self.last_window,
            }),
        );
    }
}
