//! Loopback timing monitor job (Q3): correlates the generator history with the loopback
//! input every hop, publishes `timing` frames and reports state changes (lock, jump, loss,
//! drift warning) to control, which commits them as `timing` events.
//!
//! Most of the time the generator is off. A window without stimulus is judged from its
//! newest W history samples alone, which is all the presence check looks at, so the full
//! reference slice (window plus search range, up to a second further back) is read only
//! while there is something to correlate. Between hops the job has nothing to do with the
//! audio, so it sleeps through hand-offs until its next window is complete.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use ac2_audio::{HistoryError, HistoryReader};
use ac2_core::timing::{LoopbackTiming, Outcome, TimingConfig, TimingEvent};
use ac2_proto::frame::{FrameData, ProtectionFlags, TimingMeta, TimingWindow};
use ac2_proto::model::{Drift, TimingStatus};
use ac2_proto::topic::Topic;
use ac2_proto::units::{Db, Dbfs, Rev, SampleIndex, Samples, Seconds, SessionEpoch, WallNs};

use super::{Analysis, Due, Emitter, Flush, JobCmd, Pace, StampArgs};
use crate::control::ControlMsg;
use crate::conv;
use crate::fanout::Block;

pub(crate) struct Timing {
    mon: LoopbackTiming,
    history: HistoryReader,
    idx: usize,
    window: usize,
    hop: usize,
    /// Loopback samples from capture index `ring_start`, contiguous so a window is a slice.
    ring: Vec<f32>,
    ring_start: u64,
    next_start: Option<u64>,
    end: Option<u64>,
    wall: u64,
    reference: Vec<f32>,
    /// Windows that needed the whole reference slice (stimulus present).
    full_reads: u64,
    status: TimingStatus,
    reported: Option<TimingStatus>,
    last_window: Option<TimingWindow>,
    last_lock_wall: u64,
    /// Newest window of the drift estimate published, and the wall time it arrived at.
    drift_end: Option<u64>,
    drift_wall: u64,
    to_control: Sender<ControlMsg>,
    epoch: SessionEpoch,
    /// Windows measured; the `timing` result changes with each.
    generation: u64,
    pace: Pace,
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
            ring: Vec::new(),
            ring_start: 0,
            next_start: None,
            end: None,
            wall: 0,
            reference: Vec::new(),
            full_reads: 0,
            status: initial,
            reported: Some(initial),
            last_window: None,
            last_lock_wall: initial.last_lock.map_or(0, |l| l.at.0),
            drift_end: None,
            drift_wall: 0,
            to_control,
            epoch,
            generation: 0,
            pace: Pace::new(Duration::ZERO),
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
            let off = (start - self.ring_start) as usize;
            let range = self.mon.search_range();
            // Presence first, from the newest W samples only.
            match self.read_history(range.newest_start(start), self.window) {
                Ok(()) => {}
                Err(HistoryError::NotYetWritten { .. }) => break,
                Err(e @ HistoryError::Overwritten { .. }) => {
                    tracing::debug!("timing window skipped: {e}");
                    self.advance(start);
                    continue;
                }
            }
            let capture = &self.ring[off..off + self.window];
            let quiet = self
                .mon
                .process_if_no_stimulus(start, capture, &self.reference);
            let result = match quiet {
                Ok(Some(r)) => Ok(r),
                Err(e) => Err(e),
                Ok(None) => {
                    self.full_reads += 1;
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
                    let capture = &self.ring[off..off + self.window];
                    self.mon
                        .process_window(start, capture, &self.reference, range)
                }
            };
            match result {
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
                    self.generation += 1;
                }
                Err(e) => tracing::error!("timing window: {e:?}"),
            }
            self.advance(start);
        }
    }

    fn advance(&mut self, start: u64) {
        let next = start + self.hop as u64;
        self.next_start = Some(next);
        let drop = usize::try_from(next.saturating_sub(self.ring_start))
            .unwrap_or(usize::MAX)
            .min(self.ring.len());
        self.ring.drain(..drop);
        self.ring_start += drop as u64;
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
        let drift = t.drift();
        if drift.map(|d| d.end_sample) != self.drift_end {
            self.drift_end = drift.map(|d| d.end_sample);
            self.drift_wall = self.wall;
        }
        let judged_after = self.mon.config().drift_min_span_s;
        self.status = TimingStatus {
            epoch: t.epoch(),
            state: conv::timing_state(t.state()),
            last_lock: t
                .last_lock()
                .map(|l| conv::last_lock(l, WallNs(self.last_lock_wall))),
            drift: drift.map(|d| Drift {
                ppm: d.ppm,
                span: Seconds(d.span_s),
                warning: d.warning,
                at: WallNs(self.drift_wall),
            }),
            internal_reference: t.internal_reference_allowed(),
        };
        // Control commits only changes an operator must see: state, lock offset / epoch,
        // the drift as shown (warning, judged or not, value at the resolution displayed) and
        // internal-reference availability; not every refreshed age or regression wobble.
        let key = |s: &TimingStatus| {
            (
                s.epoch,
                s.state,
                s.last_lock.map(|l| (l.epoch, l.offset)),
                s.drift.map(|d| {
                    let step = if d.warning && d.ppm.abs() < 10.0 {
                        0.1
                    } else {
                        1.0
                    };
                    (
                        d.warning,
                        d.span.0 >= judged_after,
                        (d.ppm / step).round() as i64,
                    )
                }),
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

    fn emit(&mut self, e: &Emitter) -> Flush {
        let Some(end) = self.end else {
            return Flush::Done;
        };
        if self.generation == 0 {
            // Nothing measured yet: the committed status in `state` is all there is.
            return Flush::Done;
        }
        let stamp = StampArgs {
            audio_sample: end.saturating_sub(1),
            config_rev: Rev(0),
            applied_at: 0,
            wall_ns: self.wall,
            grid_id: None,
            protection: ProtectionFlags::NONE,
        };
        let due = self
            .pace
            .due(e, Topic::Timing, self.generation, &stamp, Instant::now());
        if due == Due::Send
            && !e.send(
                stamp,
                FrameData::Timing(TimingMeta {
                    status: self.status,
                    window: self.last_window,
                }),
            )
        {
            self.pace.unsent();
        }
        Flush::from_due(due)
    }

    fn frames_needed(&self) -> Option<u64> {
        // The next window needs audio up to `next_start + W`; until then hand-offs only add
        // samples to the ring.
        let need = self.next_start? + self.window as u64;
        let have = self.ring_start + self.ring.len() as u64;
        need.checked_sub(have).filter(|&m| m > 0)
    }

    fn capture(&mut self) -> Option<(StampArgs, FrameData)> {
        None
    }
}

#[cfg(test)]
#[path = "timing_tests.rs"]
mod tests;
