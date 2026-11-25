//! A transfer's bins of each analysis stage (`bins/<meas>/<stage>`), for a transfer at the
//! `bins` resolution: each stage's own H1 and γ² where that stage alone serves the curve.
//!
//! Each stage frame carries the stamp of the `tf` formed in the same emit, so a client can
//! tell a stage frame of the shown curve from an older one. A stage's own newest block only
//! paces it: protection rejects blocks while audio flows, and a stamp of the stage's own
//! would stall then while the `tf`'s moves on. A stage goes out when its average changed,
//! and is refreshed like any topic while audio comes, only while subscribed.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use ac2_core::mic_curve::Correction;
use ac2_core::mtw::{DrawnBins, Mtw};
use ac2_proto::Frame;
use ac2_proto::frame::{BinLayout, BinStage, BinsFrame, BinsMeta, FrameData};
use ac2_proto::model::FreqRange;
use ac2_proto::topic::{MAX_BIN_STAGES, Topic};
use ac2_proto::units::{Hz, MeasId, Rev, Samples, Seconds};
use num_complex::Complex64;

use super::{Due, Emitter, Flush, JobMsg, Live, Pace, Probe, StampArgs};

/// What every stage frame of an emit repeats from that emit's `tf`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Shared {
    pub(crate) delay: Seconds,
    pub(crate) nudged: Seconds,
    pub(crate) mic_range: Option<FreqRange>,
    /// A stopped cross-rig job: its `tf` shows a kept snapshot, which no stage average
    /// matches, so every stage says it has none.
    pub(crate) stopped: bool,
}

/// The stage streams of one transfer job.
pub(crate) struct StageStreams {
    meas: MeasId,
    stages: Vec<Stage>,
    layout: BinLayout,
    h1: Vec<Complex64>,
    coh: Vec<f64>,
}

struct Stage {
    drawn: DrawnBins,
    topic: Topic,
    pace: Pace,
    /// The mic curve at each drawn bin's frequency, dB (subtracted from `mag`).
    corr_db: Option<Vec<f64>>,
    /// What the last frame was formed from; a change advances `generation`.
    formed_from: Option<Key>,
    generation: u64,
}

/// What a stage frame depends on besides its arrays' values: the stage's newest block, and
/// the static fields the freshness rule compares with the `tf`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Key {
    block_end: Option<u64>,
    delay: Seconds,
    nudged: Seconds,
    config_rev: Rev,
    stopped: bool,
}

/// Attempts at handing a final frame to a full channel, a millisecond apart: a stopping
/// job waits at most this long per frame, which bounds the join of a stop.
pub(crate) const FINAL_TRIES: u32 = 20;

fn corr_db(d: &DrawnBins, c: &Correction) -> Vec<f64> {
    d.range().map(|k| c.db(k as f64 * d.bin_hz)).collect()
}

impl StageStreams {
    pub(crate) fn new(meas: MeasId, mtw: &Mtw, corr: Option<&Correction>) -> Self {
        let l = mtw.layout();
        let drawn = l.drawn_bins();
        let stages = drawn
            .iter()
            .take(usize::from(MAX_BIN_STAGES))
            .map(|d| Stage {
                drawn: *d,
                topic: Topic::Bins {
                    meas,
                    stage: d.stage as u8,
                },
                pace: Pace::new(Duration::ZERO),
                corr_db: corr.map(|c| corr_db(d, c)),
                formed_from: None,
                generation: 0,
            })
            .collect::<Vec<_>>();
        let layout = BinLayout {
            stages: stages
                .iter()
                .map(|s| BinStage {
                    first_bin: s.drawn.first as u32,
                    bins: s.drawn.count as u32,
                    bin_hz: Hz(s.drawn.bin_hz),
                })
                .collect(),
            blends: l
                .crossovers
                .iter()
                .map(|c| FreqRange {
                    lo_hz: Hz(c.lo_hz),
                    hi_hz: Hz(c.hi_hz),
                })
                .collect(),
        };
        Self {
            meas,
            stages,
            layout,
            h1: Vec::new(),
            coh: Vec::new(),
        }
    }

    /// The layout a `tf` frame states.
    pub(crate) fn layout(&self) -> Box<BinLayout> {
        Box::new(self.layout.clone())
    }

    /// A new mic curve (or none): rebuilt at every drawn bin.
    pub(crate) fn set_correction(&mut self, c: Option<&Correction>) {
        for s in &mut self.stages {
            s.corr_db = c.map(|c| corr_db(&s.drawn, c));
            // The arrays change with the same averages.
            s.formed_from = None;
        }
    }

    /// Sends what is due of every subscribed stage, stamped `stamp` (the `tf`'s of this
    /// emit); unsubscribed stages are not formed.
    pub(crate) fn emit(
        &mut self,
        e: &Emitter,
        mtw: &Mtw,
        stamp: StampArgs,
        shared: &Shared,
        now: Instant,
    ) -> Flush {
        let stamp = StampArgs {
            grid_id: None,
            ..stamp
        };
        let mut flush = Flush::Done;
        for i in 0..self.stages.len() {
            let generation = self.generation(i, mtw, &stamp, shared);
            let s = &mut self.stages[i];
            let due = s.pace.due(e, s.topic, generation, &stamp, now);
            if due == Due::Send {
                let f = self.frame(i, mtw, shared);
                if !e.send(stamp, FrameData::Bins(f)) {
                    self.stages[i].pace.unsent();
                }
            }
            flush = flush.and(Flush::from_due(due));
        }
        flush
    }

    /// A frame of every stage, subscribed or not, stamped `stamp` (the final `tf`'s): a
    /// stage first zoomed into after a stop then shows the same averages as the curve.
    pub(crate) fn final_frames(
        &mut self,
        e: &Emitter,
        mtw: &Mtw,
        stamp: StampArgs,
        shared: &Shared,
        kept: &mut Vec<Frame>,
    ) {
        let stamp = StampArgs {
            grid_id: None,
            ..stamp
        };
        for i in 0..self.stages.len() {
            let f = self.frame(i, mtw, shared);
            send_final(e, stamp, FrameData::Bins(f), kept);
        }
    }

    /// Every stage's frame as it would go out now, unquantised: what a math over this
    /// transfer combines per bin.
    pub(crate) fn all_frames(&mut self, mtw: &Mtw, shared: &Shared) -> Vec<BinsFrame> {
        (0..self.stages.len())
            .map(|i| self.frame(i, mtw, shared))
            .collect()
    }

    /// The stage's pace generation: advances whenever what its frame is formed from does.
    pub(super) fn generation(
        &mut self,
        i: usize,
        mtw: &Mtw,
        stamp: &StampArgs,
        shared: &Shared,
    ) -> u64 {
        let s = &mut self.stages[i];
        let key = Key {
            block_end: mtw.stage_last_block_end(s.drawn.stage),
            delay: shared.delay,
            nudged: shared.nudged,
            config_rev: stamp.config_rev,
            stopped: shared.stopped,
        };
        if s.formed_from != Some(key) {
            s.formed_from = Some(key);
            s.generation += 1;
        }
        s.generation
    }

    pub(super) fn frame(&mut self, i: usize, mtw: &Mtw, shared: &Shared) -> BinsFrame {
        let s = &self.stages[i];
        let d = s.drawn;
        let blocks = if shared.stopped {
            self.h1.clear();
            self.h1.resize(d.count, Complex64::new(f64::NAN, f64::NAN));
            self.coh.clear();
            self.coh.resize(d.count, f64::NAN);
            0
        } else {
            mtw.stage_bins_into(d.stage, d.range(), &mut self.h1, &mut self.coh)
        };
        let mut mag: Vec<f32> = self
            .h1
            .iter()
            .map(|h| (20.0 * h.norm().log10()) as f32)
            .collect();
        if let Some(c) = &s.corr_db {
            for (m, db) in mag.iter_mut().zip(c) {
                *m -= *db as f32;
            }
        }
        let phase = self
            .h1
            .iter()
            .map(|h| h.arg().to_degrees() as f32)
            .collect();
        let coh = self.coh.iter().map(|c| *c as f32).collect();
        BinsFrame {
            meas: self.meas,
            meta: BinsMeta {
                stage: d.stage as u8,
                factor: d.factor as u32,
                sample_rate_hz: Hz(d.rate_hz),
                bin_hz: Hz(d.bin_hz),
                first_bin: d.first as u32,
                hop: Samples(d.hop as i64),
                delay: shared.delay,
                nudged: shared.nudged,
                mic_curve: s.corr_db.is_some(),
                mic_range: shared.mic_range,
                blocks,
                eff_avg: if blocks > 0 {
                    mtw.eff_avg(d.stage, 1) as f32
                } else {
                    f32::NAN
                },
            },
            mag,
            phase,
            coh,
        }
    }
}

/// A live operand's answer to a math over its stage bins: its `tf` and, from the same
/// averages and stamp, every stage's frame (`None` when it keeps no stage bins).
#[derive(Debug)]
pub(crate) struct OperandBins {
    pub(crate) tf: Live,
    pub(crate) stages: Option<Vec<BinsFrame>>,
}

impl Probe {
    /// As [`Probe::request`], answered with every stage's bins beside the `tf`.
    pub(crate) fn request_bins(&self) -> Option<Receiver<Option<OperandBins>>> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.tx.send(JobMsg::CaptureBins(tx)).ok()?;
        self.thread.unpark();
        Some(rx)
    }
}

/// Hands a final frame to the I/O thread and keeps it in `kept` (its `seq` unset), from
/// where the kept result goes out again when it is relabelled. A stop's frames may find the
/// I/O thread's queue full: tried [`FINAL_TRIES`] times, then dropped with a warning.
pub(crate) fn send_final(e: &Emitter, stamp: StampArgs, data: FrameData, kept: &mut Vec<Frame>) {
    kept.push(e.frame(stamp, 0, data.clone()));
    let topic = data.topic();
    for _ in 0..FINAL_TRIES {
        if e.send(stamp, data.clone()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    tracing::warn!("{topic}: final frame dropped, the I/O thread took none");
}
