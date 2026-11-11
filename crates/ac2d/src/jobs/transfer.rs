//! Transfer-function job: protection → MTW ladder → optional smoothing → `tf` frames, plus
//! the live IR view (only while someone subscribes to it) and input meters. The measurement
//! input's mic curve, when on, is subtracted from the displayed magnitude (never the phase,
//! the IR or the delay finder: decision 7c).

use ac2_core::grid::LogGrid;
use ac2_core::ir_view::{IrEngine, UniformTf, amp_db};
use ac2_core::mic_curve::Correction;
use ac2_core::mtw::{Ladder, Mtw, MtwConfig, MtwFrame, SampleGate, Validity};
use ac2_core::protection::{BlockDecision, BlockLevels, Guard, ProtectionConfig};
use ac2_core::smoothing::{Smoother, SmoothingMode, TfColumns};
use ac2_proto::frame::{
    FrameData, IrFrame, IrMeta, ProtectionFlags, TfFrame, TfMeta, ValidityMask,
};
use ac2_proto::grid::GridId;
use ac2_proto::model::{DelayState, TransferConfig};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{Hz, MeasId, Rev, Seconds};
use num_complex::Complex64;

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use ac2_proto::units::SessionEpoch;

use super::cross_rig::{ClockGate, CrossEvent, CrossRig};
use super::finder::{Finder, RelockSeen};
use super::{
    Analysis, Due, Emitter, Flush, JobCmd, JobCtx, LevelsMeter, Pace, SmoothingChange, StampArgs,
};
use crate::control::ControlMsg;
use crate::conv;
use crate::fanout::Block;

/// Zero-lag correlation above which reference and measurement are taken to be the same
/// signal. An acoustic path always delays the measurement, and broadband programme or noise
/// decorrelates within a few samples, so this only happens when one source is patched to
/// both inputs.
const ROUTING_CORRELATION: f64 = 0.99;
/// Measurement level above which a silent reference means swapped inputs, dBFS.
const ROUTING_SIGNAL_DBFS: f64 = -60.0;
/// A mis-patch is a standing fault, not a moment: CHECK ROUTING rises only after the finding
/// has held this long, and clears after it has been absent this long, so room noise
/// hovering at the level rule's threshold cannot flash it.
const ROUTING_RAISE_S: f64 = 1.0;
const ROUTING_CLEAR_S: f64 = 2.0;

/// What one interval of the routing check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Routing {
    /// Nothing to say (or nothing measured).
    Clean,
    /// The same signal on both inputs: one source patched to both.
    Identical,
    /// A silent reference with a live measurement input: the inputs may be swapped, or the
    /// reference is simply missing (which NO REFERENCE already reports).
    ReferenceSilent,
}

/// Zero-lag routing check over one frame interval.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct RoutingCheck {
    rr: f64,
    mm: f64,
    rm: f64,
    n: u64,
}

impl RoutingCheck {
    pub(crate) fn push(&mut self, r: &[f32], m: &[f32]) {
        for (a, b) in r.iter().zip(m) {
            let (a, b) = (f64::from(*a), f64::from(*b));
            self.rr += a * a;
            self.mm += b * b;
            self.rm += a * b;
        }
        self.n += r.len().min(m.len()) as u64;
    }

    /// What the interval looks like, given the reference floor, and its length in samples;
    /// then a new interval.
    pub(crate) fn take(&mut self, reference_floor_dbfs: f64) -> (Routing, u64) {
        let c = *self;
        *self = Self::default();
        if c.n == 0 {
            return (Routing::Clean, 0);
        }
        let n = c.n as f64;
        let ref_db = ac2_core::spectrum::rms_dbfs((c.rr / n).sqrt());
        let meas_db = ac2_core::spectrum::rms_dbfs((c.mm / n).sqrt());
        if ref_db < reference_floor_dbfs && meas_db > ROUTING_SIGNAL_DBFS {
            return (Routing::ReferenceSilent, c.n);
        }
        if ref_db >= reference_floor_dbfs && c.mm > 0.0 {
            let corr = c.rm / (c.rr * c.mm).sqrt();
            if corr.abs() >= ROUTING_CORRELATION {
                return (Routing::Identical, c.n);
            }
        }
        (Routing::Clean, c.n)
    }
}

/// CHECK ROUTING with time hysteresis (see [`ROUTING_RAISE_S`]).
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct RoutingLatch {
    up: bool,
    present_s: f64,
    absent_s: f64,
}

impl RoutingLatch {
    /// One interval of `dt_s` seconds in which the finding was (or was not) present.
    pub(crate) fn step(&mut self, present: bool, dt_s: f64) -> bool {
        if present {
            self.present_s += dt_s;
            self.absent_s = 0.0;
            if self.present_s >= ROUTING_RAISE_S {
                self.up = true;
            }
        } else {
            self.absent_s += dt_s;
            self.present_s = 0.0;
            if self.absent_s >= ROUTING_CLEAR_S {
                self.up = false;
            }
        }
        self.up
    }
}

pub(crate) struct Transfer {
    meas: MeasId,
    cfg: TransferConfig,
    fs: f64,
    ref_idx: usize,
    meas_idx: usize,
    mtw: Mtw,
    guard: Guard,
    smoother: Option<(Smoother, SmoothingMode)>,
    grid_id: GridId,
    /// Where the grid is finer than the stage serving it; fixed by the layout and grid.
    unresolved: ac2_proto::model::Unresolved,
    delay_s: f64,
    /// Applied delay in samples, fractions included.
    delay_samples: f64,
    /// The part of the applied delay the operator's nudges added to the arrival, samples.
    nudged_samples: f64,
    config_rev: Rev,
    applied_at: u64,
    apply_pending: bool,
    levels: LevelsMeter,
    routing: RoutingCheck,
    check_routing: RoutingLatch,
    rbuf: Vec<f32>,
    mbuf: Vec<f32>,
    end: Option<u64>,
    wall: u64,
    discontinuity_until: u64,
    weak_until: u64,
    finder: Finder,
    epoch: SessionEpoch,
    to_control: Sender<ControlMsg>,
    grid: LogGrid,
    /// Mic-curve correction per column.
    corr: Option<ColumnCorrection>,
    /// Advances whenever the result may have changed: a block averaged, a restart, a
    /// command.
    generation: u64,
    tf_pace: Pace,
    ir_pace: Pace,
    /// The newest MTW frame; its buffers are reused from frame to frame.
    frame: MtwFrame,
    /// Scratch for the smoother's corrected H1 and the IR view's uniform-bin H1.
    hbuf: Vec<Complex64>,
    ir: IrEngine,
    /// Protection flags of the newest emit, for a capture's stamp.
    last_prot: ProtectionFlags,
    /// Its channels are on two rigs: it averages only while their clocks agree.
    cross: Option<CrossRig>,
}

/// The mic-curve correction at each column, in dB (subtracted from `mag`) and as the linear
/// gain that does the same to H1 before smoothing; both fixed until the curve changes.
struct ColumnCorrection {
    db: Vec<f64>,
    gain: Vec<f64>,
}

fn column_correction(grid: &LogGrid, c: &Correction) -> ColumnCorrection {
    let db: Vec<f64> = grid.frequencies().into_iter().map(|f| c.db(f)).collect();
    let gain = db.iter().map(|d| 10f64.powf(-d / 20.0)).collect();
    ColumnCorrection { db, gain }
}

/// The kernel for `smoothing` on `grid`.
fn smoother(
    grid: LogGrid,
    smoothing: Option<ac2_proto::model::Smoothing>,
) -> Option<(Smoother, SmoothingMode)> {
    smoothing.map(|s| {
        let (f, m) = conv::smoothing(s);
        (Smoother::new(grid, f), m)
    })
}

/// The delay tracking compares its whole-sample estimates with: the arrival (the applied
/// delay less the operator's nudges, which are a deliberate offset from it and stay when the
/// arrival moves) to the nearest sample (tracking moves in whole samples; a fraction the
/// operator set stays until the arrival moves by a sample or more).
fn held(delay_samples: f64, nudged_samples: f64) -> i64 {
    (delay_samples - nudged_samples).round() as i64
}

/// How long the deepest MTW stage takes to fill from a restart, s: the span within which
/// late remote audio keeps the measurement from ever settling.
fn settle_s(mtw: &Mtw) -> f64 {
    mtw.layout()
        .stages
        .iter()
        .zip(mtw.stage_averaging())
        .map(|(s, a)| s.first_block_s() + s.hop_s() * a.full_blocks())
        .fold(0.0, f64::max)
}

/// Why a transfer job cannot start.
pub(crate) type StartError = String;

impl Transfer {
    pub(crate) fn new(
        ctx: JobCtx<TransferConfig>,
        sample_rate: u32,
        (ref_idx, meas_idx): (usize, usize),
        delay: &DelayState,
        to_control: Sender<ControlMsg>,
        correction: Option<&Correction>,
        cross_rig: bool,
    ) -> Result<Self, StartError> {
        let JobCtx {
            meas,
            cfg,
            config_rev,
            epoch,
        } = ctx;
        let (delay_samples, delay_s, nudged_samples) =
            (delay.applied_samples, delay.applied.0, delay.nudged_samples);
        let fs = f64::from(sample_rate);
        let grid = LogGrid {
            ppo: cfg.grid().ppo,
            k_min: cfg.grid().k_min,
            k_max: cfg.grid().k_max,
        };
        let averaging = conv::tf_averaging(cfg.averaging).ok_or("invalid averaging")?;
        let mut mtw = Mtw::new(MtwConfig {
            sample_rate_hz: fs,
            ladder: Ladder::Standard,
            depth: conv::depth(cfg.depth).ok_or("invalid depth policy")?,
            averaging,
            grid,
            delay_samples,
        })
        .map_err(|e| e.to_string())?;
        let smoother = smoother(grid, cfg.smoothing);
        let grid_id = ac2_proto::grid::GridDef::Log {
            ppo: grid.ppo,
            k_min: grid.k_min,
            k_max: grid.k_max,
        }
        .id();
        let mut finder = Finder::new(fs);
        finder.track(delay.tracking, held(delay_samples, nudged_samples));
        finder.set_paused(delay.awaiting_pick);
        let cross = cross_rig.then(|| {
            finder.clock_hold(true, false);
            CrossRig::new(fs, settle_s(&mtw))
        });
        let frame = mtw.frame();
        let unresolved =
            crate::sweep::unresolved(cfg.resolution, &mtw.layout().unresolved_ranges(&grid));
        Ok(Self {
            unresolved,
            corr: correction.map(|c| column_correction(&grid, c)),
            generation: 0,
            tf_pace: Pace::new(Duration::ZERO),
            ir_pace: Pace::new(Duration::ZERO),
            frame,
            hbuf: Vec::new(),
            ir: IrEngine::default(),
            last_prot: ProtectionFlags::NONE,
            grid,
            finder,
            epoch,
            to_control,
            levels: LevelsMeter::new(
                vec![ref_idx, meas_idx],
                vec![cfg.reference_input, cfg.measurement_input],
                sample_rate,
            ),
            meas,
            cfg,
            fs,
            ref_idx,
            meas_idx,
            mtw,
            guard: Guard::new(ProtectionConfig::default()),
            smoother,
            grid_id,
            delay_s,
            delay_samples,
            nudged_samples,
            config_rev,
            applied_at: 0,
            apply_pending: true,
            routing: RoutingCheck::default(),
            check_routing: RoutingLatch::default(),
            rbuf: Vec::new(),
            mbuf: Vec::new(),
            end: None,
            wall: 0,
            discontinuity_until: 0,
            weak_until: 0,
            cross,
        })
    }

    fn tell(&self, event: CrossEvent) {
        let _ = self.to_control.send(ControlMsg::CrossRig {
            meas: self.meas,
            epoch: self.epoch,
            event,
        });
    }

    /// The arrival: the applied delay less the operator's nudges, samples.
    fn arrival(&self) -> f64 {
        self.delay_samples - self.nudged_samples
    }

    /// The clocks between the rigs judged anew.
    fn set_gate(&mut self, gate: ClockGate) {
        let (end, arrival) = (self.end.unwrap_or(0), self.arrival());
        let Some(x) = &mut self.cross else {
            return;
        };
        match x.set_gate(gate, end, arrival) {
            Some(true) => {
                self.finder.clock_hold(true, false);
                let held = x.held().is_some();
                self.tell(CrossEvent::Stopped { held });
            }
            Some(false) => {
                // Averages taken across the stop would mix two alignments; the framing
                // restarts too, as the blocks of the stop never reached the ladder.
                self.mtw.restart();
                let relock = x.relock();
                let tracking = self.finder.tracking();
                self.finder.clock_hold(false, relock && tracking);
                if relock && !tracking {
                    self.tell(CrossEvent::FindAgain);
                }
            }
            None => {}
        }
    }

    /// The remote input slipped: the arrival moves by exactly the slip, without waiting for
    /// tracking to find it.
    fn slipped(&mut self, by: i64) {
        self.delay_samples += by as f64;
        self.delay_s = self.delay_samples / self.fs;
        self.mtw.set_delay(self.delay_samples);
        // The history straddles the slip: nothing before it lines up with what follows.
        self.finder.restart();
        self.finder
            .set_held(held(self.delay_samples, self.nudged_samples));
        if let Some(x) = &mut self.cross {
            x.slipped(by);
        }
        self.tell(CrossEvent::Slipped { by });
    }

    /// What a block means for a cross-rig job beyond its averages: late remote audio, the
    /// kept results and the delay's drift.
    fn cross_block(&mut self, b: &Block, late: bool) {
        let end = b.end_sample();
        let arrival_s = self.arrival() / self.fs;
        let tracking = self.finder.tracking();
        let filled = self.mtw.fill() > 0.0;
        let Some(x) = &mut self.cross else {
            return;
        };
        let mut events = Vec::new();
        if late && let Some(on) = x.late.seen(b.start_sample) {
            events.push(CrossEvent::Late(on));
        }
        if let Some(on) = x.late.tick(end) {
            events.push(CrossEvent::Late(on));
        }
        let t = end as f64 / self.fs;
        if x.running() && tracking && x.drift.due(t) {
            if let Some(c) = x.drift.add(t, arrival_s) {
                events.push(CrossEvent::Drift(c));
            }
        } else if !tracking {
            x.drift.clear();
        }
        let due = x.snapshot_due(end) && filled;
        for e in events {
            self.tell(e);
        }
        if due {
            let (shown, raw) = (self.tf_frame(true), self.tf_frame(false));
            if let Some(x) = &mut self.cross {
                x.keep(end, shown, raw);
            }
        }
    }

    /// A frame of a stopped cross-rig job: the held result, else no curve at all.
    fn stopped_frame(&self, raw: bool) -> Option<TfFrame> {
        let x = self.cross.as_ref().filter(|x| !x.running())?;
        Some(match x.held() {
            Some(h) if raw => h.raw.clone(),
            Some(h) => h.shown.clone(),
            None => self.blank_frame(),
        })
    }

    /// A frame without a curve: every column settling.
    fn blank_frame(&self) -> TfFrame {
        let n = self.grid.frequencies().len();
        TfFrame {
            range: None,
            meas: self.meas,
            meta: TfMeta {
                delay: Seconds(self.delay_s),
                nudged: Seconds(self.nudged_samples / self.fs),
                smoothing: self.cfg.smoothing,
                mic_curve: self.corr.is_some(),
                math: None,
                unresolved: Some(self.unresolved.clone()),
                filled: 0.0,
            },
            mag: vec![f32::NAN; n],
            phase: vec![f32::NAN; n],
            coh: vec![f32::NAN; n],
            validity: vec![ValidityMask::SETTLING; n],
            align: None,
        }
    }

    fn set_correction(&mut self, c: Option<&Correction>) {
        self.corr = c.map(|c| column_correction(&self.grid, c));
    }

    fn mark_discontinuity(&mut self, at: u64) {
        // The flag stays up for a second so a client sees the restart.
        self.discontinuity_until = at + self.fs as u64;
    }

    fn ir_frame(&mut self) -> Option<IrFrame> {
        if !self.mtw.stage_h1_into(0, &mut self.hbuf) {
            return None;
        }
        let tf = UniformTf {
            h: &self.hbuf,
            sample_rate: self.fs,
            inserted_delay_s: self.delay_s,
        };
        let linear: Vec<f32> = self
            .ir
            .impulse(tf)
            .ok()?
            .iter()
            .map(|v| *v as f32)
            .collect();
        let n = linear.len();
        let etc: Vec<f32> = self
            .ir
            .envelope()
            .iter()
            .map(|v| amp_db(*v) as f32)
            .collect();
        Some(IrFrame {
            meas: self.meas,
            meta: IrMeta {
                sample_rate: Hz(self.fs),
                // Sample n/2 is time zero, the inserted delay.
                t0: Seconds(-((n / 2) as f64) / self.fs),
                dt: Seconds(1.0 / self.fs),
                inserted_delay: Seconds(self.delay_s),
            },
            linear,
            etc: Some(etc),
        })
    }

    /// The current transfer function from the newest MTW frame: the measured curve with the
    /// mic curve taken off, display-smoothed when `smooth` and smoothing is on.
    fn tf_frame(&mut self, smooth: bool) -> TfFrame {
        self.mtw.frame_into(&mut self.frame);
        let f = &self.frame;
        let validity: Vec<ValidityMask> = f
            .columns
            .iter()
            .map(|c| match c.validity {
                Validity::Valid => ValidityMask::NONE,
                Validity::Thinned => ValidityMask::THINNED,
                Validity::OutOfBand => ValidityMask::OUT_OF_BAND,
                Validity::Settling => ValidityMask::SETTLING,
                Validity::NoReference => ValidityMask::NO_REFERENCE,
                Validity::NoMeasurement => ValidityMask::NO_MEASUREMENT,
            })
            .collect();
        let smoother = self.smoother.as_ref().filter(|_| smooth);
        let (mag, phase) = match smoother {
            None => {
                let mut mag: Vec<f32> = f.magnitude_db.iter().map(|v| *v as f32).collect();
                if let Some(c) = &self.corr {
                    for (m, d) in mag.iter_mut().zip(&c.db) {
                        *m -= *d as f32;
                    }
                }
                (mag, f.phase_deg.iter().map(|v| *v as f32).collect())
            }
            // Display smoothing averages the corrected curve, so a capture re-smoothed at
            // this setting reads the same as this frame.
            Some((sm, mode)) => {
                let valid: Vec<bool> = validity.iter().map(|v| *v == ValidityMask::NONE).collect();
                let h: &[Complex64] = match &self.corr {
                    None => &f.h1,
                    Some(c) => {
                        self.hbuf.clear();
                        self.hbuf
                            .extend(f.h1.iter().zip(&c.gain).map(|(h, g)| h * g));
                        &self.hbuf
                    }
                };
                let s = sm.smooth(
                    TfColumns {
                        h,
                        coherence: &f.coherence,
                        valid: &valid,
                    },
                    *mode,
                );
                s.h.iter()
                    .zip(&s.valid)
                    .map(|(h, ok)| {
                        if *ok {
                            (
                                (20.0 * h.norm().log10()) as f32,
                                h.arg().to_degrees() as f32,
                            )
                        } else {
                            (f32::NAN, f32::NAN)
                        }
                    })
                    .unzip()
            }
        };
        TfFrame {
            range: None,
            meas: self.meas,
            meta: TfMeta {
                delay: Seconds(self.delay_s),
                nudged: Seconds(self.nudged_samples / self.fs),
                smoothing: self.cfg.smoothing,
                mic_curve: self.corr.is_some(),
                math: None,
                unresolved: Some(self.unresolved.clone()),
                filled: self.mtw.fill() as f32,
            },
            mag,
            phase,
            coh: f.coherence.iter().map(|v| *v as f32).collect(),
            validity,
            align: None,
        }
    }

    fn stamp(&self, end: u64, protection: ProtectionFlags) -> StampArgs {
        StampArgs {
            audio_sample: end.saturating_sub(1),
            config_rev: self.config_rev,
            applied_at: self.applied_at,
            wall_ns: self.wall,
            grid_id: Some(self.grid_id),
            protection,
        }
    }
}

impl Analysis for Transfer {
    fn result_generation(&self) -> Option<u64> {
        Some(self.generation)
    }

    fn push(&mut self, b: &Block) {
        let contiguous = self.end == Some(b.start_sample);
        let stream_break = !contiguous || b.restarts_averages(&[]);
        let cols = [self.ref_idx, self.meas_idx];
        let remote_break = cols.iter().any(|&c| b.marks.breaks(c));
        // A slip moves the remote side's map: on the measurement side the arrival moves
        // with it, on the reference side against it.
        let slip = b.marks.get(self.meas_idx).map_or(0, |m| m.slip)
            - b.marks.get(self.ref_idx).map_or(0, |m| m.slip);
        let late = slip == 0 && cols.iter().any(|&c| !b.marks.gaps(c).is_empty());
        if self.end.is_some() && (stream_break || remote_break) {
            if contiguous {
                // An xrun, an overflow or missing remote samples can leave the counter
                // contiguous while audio was lost; the block grid must restart all the same.
                self.mtw.restart();
            }
            self.mark_discontinuity(b.start_sample);
            // Missing remote samples leave the map where it was, and so the delay: the
            // finder keeps its history and its held delay.
            if stream_break {
                self.finder.restart();
            }
        }
        if self.end.is_some() && slip != 0 {
            self.slipped(slip);
        }
        if self.apply_pending {
            self.applied_at = b.start_sample;
            self.apply_pending = false;
        }
        b.channel_into(self.ref_idx, &mut self.rbuf);
        b.channel_into(self.meas_idx, &mut self.mbuf);
        if self.cross.as_ref().is_some_and(|x| !x.running()) {
            // Stopped: no average, no tracking; the finder's history and the meters go on.
            self.finder.push(b.start_sample, &self.rbuf, &self.mbuf);
            self.levels.push(b);
            self.end = Some(b.end_sample());
            self.wall = b.wall_ns;
            self.cross_block(b, late);
            return;
        }
        let lv = BlockLevels::from_samples(&self.rbuf, &self.mbuf, self.fs);
        let gate = match self.guard.process(&lv) {
            BlockDecision::Accept => SampleGate::Accept,
            BlockDecision::HoldWeakReference => {
                self.weak_until = b.end_sample() + (self.fs / 2.0) as u64;
                SampleGate::Reject
            }
            BlockDecision::RejectClip | BlockDecision::PauseNoReference => SampleGate::Reject,
        };
        match self.mtw.push(b.start_sample, &self.rbuf, &self.mbuf, gate) {
            Ok(o) if o.blocks_accumulated > 0 || o.restarted => self.generation += 1,
            Ok(_) => {}
            Err(e) => tracing::error!("transfer {}: {e}", self.meas),
        }
        let pushed = self.finder.push(b.start_sample, &self.rbuf, &self.mbuf);
        if let Some(d) = pushed.moved {
            tracing::info!(
                "measurement {}: tracking moves the delay to {d:.3} samples",
                self.meas.0
            );
            let _ = self.to_control.send(ControlMsg::DelayTracked {
                meas: self.meas,
                epoch: self.epoch,
                samples: d,
            });
        }
        match pushed.relock {
            Some(RelockSeen::Agreed) => {
                let was = self.cross.as_ref().map_or(0.0, CrossRig::was);
                let now = pushed.moved.unwrap_or_else(|| self.arrival());
                self.tell(CrossEvent::Relocked { was, now });
            }
            Some(RelockSeen::Ambiguous(r)) => self.tell(CrossEvent::RelockAmbiguous(r)),
            None => {}
        }
        self.routing.push(&self.rbuf, &self.mbuf);
        self.levels.push(b);
        self.end = Some(b.end_sample());
        self.wall = b.wall_ns;
        if self.cross.is_some() {
            self.cross_block(b, late);
        }
    }

    fn command(&mut self, c: JobCmd) {
        self.generation += 1;
        match c {
            JobCmd::SetDelay {
                samples,
                seconds,
                nudged_samples,
                rev,
                resume,
            } => {
                self.delay_samples = samples;
                self.nudged_samples = nudged_samples;
                self.delay_s = seconds;
                self.config_rev = rev;
                self.apply_pending = true;
                let change = self.mtw.set_delay(samples);
                tracing::debug!(
                    "measurement {}: delay {samples} samples ({})",
                    self.meas.0,
                    if change.restarted {
                        "restarted".to_owned()
                    } else {
                        format!("stages kept {:#b}", change.kept)
                    }
                );
                self.finder.set_held(held(samples, nudged_samples));
                if resume {
                    self.finder.set_paused(false);
                    if let Some(x) = &mut self.cross {
                        x.drift.clear();
                    }
                }
            }
            JobCmd::Find {
                token,
                band,
                observation,
            } => {
                let result = self.finder.find(
                    band,
                    observation,
                    held(self.delay_samples, self.nudged_samples),
                );
                let _ = self.to_control.send(ControlMsg::DelayFound {
                    token,
                    result: Box::new(result),
                });
            }
            JobCmd::Track { enabled } => {
                self.finder
                    .track(enabled, held(self.delay_samples, self.nudged_samples));
            }
            JobCmd::Reset => self.mtw.reset_averages(),
            JobCmd::Cal(cal) => self.set_correction(cal.correction.as_deref()),
            JobCmd::Clocks(gate) => self.set_gate(gate),
            JobCmd::Smoothing {
                change: SmoothingChange::Transfer(smoothing),
                rev,
            } => {
                self.cfg.smoothing = smoothing;
                self.smoother = smoother(self.grid, smoothing);
                self.config_rev = rev;
                self.apply_pending = true;
            }
            JobCmd::Smoothing { .. } | JobCmd::Spl { .. } => {}
        }
    }

    fn emit(&mut self, e: &Emitter) -> Flush {
        let Some(end) = self.end else {
            return Flush::Done;
        };
        let (routing, n) = self.routing.take(self.guard.config().reference_floor_dbfs);
        let banners = self.guard.banners();
        // With NO REFERENCE up, a silent reference is already reported; only identical inputs
        // add something then. Without it, a silent reference beside a live measurement input
        // still points at swapped inputs.
        let finding = match routing {
            Routing::Identical => true,
            Routing::ReferenceSilent => !banners.no_reference,
            Routing::Clean => false,
        };
        let check_routing = self.check_routing.step(finding, n as f64 / self.fs);
        let mut prot = ProtectionFlags::NONE;
        if banners.no_reference {
            prot = prot.with(ProtectionFlags::NO_REFERENCE);
        }
        if banners.no_signal {
            prot = prot.with(ProtectionFlags::NO_SIGNAL);
        }
        if banners.clip || self.levels.any_clip_held() {
            prot = prot.with(ProtectionFlags::CLIP);
        }
        if self.weak_until > end {
            prot = prot.with(ProtectionFlags::WEAK_REFERENCE);
        }
        if self.discontinuity_until > end {
            prot = prot.with(ProtectionFlags::DISCONTINUITY);
        }
        if check_routing {
            prot = prot.with(ProtectionFlags::CHECK_ROUTING);
        }
        self.last_prot = prot;
        let stamp = self.stamp(end, prot);
        let now = Instant::now();

        let tf_topic = Topic::Data {
            meas: self.meas,
            stream: Stream::Tf,
        };
        let tf_due = self.tf_pace.due(e, tf_topic, self.generation, &stamp, now);
        if tf_due == Due::Send {
            let f = match self.stopped_frame(false) {
                Some(f) => f,
                None => self.tf_frame(true),
            };
            if !e.send(stamp, FrameData::Tf(f)) {
                self.tf_pace.unsent();
            }
        }

        let ir_topic = Topic::Data {
            meas: self.meas,
            stream: Stream::Ir,
        };
        let ir_due = self.ir_pace.due(e, ir_topic, self.generation, &stamp, now);
        let stopped = self.cross.as_ref().is_some_and(|x| !x.running());
        if ir_due == Due::Send
            && !stopped
            && let Some(ir) = self.ir_frame()
        {
            let sent = e.send(
                StampArgs {
                    grid_id: None,
                    ..stamp
                },
                FrameData::Ir(ir),
            );
            if !sent {
                self.ir_pace.unsent();
            }
        }
        self.levels.send(e, self.meas, stamp);
        Flush::from_due(tf_due).and(Flush::from_due(ir_due))
    }

    fn capture(&mut self) -> Option<(StampArgs, FrameData)> {
        let end = self.end?;
        let stamp = self.stamp(end, self.last_prot);
        if let Some(x) = self.cross.as_ref().filter(|x| !x.running()) {
            // Stopped: only a result from before the clocks parted is one.
            return x.held().map(|h| (stamp, FrameData::Tf(h.raw.clone())));
        }
        Some((stamp, FrameData::Tf(self.tf_frame(false))))
    }

    fn capture_ir(&mut self) -> Option<IrFrame> {
        if self.cross.as_ref().is_some_and(|x| !x.running()) {
            return None;
        }
        self.ir_frame()
    }
}

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "transfer_cross_tests.rs"]
mod cross_tests;
