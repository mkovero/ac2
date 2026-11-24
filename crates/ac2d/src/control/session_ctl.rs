//! Session commands: open, close, reopen, preview, detection, and per-input calibration refresh.

use std::sync::Arc;
use std::time::Instant;

use ac2_audio::Backend;
use ac2_proto::event::{Change, Patch};
use ac2_proto::model::{
    Availability, BackendInfo, GenAction, Measurement, Session, SessionConfig, SweepFailure,
};
use ac2_proto::units::{Dbfs, LeaseToken, Rev, Seconds, SessionEpoch};
use ac2_proto::{ProtoError, ReplyBody};

use crate::conv;
use crate::preview::Preview;
use crate::route::Peer;
use crate::session::{self, Runtime};
use crate::util::{internal, invalid, missing, refused};

use super::{
    Control, ControlMsg, PendingReply, PreviewKey, PreviewOpening, delay_samples, recovery,
};

impl Control {
    pub(super) fn session_open(
        &mut self,
        client: &Peer,
        config: SessionConfig,
    ) -> Result<ReplyBody, ProtoError> {
        if config.backend == Some(ac2_proto::model::BackendKind::Replay) {
            return Err(invalid("a recording is played with session.replay"));
        }
        // Refused when the operator opens the session only: one the daemon reopens on its
        // own (monitor at start, after a power cut) keeps its remote inputs, and the joiner
        // reports why one gives no audio.
        self.check_remote_inputs(&config)?;
        self.open_session(client, config, None)
            .map(ReplyBody::Session)
    }

    /// Opens a session on `config` (a replay of `replay`, on the replay backend).
    pub(super) fn open_session(
        &mut self,
        client: &Peer,
        config: SessionConfig,
        replay: Option<ac2_proto::model::ReplayInfo>,
    ) -> Result<Session, ProtoError> {
        session::validate(&config)?;
        let backend = self.backend_for(config.backend)?;
        let blocked = Instant::now();
        // The preview may hold the very device the session is about to open.
        self.close_preview();
        if self.session.is_some() || self.audio_recovery.is_some() {
            self.session_close(client);
        }
        let epoch = SessionEpoch(self.epoch().0 + 1);
        let rt = Runtime::open(
            &*backend,
            &config,
            &[],
            self.s.limits(),
            epoch,
            self.s.to_self.clone(),
            self.s.fps,
            self.replay_marks.clone(),
        );
        self.excuse_blocked(blocked);
        let mut rt = match rt {
            Ok(rt) => rt,
            Err(e) => {
                tracing::warn!("session open failed: {}", e.msg);
                return Err(e);
            }
        };
        rt.open.replay = replay;
        rt.link_spares(&self.spare_links);
        let s = Session {
            epoch,
            open: Some(rt.open.clone()),
            stopped: None,
        };
        self.session = Some(rt);
        self.commit(Change::Session(s.clone()));
        self.after_open();
        self.release_replay();
        Ok(s)
    }

    /// Stops what runs on a stream about to close: the sweep, the jobs, the generator source
    /// and the published frames of the session.
    pub(super) fn wind_down(&mut self, recording: ac2_proto::model::RecordingEnd) {
        self.abort_runs(SweepFailure::SessionClosed, "the audio I/O reopened");
        self.end_recording(recording);
        self.stop_all_jobs();
        self.out.level = None;
        self.out.source = None;
        self.s.outbox.clear(b"d/");
        self.s.outbox.clear(b"timing");
        self.s.outbox.clear(b"session/levels");
    }

    /// Starts jobs and re-derives sample-rate dependent state for a freshly opened stream.
    pub(super) fn after_open(&mut self) {
        // A stream opened on another thread (recovery) may predate a change of the system
        // max level.
        if let Some(rt) = &self.session {
            rt.gen_handle.set_max_level(self.s.max_level);
        }
        self.remote_attach();
        let Some(fs) = self.session.as_ref().map(|r| f64::from(r.sample_rate)) else {
            return;
        };
        let ms: Vec<Measurement> = self.store.state().measurements.clone();
        for mut m in ms {
            let mut changed = false;
            if let Some(d) = &mut m.delay {
                let samples = delay_samples(d.applied.0, fs);
                let nudged = delay_samples(d.nudged.0, fs);
                if samples != d.applied_samples || nudged != d.nudged_samples {
                    d.applied_samples = samples;
                    d.applied = Seconds(samples / fs);
                    d.nudged_samples = nudged;
                    d.nudged = Seconds(nudged / fs);
                    m.config_rev = Rev(self.store.rev().0 + 1);
                    changed = true;
                }
            }
            if m.running {
                match self.start_job(&m) {
                    Ok(Some(g)) => {
                        let id = self.register_grid(g);
                        if m.grid_id != Some(id) {
                            m.grid_id = Some(id);
                            changed = true;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!("measurement {} not started: {}", m.id.0, e.msg),
                }
            }
            if changed {
                self.commit(Change::Measurement(Patch::Set(m)));
            }
        }
        self.resume_walks();
        self.start_timing_job();
        self.start_session_levels();
    }

    pub(super) fn session_close(&mut self, client: &Peer) {
        self.abort_runs(SweepFailure::SessionClosed, "the audio I/O closed");
        self.end_recording(ac2_proto::model::RecordingEnd::SessionClosed);
        self.stop_sim(ac2_proto::model::SimCancel::SessionClosed);
        self.stop_all_jobs();
        let g = self.store.state().generator.clone();
        if g.armed || g.firing {
            self.stop_output();
            let mut g = g;
            g.armed = false;
            g.firing = false;
            self.audit(&mut g, GenAction::Stop, Some(client));
            self.commit(Change::Generator(g));
        } else {
            self.stop_output();
        }
        // A close ends the attempts to reopen a stopped session too; an attempt still
        // running closes what it opens.
        let recovering = self.audio_recovery.take().is_some();
        let rt = self.session.take();
        let had = rt.is_some() || recovering;
        if let Some(rt) = rt {
            self.keep_spare_links(&rt);
            recovery::close_bounded(rt, recovery::CLOSE_BOUND);
        }
        if had {
            self.health.session_closed();
            self.s.outbox.clear(b"d/");
            self.s.outbox.clear(b"timing");
            self.s.outbox.clear(b"session/levels");
            let epoch = SessionEpoch(self.epoch().0 + 1);
            self.commit(Change::Session(Session {
                epoch,
                open: None,
                stopped: None,
            }));
        }
    }

    /// Remembers what feeds `rt`'s spare inputs before the daemon closes it. A stream that
    /// cannot say (its host gone) leaves the last reading in place.
    pub(super) fn keep_spare_links(&mut self, rt: &Runtime) {
        if let Some(l) = rt.spare_links() {
            self.spare_links = l;
        }
    }

    /// Reopens the stream with the same configuration and the given generator routes: a
    /// device change is a configuration change, so it starts a new session epoch (5b).
    pub(super) fn reopen(&mut self, routes: &[u16]) -> Result<(), ProtoError> {
        let epoch = SessionEpoch(self.epoch().0 + 1);
        self.reopen_at(routes, epoch)
    }

    /// [`Self::reopen`] into a given (newer) epoch.
    pub(super) fn reopen_at(
        &mut self,
        routes: &[u16],
        epoch: SessionEpoch,
    ) -> Result<(), ProtoError> {
        let Some(rt) = self.session.take() else {
            return Err(invalid("no open session"));
        };
        let config = rt.open.config.clone();
        let replay = rt.open.replay.clone();
        let last_open = rt.open.clone();
        let backend = match self.backend_for(Some(rt.open.backend)) {
            Ok(b) => b,
            Err(e) => {
                self.session = Some(rt);
                return Err(e);
            }
        };
        let blocked = Instant::now();
        self.wind_down(ac2_proto::model::RecordingEnd::SessionReopened);
        self.keep_spare_links(&rt);
        recovery::close_bounded(rt, recovery::CLOSE_BOUND);
        let rt = Runtime::open(
            &*backend,
            &config,
            routes,
            self.s.limits(),
            epoch,
            self.s.to_self.clone(),
            self.s.fps,
            self.replay_marks.clone(),
        );
        self.excuse_blocked(blocked);
        match rt {
            Ok(mut rt) => {
                rt.open.replay = replay;
                rt.link_spares(&self.spare_links);
                let s = Session {
                    epoch,
                    open: Some(rt.open.clone()),
                    stopped: None,
                };
                self.session = Some(rt);
                self.commit(Change::Session(s));
                self.after_open();
                self.release_replay();
                Ok(())
            }
            Err(e) => {
                // The session stays open, its audio stopped, and the attempts carry on.
                self.reopen_failed(last_open, routes.to_vec(), epoch, &e);
                Err(e)
            }
        }
    }

    // -- backends, preview, loopback detection ----------------------------------------------

    /// The open input device's channel names as its backend reports them (empty when it
    /// reports none).
    pub(super) fn input_channel_names(&self, rt: &Runtime) -> Vec<String> {
        self.backend_for(Some(rt.open.backend))
            .ok()
            .and_then(|b| b.enumerate().ok())
            .and_then(|devs| {
                devs.into_iter()
                    .find(|d| d.id.0 == rt.open.input_device.0)
                    .and_then(|d| d.input.and_then(|i| i.channel_names))
            })
            .unwrap_or_default()
    }

    /// The backend `kind` names; `None` = the default one.
    pub(super) fn backend_for(
        &self,
        kind: Option<ac2_proto::model::BackendKind>,
    ) -> Result<Arc<dyn Backend>, ProtoError> {
        match kind {
            Some(ac2_proto::model::BackendKind::Replay) => {
                self.replay_backend.clone().map(|b| b as Arc<dyn Backend>)
            }
            None => self.s.backends.first().cloned(),
            Some(k) => self
                .s
                .backends
                .iter()
                .find(|b| conv::backend_kind(b.kind()) == k)
                .cloned(),
        }
        .ok_or_else(|| missing(format!("this daemon offers no {kind:?} backend")))
    }

    /// Starts `session.devices` on its own thread: a driver may take its time to list its
    /// devices. Answered with [`ControlMsg::Devices`].
    pub(super) fn start_devices(&mut self) -> Result<u64, ProtoError> {
        let backends = self.s.backends.clone();
        let token = self.ids.token();
        let to = self.s.to_self.clone();
        std::thread::Builder::new()
            .name("ac2d-devices".into())
            .spawn(move || {
                let backends = backend_infos(&backends);
                let _ = to.send(ControlMsg::Devices { token, backends });
            })
            .map_err(|e| internal(format!("cannot list the devices: {e}")))?;
        Ok(token)
    }

    /// Closes the preview, and one still opening as soon as it has: whatever comes next
    /// needs its device. Waits for an opening one, at most [`PREVIEW_OPEN_WAIT`].
    pub(super) fn close_preview(&mut self) {
        if let Some(o) = self.pending.preview.take() {
            let r = o
                .result
                .recv_timeout(PREVIEW_OPEN_WAIT)
                .unwrap_or_else(|_| Err(internal("the preview did not open in time")));
            self.preview_settled(o, r, true);
        }
        if let Some(p) = self.preview.take() {
            p.close();
            self.s.outbox.clear(b"session/preview");
        }
    }

    /// `session.preview_stop`: the preview closes; one still opening closes once it has,
    /// without waiting for it here.
    pub(super) fn stop_preview(&mut self) {
        if let Some(o) = &mut self.pending.preview {
            o.stopped = true;
        }
        if let Some(p) = self.preview.take() {
            p.close();
            self.s.outbox.clear(b"session/preview");
        }
    }

    /// `session.preview`: the running preview renewed (`Some`), or `waiter` answered once
    /// the device opens on its own thread (`None`).
    pub(super) fn start_preview(
        &mut self,
        key: PreviewKey,
        waiter: PendingReply,
    ) -> Result<Option<ReplyBody>, ProtoError> {
        if self.pending.detecting.is_some() {
            // The burst holds the device; a preview asked for before the detection started
            // must not reopen it under the burst.
            return Err(refused(
                "a loopback detection holds the device; the meters return with its result",
            ));
        }
        if let Some(o) = &mut self.pending.preview {
            if o.key != key {
                return Err(refused(
                    "another preview is still opening; ask again once it has answered",
                ));
            }
            o.stopped = false;
            o.waiters.push(waiter);
            return Ok(None);
        }
        // The declared kinds and pairs travel in every frame's meta, fixed when the stream
        // opens: a changed declaration reopens the preview like another device would.
        if let Some(p) = &mut self.preview
            && p.backend == key.backend
            && p.device == key.device
            && p.meta.inputs == key.inputs
            && p.meta.pairs == key.pairs
        {
            match p.dead(Instant::now()) {
                None => {
                    p.renew();
                    return Ok(Some(ReplyBody::Preview(p.wire())));
                }
                Some(why) => {
                    tracing::warn!(
                        "preview of {:?} {:?}: {why}; reopening it",
                        key.backend,
                        key.device.0
                    );
                }
            }
        }
        let backend = self.backend_for(Some(key.backend))?;
        self.close_preview();
        let token = self.ids.token();
        let (tx, rx) = std::sync::mpsc::channel();
        let to = self.s.to_self.clone();
        let what = (key.backend, key.device.clone());
        let (inputs, pairs) = (key.inputs.clone(), key.pairs.clone());
        let (max_level, env, levels) = (
            self.s.max_level,
            self.env_at(self.epoch()),
            Arc::clone(&self.levels),
        );
        std::thread::Builder::new()
            .name("ac2d-preview-open".into())
            .spawn(move || {
                let r = Preview::open(&*backend, what, inputs, pairs, max_level, env, levels);
                // The result first: the message only says it is there.
                let _ = tx.send(r);
                let _ = to.send(ControlMsg::PreviewOpened { token });
            })
            .map_err(|e| internal(format!("cannot open the preview: {e}")))?;
        self.pending.preview = Some(PreviewOpening {
            token,
            key,
            waiters: vec![waiter],
            stopped: false,
            result: rx,
        });
        Ok(None)
    }

    /// The preview opening under `token` reported back.
    pub(super) fn preview_opened(&mut self, token: u64) {
        let Some(o) = self.pending.preview.take_if(|o| o.token == token) else {
            return;
        };
        let r = o
            .result
            .try_recv()
            .unwrap_or_else(|_| Err(internal("the preview opening ended without a result")));
        self.preview_settled(o, r, false);
    }

    /// An opening preview's result `r`: kept and sent to every waiter, or closed when it was
    /// stopped (or `closing`) meanwhile.
    fn preview_settled(
        &mut self,
        o: PreviewOpening,
        r: Result<Preview, ProtoError>,
        closing: bool,
    ) {
        let r = match r {
            Ok(p) if o.stopped || closing => {
                p.close();
                Err(refused("the preview was stopped before its device opened"))
            }
            Ok(p) => {
                let wire = p.wire();
                self.preview = Some(p);
                Ok(ReplyBody::Preview(wire))
            }
            Err(e) => Err(e),
        };
        let now = Instant::now();
        for w in o.waiters {
            self.answer(&w.route, &w.client, w.id, r.clone(), now);
        }
    }

    /// Checks a `session.detect_loopback` and starts it on its own thread.
    pub(super) fn start_detect(
        &mut self,
        client: &Peer,
        token: LeaseToken,
        kind: ac2_proto::model::BackendKind,
        (input_device, output_device): (ac2_proto::model::DeviceId, ac2_proto::model::DeviceId),
        output: u16,
        level: Option<Dbfs>,
    ) -> Result<u64, ProtoError> {
        self.check_lease(Instant::now());
        self.lease_check(client, token)?;
        let Some(level) = level else {
            return Err(refused(
                "type the burst level: loopback detection has no default level",
            ));
        };
        if !level.0.is_finite() {
            return Err(invalid("level must be finite"));
        }
        if level.0 > self.s.ceiling_dbfs {
            return Err(refused(format!(
                "{:.1} dBFS is above the global maximum {:.1} dBFS",
                level.0, self.s.ceiling_dbfs
            )));
        }
        let g = &self.store.state().generator;
        if g.armed || g.firing {
            return Err(refused(
                "the stimulus is armed: stop it before detecting the loopback",
            ));
        }
        if self.pending.detecting.is_some() {
            return Err(refused("a loopback detection is already running"));
        }
        let backend = self.backend_for(Some(kind))?;
        self.close_preview();
        let token = self.ids.token();
        let req = crate::detect::DetectRequest {
            backend,
            kind,
            input_device,
            output_device,
            output,
            level_dbfs: level.0,
            ceiling_dbfs: self.s.ceiling_dbfs,
            max_level: self.s.max_level,
        };
        tracing::info!(
            target: "ac2d::audit",
            "loopback detection on output {} by {}",
            output + 1,
            client.id().0
        );
        let to = self.s.to_self.clone();
        std::thread::Builder::new()
            .name("ac2d-detect".into())
            .spawn(move || {
                let result = crate::detect::run(&req);
                let _ = to.send(ControlMsg::LoopbackDetected {
                    token,
                    result: Box::new(result),
                });
            })
            .map_err(|e| internal(format!("cannot start detection: {e}")))?;
        Ok(token)
    }

    /// Opens `device`'s driver control panel on a thread of its own, and answers once the
    /// driver returns or after [`PANEL_WAIT`] while a modal panel stays open: the call
    /// blocks for as long as such a dialog shows, and the client is not kept waiting on it.
    pub(super) fn start_panel(
        &mut self,
        kind: ac2_proto::model::BackendKind,
        device: ac2_proto::model::DeviceId,
    ) -> Result<u64, ProtoError> {
        let backend = self.backend_for(Some(kind))?;
        let token = self.ids.token();
        let selector = ac2_audio::DeviceSelector::Id(ac2_audio::DeviceId(device.0.clone()));
        tracing::info!("driver control panel of {:?} ({kind:?})", device.0);
        let to = self.s.to_self.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("ac2d-panel".into())
            .spawn(move || {
                let _ = tx.send(backend.control_panel(&selector));
            })
            .map_err(|e| internal(format!("cannot open the panel: {e}")))?;
        std::thread::Builder::new()
            .name("ac2d-panel-wait".into())
            .spawn(move || {
                let result = match rx.recv_timeout(PANEL_WAIT) {
                    Ok(r) => r.map_err(session::audio_err),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(()),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        Err(internal("the control panel call ended without an answer"))
                    }
                };
                let _ = to.send(ControlMsg::PanelShown { token, result });
            })
            .map_err(|e| internal(format!("cannot open the panel: {e}")))?;
        Ok(token)
    }
}

/// Every backend with its devices, or why it offers none.
fn backend_infos(backends: &[Arc<dyn Backend>]) -> Vec<BackendInfo> {
    backends
        .iter()
        .map(|b| {
            let (availability, devices) = match b.enumerate() {
                Ok(d) => (
                    Availability::Available,
                    d.iter().map(conv::device_info).collect(),
                ),
                Err(ac2_audio::AudioError::Unavailable { reason, .. }) => (
                    Availability::Unavailable {
                        reason: reason.to_string(),
                    },
                    Vec::new(),
                ),
                Err(e) => (
                    Availability::Unavailable {
                        reason: e.to_string(),
                    },
                    Vec::new(),
                ),
            };
            BackendInfo {
                kind: conv::backend_kind(b.kind()),
                description: crate::backend::describe(b.kind()),
                availability,
                devices,
            }
        })
        .collect()
}

/// How long closing the preview waits for one still opening before it goes on: what comes
/// next needs the device, and a driver that has not opened it by then is not going to.
const PREVIEW_OPEN_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long `session.control_panel` waits for the driver before answering that its panel
/// is showing: within a client's reply deadline (1.5 s per try).
const PANEL_WAIT: std::time::Duration = std::time::Duration::from_millis(1200);
