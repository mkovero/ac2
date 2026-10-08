//! Measurement jobs on the open session: start and stop, timing, delay setting, finding and tracking.

use std::sync::Arc;
use std::time::Instant;

use ac2_core::delay::FinderResult;
use ac2_proto::event::{Change, Patch};
use ac2_proto::grid::GridDef;
use ac2_proto::model::{DelayOutcome, DelayState, FinderBand, MeasKind, Measurement};
use ac2_proto::units::{MeasId, Rev, Seconds, SessionEpoch, WallNs};
use ac2_proto::{ErrorCode, ProtoError, ReplyBody};

use crate::calstore::{self, InputCal};
use crate::conv;
use crate::jobs::{self, Analysis, JobCmd, JobEnv, block_index};
use crate::session::Runtime;
use crate::util::{perr, wall_ns};

use super::{Control, DelaySource, MAX_DELAY_S, check_find, delay_samples, static_grid};

/// A tracked arrival closer than this to the applied one, samples, leaves the delay alone:
/// half the tracker's fine agreement, below which two estimates are the same arrival, and a
/// phase step of under 2° at 10 kHz (48 kHz) that a resettle would not be worth.
const TRACK_TOLERANCE_SAMPLES: f64 = 0.05;

impl Control {
    pub(super) fn job_env(&self, rt: &Runtime) -> JobEnv {
        self.env_at(rt.epoch)
    }

    pub(super) fn env_at(&self, epoch: SessionEpoch) -> JobEnv {
        JobEnv {
            ctx: self.s.ctx.clone(),
            endpoint: self.s.endpoint.clone(),
            incarnation: self.s.incarnation,
            epoch,
            seqs: Arc::clone(&self.seqs),
            interest: Arc::clone(&self.s.interest),
            fps: self.s.fps,
        }
    }

    /// The calibration and mic curve a job on `input` of the open session uses (Q7 §3).
    pub(super) fn input_cal(&self, rt: &Runtime, input: u16) -> InputCal {
        calstore::resolve(self.store.state(), &self.cal, &rt.open.input_device, input)
    }

    /// Hands every running job its input's current calibration and mic curve.
    pub(super) fn refresh_cal(&self) {
        let Some(rt) = self.session.as_ref() else {
            return;
        };
        for m in &self.store.state().measurements {
            let Some(job) = self.jobs.get(&m.id) else {
                continue;
            };
            let input = match &m.config.kind {
                MeasKind::Spectrum { config } => config.input,
                MeasKind::Rta { config } => config.input,
                MeasKind::Spl { config } => config.input,
                MeasKind::Transfer { config } => config.measurement_input,
                // Its operands' mic curves are in their results already.
                MeasKind::Math { .. } => continue,
                // Runs only when fired; each run reads the input's mic then.
                MeasKind::Sweep { .. } => continue,
            };
            job.send(JobCmd::Cal(Box::new(self.input_cal(rt, input))));
        }
    }

    /// Starts the job of `m` on the open session; returns its grid when it has one.
    pub(super) fn start_job(&mut self, m: &Measurement) -> Result<Option<GridDef>, ProtoError> {
        if self.session.is_none() || !m.config.kind.is_job() {
            return Ok(None);
        }
        let mut leq = matches!(m.config.kind, MeasKind::Spl { .. }).then(|| self.leq_setup(m.id));
        // A spectrum publishes display columns but captures every bin: `trace.capture`
        // looks the capture's grid up by id.
        if let (MeasKind::Spectrum { config }, Some(rt)) = (&m.config.kind, &self.session) {
            let g = jobs::spectrum::capture_grid(config, rt.sample_rate);
            self.register_grid(g);
        }
        // A math channel combines on one grid and, a spectrum's, publishes on another.
        let math = match (&m.config.kind, &self.session) {
            (MeasKind::Math { config }, Some(rt)) => {
                let (grids, stored) = self.math_setup(m.id, config, rt.sample_rate)?;
                self.register_grid(grids.grid.clone());
                if let Some((g, _)) = &grids.display {
                    self.register_grid(g.clone());
                }
                Some((grids, stored))
            }
            _ => None,
        };
        let Some(rt) = self.session.as_ref() else {
            return Ok(None);
        };
        let fs = rt.sample_rate;
        let idx = |input: u16| {
            block_index(&rt.input_map, input).ok_or_else(|| {
                perr(
                    ErrorCode::Invalid,
                    format!("input {input} is not captured by the session"),
                )
            })
        };
        let inv = |e: String| perr(ErrorCode::Invalid, e);
        let (analysis, grid): (Box<dyn Analysis>, Option<GridDef>) = match &m.config.kind {
            MeasKind::Transfer { config } => {
                let d = m.delay.clone().unwrap_or(DelayState {
                    applied: Seconds(0.0),
                    applied_samples: 0.0,
                    nudged: Seconds(0.0),
                    nudged_samples: 0.0,
                    tracking: false,
                    awaiting_pick: false,
                    last_finding: None,
                });
                let a = jobs::transfer::Transfer::new(
                    m.id,
                    config.clone(),
                    fs,
                    idx(config.reference_input)?,
                    idx(config.measurement_input)?,
                    d.applied_samples,
                    d.applied.0,
                    d.nudged_samples,
                    m.config_rev,
                    d.tracking,
                    d.awaiting_pick,
                    rt.epoch,
                    self.s.to_self.clone(),
                    self.input_cal(rt, config.measurement_input)
                        .correction
                        .as_deref(),
                )
                .map_err(inv)?;
                (Box::new(a), static_grid(&m.config.kind))
            }
            MeasKind::Spectrum { config } => {
                let a = jobs::spectrum::Spectrum::new(
                    m.id,
                    config.clone(),
                    fs,
                    idx(config.input)?,
                    self.input_cal(rt, config.input),
                    m.config_rev,
                )
                .map_err(inv)?;
                (Box::new(a), Some(jobs::spectrum::grid(config, fs)))
            }
            MeasKind::Rta { config } => {
                let bank = jobs::rta::bank(config, fs).map_err(inv)?;
                let g = jobs::rta::grid(config, &bank);
                let a = jobs::rta::Rta::new(
                    m.id,
                    config.clone(),
                    fs,
                    idx(config.input)?,
                    self.input_cal(rt, config.input),
                    m.config_rev,
                )
                .map_err(inv)?;
                (Box::new(a), Some(g))
            }
            MeasKind::Spl { config } => {
                let (input, cal) = (idx(config.input)?, self.input_cal(rt, config.input));
                let config = config.clone();
                let leq = leq
                    .take()
                    .ok_or_else(|| perr(ErrorCode::Internal, "SPL meter without its log"))?;
                let a = jobs::spl::Spl::new(m.id, config, fs, input, cal, m.config_rev, leq)
                    .map_err(inv)?;
                (Box::new(a), None)
            }
            MeasKind::Sweep { .. } => return Ok(None),
            MeasKind::Math { config } => {
                let (grids, stored) =
                    math.ok_or_else(|| perr(ErrorCode::Internal, "math channel without setup"))?;
                let shown = grids
                    .display
                    .as_ref()
                    .map_or_else(|| grids.grid.clone(), |(g, _)| g.clone());
                let a = jobs::math::MathJob::new(
                    m.id,
                    config.clone(),
                    grids,
                    stored,
                    rt.epoch,
                    Arc::clone(&self.probes),
                    m.config_rev,
                );
                (Box::new(a), Some(shown))
            }
        };
        // Math channels ask their live operands for results; a math channel is never one.
        let probed = matches!(
            m.config.kind,
            MeasKind::Transfer { .. } | MeasKind::Spectrum { .. } | MeasKind::Rta { .. }
        );
        let fid = self.next_fanout_id;
        self.next_fanout_id += 1;
        let (handle, tx) = jobs::spawn(
            format!("ac2d-meas-{}", m.id.0),
            self.job_env(rt),
            fid,
            analysis,
        )
        .map_err(|e| perr(ErrorCode::Internal, format!("cannot start job: {e}")))?;
        rt.fanout.attach(fid, tx);
        self.probes.set(m.id, handle.probe().filter(|_| probed));
        if let Some(old) = self.jobs.insert(m.id, handle)
            && let Some(rt) = &self.session
        {
            rt.fanout.detach(old.fanout_id);
        }
        tracing::info!("measurement {} running", m.id.0);
        Ok(grid)
    }

    pub(super) fn stop_job(&mut self, id: MeasId) {
        let orphaned: Vec<u64> = self
            .pending_finds
            .iter()
            .filter(|(_, p)| p.meas == id)
            .map(|(t, _)| *t)
            .collect();
        for t in orphaned {
            if let Some(p) = self.pending_finds.remove(&t) {
                let e = perr(
                    ErrorCode::Invalid,
                    "the measurement stopped during delay.find",
                );
                self.answer(&p.routing_id, &p.client, p.id, Err(e), Instant::now());
            }
        }
        self.probes.set(id, None);
        if let Some(h) = self.jobs.remove(&id) {
            if let Some(rt) = &self.session {
                rt.fanout.detach(h.fanout_id);
            }
            drop(h);
        }
    }

    pub(super) fn stop_all_jobs(&mut self) {
        let ids: Vec<MeasId> = self.jobs.keys().copied().collect();
        for id in ids {
            self.stop_job(id);
        }
        if let Some(h) = self.timing_job.take() {
            if let Some(rt) = &self.session {
                rt.fanout.detach(h.fanout_id);
            }
            drop(h);
        }
    }

    /// Starts the session input meters; the fan-out publishes them, they stop with it.
    pub(super) fn start_session_levels(&self) {
        let Some(rt) = self.session.as_ref() else {
            return;
        };
        rt.fanout
            .start_levels(self.job_env(rt), rt.input_map.clone(), self.store.rev());
    }

    pub(super) fn start_timing_job(&mut self) {
        // Drift belongs to the two clocks of one stream; a stream just opened has shown none,
        // whether or not it has a loopback to show one on.
        let mut t = self.store.state().timing;
        if t.drift.take().is_some() {
            self.commit(Change::Timing(t));
        }
        let Some(rt) = self.session.as_ref() else {
            return;
        };
        let Some(lb) = rt.open.config.loopback else {
            return;
        };
        let (Some(history), Some(idx)) = (rt.history.clone(), block_index(&rt.input_map, lb.input))
        else {
            return;
        };
        let a = jobs::timing::Timing::new(
            rt.sample_rate,
            idx,
            history,
            self.s.to_self.clone(),
            rt.epoch,
            self.store.state().timing,
        );
        let fid = self.next_fanout_id;
        self.next_fanout_id += 1;
        match jobs::spawn("ac2d-timing".into(), self.job_env(rt), fid, Box::new(a)) {
            Ok((h, tx)) => {
                rt.fanout.attach(fid, tx);
                self.timing_job = Some(h);
            }
            Err(e) => tracing::error!("cannot start the timing monitor: {e}"),
        }
    }

    /// Applies `delay`. An explicit operator value drops the last finding: it no longer
    /// describes the applied delay, and a refusal must not keep showing as the reason there
    /// is no delay. The operator's insert or value resolves an ambiguous finding, so tracking
    /// resumes (decision 1c); a delay tracking moved changes neither.
    ///
    /// An insert is a new arrival: nothing is nudged from it. A nudge or a typed value moves
    /// the applied delay away from the arrival and keeps the arrival, so the view moves this
    /// curve alone: a typed value is the operator's refinement of where this measurement
    /// should sit, the same act as a run of steps, while only the finder knows an arrival.
    /// Tracking moves the arrival and keeps the operator's offset from it: its `delay`
    /// already includes that offset.
    pub(super) fn set_delay(
        &mut self,
        meas: MeasId,
        delay: Seconds,
        source: DelaySource,
    ) -> Result<ReplyBody, ProtoError> {
        self.transfer_delay(meas)?;
        if !(delay.0.is_finite() && delay.0.abs() <= MAX_DELAY_S) {
            return Err(perr(
                ErrorCode::Invalid,
                format!("delay must be finite and within ±{MAX_DELAY_S} s"),
            ));
        }
        let Some(fs) = self.session.as_ref().map(|r| f64::from(r.sample_rate)) else {
            return Err(perr(
                ErrorCode::Invalid,
                "no open session: the delay in samples depends on its sample rate",
            ));
        };
        let samples = delay_samples(delay.0, fs);
        let mut m = self.meas(meas)?.clone();
        let nudged = match (&m.delay, source) {
            (_, DelaySource::Insert) | (None, _) => 0.0,
            (Some(d), DelaySource::Nudge | DelaySource::Typed) => {
                delay_samples((d.nudged_samples + samples - d.applied_samples) / fs, fs)
            }
            (Some(d), DelaySource::Tracking) => d.nudged_samples,
        };
        if (nudged / fs).abs() > MAX_DELAY_S {
            return Err(perr(
                ErrorCode::Invalid,
                format!("the delay may be nudged by at most ±{MAX_DELAY_S} s from the arrival"),
            ));
        }
        let rev = Rev(self.store.rev().0 + 1);
        let operator = source != DelaySource::Tracking;
        if let Some(d) = &mut m.delay {
            d.applied = Seconds(samples / fs);
            d.applied_samples = samples;
            d.nudged = Seconds(nudged / fs);
            d.nudged_samples = nudged;
            if source == DelaySource::Typed {
                d.last_finding = None;
            }
            if operator {
                d.awaiting_pick = false;
            }
        }
        m.config_rev = rev;
        if let Some(j) = self.jobs.get(&meas) {
            j.send(JobCmd::SetDelay {
                samples,
                seconds: samples / fs,
                nudged_samples: nudged,
                rev,
                resume: operator,
            });
        }
        self.commit(Change::Measurement(Patch::Set(m.clone())));
        Ok(ReplyBody::Measurement(m))
    }

    pub(super) fn start_find(
        &mut self,
        meas: MeasId,
        band: FinderBand,
        observation: Option<Seconds>,
    ) -> Result<u64, ProtoError> {
        self.transfer_delay(meas)?;
        let job = self.jobs.get(&meas).ok_or_else(|| {
            perr(
                ErrorCode::Invalid,
                "the measurement is not running; the finder needs live audio",
            )
        })?;
        let fs = self
            .session
            .as_ref()
            .map(|r| f64::from(r.sample_rate))
            .ok_or_else(|| perr(ErrorCode::Invalid, "no open session"))?;
        let band = conv::finder_band(band);
        check_find(band, observation.map(|o| o.0), fs)?;
        let token = self.next_token;
        self.next_token += 1;
        job.send(JobCmd::Find {
            token,
            band,
            observation: observation.map(|o| o.0),
        });
        Ok(token)
    }

    pub(super) fn finish_find(&mut self, token: u64, result: Result<FinderResult, String>) {
        let Some(p) = self.pending_finds.remove(&token) else {
            return;
        };
        let fs = self.session.as_ref().map(|r| f64::from(r.sample_rate));
        let reply = match (result, fs) {
            (Err(e), _) => Err(perr(ErrorCode::Invalid, format!("delay finder: {e}"))),
            (Ok(_), None) => Err(perr(ErrorCode::Invalid, "the session closed")),
            (Ok(r), Some(fs)) => {
                let f = conv::delay_finding(&r, fs, WallNs(wall_ns()));
                match self.meas(p.meas).cloned() {
                    Err(e) => Err(e),
                    Ok(mut m) => {
                        if let Some(d) = &mut m.delay {
                            // The job paused tracking on an ambiguous result (1c).
                            d.awaiting_pick = matches!(f.outcome, DelayOutcome::Ambiguous { .. });
                            d.last_finding = Some(f.clone());
                        }
                        self.commit(Change::Measurement(Patch::Set(m)));
                        Ok(ReplyBody::DelayFinding(f))
                    }
                }
            }
        };
        self.answer(&p.routing_id, &p.client, p.id, reply, Instant::now());
    }

    pub(super) fn tracked(&mut self, meas: MeasId, epoch: SessionEpoch, samples: f64) {
        let Some(fs) = self
            .session
            .as_ref()
            .filter(|r| r.epoch == epoch)
            .map(|r| f64::from(r.sample_rate))
        else {
            return;
        };
        let Ok(m) = self.meas(meas) else {
            return;
        };
        let Some(d) = &m.delay else {
            return;
        };
        // `samples` is the arrival; the operator's nudge from it stays on top. A move smaller
        // than the tracker's own scatter would only shuffle the fraction's phase rotation
        // back and forth without aligning anything better.
        if d.tracking
            && ((d.applied_samples - d.nudged_samples) - samples).abs() > TRACK_TOLERANCE_SAMPLES
            && let Err(e) = self.set_delay(
                meas,
                Seconds((samples + d.nudged_samples) / fs),
                DelaySource::Tracking,
            )
        {
            tracing::warn!("tracked delay not applied: {}", e.msg);
        }
    }
}
