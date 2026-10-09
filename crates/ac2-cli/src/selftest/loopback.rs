//! Output→input timing through a loopback cable: the daemon's Q3 monitor
//! ([`LoopbackTiming`]) run over the generator history and the loopback input.

use ac2_audio::{BlockHeader, HistoryError, HistoryReader};
use ac2_core::timing::{LoopbackTiming, Outcome, TimingConfig, TimingEvent};
use serde::Serialize;

/// What the loopback showed over the run.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LoopbackReport {
    /// Windows correlated.
    pub windows: u64,
    /// Windows that gave a confident offset.
    pub measured: u64,
    /// Offset at the end of the run (capture index − output index), samples; `None` if it
    /// never locked.
    pub offset_samples: Option<i64>,
    /// The same, ms.
    pub offset_ms: Option<f64>,
    /// Locks (the first, and each re-lock after a continuity break).
    pub locks: u64,
    /// Offset changes within one lock: `(from, to)` samples.
    pub jumps: Vec<(i64, i64)>,
    /// Times the lock was lost while the stimulus played.
    pub lost: u64,
    /// Output/input clock drift, ppm, once the regression spans enough time to judge.
    pub drift_ppm: Option<f64>,
    /// Seconds the drift regression spans.
    pub drift_span_s: Option<f64>,
    /// The drift exceeds the threshold: input and output are on different clocks.
    pub drift_warning: bool,
    /// Loopback level of the last window, dBFS RMS.
    pub loopback_dbfs: Option<f64>,
    /// Peak-to-sidelobe ratio of the last confident window, dB.
    pub psr_db: Option<f64>,
}

/// Correlates every hop of the loopback input against the generator history.
pub struct LoopbackCheck {
    mon: LoopbackTiming,
    history: HistoryReader,
    /// Block channel of the loopback input.
    channel: usize,
    window: usize,
    hop: usize,
    rate: f64,
    /// Loopback samples from capture index `ring_start`, contiguous.
    ring: Vec<f32>,
    ring_start: u64,
    next_start: Option<u64>,
    reference: Vec<f32>,
    report: LoopbackReport,
}

impl std::fmt::Debug for LoopbackCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoopbackCheck")
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

impl LoopbackCheck {
    /// Monitors block channel `channel` against `history` at `sample_rate`.
    pub fn new(sample_rate: u32, channel: usize, history: HistoryReader) -> Self {
        let cfg = TimingConfig::for_rate(f64::from(sample_rate));
        Self {
            window: cfg.window,
            hop: cfg.hop.max(1),
            rate: f64::from(sample_rate),
            mon: LoopbackTiming::new(cfg),
            history,
            channel,
            ring: Vec::new(),
            ring_start: 0,
            next_start: None,
            reference: Vec::new(),
            report: LoopbackReport::default(),
        }
    }

    /// One capture block (interleaved).
    pub fn add_block(&mut self, h: &BlockHeader, samples: &[f32]) {
        // A window must not span a break: the offset across it is unknown, so the monitor
        // starts a new epoch exactly as the daemon's does.
        if self.next_start.is_none() || h.flags.breaks_continuity() {
            self.mon.new_epoch();
            self.ring.clear();
            self.ring_start = h.start_sample;
            self.next_start = Some(h.start_sample);
        }
        let ch = usize::from(h.channels.max(1));
        self.ring
            .extend(samples.iter().skip(self.channel).step_by(ch).copied());
        self.run_windows();
    }

    /// Reads output indices `[start, start + len)`; indices before the stream began are
    /// silence.
    fn read_history(&mut self, start: i64, len: usize) -> Result<(), HistoryError> {
        self.reference.clear();
        self.reference.resize(len, 0.0);
        let skip = usize::try_from(-start.min(0)).unwrap_or(len).min(len);
        self.history
            .read(start.max(0) as u64, &mut self.reference[skip..])
    }

    fn run_windows(&mut self) {
        while let Some(start) = self.next_start {
            if start < self.ring_start
                || self.ring_start + (self.ring.len() as u64) < start + self.window as u64
            {
                return;
            }
            let range = self.mon.search_range();
            match self.read_history(
                range.reference_start(start),
                range.reference_len(self.window),
            ) {
                Ok(()) => {}
                Err(HistoryError::NotYetWritten { .. }) => return,
                Err(HistoryError::Overwritten { .. }) => {
                    self.advance(start);
                    continue;
                }
            }
            let off = (start - self.ring_start) as usize;
            let capture = &self.ring[off..off + self.window];
            if let Ok((m, events)) = self
                .mon
                .process_window(start, capture, &self.reference, range)
            {
                self.report.windows += 1;
                self.report.loopback_dbfs = Some(m.loopback_dbfs);
                if let Outcome::Offset(p) = m.outcome {
                    self.report.measured += 1;
                    self.report.psr_db = Some(p.psr_db);
                }
                for e in events.iter() {
                    match *e {
                        TimingEvent::Locked { .. } => self.report.locks += 1,
                        TimingEvent::Jump { from, to, .. } => self.report.jumps.push((from, to)),
                        TimingEvent::Lost { .. } => self.report.lost += 1,
                        TimingEvent::StimulusOff { .. } | TimingEvent::DriftWarning { .. } => {}
                    }
                }
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

    /// The totals.
    pub fn finish(self) -> LoopbackReport {
        let mut r = self.report;
        let t = self.mon.tracker();
        r.offset_samples = t.last_lock().map(|l| l.offset);
        r.offset_ms = r.offset_samples.map(|o| o as f64 / self.rate * 1e3);
        if let Some(d) = t.drift()
            && d.span_s >= self.mon.config().drift_min_span_s
        {
            r.drift_ppm = Some(d.ppm);
            r.drift_span_s = Some(d.span_s);
            r.drift_warning = d.warning;
        }
        r
    }
}
