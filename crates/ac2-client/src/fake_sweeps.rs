//! Recording and sweep commands of the fake daemon. A sweep run (`sweep.run`) is stored at
//! once (no audio): the synthetic capture response with H2 at −40 dB rising 12 dB/oct below
//! 100 Hz, H3 at −50 dB, H4 and H5 in the noise, the floor at −80 dB.

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Change, ErrorCode, GridDef, ProtoError, ReplyBody};
use ac2_traces::columns::Columns;
use ac2_traces::meta;

use super::traces::synthetic;
use super::{Shared, err};

impl Shared {
    /// `sweep.run` (lease already checked): needs the generator armed and a level, then
    /// stores a synthetic sweep trace and reports the run playing, then done.
    /// `rec.start`: a recording that never grows (the fake writes no file); its size is
    /// the header alone.
    pub(super) fn rec_start(
        &mut self,
        client: &ClientId,
        req: RecordRequest,
    ) -> Result<ReplyBody, ProtoError> {
        if self
            .state
            .recording
            .as_ref()
            .is_some_and(RecordingRun::active)
        {
            return Err(err(ErrorCode::Refused, "a recording is running"));
        }
        let Some(open) = self.state.session.open.clone() else {
            return Err(err(ErrorCode::Refused, "no open session to record"));
        };
        if req.inputs.is_empty()
            || req
                .inputs
                .iter()
                .any(|i| !open.config.input_channels.contains(i))
        {
            return Err(err(
                ErrorCode::Invalid,
                "inputs not captured by the session",
            ));
        }
        let name = req.name.clone().unwrap_or_else(|| "rec-fake".into());
        let run = RecordingRun {
            path: format!("/fake/recordings/{name}.wav"),
            name,
            inputs: req.inputs,
            sample_rate_hz: open.sample_rate_hz,
            session_epoch: self.state.session.epoch,
            start_sample: SampleIndex(0),
            started_at: WallNs(self.now_ns()),
            started_by: client.clone(),
            frames: 0,
            bytes: 116,
            discontinuities: 0,
            max_duration: req.max_duration,
            max_bytes: req.max_bytes,
            status: RecordingStatus::Recording,
        };
        self.commit(Change::Recording(run.clone()));
        Ok(ReplyBody::Recording(run))
    }

    /// `rec.stop`.
    pub(super) fn rec_stop(&mut self) -> Result<ReplyBody, ProtoError> {
        let Some(mut run) = self.state.recording.clone().filter(RecordingRun::active) else {
            return Err(err(ErrorCode::Refused, "nothing is being recorded"));
        };
        run.status = RecordingStatus::Ended {
            reason: RecordingEnd::Stopped,
        };
        self.commit(Change::Recording(run.clone()));
        Ok(ReplyBody::Recording(run))
    }

    /// `sweep.run`, as the daemon: the measurement's settings, a run stored under it.
    pub(super) fn sweep_run(
        &mut self,
        client: &ClientId,
        meas: MeasId,
        name: Option<String>,
    ) -> Result<ReplyBody, ProtoError> {
        let m = self.meas(meas)?;
        let MeasKind::Sweep { config: req } = m.config.kind.clone() else {
            return Err(err(ErrorCode::Invalid, "not a sweep measurement"));
        };
        let number = self
            .state
            .traces
            .iter()
            .filter_map(|t| match &t.source {
                TraceSource::Sweep {
                    meas: x, number, ..
                } if *x == meas => Some(*number),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            + 1;
        let name = name.unwrap_or_else(|| format!("Run {number}"));
        meta::check_edit(&name, None).map_err(|e| err(ErrorCode::Invalid, e))?;
        let level = req.level;
        if level.0 > self.state.generator.ceiling.0 {
            return Err(err(ErrorCode::Refused, "level above ceiling"));
        }
        if !self.state.generator.armed || self.state.generator.firing {
            return Err(err(
                ErrorCode::Refused,
                "arm the stimulus first (gen.set armed)",
            ));
        }
        req.check_inputs()
            .map_err(|e| err(ErrorCode::Invalid, e.to_string()))?;
        let reference = req.reference_input;
        let (f1, f2) = (req.sweep.start.0, req.sweep.end.0);
        let ppo = req.resolution.ppo();
        let k = |f: f64| (f / 1000.0).log2() * f64::from(ppo);
        let grid = GridDef::Log {
            ppo,
            k_min: k(f1).floor() as i32,
            k_max: k(f2).ceil() as i32,
        };
        let id = SweepId(self.traces.next_sweep.max(1));
        self.traces.next_sweep = id.0 + 1;
        let rate = req.sweep.duration.0 / (f2 / f1).ln();
        let mut run = SweepRun {
            id,
            meas,
            owner: client.clone(),
            name: name.clone(),
            reference_input: reference,
            measurement_inputs: req.measurement_inputs.clone(),
            outputs: req.outputs.clone(),
            level,
            sweep: req.sweep,
            sweep_duration: req.sweep.duration,
            post_roll: Seconds(1.0),
            repeats: req.repeats,
            gate: req.gate,
            lf_harmonics: req.lf_harmonics,
            resolution: req.resolution,
            status: SweepStatus::Playing { repeat: 1 },
            started_at: WallNs(1_790_000_000_000_000_000),
        };
        let started = run.clone();
        self.commit(Change::Sweep(run.clone()));
        let several = req.measurement_inputs.len() > 1;
        let mut traces = Vec::new();
        for &measurement in &req.measurement_inputs {
            let (columns, sweep) = synthetic_sweep(&grid, f2, rate, req.repeats);
            let tid = self.alloc_trace();
            let mut meta = Self::new_meta(
                tid,
                if several {
                    format!("{name} · Input {}", measurement + 1)
                } else {
                    name.clone()
                },
                None,
                TraceKind::Sweep,
                TraceSource::Sweep {
                    meas,
                    meas_name: m.config.name.clone(),
                    run: id,
                    number,
                    epoch: self.state.session.epoch,
                    sweep: req.sweep,
                    level,
                    repeats: req.repeats,
                    lf_harmonics: req.lf_harmonics,
                    reference_input: reference,
                    measurement_input: measurement,
                },
                &grid,
                sweep.info.arrival,
            );
            meta.mic = None;
            meta.edit.owner = TraceOwner::Meas { meas };
            self.add_trace(meta, grid.clone(), columns);
            self.traces.sweeps.insert(tid, sweep);
            traces.push(tid);
        }
        run.status = SweepStatus::Done { traces };
        self.commit(Change::Sweep(run));
        // As the daemon: a sweep is one shot, the generator ends disarmed.
        self.state.generator.armed = false;
        self.state.generator.firing = false;
        self.generator_changed(GenAction::Stop, None);
        Ok(ReplyBody::Sweep(started))
    }
}

/// A synthetic sweep result on `grid`: the capture response, H2 at −40 dB rising 12 dB/oct
/// below 100 Hz, H3 at −50 dB, H4 and H5 below the −80 dB floor, nothing above `f2 / k`.
fn synthetic_sweep(grid: &GridDef, f2: f64, rate: f64, repeats: u8) -> (Columns, SweepData) {
    let f = grid.centres();
    let mut columns = synthetic(grid);
    columns.coherence = None;
    let curve = |k: f64, level: &dyn Fn(f64) -> f64| DistortionCurve {
        level_db: f
            .iter()
            .map(|x| {
                if k * x <= f2 {
                    level(*x) as f32
                } else {
                    f32::NAN
                }
            })
            .collect(),
        floor_db: f
            .iter()
            .map(|x| if k * x <= f2 { -80.0 } else { f32::NAN })
            .collect(),
    };
    let h2 = |x: f64| -40.0 + 10.0 * (1.0 + (100.0 / x).powi(4)).log10();
    let h3 = |_: f64| -50.0;
    let low = |_: f64| -84.0;
    let harmonics = vec![
        HarmonicCurve {
            order: 2,
            curve: curve(2.0, &h2),
        },
        HarmonicCurve {
            order: 3,
            curve: curve(3.0, &h3),
        },
        HarmonicCurve {
            order: 4,
            curve: curve(4.0, &low),
        },
        HarmonicCurve {
            order: 5,
            curve: curve(5.0, &low),
        },
    ];
    let thd = curve(2.0, &|x: f64| {
        let p = |db: f64| 10f64.powf(db / 10.0);
        let mut s = p(h2(x));
        if 3.0 * x <= f2 {
            s += p(-50.0);
        }
        10.0 * s.log10()
    });
    let fs = 48_000.0;
    let dt = 1.0 / fs;
    let t0 = -rate * 5f64.ln() - 0.01;
    let n = ((0.25 - t0) / dt) as usize;
    let at = |t: f64| ((t - t0) / dt).round() as usize;
    let mut linear = vec![0.0f32; n];
    let mut etc_db = vec![-90.0f32; n];
    for (k, a) in [(1.0f64, 0.5f32), (2.0, 0.005), (3.0, 0.0016)] {
        let i = at(-rate * k.ln());
        if i < n {
            linear[i] = a;
            etc_db[i] = 20.0 * a.log10();
        }
    }
    // A loopback far cleaner than the system: its own floor 40 dB lower.
    let reference_harmonics = (2u8..=5)
        .map(|order| {
            let k = f64::from(order);
            let c = curve(k, &|_| -96.0 - 6.0 * (k - 2.0));
            HarmonicCurve {
                order,
                curve: DistortionCurve {
                    floor_db: c.floor_db.iter().map(|v| v - 40.0).collect(),
                    ..c
                },
            }
        })
        .collect();
    let sweep = SweepData {
        harmonics,
        thd,
        reference_harmonics: Some(reference_harmonics),
        ir: TraceIr {
            t0: Seconds(t0),
            dt: Seconds(dt),
            linear,
            etc_db,
        },
        info: SweepInfo {
            sample_rate: Hz(fs),
            rate: Seconds(rate),
            duration: Seconds(rate * (f2 / 20.0).ln()),
            repeats,
            arrival: Seconds(0.0125),
            reference_level: Db(0.0),
            window_pre: Seconds(0.1 * rate * 1.2f64.ln()),
            window_post: Seconds(0.9 * rate * 1.25f64.ln()),
            gate_pre: Seconds(0.1 * rate * 2f64.ln()),
            gate: Seconds(0.25),
            floor_margin: Db(6.0),
            clipped: false,
        },
        room: Some(synthetic_room(f2)),
        unresolved: None,
    };
    let unresolved = synthetic_unresolved(grid, &sweep.info);
    (
        columns,
        SweepData {
            unresolved,
            ..sweep
        },
    )
}

/// Where a cell of `1/window` is wider than a column: below `κ(ppo) · cell`, with
/// `κ(N) = 1/(2^(1/2N) − 2^(−1/2N))`; the response's linear window and H2's share of the
/// harmonic window (the daemon's analysis works this out per column).
fn synthetic_unresolved(grid: &GridDef, info: &SweepInfo) -> Option<SweepUnresolved> {
    let GridDef::Log { ppo, k_min, .. } = *grid else {
        return None;
    };
    let resolution = Resolution::from_ppo(ppo)?;
    let h = 0.5 / f64::from(ppo);
    let kappa = 1.0 / (2f64.powf(h) - 2f64.powf(-h));
    let lo = 1000.0 * 2f64.powf((f64::from(k_min) - 0.5) / f64::from(ppo));
    let below = |cell: f64| Unresolved {
        resolution,
        ranges: (kappa * cell > lo)
            .then_some(FreqRange {
                lo: Hz(lo),
                hi: Hz(kappa * cell),
            })
            .into_iter()
            .collect(),
    };
    Some(SweepUnresolved {
        response: below(1.0 / (info.gate_pre.0 + info.gate.0)),
        harmonics: below(1.0 / (2.0 * (info.window_pre.0 + info.window_post.0))),
    })
}

/// Synthetic room parameters: a hall of T ≈ 1.1 s falling to 0.7 s at the top, 52 dB of
/// decay range in the octaves (40 dB at 63 Hz: T30 refused there; the 63 Hz EDT too short
/// for its band), octave bands up to `f2`.
fn synthetic_room(f2: f64) -> RoomAcoustics {
    let band = |centre: Option<f64>, t: f64, range: f64| {
        let v = |value: f64| RoomValue::Value { value };
        let need = |needed: f64, x: f64| {
            if range >= needed {
                v(x)
            } else {
                RoomValue::Refused {
                    reason: RoomRefusal::InsufficientRange {
                        range: Db(range),
                        needed: Db(needed),
                    },
                }
            }
        };
        let k = 6.0 * std::f64::consts::LN_10 / t;
        let c = |ms: f64| {
            10.0 * ((0.3 + 1.0 - (-k * ms / 1000.0).exp()) / (-k * ms / 1000.0).exp()).log10()
        };
        let early = 0.3 + 1.0 - (-k * 0.05f64).exp();
        RoomBand {
            centre: centre.map(Hz),
            onset: Seconds(0.0),
            truncation: Seconds(t * range / 60.0),
            decay_range: Some(Db(range)),
            edt: if centre.is_some_and(|c| c < 70.0) {
                RoomValue::Refused {
                    reason: RoomRefusal::FilterLimited {
                        bandwidth_decay: 6.5,
                    },
                }
            } else {
                v(t * 0.95)
            },
            t20: need(35.0, t),
            t30: need(45.0, t * 1.02),
            c50: v(c(50.0)),
            c80: v(c(80.0)),
            d50: v(early / 1.3),
            curvature: (range >= 45.0).then_some(2.0),
        }
    };
    let octave = [63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0]
        .iter()
        .filter(|&&f| f * std::f64::consts::SQRT_2 <= f2)
        .map(|&f| {
            let t = 1.1 - 0.4 * ((f / 63.0).log2() / 7.0);
            band(Some(f), t, if f < 100.0 { 40.0 } else { 52.0 })
        })
        .collect();
    RoomAcoustics {
        broadband: band(None, 0.95, 58.0),
        octave,
        third: Vec::new(),
        span_end: Seconds(0.99),
    }
}
