//! Trace and session commands of the fake daemon, on the same `ac2-traces` code the real
//! daemon uses. Captures are synthetic: a flat −6 dB response with a 2nd-order roll-off
//! below 80 Hz, the measurement's delay compensated, coherence 0.95. A sweep run (`sweep.run`)
//! is stored at once (no audio): that response with H2 at −40 dB rising 12 dB/oct below
//! 100 Hz, H3 at −50 dB, H4 and H5 in the noise, the floor at −80 dB.

use std::collections::HashMap;
use std::path::PathBuf;

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Change, ErrorCode, ErrorDetail, GridDef, Patch, ProtoError, ReplyBody};
use ac2_traces::columns::{Columns, StoredTrace, frequencies};
use ac2_traces::meta;
use ac2_traces::ops::{self, Derived};
use ac2_traces::session::{self, SavedDelay, SavedMeasurement, SessionError};
use ac2_traces::text;

use super::{Shared, err};

/// Trace data held by the fake.
#[derive(Debug, Default)]
pub(super) struct FakeTraces {
    pub(super) data: HashMap<TraceId, (GridDef, Columns)>,
    pub(super) sweeps: HashMap<TraceId, SweepData>,
    pub(super) next_sweep: u32,
    /// Mic curves applied to traces after capture.
    pub(super) mic_curves: HashMap<TraceId, ac2_traces::mic::Correction>,
    /// Points (Hz, dB) of the imported mic curves.
    pub(super) curve_points: HashMap<MicCurveId, Vec<[f64; 2]>>,
}

fn session_err(e: SessionError) -> ProtoError {
    let msg = e.to_string();
    match e {
        SessionError::NotFound(_) => err(ErrorCode::NotFound, msg),
        SessionError::Version { found, .. } => ProtoError {
            code: ErrorCode::Unsupported,
            msg,
            detail: Some(ErrorDetail::SessionVersion {
                found,
                supported: session::VERSION,
            }),
        },
        SessionError::Io { .. } => err(ErrorCode::Internal, msg),
        _ => err(ErrorCode::Invalid, msg),
    }
}

/// Synthetic transfer columns on `grid`.
fn synthetic(grid: &GridDef) -> Columns {
    let f = frequencies(grid);
    Columns {
        mag_db: f
            .iter()
            .map(|f| {
                let x = (f / 80.0).powi(2);
                (-6.0 + 10.0 * (x * x / (1.0 + x * x)).log10()) as f32
            })
            .collect(),
        phase_deg: Some(
            f.iter()
                .map(|f| (180.0 * 80.0 / (f + 80.0)) as f32)
                .collect(),
        ),
        coherence: Some(vec![0.95; f.len()]),
    }
}

impl Shared {
    fn trace(&self, id: TraceId) -> Result<StoredTrace, ProtoError> {
        let meta = self
            .state
            .traces
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .ok_or_else(|| err(ErrorCode::NotFound, format!("no trace {id}")))?;
        let (grid, columns) = self
            .traces
            .data
            .get(&id)
            .cloned()
            .ok_or_else(|| err(ErrorCode::Internal, format!("trace {id} has no data")))?;
        Ok(StoredTrace {
            meta,
            grid,
            columns,
            sweep: self.traces.sweeps.get(&id).cloned(),
            mic_curve: self.traces.mic_curves.get(&id).cloned(),
        })
    }

    fn add_trace(&mut self, meta: TraceMeta, grid: GridDef, columns: Columns) -> ReplyBody {
        self.grids.insert(grid.id(), grid.clone());
        for other in meta::take_slot(&self.state.traces, meta.id, meta.edit.slot) {
            self.commit(Change::Trace(Patch::Set(other)));
        }
        self.traces.data.insert(meta.id, (grid, columns));
        self.commit(Change::Trace(Patch::Set(meta.clone())));
        ReplyBody::Trace(meta)
    }

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
        let (reference, measurement) = (req.reference_input, req.measurement_input);
        let (f1, f2) = (req.sweep.start.0, req.sweep.end.0);
        let k = |f: f64| (f / 1000.0).log2() * 48.0;
        let grid = GridDef::Log {
            ppo: 48,
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
            measurement_input: measurement,
            outputs: req.outputs.clone(),
            level,
            sweep: req.sweep,
            sweep_duration: req.sweep.duration,
            post_roll: Seconds(1.0),
            repeats: req.repeats,
            gate: req.gate,
            status: SweepStatus::Playing { repeat: 1 },
            started_at: WallNs(1_790_000_000_000_000_000),
        };
        let started = run.clone();
        self.commit(Change::Sweep(run.clone()));
        let (columns, sweep) = synthetic_sweep(&grid, f2, rate, req.repeats);
        let tid = self.alloc_trace();
        let mut meta = Self::new_meta(
            tid,
            name,
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
                reference_input: reference,
                measurement_input: measurement,
            },
            &grid,
            sweep.info.arrival,
        );
        meta.mic = None;
        meta.edit.owner = TraceOwner::Meas { meas };
        self.add_trace(meta, grid, columns);
        self.traces.sweeps.insert(tid, sweep);
        run.status = SweepStatus::Done { trace: tid };
        self.commit(Change::Sweep(run));
        // As the daemon: a sweep is one shot, the generator ends disarmed.
        self.state.generator.armed = false;
        self.state.generator.firing = false;
        self.generator_changed(GenAction::Stop, None);
        Ok(ReplyBody::Sweep(started))
    }

    fn alloc_trace(&mut self) -> TraceId {
        let id = TraceId(self.next_id);
        self.next_id += 1;
        id
    }

    fn new_meta(
        id: TraceId,
        name: String,
        slot: Option<u8>,
        kind: TraceKind,
        source: TraceSource,
        grid: &GridDef,
        delay: Seconds,
    ) -> TraceMeta {
        TraceMeta {
            id,
            edit: meta::new_edit(id, name, slot, TraceOwner::Imported),
            kind,
            source,
            grid_id: grid.id(),
            delay,
            depth: None,
            cal: CalState::Uncalibrated,
            mic: None,
            mic_curve: None,
            created_at: WallNs(1_790_000_000_000_000_000),
        }
    }

    pub(super) fn trace_capture(
        &mut self,
        meas: MeasId,
        name: String,
        slot: Option<u8>,
    ) -> Result<ReplyBody, ProtoError> {
        meta::check_edit(&name, slot).map_err(|e| err(ErrorCode::Invalid, e))?;
        let m = self.meas(meas)?;
        let MeasKind::Transfer { config } = &m.config.kind else {
            return Err(err(
                ErrorCode::Invalid,
                "the fake captures transfer measurements only",
            ));
        };
        let grid = GridDef::Log {
            ppo: config.grid.ppo,
            k_min: config.grid.k_min,
            k_max: config.grid.k_max,
        };
        let id = self.alloc_trace();
        let mut t = Self::new_meta(
            id,
            name,
            slot,
            TraceKind::Transfer,
            TraceSource::Captured {
                meas,
                meas_name: m.config.name.clone(),
                epoch: self.state.session.epoch,
                at_sample: SampleIndex(48_000),
            },
            &grid,
            m.delay.as_ref().map_or(Seconds(0.0), |d| d.applied),
        );
        t.edit.smoothing = config.smoothing;
        t.edit.owner = TraceOwner::Meas { meas };
        t.depth = Some(config.depth);
        let c = synthetic(&grid);
        Ok(self.add_trace(t, grid, c))
    }

    pub(super) fn trace_get(&self, id: TraceId) -> Result<ReplyBody, ProtoError> {
        Ok(ReplyBody::TraceData(Box::new(self.trace(id)?.data())))
    }

    pub(super) fn trace_update(
        &mut self,
        id: TraceId,
        edit: TraceEdit,
    ) -> Result<ReplyBody, ProtoError> {
        meta::check_values(&edit).map_err(|e| err(ErrorCode::Invalid, e))?;
        let mut t = self.trace(id)?.meta;
        if !meta::lock_allows(&t.edit, &edit) {
            return Err(err(ErrorCode::Refused, format!("trace {id} is locked")));
        }
        for other in meta::take_slot(&self.state.traces, id, edit.slot) {
            self.commit(Change::Trace(Patch::Set(other)));
        }
        t.edit = edit;
        self.commit(Change::Trace(Patch::Set(t.clone())));
        Ok(ReplyBody::Trace(t))
    }

    pub(super) fn trace_delete(&mut self, id: TraceId) -> Result<ReplyBody, ProtoError> {
        let t = self.trace(id)?;
        if t.meta.edit.locked {
            return Err(err(ErrorCode::Refused, format!("trace {id} is locked")));
        }
        self.traces.data.remove(&id);
        self.traces.sweeps.remove(&id);
        self.traces.mic_curves.remove(&id);
        let rev = self.commit(Change::Trace(Patch::Deleted(id)));
        Ok(ReplyBody::Ack { rev })
    }

    fn derived(
        &mut self,
        name: String,
        source: TraceSource,
        d: Derived,
        owner: TraceOwner,
    ) -> Result<ReplyBody, ProtoError> {
        meta::check_edit(&name, None).map_err(|e| err(ErrorCode::Invalid, e))?;
        let id = self.alloc_trace();
        let mut t = Self::new_meta(id, name, None, d.kind, source, &d.grid, d.delay);
        t.edit.owner = owner;
        Ok(self.add_trace(t, d.grid, d.columns))
    }

    pub(super) fn trace_average(
        &mut self,
        ids: Vec<TraceId>,
        method: AverageMethod,
        reference: DelayReference,
        name: String,
    ) -> Result<ReplyBody, ProtoError> {
        let inputs = ids
            .iter()
            .map(|i| self.trace(*i))
            .collect::<Result<Vec<_>, _>>()?;
        let refs: Vec<&StoredTrace> = inputs.iter().collect();
        let d = ops::average(&refs, method, reference)
            .map_err(|e| err(ErrorCode::Invalid, e.to_string()))?;
        // As the daemon: with its inputs when they share an owner.
        let owner = refs
            .first()
            .map_or(TraceOwner::Imported, |t| t.meta.edit.owner);
        let owner = if refs.iter().all(|t| t.meta.edit.owner == owner) {
            owner
        } else {
            TraceOwner::Imported
        };
        self.derived(
            name,
            TraceSource::Average {
                traces: ids,
                method,
                reference,
            },
            d,
            owner,
        )
    }

    pub(super) fn trace_import(
        &mut self,
        file_name: String,
        format: ImportFormat,
        role: ImportRole,
        content: &[u8],
    ) -> Result<ReplyBody, ProtoError> {
        let imp = text::import(content, format, role).map_err(|e| ProtoError {
            code: ErrorCode::Invalid,
            msg: e.to_string(),
            detail: Some(ErrorDetail::Import {
                line: e.line,
                problem: e.problem,
            }),
        })?;
        let name = imp.name.clone().unwrap_or_else(|| {
            std::path::Path::new(&file_name)
                .file_stem()
                .map_or_else(|| file_name.clone(), |s| s.to_string_lossy().into_owned())
        });
        let id = self.alloc_trace();
        let t = Self::new_meta(
            id,
            name,
            None,
            imp.kind,
            TraceSource::Imported {
                file_name,
                format: imp.format,
                notes: imp.notes,
            },
            &imp.grid,
            imp.delay.unwrap_or(Seconds(0.0)),
        );
        if let Some(sw) = imp.sweep {
            self.traces.sweeps.insert(id, sw);
        }
        Ok(self.add_trace(t, imp.grid, imp.columns))
    }

    /// `trace.mic_curve`, as the daemon: the named curve of the mic library, normalised at
    /// the trace's calibrator frequency (1 kHz without one).
    pub(super) fn trace_mic_curve(
        &mut self,
        id: TraceId,
        curve: Option<MicCurveId>,
    ) -> Result<ReplyBody, ProtoError> {
        let mut t = self.trace(id)?.meta;
        ac2_traces::mic::check(&t).map_err(|e| {
            let code = match e {
                ac2_traces::mic::MicCurveError::Target => ErrorCode::Invalid,
                _ => ErrorCode::Refused,
            };
            err(code, format!("trace {id}: {e}"))
        })?;
        match curve {
            None => {
                if t.mic_curve.take().is_none() {
                    return Err(err(
                        ErrorCode::NotFound,
                        format!("trace {id} has no mic curve applied after capture"),
                    ));
                }
                self.traces.mic_curves.remove(&id);
            }
            Some(m) => {
                let r = ac2_proto::cal::curve(&self.state.mics, &m.mic, &m.label)
                    .cloned()
                    .ok_or_else(|| {
                        err(
                            ErrorCode::NotFound,
                            format!(
                                "no mic curve {:?} of {:?} in the mic library",
                                m.label, m.mic
                            ),
                        )
                    })?;
                let points = self.traces.curve_points.get(&m).ok_or_else(|| {
                    err(
                        ErrorCode::Internal,
                        format!("the mic curve {m:?} has no points"),
                    )
                })?;
                let f_norm = match &t.cal {
                    CalState::Calibrated { key, .. } => self
                        .state
                        .calibrations
                        .iter()
                        .find(|e| e.key == *key)
                        .map(|e| ac2_proto::cal::f_norm(&e.spl).0),
                    CalState::Uncalibrated => None,
                }
                .unwrap_or(1000.0);
                let k = ac2_traces::mic::correction(points, f_norm)
                    .map_err(|x| err(ErrorCode::Internal, x))?;
                t.mic_curve = Some(Box::new(TraceMicCurve {
                    mic: m.mic,
                    curve: r,
                    f_norm: Hz(f_norm),
                }));
                self.traces.mic_curves.insert(id, k);
            }
        }
        self.commit(Change::Trace(Patch::Set(t.clone())));
        Ok(ReplyBody::Trace(t))
    }

    pub(super) fn trace_export(&self, id: TraceId) -> Result<ReplyBody, ProtoError> {
        let t = self.trace(id)?;
        Ok(ReplyBody::Export {
            file_name: meta::file_name(&t.meta.edit.name, "csv"),
            content: Blob(text::export_csv(&t).into_bytes()),
        })
    }

    fn session_path(&self, r: &SessionRef) -> Result<(String, PathBuf), ProtoError> {
        let root = self
            .opts
            .session_dir
            .clone()
            .ok_or_else(|| err(ErrorCode::Unsupported, "the fake has no session directory"))?;
        match r {
            SessionRef::Name { name } => {
                session::validate_name(name).map_err(session_err)?;
                Ok((name.clone(), root.join(name)))
            }
            SessionRef::Path { path } => {
                let p = PathBuf::from(path);
                let n = p
                    .file_name()
                    .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
                Ok((n, p))
            }
        }
    }

    fn file_info(name: String, dir: &std::path::Path, m: &session::Manifest) -> SessionFile {
        SessionFile {
            name,
            path: dir.to_string_lossy().into_owned(),
            saved_at: m.saved_at,
            measurements: m.measurements.len() as u32,
            traces: m.traces.len() as u32,
        }
    }

    pub(super) fn file_save(&mut self, r: &SessionRef) -> Result<ReplyBody, ProtoError> {
        let (name, dir) = self.session_path(r)?;
        let traces = self
            .state
            .traces
            .iter()
            .map(|t| self.trace(t.id))
            .collect::<Result<Vec<_>, _>>()?;
        let s = session::Session {
            saved_at: WallNs(self.now_ns()),
            measurements: self
                .state
                .measurements
                .iter()
                .map(|m| SavedMeasurement {
                    id: m.id,
                    config: m.config.clone(),
                    running: m.running,
                    frozen: m.frozen,
                    delay: m.delay.as_ref().map(|d| SavedDelay {
                        applied: d.applied,
                        tracking: d.tracking,
                    }),
                })
                .collect(),
            spl_logs: self
                .state
                .measurements
                .iter()
                .filter_map(|m| match &m.config.kind {
                    MeasKind::Spl { config } => Some(session::SavedSplLog {
                        info: ac2_traces::spl_log::SplLogInfo {
                            meas: m.id,
                            name: m.config.name.clone(),
                            input: config.input,
                            mic: None,
                        },
                        rows: self.spl_rows.get(&m.id).cloned().unwrap_or_default(),
                    }),
                    _ => None,
                })
                .collect(),
            traces,
        };
        let m = session::save(&dir, &s).map_err(session_err)?;
        Ok(ReplyBody::SessionFile(Self::file_info(name, &dir, &m)))
    }

    pub(super) fn file_load(
        &mut self,
        client: &ClientId,
        r: &SessionRef,
    ) -> Result<ReplyBody, ProtoError> {
        let (name, dir) = self.session_path(r)?;
        let m = session::read_manifest(&dir).map_err(session_err)?;
        let s = session::load(&dir).map_err(session_err)?;
        self.lease = None;
        self.state.generator.owner = None;
        self.state.generator.armed = false;
        self.state.generator.firing = false;
        self.generator_changed(GenAction::Stop, Some(client.clone()));
        for id in self
            .state
            .measurements
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>()
        {
            if self.state.spl_logs.iter().any(|l| l.meas == id) {
                self.commit(Change::SplLog(Patch::Deleted(id)));
            }
            self.commit(Change::Measurement(Patch::Deleted(id)));
        }
        self.spl_rows.clear();
        for l in s.spl_logs {
            self.spl_rows.insert(l.info.meas, l.rows);
        }
        for id in self.state.traces.iter().map(|t| t.id).collect::<Vec<_>>() {
            self.commit(Change::Trace(Patch::Deleted(id)));
        }
        self.traces.data.clear();
        self.traces.sweeps.clear();
        self.traces.mic_curves.clear();
        let newest = s
            .traces
            .iter()
            .filter_map(|t| match t.meta.source {
                TraceSource::Captured { epoch, .. } => Some(epoch.0),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let session = Session {
            epoch: SessionEpoch(self.state.session.epoch.0.max(newest) + 1),
            open: self.state.session.open.clone(),
            stopped: None,
        };
        self.commit(Change::Session(session));
        for sm in s.measurements {
            self.next_id = self.next_id.max(sm.id.0 + 1);
            let grid_id = match &sm.config.kind {
                MeasKind::Transfer { config } => {
                    let g = GridDef::Log {
                        ppo: config.grid.ppo,
                        k_min: config.grid.k_min,
                        k_max: config.grid.k_max,
                    };
                    let id = g.id();
                    self.grids.insert(id, g);
                    Some(id)
                }
                _ => None,
            };
            let delay = sm.delay.map(|d| DelayState {
                applied: d.applied,
                applied_samples: d.applied.0 * 48_000.0,
                tracking: d.tracking,
                awaiting_pick: false,
                last_finding: None,
            });
            let meas = Measurement {
                id: sm.id,
                config: sm.config,
                config_rev: Rev(self.rev.0 + 1),
                running: sm.running,
                frozen: sm.frozen,
                delay,
                grid_id,
            };
            self.put_meas(meas);
        }
        for t in s.traces {
            self.next_id = self.next_id.max(t.meta.id.0 + 1);
            self.grids.insert(t.grid.id(), t.grid.clone());
            if let Some(sw) = t.sweep {
                self.traces.sweeps.insert(t.meta.id, sw);
            }
            if let Some(k) = t.mic_curve {
                self.traces.mic_curves.insert(t.meta.id, k);
            }
            self.traces.data.insert(t.meta.id, (t.grid, t.columns));
            self.commit(Change::Trace(Patch::Set(t.meta)));
        }
        Ok(ReplyBody::SessionFile(Self::file_info(name, &dir, &m)))
    }

    pub(super) fn file_list(&self) -> Result<ReplyBody, ProtoError> {
        let Some(root) = self.opts.session_dir.clone() else {
            return Err(err(
                ErrorCode::Unsupported,
                "the fake has no session directory",
            ));
        };
        let l = session::list(&root).map_err(session_err)?;
        Ok(ReplyBody::Sessions(
            l.into_iter()
                .map(|(n, p, m)| Self::file_info(n, &p, &m))
                .collect(),
        ))
    }
}

/// A synthetic sweep result on `grid`: the capture response, H2 at −40 dB rising 12 dB/oct
/// below 100 Hz, H3 at −50 dB, H4 and H5 below the −80 dB floor, nothing above `f2 / k`.
fn synthetic_sweep(grid: &GridDef, f2: f64, rate: f64, repeats: u8) -> (Columns, SweepData) {
    let f = frequencies(grid);
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
    let sweep = SweepData {
        harmonics,
        thd,
        ir: SweepIr {
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
    };
    (columns, sweep)
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
