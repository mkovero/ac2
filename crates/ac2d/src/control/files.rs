//! `file.save` / `file.load` / `file.list` / `file.salvage`: sessions on the daemon host
//! (format: `ac2_traces::session`; the working state's session and recovery copy:
//! [`super::working`]; archives from and to clients: [`super::transfer`]).
//!
//! A load replaces every measurement and trace, stops and disarms the generator and drops
//! its owner, and starts a new session epoch newer than any epoch recorded in the loaded
//! traces: a trace captured in another run must never look like it shares the live time
//! base just because the epoch counters happen to match.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ac2_proto::event::{Change, Patch};
use ac2_proto::grid::GridDef;
use ac2_proto::model::{
    Compared, DelayState, GenAction, ImportFormat, ImportRole, MeasKind, MeasScope, Measurement,
    Salvage, SalvageRefusal, Session, SessionFile, SessionOrigin, SessionRef, SweepFailure,
    TraceKind, TraceOwner,
};
use ac2_proto::units::{ClientId, Db, MeasId, Rev, Seconds, SessionEpoch, TransferId, WallNs};
use ac2_proto::{ErrorCode, ErrorDetail, ProtoError, ReplyBody};
use ac2_traces::meta;
use ac2_traces::session::{
    self, Manifest, SavedDelay, SavedMeasurement, Session as SessionData, SessionError,
};

use super::file_jobs::{FileDone, Pending, ReplaceFrom, ReplaceRead, Work};
use super::{Control, check_delay, delay_samples, maths, static_grid, validate_meas};
use crate::util::{perr, perr_detail, wall_ns};

pub(super) fn session_err(e: SessionError) -> ProtoError {
    let msg = e.to_string();
    match e {
        SessionError::NotFound(_) => perr(ErrorCode::NotFound, msg),
        SessionError::Version { found, .. } => perr_detail(
            ErrorCode::Unsupported,
            msg,
            ErrorDetail::SessionVersion {
                found,
                supported: session::VERSION,
            },
        ),
        SessionError::NotASession(_) | SessionError::BadName(_) | SessionError::Corrupt { .. } => {
            perr(ErrorCode::Invalid, msg)
        }
        SessionError::Io { .. } => perr(ErrorCode::Internal, msg),
    }
}

pub(super) fn info(name: &str, dir: &Path, m: &Manifest) -> SessionFile {
    SessionFile {
        name: name.to_owned(),
        path: dir.to_string_lossy().into_owned(),
        saved_at: m.saved_at,
        measurements: u32::try_from(m.measurements.len()).unwrap_or(u32::MAX),
        traces: u32::try_from(m.traces.len()).unwrap_or(u32::MAX),
    }
}

impl Control {
    /// The directory a reference names. Paths are refused in network mode: a remote client
    /// may only use the daemon's own session directory.
    pub(super) fn session_path(&self, r: &SessionRef) -> Result<(String, PathBuf), ProtoError> {
        match r {
            SessionRef::Name { name } => {
                session::validate_name(name).map_err(session_err)?;
                Ok((name.clone(), self.s.session_dir.join(name)))
            }
            SessionRef::Path { path } => {
                if self.s.network {
                    return Err(perr(
                        ErrorCode::Refused,
                        "remote clients name sessions; paths are for local clients only",
                    ));
                }
                let p = PathBuf::from(path);
                if !p.is_absolute() {
                    return Err(perr(
                        ErrorCode::Invalid,
                        "a session path must be absolute (the daemon's working directory is not the client's)",
                    ));
                }
                let name = p
                    .file_name()
                    .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
                Ok((name, p))
            }
            SessionRef::Upload { .. } => Err(perr(
                ErrorCode::Invalid,
                "an upload is opened with file.load or salvaged with file.salvage only",
            )),
        }
    }

    /// The origin a save to, or load from, `r` gives the working state.
    fn origin_of(r: &SessionRef, dir: &Path) -> SessionOrigin {
        match r {
            SessionRef::Name { name } => SessionOrigin::Name { name: name.clone() },
            SessionRef::Path { .. } | SessionRef::Upload { .. } => SessionOrigin::Path {
                path: dir.to_string_lossy().into_owned(),
            },
        }
    }

    /// The measurements as a session saves them: the session's only, none filed under a
    /// rig meter ([`super::rig_meas`]).
    pub(super) fn saved_measurements(&self) -> Vec<SavedMeasurement> {
        self.store
            .state()
            .measurements
            .iter()
            .filter(|m| m.config.scope == MeasScope::Session)
            .map(|m| {
                let mut sm = super::rig_meas::saved_of(m);
                if let MeasKind::Math { config } = &mut sm.config.kind {
                    config.owner = self.session_owner(config.owner);
                }
                sm
            })
            .collect()
    }

    /// Everything a session holds, as of now; without the SPL logs' rows unless
    /// `with_logs` (the autosave keeps its logs on disk itself).
    pub(super) fn session_data(&self, with_logs: bool) -> Result<SessionData, ProtoError> {
        let traces = self
            .store
            .state()
            .traces
            .iter()
            .map(|t| {
                self.stored(t.id).map(|mut s| {
                    s.meta.edit.owner = self.session_owner(s.meta.edit.owner);
                    s
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SessionData {
            saved_at: WallNs(wall_ns()),
            measurements: self.saved_measurements(),
            spl_logs: if with_logs {
                self.saved_spl_logs(MeasScope::Session)
            } else {
                Vec::new()
            },
            traces,
            walks: self.store.state().walks.clone(),
            plans: self.store.state().walk_plans.clone(),
        })
    }

    /// `file.save`: the content now, written by a worker ([`super::file_jobs`]).
    pub(super) fn begin_save(
        &mut self,
        r: &SessionRef,
        overwrite: bool,
    ) -> Result<(Pending, Work), ProtoError> {
        let (name, dir) = self.session_path(r)?;
        let origin = Self::origin_of(r, &dir);
        // Its own session is saved again without asking.
        let ask = !overwrite && self.working.origin.as_ref() != Some(&origin);
        let data = self.session_data(true)?;
        let hash = self.content_hash();
        let (n, d) = (name.clone(), dir.clone());
        let work: Work = Box::new(move || {
            FileDone::Save((|| {
                if ask && let Some(p) = session::peek(&d) {
                    return Err(super::transfer::exists(&n, p.saved_at.unwrap_or_default()));
                }
                session::save(&d, &data).map_err(session_err)
            })())
        });
        Ok((
            Pending::Save {
                name,
                dir,
                origin,
                hash,
            },
            work,
        ))
    }

    pub(super) fn finish_save(
        &mut self,
        name: &str,
        dir: &Path,
        origin: SessionOrigin,
        hash: u64,
        written: Result<Manifest, ProtoError>,
    ) -> Result<ReplyBody, ProtoError> {
        let m = written?;
        tracing::info!("session saved to {}", dir.display());
        self.mark_saved_as(Some(origin), m.saved_at, hash);
        Ok(ReplyBody::SessionFile(info(name, dir, &m)))
    }

    /// `file.list`: read by a worker; it changes nothing, so it runs beside anything.
    pub(super) fn begin_list(&self) -> (Pending, Work) {
        let dir = self.s.session_dir.clone();
        let work: Work =
            Box::new(move || {
                FileDone::List(session::list(&dir).map_err(session_err).map(|l| {
                    ReplyBody::Sessions(l.iter().map(|(n, p, m)| info(n, p, m)).collect())
                }))
            });
        (Pending::List, work)
    }

    /// `file.load`: read (an upload unpacked first) and checked by a worker, committed by
    /// [`Self::finish_replace`].
    pub(super) fn begin_load(
        &mut self,
        client: &ClientId,
        r: &SessionRef,
        discard: bool,
    ) -> Result<(Pending, Work), ProtoError> {
        self.guard_unsaved(discard)?;
        let (from, src) = match r {
            SessionRef::Upload { transfer } => {
                let (file, bytes) = self.transfers.upload(client, *transfer)?;
                let src = (file, bytes.to_vec());
                let stage = self.stage_path("upload");
                (
                    ReplaceFrom::Upload {
                        transfer: *transfer,
                        file: src.0,
                        stage: stage.clone(),
                    },
                    Source::Upload(stage, src.1),
                )
            }
            _ => {
                let (name, dir) = self.session_path(r)?;
                let origin = Self::origin_of(r, &dir);
                let src = Source::Dir(dir.clone());
                (ReplaceFrom::Dir { name, dir, origin }, src)
            }
        };
        self.begin_replace(client, discard, from, src)
    }

    /// `file.new`: a replace with the empty session.
    pub(super) fn begin_new(
        &mut self,
        client: &ClientId,
        discard: bool,
    ) -> Result<(Pending, Work), ProtoError> {
        self.guard_unsaved(discard)?;
        self.begin_replace(client, discard, ReplaceFrom::Empty, Source::Empty)
    }

    /// The working state as it is goes to the recovery copy when it has unsaved changes:
    /// captured here, written by the worker after the new session read well.
    fn begin_replace(
        &mut self,
        client: &ClientId,
        discard: bool,
        from: ReplaceFrom,
        src: Source,
    ) -> Result<(Pending, Work), ProtoError> {
        let kept = self.kept()?;
        let work: Work = Box::new(move || {
            let read = src.read();
            // A session that cannot replace the working state must not take the recovery
            // copy's place.
            let kept = if read.is_ok() {
                kept.map(super::working::Kept::stage)
            } else {
                None
            };
            FileDone::Replace(ReplaceRead { read, kept })
        });
        Ok((
            Pending::Replace {
                client: client.clone(),
                discard,
                from,
                standing: self.standing(),
            },
            work,
        ))
    }

    /// Commits a `file.load` / `file.new` its worker read: only onto the working state it
    /// began on (`super::file_jobs`).
    pub(super) fn finish_replace(
        &mut self,
        client: &ClientId,
        discard: bool,
        from: ReplaceFrom,
        standing: (u64, u64),
        r: ReplaceRead,
    ) -> Result<ReplyBody, ProtoError> {
        let ReplaceRead { read, kept } = r;
        let checked = read.and_then(|(m, data)| {
            if self.standing() != standing {
                return Err(perr(
                    ErrorCode::Conflict,
                    "the working state changed while the session was being read; nothing \
                     was replaced: ask again",
                ));
            }
            self.guard_unsaved(discard)?;
            // Refused before the working state is kept aside: a load that cannot happen
            // must not take the recovery copy's place.
            check_session(&data)?;
            Ok((m, data))
        });
        let (m, data) = match checked {
            Ok(x) => x,
            Err(e) => {
                if let Some(k) = kept {
                    k.discard();
                }
                return Err(e);
            }
        };
        if let Some(k) = kept {
            self.keep(k);
        }
        let epoch = self.replace_session(Some(client), data)?;
        let read_manifest =
            |m: Option<Manifest>| m.ok_or_else(|| perr(ErrorCode::Internal, "no manifest read"));
        match from {
            ReplaceFrom::Empty => {
                self.set_working(super::working::Persist::untitled());
                tracing::info!("new untitled session");
                Ok(ReplyBody::Ack {
                    rev: self.store.rev(),
                })
            }
            ReplaceFrom::Dir { name, dir, origin } => {
                let m = read_manifest(m)?;
                tracing::info!(
                    "session loaded from {} (epoch {}, disarmed)",
                    dir.display(),
                    epoch.0
                );
                self.mark_saved(Some(origin), m.saved_at);
                Ok(ReplyBody::SessionFile(info(&name, &dir, &m)))
            }
            ReplaceFrom::Upload {
                transfer,
                file,
                stage,
            } => {
                let m = read_manifest(m)?;
                self.transfers.remove(transfer);
                tracing::info!("session loaded from {file} (epoch {}, disarmed)", epoch.0);
                self.mark_saved(
                    Some(SessionOrigin::Client { path: file.clone() }),
                    m.saved_at,
                );
                let mut f = info(super::transfer::file_part(&file), &stage, &m);
                f.path = file;
                Ok(ReplyBody::SessionFile(f))
            }
        }
    }

    /// `file.salvage`: each trace file of the session through `trace.import`'s reader. A
    /// worker reads the files (an upload unpacked first); [`Self::finish_salvage`] imports.
    pub(super) fn begin_salvage(
        &mut self,
        client: &ClientId,
        r: &SessionRef,
    ) -> Result<(Pending, Work), ProtoError> {
        let (transfer, src) = match r {
            SessionRef::Upload { transfer } => {
                let bytes = self.transfers.upload(client, *transfer)?.1.to_vec();
                (
                    Some(*transfer),
                    Source::Upload(self.stage_path("upload"), bytes),
                )
            }
            _ => (None, Source::Dir(self.session_path(r)?.1)),
        };
        let work: Work = Box::new(move || FileDone::Salvage(src.read_traces()));
        Ok((Pending::Salvage { transfer }, work))
    }

    pub(super) fn finish_salvage(
        &mut self,
        transfer: Option<TransferId>,
        read: Result<SalvageRead, ProtoError>,
    ) -> Result<ReplyBody, ProtoError> {
        let SalvageRead {
            dir,
            version,
            files,
        } = read?;
        let mut imported = Vec::new();
        let mut refused = Vec::new();
        for (rel, content) in files {
            let content = match content {
                Ok(c) => c,
                Err(reason) => {
                    refused.push(SalvageRefusal { file: rel, reason });
                    continue;
                }
            };
            // A target stays a target; everything else comes back as a measured trace.
            let role = if ac2_traces::text::header_kind(&content) == Some(TraceKind::Target) {
                ImportRole::Target
            } else {
                ImportRole::Trace
            };
            match self.trace_import(
                rel.clone(),
                ImportFormat::Ac2Csv,
                role,
                None,
                &content,
                TraceOwner::Imported,
            ) {
                Ok(ReplyBody::Trace(t)) => imported.push(t),
                Ok(_) => {}
                Err(e) => refused.push(SalvageRefusal {
                    file: rel,
                    reason: e.msg,
                }),
            }
        }
        if let Some(t) = transfer {
            self.transfers.remove(t);
        }
        tracing::info!(
            "salvaged {} traces from {} ({} refused)",
            imported.len(),
            dir,
            refused.len()
        );
        Ok(ReplyBody::Salvage(Salvage {
            version,
            imported,
            refused,
        }))
    }

    /// Replaces every session measurement and every trace with `data`, disarmed (module
    /// docs); the rig meters go on ([`super::rig_meas`]). Returns the new session epoch.
    pub(super) fn replace_session(
        &mut self,
        client: Option<&ClientId>,
        mut data: SessionData,
    ) -> Result<SessionEpoch, ProtoError> {
        // Everything is checked before anything changes: a refused load leaves the state
        // as it was.
        self.prepare_load(&mut data)?;
        let mut grids = check_session(&data)?;
        let ids: Vec<MeasId> = data.measurements.iter().map(|m| m.id).collect();
        super::walks::check_walks(&data.walks, &data.plans, &ids)?;

        // Disarmed, no owner; a sweep run in progress is discarded.
        self.abort_sweep(SweepFailure::Stopped, "a session was loaded");
        self.stop_output();
        self.out.lease = None;
        let mut g = self.store.state().generator.clone();
        if g.owner.is_some() || g.armed || g.firing {
            g.owner = None;
            g.armed = false;
            g.firing = false;
            self.audit(&mut g, GenAction::Stop, client);
            self.commit(Change::Generator(g));
        }

        // Out with the old; the rig meters, their jobs and logs stay.
        let old_meas: Vec<MeasId> = self
            .store
            .state()
            .measurements
            .iter()
            .filter(|m| m.config.scope == MeasScope::Session)
            .map(|m| m.id)
            .collect();
        for id in old_meas {
            self.stop_job(id);
            self.drop_spl_log(id);
            self.commit(Change::Measurement(Patch::Deleted(id)));
        }
        self.clear_walks();
        let old_traces: Vec<_> = self.store.state().traces.iter().map(|t| t.id).collect();
        for id in old_traces {
            self.commit(Change::Trace(Patch::Deleted(id)));
        }
        self.traces.clear();

        // A new epoch, newer than every epoch the loaded traces were captured in.
        let newest = data
            .traces
            .iter()
            .filter_map(|t| t.meta.source.shared_epoch().map(|e| e.0))
            .max()
            .unwrap_or(0);
        let epoch = SessionEpoch(self.epoch().0.max(newest) + 1);
        if self.session.is_some() {
            // The stream reopens without generator routes: nothing is armed after a load.
            // A device that fails to reopen leaves the session open with its audio stopped
            // and being reopened (`reopen_at`); the saved state still loads.
            if let Err(e) = self.reopen_at(&[], epoch) {
                tracing::warn!("session load: the audio stream did not reopen: {}", e.msg);
            }
        } else if self.audio_recovery.is_some() {
            // The session's audio is stopped: it stays open and keeps being reopened, now in
            // the loaded state's epoch.
            self.rebase_recovery(epoch);
        } else {
            self.commit(Change::Session(Session {
                epoch,
                open: None,
                stopped: None,
            }));
        }

        // In with the new: the SPL logs first, so a meter's job carries on from its log.
        for l in data.spl_logs {
            self.set_spl_log(
                l.info.meas,
                crate::leq_log::LeqLog::from_rows(l.rows, l.bands),
            );
        }
        // Traces first: a math channel's stored operands must be there when it starts.
        for t in data.traces {
            self.register_grid(t.grid.clone());
            self.traces
                .insert(t.meta.id, t.grid, t.columns, t.sweep, t.ir, t.mic_curve);
            self.commit(Change::Trace(Patch::Set(t.meta)));
        }
        let fs = self.session.as_ref().map(|r| f64::from(r.sample_rate));
        let mut loaded = Vec::with_capacity(data.measurements.len());
        for sm in data.measurements {
            self.next_meas = self.next_meas.max(sm.id.0.saturating_add(1));
            let delay = matches!(sm.config.kind, MeasKind::Transfer { .. }).then(|| {
                let d = sm.delay.unwrap_or(SavedDelay {
                    applied: Seconds(0.0),
                    nudged: Seconds(0.0),
                    tracking: false,
                });
                let samples = fs.map_or(0.0, |fs| delay_samples(d.applied.0, fs));
                let nudged = fs.map_or(0.0, |fs| delay_samples(d.nudged.0, fs));
                DelayState {
                    applied: fs.map_or(d.applied, |fs| Seconds(samples / fs)),
                    applied_samples: samples,
                    nudged: fs.map_or(d.nudged, |fs| Seconds(nudged / fs)),
                    nudged_samples: nudged,
                    tracking: d.tracking,
                    awaiting_pick: false,
                    last_finding: None,
                }
            });
            let grid_id = grids.remove(&sm.id).map(|g| self.register_grid(g));
            let running = sm.running && sm.config.kind.is_job();
            // A math channel takes its owner's family; an index no theme has is no choice.
            let colour = sm
                .colour
                .filter(|c| c.is_valid() && !matches!(sm.config.kind, MeasKind::Math { .. }));
            // A layout no client could draw is no layout.
            let layout = sm.layout.filter(|l| l.check().is_ok());
            // A list naming itself, an entry twice or past the cap is cut to what
            // `meas.compare` takes; entries already gone are pruned once all is loaded.
            let mut compared: Vec<Compared> = Vec::new();
            for c in sm.compared {
                if c != (Compared::Meas { meas: sm.id })
                    && !compared.contains(&c)
                    && compared.len() < Compared::MAX
                {
                    compared.push(c);
                }
            }
            let mut meas = Measurement {
                id: sm.id,
                config: sm.config,
                config_rev: Rev(self.store.rev().0 + 1),
                // A sweep measurement never runs on its own: only `sweep.run` plays it.
                running,
                delay,
                grid_id,
                colour,
                layout,
                compared,
                // One no command could set is none.
                offset: Some(sm.offset)
                    .filter(|o| Measurement::offset_ok(*o))
                    .unwrap_or(Db(0.0)),
                eq: sm.eq,
            };
            self.ensure_spl_log(&meas, None);
            meas.config_rev = Rev(self.store.rev().0 + 1);
            self.commit(Change::Measurement(Patch::Set(meas.clone())));
            loaded.push(meas);
        }
        // The EQ curves are the daemon's: recomputed from their sets, not trusted from the
        // file.
        for m in &loaded {
            self.eq_refresh(m.id);
        }
        // Then the jobs, once every measurement is there: a math channel looks its live
        // operands up whatever their order.
        if self.session.is_some() {
            for mut meas in loaded.into_iter().filter(|m| m.running) {
                match self.start_job(&meas) {
                    Ok(Some(g)) => {
                        let id = Some(self.register_grid(g));
                        if id != meas.grid_id {
                            meas.grid_id = id;
                            self.commit(Change::Measurement(Patch::Set(meas)));
                        }
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!("measurement {} not started: {}", meas.id, e.msg),
                }
            }
        }
        self.load_walks(data.walks, data.plans);
        self.prune_compared();
        Ok(epoch)
    }
}

/// Checks `data` before a load replaces anything with it: everything the live commands
/// would refuse is refused here too. Returns the grid of each measurement that has one before
/// a session runs; a transfer math channel's is its operands' (checked against the loaded
/// measurements and traces).
pub(super) fn check_session(data: &SessionData) -> Result<HashMap<MeasId, GridDef>, ProtoError> {
    let mut grids: HashMap<MeasId, GridDef> = HashMap::new();
    for sm in &data.measurements {
        validate_meas(&sm.config)?;
        // A saved delay meets the limits `delay.set` and `delay.nudge` enforce.
        if let Some(d) = &sm.delay {
            check_delay(&format!("measurement {} saved delay", sm.id), d.applied)?;
            check_delay(&format!("measurement {} saved nudge", sm.id), d.nudged)?;
        }
        if data.measurements.iter().filter(|o| o.id == sm.id).count() > 1 {
            return Err(perr(
                ErrorCode::Invalid,
                format!("measurement {} appears twice in the session", sm.id),
            ));
        }
        let grid = match &sm.config.kind {
            MeasKind::Math { config } => {
                let meas = |id| {
                    data.measurements
                        .iter()
                        .find(|o: &&SavedMeasurement| o.id == id)
                        .map(|o| (o.config.name.as_str(), &o.config.kind))
                };
                let trace = |id| {
                    data.traces
                        .iter()
                        .find(|t: &&ac2_traces::StoredTrace| t.meta.id == id)
                        .map(|t| (&t.meta, t.grid.clone()))
                };
                let eq = |id, set| {
                    data.measurements
                        .iter()
                        .any(|o: &SavedMeasurement| o.id == id && o.eq.iter().any(|s| s.id == set))
                };
                maths::check_operands(
                    Some(sm.id),
                    config,
                    &maths::Lookup {
                        meas: &meas,
                        trace: &trace,
                        eq: &eq,
                        epoch: None,
                    },
                )?
            }
            k => static_grid(k),
        };
        if let Some(g) = grid {
            grids.insert(sm.id, g);
        }
    }
    // EQ sets: ids unique in the session, each curve one of its traces.
    let mut set_ids = std::collections::HashSet::new();
    for sm in &data.measurements {
        for s in &sm.eq {
            if !set_ids.insert(s.id) {
                return Err(perr(
                    ErrorCode::Invalid,
                    format!("EQ set {} appears twice in the session", s.id.0),
                ));
            }
            if s.body.sections.len() > ac2_proto::model::EqSet::MAX_SECTIONS
                || !data.traces.iter().any(|t| t.meta.id == s.trace)
            {
                return Err(perr(
                    ErrorCode::Invalid,
                    format!(
                        "EQ set {} of measurement {} is not one ac2d keeps",
                        s.id.0, sm.id
                    ),
                ));
            }
        }
    }
    // Every owner is a measurement of the session that can own (not a math channel).
    let owner_ok = |o: TraceOwner| match o.meas() {
        None => true,
        Some(id) => data
            .measurements
            .iter()
            .any(|m| m.id == id && !matches!(m.config.kind, MeasKind::Math { .. })),
    };
    let owners = data
        .traces
        .iter()
        .map(|t| (format!("trace {}", t.meta.id), t.meta.edit.owner))
        .chain(
            data.measurements
                .iter()
                .filter_map(|m| match &m.config.kind {
                    MeasKind::Math { config } => {
                        Some((format!("math channel {}", m.id), config.owner))
                    }
                    _ => None,
                }),
        );
    for (what, owner) in owners {
        if !owner_ok(owner) {
            return Err(perr(
                ErrorCode::Invalid,
                format!("{what} is filed under a measurement the session does not hold"),
            ));
        }
    }
    // An SPL log belongs to an SPL meter of the session, one log each, and holds rows the
    // meter could have written: a log is the record of a meter, never kept without one.
    for l in &data.spl_logs {
        let id = l.info.meas;
        match data.measurements.iter().find(|m| m.id == id) {
            None => {
                return Err(perr(
                    ErrorCode::Invalid,
                    format!("SPL log of measurement {id}, which the session does not hold"),
                ));
            }
            Some(m) if !matches!(m.config.kind, MeasKind::Spl { .. }) => {
                return Err(perr(
                    ErrorCode::Invalid,
                    format!("SPL log of measurement {id}, which is not an SPL meter"),
                ));
            }
            Some(_) => {}
        }
        if data.spl_logs.iter().filter(|o| o.info.meas == id).count() > 1 {
            return Err(perr(
                ErrorCode::Invalid,
                format!("measurement {id} has two SPL logs"),
            ));
        }
        let row = |i: usize, e: &str| {
            perr(
                ErrorCode::Invalid,
                format!("SPL log of measurement {id}, row {}: {e}", i + 1),
            )
        };
        for (i, r) in l.rows.iter().enumerate() {
            ac2_proto::spl_log::check_row(r).map_err(|e| row(i, e))?;
        }
        for (i, r) in l.bands.iter().enumerate() {
            ac2_traces::band_log::check_row(r).map_err(|e| row(i, e))?;
        }
    }
    for t in &data.traces {
        // Data a crafted file could hold that no capture produces (columns off the grid,
        // non-finite values, an unbounded impulse response).
        ac2_traces::check::trace(t)
            .map_err(|e| perr(ErrorCode::Invalid, format!("trace {}: {e}", t.meta.id)))?;
        // A saved edit meets the limits `trace.update` enforces.
        meta::check_values(&t.meta.edit)
            .map_err(|e| perr(ErrorCode::Invalid, format!("trace {}: {e}", t.meta.id)))?;
        if data
            .traces
            .iter()
            .filter(|o| o.meta.id == t.meta.id)
            .count()
            > 1
        {
            return Err(perr(
                ErrorCode::Invalid,
                format!("trace {} appears twice in the session", t.meta.id),
            ));
        }
    }
    Ok(grids)
}

/// What a replace reads its new working state from, on the worker.
pub(super) enum Source {
    Dir(PathBuf),
    /// An upload's archive, unpacked into the staging directory first.
    Upload(PathBuf, Vec<u8>),
    Empty,
}

/// What `file.salvage`'s worker read: where from (for the log), the session's format
/// version, and each trace file (`traces/<file>`) with its content or why it was not read.
pub(crate) struct SalvageRead {
    dir: String,
    version: Option<u32>,
    files: Vec<(String, Result<Vec<u8>, String>)>,
}

/// The trace files of session directory `dir`. Disk work.
fn read_traces(dir: &Path) -> Result<SalvageRead, ProtoError> {
    if !dir.join(session::MANIFEST).is_file() {
        return Err(session_err(SessionError::NotASession(dir.to_owned())));
    }
    let version = session::peek(dir).map(|p| p.version);
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(dir.join("traces")) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("csv")) && !p.is_dir())
            .collect(),
        Err(_) => Vec::new(),
    };
    paths.sort();
    let files = paths
        .into_iter()
        .map(|p| {
            let rel = format!(
                "traces/{}",
                p.file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default()
            );
            (rel, std::fs::read(&p).map_err(|e| e.to_string()))
        })
        .collect();
    Ok(SalvageRead {
        dir: dir.display().to_string(),
        version,
        files,
    })
}

impl Source {
    /// `file.salvage`'s read (an upload unpacked into the staging directory first, removed
    /// after). Disk work.
    fn read_traces(self) -> Result<SalvageRead, ProtoError> {
        match self {
            Self::Dir(dir) => read_traces(&dir),
            Self::Upload(stage, bytes) => {
                let r =
                    super::transfer::unpack_into(&bytes, &stage).and_then(|()| read_traces(&stage));
                super::transfer::remove_dir(&stage);
                r
            }
            Self::Empty => Err(perr(ErrorCode::Internal, "nothing to salvage")),
        }
    }

    /// The session (with its manifest), checked as a load checks it. Disk work.
    fn read(self) -> Result<(Option<Manifest>, SessionData), ProtoError> {
        let read_dir = |dir: &Path| {
            let m = session::read_manifest(dir).map_err(session_err)?;
            let data = session::load(dir).map_err(session_err)?;
            check_session(&data)?;
            Ok((Some(m), data))
        };
        match self {
            Self::Dir(dir) => read_dir(&dir),
            Self::Upload(stage, bytes) => {
                let r =
                    super::transfer::unpack_into(&bytes, &stage).and_then(|()| read_dir(&stage));
                super::transfer::remove_dir(&stage);
                r
            }
            Self::Empty => Ok((
                None,
                SessionData {
                    saved_at: WallNs(wall_ns()),
                    measurements: Vec::new(),
                    spl_logs: Vec::new(),
                    traces: Vec::new(),
                    walks: Vec::new(),
                    plans: Vec::new(),
                },
            )),
        }
    }
}
