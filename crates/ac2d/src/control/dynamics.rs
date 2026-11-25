//! `dynamics.run`: a run of a dynamics measurement (`docs/design/dynamics.md`); its result
//! is one dynamics trace the measurement owns, holding every measured input.
//!
//! A run plays under the stimulus lease as a sweep does: the caller holds the lease and has
//! armed the generator. The job is attached to the capture fan-out before the programme is
//! handed to the output, so it hears the linearity check from its start. The job analyses
//! as the audio arrives and publishes the rests it finds recovered to the programme source;
//! once the last rest is in, the output is silenced, the generator disarmed, and the trace
//! stored. Anything that stops the stimulus or the stream aborts the run (with the
//! programme's fade-out) and discards it.
//!
//! The source counts its longest rests itself, so the run's length is bounded whatever the
//! job does; a job that has not finished a few seconds past that bound has lost the
//! programme, and the run fails rather than waiting for ever.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_core::dynamics::{
    self as core, DynamicsResult, Onset, Phase, Programme, ProgrammeConfig, Release,
};
use ac2_proto::model::{
    DynamicsData, DynamicsEdge, DynamicsFailure, DynamicsHarmonics, DynamicsInfo, DynamicsInput,
    DynamicsLevel, DynamicsOnset, DynamicsOvershoot, DynamicsPhase, DynamicsPoint, DynamicsRow,
    DynamicsRun, DynamicsStatus, DynamicsStep, DynamicsThump, DynamicsTime, DynamicsWindows,
    Signal, TraceOwner,
};
use ac2_proto::units::{Db, Dbfs, DynamicsId, Hz, MeasId, TraceId};
use ac2_traces::meta;

use super::{
    CalState, Change, Control, CoreBandLimit, CoreGenerator, CoreSignal, GenAction,
    GeneratorConfig, GeneratorSettings, LeaseToken, LeasedSource, MeasKind, ProtoError, ReplyBody,
    Seconds, StreamEpoch, TraceKind, TraceMeta, TraceSource, WallNs, block_index, dbfs_to_rms,
    gen_err, jobs,
};
use crate::jobs::JobHandle;
use crate::route::Peer;
use crate::util::{internal, invalid, refused, wall_ns};

/// How long past the programme's longest run the job may take to see its end: the capture
/// latency and the analysis of the last rest, with room to spare.
const FINISH_GRACE: Duration = Duration::from_secs(10);

pub(super) struct ActiveDynamics {
    pub(super) run: DynamicsRun,
    meas_name: String,
    number: u32,
    epoch: StreamEpoch,
    programme: Arc<Programme>,
    /// The job; gone once the run has ended.
    job: Option<JobHandle>,
    /// When the run has certainly played out.
    deadline: Instant,
    /// The measured inputs' names, in the run's order.
    input_names: Vec<String>,
}

fn onset(o: DynamicsOnset) -> Onset {
    match o {
        DynamicsOnset::Zero => Onset::Zero,
        DynamicsOnset::Peak => Onset::Peak,
    }
}

impl Control {
    pub(super) fn dynamics_run(
        &mut self,
        client: &Peer,
        token: LeaseToken,
        meas: MeasId,
        name: Option<String>,
        rests_from: Option<TraceId>,
    ) -> Result<ReplyBody, ProtoError> {
        self.check_lease(Instant::now());
        self.lease_check(client, token)?;
        let m = self.meas(meas)?;
        let MeasKind::Dynamics { config } = &m.config.kind else {
            return Err(invalid(format!(
                "{} is not a dynamics measurement",
                m.config.name
            )));
        };
        let req = config.clone();
        let meas_name = m.config.name.clone();
        let number = self.next_run_number(meas);
        let name = name.unwrap_or_else(|| format!("Run {number}"));
        meta::check_edit(&name, None).map_err(invalid)?;
        req.check_inputs().map_err(invalid)?;
        if self.sweep.as_ref().is_some_and(|s| s.run.active())
            || self.dynamics.as_ref().is_some_and(|d| d.run.active())
        {
            return Err(refused("a sweep or dynamics run is already playing"));
        }
        if self.pending.detecting.is_some() {
            return Err(refused(
                "a loopback detection is playing its burst; run once it is done",
            ));
        }
        let g = &self.store.state().generator;
        if g.firing {
            return Err(refused("the stimulus is firing: stop it before running"));
        }
        if !g.armed {
            return Err(refused(
                "arm the stimulus first (gen.set armed): a dynamics run fires like any stimulus",
            ));
        }
        let replay = match rests_from {
            None => None,
            Some(t) => {
                let d = self.traces.dynamics(t).ok_or_else(|| {
                    invalid(format!("trace {t} is not a dynamics trace to replay"))
                })?;
                Some(d.info.steps.iter().map(|s| s.rest.0).collect::<Vec<_>>())
            }
        };
        let Some(rt) = self.stream.as_ref() else {
            return Err(refused("no open stream to play into"));
        };
        let fs = f64::from(rt.sample_rate);
        let programme = Programme::new(
            &ProgrammeConfig {
                carrier_hz: req.carrier.0,
                onset: onset(req.onset),
                pilot_dbfs: req.pilot.0,
                bursts_dbfs: req.bursts.iter().map(|b| b.0).collect(),
                durations_s: req.durations.iter().map(|d| d.0).collect(),
                rest_max_s: req.rest_max.0,
                replay_rests_s: replay,
            },
            fs,
        )
        .map_err(|e| invalid(e.to_string()))?;
        // Checked when it plays, against the maximum in force now, as a sweep's level is: a
        // measurement louder than the maximum is kept between the runs the maximum is raised for.
        let top = programme.highest_dbfs();
        if top > self.s.ceiling_dbfs {
            return Err(refused(format!(
                "{top:.1} dBFS is above the global maximum {:.1} dBFS",
                self.s.ceiling_dbfs
            )));
        }
        let idx = |input: u16, what: &str| {
            block_index(&rt.input_map, input).ok_or_else(|| {
                invalid(format!(
                    "{what} input {} is not captured by the stream",
                    input + 1
                ))
            })
        };
        let ref_idx = idx(req.reference_input, "reference")?;
        let in_idx = req
            .measurement_inputs
            .iter()
            .map(|&i| idx(i, "measured"))
            .collect::<Result<Vec<_>, _>>()?;
        if req.outputs.is_empty() {
            return Err(invalid("no output channels"));
        }
        for (i, o) in req.outputs.iter().enumerate() {
            if req.outputs[..i].contains(o) {
                return Err(invalid(format!("output {o} listed twice")));
            }
            if *o >= rt.output_channels {
                return Err(invalid(format!(
                    "output {o} is not an output of the stream"
                )));
            }
        }
        let programme = Arc::new(programme);
        let release = Release::new();
        let generator = CoreGenerator::new(&GeneratorConfig {
            signal: CoreSignal::Programme {
                programme: Arc::clone(&programme),
                release: release.clone(),
            },
            sample_rate: fs,
            seed: 0,
            band: CoreBandLimit::NONE,
            level_dbfs: top,
            ceiling_dbfs: self.s.ceiling_dbfs,
        })
        .map_err(gen_err)?;
        if !self
            .s
            .max_level
            .admits_peak(dbfs_to_rms(top) * generator.crest_factor())
        {
            return Err(refused(
                "the programme's peak would exceed the output limit",
            ));
        }

        // The job first: it must hear the linearity check from its first period.
        let id = self.ids.dynamics();
        let analyser = core::Analyser::new(&programme, in_idx.len());
        let job = jobs::dynamics::Recorder::new(
            id,
            ref_idx,
            in_idx,
            analyser,
            release,
            self.s.to_self.clone(),
        );
        let fid = self.ids.fanout();
        let (job, tx) = jobs::spawn(
            format!("ac2d-dynamics-{}", id.0),
            self.job_env(rt),
            fid,
            Box::new(job),
        )
        .map_err(|e| internal(format!("cannot start the analysis: {e}")))?;
        rt.fanout.attach(fid, tx);

        let deadline = Instant::now() + self.s.lease_expiry;
        if let Some(l) = &mut self.out.lease {
            l.deadline = deadline;
        }
        let Some(rt) = self.stream.as_mut() else {
            return Err(refused("no open stream to play into"));
        };
        let play = (|| {
            rt.set_routes(&req.outputs)?;
            self.out.gate.open_until(deadline);
            rt.gen_handle
                .set_source(Box::new(LeasedSource::new(
                    Box::new(generator),
                    Arc::clone(&self.out.gate),
                )))
                .map_err(|e| internal(e.to_string()))?;
            Ok::<bool, ProtoError>(super::generator::start_output(&rt.gen_handle))
        })();
        match play {
            Ok(true) => self.generator_edge(crate::jobs::finder::GeneratorEdge::Started),
            Ok(false) => {}
            Err(e) => {
                rt.fanout.detach(fid);
                drop(job);
                self.stop_output();
                return Err(e);
            }
        }
        // The programme is no level-controlled generator: a later fire builds its own.
        self.out.level = None;
        self.out.source = None;

        let carrier = Hz(programme.carrier_hz());
        let mut g = self.store.state().generator.clone();
        g.firing = true;
        // The carrier at the highest level, the generator's level: what the output plays
        // at its loudest.
        g.settings = Some(GeneratorSettings {
            signal: Signal::Sine { freq: carrier },
            level: Dbfs(top),
            band: None,
            outputs: req.outputs.clone(),
        });
        self.audit(&mut g, GenAction::Fire, Some(client));
        self.commit(Change::Generator(g));

        let p_s = f64::from(programme.period) / fs;
        let longest = programme.longest_seconds();
        let steps = programme.steps() as u32;
        let run = DynamicsRun {
            id,
            meas,
            owner: client.id().clone(),
            name,
            reference_input: req.reference_input,
            measurement_inputs: req.measurement_inputs.clone(),
            outputs: req.outputs,
            carrier,
            onset: req.onset,
            pilot: req.pilot,
            bursts: req.bursts,
            durations: programme
                .burst_periods
                .iter()
                .map(|&n| Seconds(f64::from(n) * p_s))
                .collect(),
            rest_max: Seconds(f64::from(programme.rest_max_periods) * p_s),
            rests_from,
            longest: Seconds(longest),
            status: DynamicsStatus::Playing {
                step: 0,
                steps,
                phase: DynamicsPhase::Waiting,
            },
            started_at: WallNs(wall_ns()),
        };
        tracing::info!(
            target: "ac2d::audit",
            "dynamics {} by {}: {steps} steps at {:.2} Hz up to {top:.1} dBFS on out {:?}, \
             inputs {:?} re {}, at most {longest:.0} s",
            id.0,
            client.id().0,
            carrier.0,
            run.outputs.iter().map(|o| o + 1).collect::<Vec<_>>(),
            run.measurement_inputs
                .iter()
                .map(|i| i + 1)
                .collect::<Vec<_>>(),
            run.reference_input + 1
        );
        self.commit(Change::Dynamics(run.clone()));
        let input_names = run
            .measurement_inputs
            .iter()
            .map(|&i| self.input_name(i))
            .collect();
        self.dynamics = Some(ActiveDynamics {
            run: run.clone(),
            meas_name,
            number,
            epoch: self.epoch(),
            programme,
            job: Some(job),
            deadline: Instant::now() + Duration::from_secs_f64(longest) + FINISH_GRACE,
            input_names,
        });
        Ok(ReplyBody::Dynamics(run))
    }

    fn active_dynamics(&mut self, id: DynamicsId) -> Option<&mut ActiveDynamics> {
        self.dynamics
            .as_mut()
            .filter(|d| d.run.id == id && d.run.active())
    }

    pub(super) fn dynamics_progress(&mut self, id: DynamicsId, phase: Phase) {
        let Some(a) = self.active_dynamics(id) else {
            return;
        };
        let DynamicsStatus::Playing { steps, .. } = a.run.status else {
            return;
        };
        let (step, phase) = match phase {
            Phase::Waiting => (0, DynamicsPhase::Waiting),
            Phase::Check => (0, DynamicsPhase::Check),
            Phase::Burst(n) => (n + 1, DynamicsPhase::Burst),
            Phase::Rest(n) => (n + 1, DynamicsPhase::Rest),
            // The result follows.
            Phase::Done => return,
        };
        a.run.status = DynamicsStatus::Playing { step, steps, phase };
        let run = a.run.clone();
        self.commit(Change::Dynamics(run));
    }

    /// The run has ended in the job: silence and disarm, then store its trace or its
    /// failure.
    pub(super) fn dynamics_done(
        &mut self,
        id: DynamicsId,
        result: Result<DynamicsResult, jobs::dynamics::Failure>,
    ) {
        if self.active_dynamics(id).is_none() {
            return;
        }
        self.end_dynamics_output();
        let r = match result {
            Ok(r) => r,
            Err((reason, msg)) => {
                self.fail_dynamics(reason, msg);
                return;
            }
        };
        if self.dynamics.as_ref().is_some_and(|a| {
            r.inputs.len() != a.run.measurement_inputs.len() || r.steps.len() != a.programme.steps()
        }) {
            self.fail_dynamics(
                DynamicsFailure::Mismatch,
                "the analysis returned no result for every input and step".into(),
            );
            return;
        }
        // The slot holds only a run in progress: an ended run is in the state store, and a
        // slot left behind would refuse deleting its measurement.
        let Some(mut a) = self.dynamics.take() else {
            return;
        };
        let trace = self.store_dynamics_trace(&a, r);
        a.run.status = DynamicsStatus::Done { trace };
        self.commit(Change::Dynamics(a.run));
    }

    /// Detaches the job and silences the output; the generator is disarmed (the lease stays
    /// with its holder): a run is one shot, and the operator arms again for the next.
    fn end_dynamics_output(&mut self) {
        let job = self.dynamics.as_mut().and_then(|a| a.job.take());
        if let (Some(job), Some(rt)) = (job, &self.stream) {
            rt.fanout.detach(job.fanout_id);
        }
        self.stop_output();
        let mut g = self.store.state().generator.clone();
        if g.firing || g.armed {
            g.firing = false;
            g.armed = false;
            self.audit(&mut g, GenAction::Stop, None);
            self.commit(Change::Generator(g));
        }
    }

    /// Fails a run whose job has not seen the programme's end well after it must have
    /// played out.
    pub(super) fn check_dynamics_deadline(&mut self, now: Instant) {
        let late = self
            .dynamics
            .as_ref()
            .is_some_and(|d| d.run.active() && d.deadline <= now);
        if late {
            self.end_dynamics_output();
            self.fail_dynamics(
                DynamicsFailure::Mismatch,
                "the programme played out but its end never reached the reference input".into(),
            );
        }
    }

    /// Ends the dynamics run in progress without a result. The caller stops the output.
    pub(super) fn abort_dynamics(&mut self, reason: DynamicsFailure, msg: &str) {
        if !self.dynamics.as_ref().is_some_and(|d| d.run.active()) {
            return;
        }
        let job = self.dynamics.as_mut().and_then(|a| a.job.take());
        if let (Some(job), Some(rt)) = (job, &self.stream) {
            rt.fanout.detach(job.fanout_id);
        }
        self.fail_dynamics(reason, msg.to_owned());
    }

    fn fail_dynamics(&mut self, reason: DynamicsFailure, msg: String) {
        let Some(mut a) = self.dynamics.take() else {
            return;
        };
        tracing::warn!("dynamics {} failed: {msg}", a.run.id.0);
        a.run.status = DynamicsStatus::Failed { reason, msg };
        self.commit(Change::Dynamics(a.run));
    }

    fn store_dynamics_trace(&mut self, a: &ActiveDynamics, r: DynamicsResult) -> TraceId {
        let run = &a.run;
        let p = &a.programme;
        let fs = p.sample_rate;
        let data = DynamicsData {
            info: DynamicsInfo {
                sample_rate_hz: Hz(fs),
                carrier: run.carrier,
                period: p.period,
                onset: run.onset,
                pilot: run.pilot,
                bursts: run.bursts.clone(),
                durations: run.durations.clone(),
                rest_max: run.rest_max,
                replayed: run.rests_from.is_some(),
                reference_input: run.reference_input,
                steps: r
                    .steps
                    .iter()
                    .map(|s| DynamicsStep {
                        level: Dbfs(s.level_dbfs),
                        burst: Seconds(s.burst_samples as f64 / fs),
                        rest: Seconds(s.rest_samples as f64 / fs),
                        recovered: s.recovered,
                    })
                    .collect(),
            },
            inputs: r
                .inputs
                .iter()
                .zip(run.measurement_inputs.iter().zip(&a.input_names))
                .map(|(i, (&input, name))| DynamicsInput {
                    input,
                    name: name.clone(),
                    delay: Seconds(i.delay_samples / fs),
                    g0: Db(i.g0_db),
                    rows: i.rows.iter().map(row).collect(),
                })
                .collect(),
            envelopes: r
                .inputs
                .iter()
                .map(|i| {
                    i.envelopes
                        .iter()
                        .map(|e| e.iter().map(point).collect())
                        .collect()
                })
                .collect(),
        };
        let grid = ac2_traces::text::dynamics::grid(run.carrier);
        let tid = self.traces.alloc();
        let t = TraceMeta {
            id: tid,
            edit: meta::new_edit(
                tid,
                run.name.clone(),
                None,
                TraceOwner::Meas { meas: run.meas },
            ),
            kind: TraceKind::Dynamics,
            source: TraceSource::Dynamics {
                meas: run.meas,
                meas_name: a.meas_name.clone(),
                run: run.id,
                number: a.number,
                epoch: a.epoch,
            },
            grid_id: grid.id(),
            delay: Seconds(0.0),
            depth: None,
            lf_window: None,
            // Gains of an input re the reference: no calibration applies.
            cal: CalState::Uncalibrated,
            mic: None,
            mic_curve: None,
            created_at: WallNs(wall_ns()),
            eq_in: Vec::new(),
            position: None,
            captured_unsettled: false,
            superseded: false,
        };
        tracing::info!(
            "dynamics {} stored as trace {tid}: {} steps, {} inputs",
            run.id.0,
            data.info.steps.len(),
            data.inputs.len()
        );
        self.traces.set_dynamics(tid, Some(Box::new(data)));
        self.add_trace(
            t,
            grid,
            ac2_traces::text::dynamics::columns(),
            None,
            None,
            None,
        );
        tid
    }
}

fn time(t: core::Time) -> DynamicsTime {
    match t {
        core::Time::Seconds(s) => DynamicsTime::Seconds { value: Seconds(s) },
        core::Time::WithinPeriod => DynamicsTime::WithinPeriod,
        core::Time::CantTell => DynamicsTime::CantTell,
        core::Time::NoGainReduction => DynamicsTime::NoGainReduction,
    }
}

fn level(r: core::Reading) -> DynamicsLevel {
    match r {
        core::Reading::Db(v) => DynamicsLevel::Db { value: Db(v) },
        core::Reading::BelowFloor => DynamicsLevel::BelowFloor,
        core::Reading::CantTell => DynamicsLevel::CantTell,
    }
}

fn harmonics(w: &core::WindowHarmonics) -> DynamicsHarmonics {
    DynamicsHarmonics {
        h: w.h.map(level),
        thd: level(w.thd),
    }
}

fn thump(t: &core::Thump) -> DynamicsThump {
    DynamicsThump {
        peak: level(t.peak),
        rms: level(t.rms),
    }
}

fn row(r: &core::StepRow) -> DynamicsRow {
    let [attack, settled, release, rest] = &r.harmonics;
    DynamicsRow {
        gr_end: Db(r.gr_end),
        gr_settled: r.gr_settled.map(Db),
        settled: r.settled,
        attack_tau: time(r.attack_tau),
        attack_2db: time(r.attack_2db),
        settle_0_5db: time(r.settle_0_5db),
        overshoot: r.overshoot.map(|o| DynamicsOvershoot {
            level: Db(o.db),
            at: Seconds(o.at),
            reference: Db(o.reference_db),
        }),
        release_tau: time(r.release_tau),
        recovery_2db: time(r.recovery_2db),
        recovery_0_5db: time(r.recovery_0_5db),
        harmonics: DynamicsWindows {
            attack: harmonics(attack),
            settled: harmonics(settled),
            release: harmonics(release),
            rest: harmonics(rest),
        },
        thump_start: thump(&r.thump[0]),
        thump_end: thump(&r.thump[1]),
        drift: Db(r.drift),
    }
}

fn point(p: &core::EnvelopePoint) -> DynamicsPoint {
    DynamicsPoint {
        edge: match p.edge {
            core::Edge::Start => DynamicsEdge::Start,
            core::Edge::End => DynamicsEdge::End,
        },
        t: p.t as f32,
        gr_db: p.gr_db,
        h_db: p.h_db,
    }
}

impl Control {
    /// A measurement whose run is playing keeps its settings until the run ends: the run's
    /// analysis reads the programme it was started with.
    pub(super) fn check_dynamics_idle(&self, meas: MeasId) -> Result<(), ProtoError> {
        if self
            .dynamics
            .as_ref()
            .is_some_and(|d| d.run.meas == meas && d.run.active())
        {
            return Err(refused(
                "its dynamics run is playing: change it once the run has ended",
            ));
        }
        Ok(())
    }
}
