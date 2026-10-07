//! `sweep.run`: a run of a sweep measurement (`docs/design/sweep-distortion.md`), with the
//! settings the measurement holds; its result is a sweep trace the measurement owns.
//!
//! A run plays under the stimulus lease like firing does: the caller must hold the lease
//! and have armed the generator. The recorder job is attached to the capture fan-out before
//! the sweep train is handed to the output, so the recording starts no later than the first
//! sweep can arrive. While the train plays the generator is `firing`; once the recording is
//! in, the output is silenced and the generator disarmed — a sweep is one shot, and the
//! operator arms again for the next — and the analysis runs on its own thread. Anything
//! that stops the stimulus or the session — `gen.stop`, `gen.release`, a forced takeover,
//! lease expiry, session close or reopen, `file.load` — aborts the run and discards its
//! audio.

use std::sync::Arc;
use std::time::Instant;

use super::{
    ActiveSweep, CalState, Change, ClientId, Control, ControlMsg, CoreBandLimit, CoreGenerator,
    CoreSignal, ErrorCode, GenAction, GeneratorConfig, GeneratorSettings, LeaseToken, LeasedSource,
    MeasKind, ProtoError, Recording, ReplyBody, Seconds, SweepAnalysis, SweepConfig, SweepError,
    SweepFailure, SweepId, SweepRun, SweepStatus, SweepTrain, TraceKind, TraceMeta, TraceSource,
    WallNs, block_index, dbfs_to_rms, gen_err, jobs, perr,
};
use crate::sweep;
use crate::util::wall_ns;
use ac2_audio::Gain;
use ac2_proto::model::Signal;
use ac2_proto::model::TraceOwner;
use ac2_proto::units::MeasId;
use ac2_traces::meta;

impl Control {
    pub(super) fn sweep_run(
        &mut self,
        client: &ClientId,
        token: LeaseToken,
        meas: MeasId,
        name: Option<String>,
    ) -> Result<ReplyBody, ProtoError> {
        self.check_lease(Instant::now());
        self.lease_check(client, token)?;
        let m = self.meas(meas)?;
        let MeasKind::Sweep { config } = &m.config.kind else {
            return Err(perr(
                ErrorCode::Invalid,
                format!("{} is not a sweep measurement", m.config.name),
            ));
        };
        let req: SweepConfig = config.clone();
        let meas_name = m.config.name.clone();
        let number = self.next_run_number(meas);
        let name = name.unwrap_or_else(|| format!("Run {number}"));
        meta::check_edit(&name, None).map_err(|e| perr(ErrorCode::Invalid, e))?;
        let level = req.level;
        if !level.0.is_finite() {
            return Err(perr(ErrorCode::Invalid, "level must be finite"));
        }
        if level.0 > self.s.ceiling_dbfs {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "{:.1} dBFS is above the global maximum {:.1} dBFS",
                    level.0, self.s.ceiling_dbfs
                ),
            ));
        }
        if self.sweep.is_some() {
            return Err(perr(ErrorCode::Refused, "a sweep is already running"));
        }
        if self.detecting.is_some() {
            return Err(perr(
                ErrorCode::Refused,
                "a loopback detection is playing its burst; sweep once it is done",
            ));
        }
        let g = &self.store.state().generator;
        if g.firing {
            return Err(perr(
                ErrorCode::Refused,
                "the stimulus is firing: stop it before sweeping",
            ));
        }
        if !g.armed {
            return Err(perr(
                ErrorCode::Refused,
                "arm the stimulus first (gen.set armed): a sweep fires like any stimulus",
            ));
        }
        let Some(rt) = self.session.as_ref() else {
            return Err(perr(ErrorCode::Refused, "no open session to play into"));
        };
        let (reference, measurement) = (req.reference_input, req.measurement_input);
        if reference == measurement {
            return Err(perr(
                ErrorCode::Invalid,
                "the reference and the measurement are the same input",
            ));
        }
        let idx = |input: u16, what: &str| {
            block_index(&rt.input_map, input).ok_or_else(|| {
                perr(
                    ErrorCode::Invalid,
                    format!("{what} input {} is not captured by the session", input + 1),
                )
            })
        };
        let (ref_idx, mic_idx) = (
            idx(reference, "reference")?,
            idx(measurement, "measurement")?,
        );
        if req.outputs.is_empty() {
            return Err(perr(ErrorCode::Invalid, "no output channels"));
        }
        for (i, o) in req.outputs.iter().enumerate() {
            if req.outputs[..i].contains(o) {
                return Err(perr(ErrorCode::Invalid, format!("output {o} listed twice")));
            }
            if *o >= rt.output_channels {
                return Err(perr(
                    ErrorCode::Invalid,
                    format!("output {o} is not an output of the session"),
                ));
            }
        }
        if !(1..=SweepConfig::MAX_REPEATS).contains(&req.repeats) {
            return Err(perr(
                ErrorCode::Invalid,
                format!("repeats must be 1 … {}", SweepConfig::MAX_REPEATS),
            ));
        }
        let fs = f64::from(rt.sample_rate);
        let (spec, timing) = sweep::spec(req.sweep, level.0, req.gate, req.tail, fs)?;
        let sweeps = (0..req.repeats)
            .map(|_| {
                CoreGenerator::new(&GeneratorConfig {
                    signal: CoreSignal::Ess(timing.emitted),
                    sample_rate: fs,
                    seed: 0,
                    band: CoreBandLimit::NONE,
                    level_dbfs: level.0,
                    ceiling_dbfs: self.s.ceiling_dbfs,
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(gen_err)?;
        let crest = sweeps.first().map_or(1.0, CoreGenerator::crest_factor);
        if dbfs_to_rms(level.0) * crest > f64::from(self.s.max_level.linear()) * (1.0 + 1e-9) {
            return Err(perr(
                ErrorCode::Refused,
                "the sweep's peak would exceed the output limit",
            ));
        }

        // The recorder first: it must hear the first sweep from its first sample.
        let id = SweepId(self.next_sweep);
        self.next_sweep += 1;
        let lead = (sweep::LEAD_S * fs).round() as usize;
        let period = timing.period_samples(fs);
        let total = lead + usize::from(req.repeats) * period;
        let recorder = jobs::sweep::Recorder::new(
            id,
            ref_idx,
            mic_idx,
            total,
            lead,
            period,
            req.repeats,
            self.s.to_self.clone(),
        );
        let fid = self.next_fanout_id;
        self.next_fanout_id += 1;
        let (job, tx) = jobs::spawn(
            format!("ac2d-sweep-{}", id.0),
            self.job_env(rt),
            fid,
            Box::new(recorder),
        )
        .map_err(|e| {
            perr(
                ErrorCode::Internal,
                format!("cannot start the recorder: {e}"),
            )
        })?;
        rt.fanout.attach(fid, tx);

        // Then the train, under the lease gate like any stimulus.
        let deadline = Instant::now() + self.s.lease_expiry;
        if let Some(l) = &mut self.lease {
            l.deadline = deadline;
        }
        let Some(rt) = self.session.as_mut() else {
            return Err(perr(ErrorCode::Refused, "no open session to play into"));
        };
        let play = (|| {
            rt.set_routes(&req.outputs)?;
            let train =
                SweepTrain::new(sweeps, timing.sweep_samples(), timing.post_roll_samples(fs));
            self.gate.open_until(deadline);
            rt.gen_handle
                .set_source(Box::new(LeasedSource::new(
                    Box::new(train),
                    Arc::clone(&self.gate),
                )))
                .map_err(|e| perr(ErrorCode::Internal, e.to_string()))?;
            rt.gen_handle.set_gain(Gain::UNITY);
            rt.gen_handle.start();
            Ok::<(), ProtoError>(())
        })();
        if let Err(e) = play {
            rt.fanout.detach(fid);
            drop(job);
            self.stop_output();
            return Err(e);
        }
        // The train is no level-controlled generator: a later fire builds its own source.
        self.level = None;
        self.source = None;

        let mut g = self.store.state().generator.clone();
        g.firing = true;
        g.settings = Some(GeneratorSettings {
            signal: Signal::Ess { sweep: req.sweep },
            level,
            band: None,
            outputs: req.outputs.clone(),
        });
        self.audit(&mut g, GenAction::Fire, Some(client));
        self.commit(Change::Generator(g));

        let run = SweepRun {
            id,
            meas,
            owner: client.clone(),
            name,
            reference_input: reference,
            measurement_input: measurement,
            outputs: req.outputs,
            level,
            sweep: req.sweep,
            sweep_duration: Seconds(timing.plan.duration_s()),
            post_roll: Seconds(timing.post_roll_samples(fs) as f64 / fs),
            repeats: req.repeats,
            gate: req.gate,
            status: SweepStatus::Playing { repeat: 1 },
            started_at: WallNs(wall_ns()),
        };
        tracing::info!(
            target: "ac2d::audit",
            "sweep {} by {}: {} × {:.2} s at {:.1} dBFS on out {:?}, input {} re {}",
            id.0,
            client.0,
            run.repeats,
            run.sweep_duration.0,
            level.0,
            run.outputs.iter().map(|o| o + 1).collect::<Vec<_>>(),
            measurement + 1,
            reference + 1
        );
        self.commit(Change::Sweep(run.clone()));
        // The sweep analysis works on the raw recordings: no curve is in its columns, so
        // the trace names the mic only (`trace.mic_curve` applies a curve afterwards).
        let mic = self
            .cal_and_mic(measurement)
            .1
            .map(|m| ac2_proto::model::MicState { curve: None, ..m });
        self.sweep = Some(ActiveSweep {
            run: run.clone(),
            meas_name,
            number,
            spec,
            job: Some(job),
            epoch: self.epoch(),
            mic,
        });
        Ok(ReplyBody::Sweep(run))
    }

    /// Ends the run in progress without a result: its recorder stops, its audio is
    /// discarded. The caller stops the output itself (it is stopping it anyway).
    pub(super) fn abort_sweep(&mut self, reason: SweepFailure, msg: &str) {
        let Some(mut a) = self.sweep.take() else {
            return;
        };
        if let (Some(job), Some(rt)) = (a.job.take(), &self.session) {
            rt.fanout.detach(job.fanout_id);
        }
        tracing::warn!("sweep {} aborted: {msg}", a.run.id.0);
        a.run.status = SweepStatus::Failed {
            reason,
            msg: msg.to_owned(),
        };
        self.commit(Change::Sweep(a.run));
    }

    fn active(&mut self, id: SweepId) -> Option<&mut ActiveSweep> {
        self.sweep.as_mut().filter(|s| s.run.id == id)
    }

    pub(super) fn sweep_progress(&mut self, id: SweepId, repeat: u8) {
        let Some(a) = self.active(id) else { return };
        match a.run.status {
            SweepStatus::Playing { repeat: r } if r < repeat => {
                a.run.status = SweepStatus::Playing { repeat };
                let run = a.run.clone();
                self.commit(Change::Sweep(run));
            }
            _ => {}
        }
    }

    /// The recording is in: the output is silenced, the generator disarmed (the lease stays
    /// with its holder) and the analysis starts.
    pub(super) fn sweep_recorded(&mut self, id: SweepId, result: Result<Recording, String>) {
        let Some(a) = self.active(id) else { return };
        let job = a.job.take();
        let spec = a.spec;
        let repeats = a.run.repeats;
        if let (Some(job), Some(rt)) = (job, &self.session) {
            rt.fanout.detach(job.fanout_id);
        }
        // The train has played out: silence the source and disarm before the (longer)
        // analysis. Staying armed would leave a sweep source one Enter away from playing
        // again without the operator having armed it.
        self.stop_output();
        let mut g = self.store.state().generator.clone();
        if g.firing || g.armed {
            g.firing = false;
            g.armed = false;
            self.audit(&mut g, GenAction::Stop, None);
            self.commit(Change::Generator(g));
        }
        let rec = match result {
            Ok(r) => r,
            Err(msg) => {
                self.fail_sweep(SweepFailure::Dropout, msg);
                return;
            }
        };
        let Some(a) = self.active(id) else { return };
        a.run.status = SweepStatus::Analysing;
        let run = a.run.clone();
        self.commit(Change::Sweep(run));
        let to = self.s.to_self.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("ac2d-sweep-analysis-{}", id.0))
            .spawn(move || {
                let result = sweep::analyse(&spec, &rec, repeats);
                let _ = to.send(ControlMsg::SweepAnalysed {
                    id,
                    result: Box::new(result),
                });
            });
        if let Err(e) = spawned {
            self.fail_sweep(
                SweepFailure::Analysis,
                format!("cannot start the analysis: {e}"),
            );
        }
    }

    pub(super) fn sweep_analysed(
        &mut self,
        id: SweepId,
        result: Result<SweepAnalysis, SweepError>,
    ) {
        if self.active(id).is_none() {
            return;
        }
        let a = match result {
            Ok(a) => a,
            Err(e) => {
                self.fail_sweep(sweep::failure(&e), e.to_string());
                return;
            }
        };
        let Some(active) = self.sweep.take() else {
            return;
        };
        let run = &active.run;
        let grid = sweep::grid(&active.spec);
        let (columns, data) = sweep::trace_data(&a);
        let tid = self.traces.alloc();
        let t = TraceMeta {
            id: tid,
            edit: meta::new_edit(
                tid,
                run.name.clone(),
                None,
                TraceOwner::Meas { meas: run.meas },
            ),
            kind: TraceKind::Sweep,
            source: TraceSource::Sweep {
                meas: run.meas,
                meas_name: active.meas_name.clone(),
                run: run.id,
                number: active.number,
                epoch: active.epoch,
                sweep: run.sweep,
                level: run.level,
                repeats: run.repeats,
                reference_input: run.reference_input,
                measurement_input: run.measurement_input,
            },
            grid_id: grid.id(),
            delay: Seconds(a.arrival_s),
            depth: None,
            // A ratio of two inputs: no calibration applies.
            cal: CalState::Uncalibrated,
            mic: active.mic.clone(),
            mic_curve: None,
            created_at: WallNs(wall_ns()),
        };
        tracing::info!(
            "sweep {} stored as trace {tid}: arrival {:.2} ms, reference {:+.1} dB{}",
            run.id.0,
            a.arrival_s * 1000.0,
            a.reference_db,
            if a.clipped { ", CLIPPED" } else { "" }
        );
        self.add_trace(t, grid, columns, Some(data));
        let mut run = active.run;
        run.status = SweepStatus::Done { trace: tid };
        self.commit(Change::Sweep(run));
    }

    fn fail_sweep(&mut self, reason: SweepFailure, msg: String) {
        let Some(mut a) = self.sweep.take() else {
            return;
        };
        tracing::warn!("sweep {} failed: {msg}", a.run.id.0);
        a.run.status = SweepStatus::Failed { reason, msg };
        self.commit(Change::Sweep(a.run));
    }
}
