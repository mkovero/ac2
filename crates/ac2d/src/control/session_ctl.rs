//! Session commands: open, close, reopen, preview, detection, and per-input calibration refresh.

use std::sync::Arc;
use std::time::Instant;

use ac2_audio::Backend;
use ac2_proto::event::{Change, Patch};
use ac2_proto::model::{
    Availability, BackendInfo, GenAction, Measurement, Session, SessionConfig, SweepFailure,
};
use ac2_proto::units::{ClientId, Dbfs, LeaseToken, Rev, Seconds, SessionEpoch};
use ac2_proto::{ErrorCode, ProtoError, ReplyBody};

use crate::conv;
use crate::preview::Preview;
use crate::session::{self, Runtime};
use crate::util::perr;

use super::{Control, ControlMsg, delay_samples, recovery};

impl Control {
    pub(super) fn session_open(
        &mut self,
        client: &ClientId,
        config: SessionConfig,
    ) -> Result<ReplyBody, ProtoError> {
        if config.backend == Some(ac2_proto::model::BackendKind::Replay) {
            return Err(perr(
                ErrorCode::Invalid,
                "a recording is played with session.replay",
            ));
        }
        self.open_session(client, config, None)
            .map(ReplyBody::Session)
    }

    /// Opens a session on `config` (a replay of `replay`, on the replay backend).
    pub(super) fn open_session(
        &mut self,
        client: &ClientId,
        config: SessionConfig,
        replay: Option<ac2_proto::model::ReplayInfo>,
    ) -> Result<Session, ProtoError> {
        session::validate(&config)?;
        let backend = self.backend_for(config.backend)?;
        // The preview may hold the very device the session is about to open.
        self.close_preview();
        if self.session.is_some() || self.recovery.is_some() {
            self.session_close(client);
        }
        let epoch = SessionEpoch(self.epoch().0 + 1);
        let mut rt = match Runtime::open(
            &*backend,
            &config,
            &[],
            self.s.limits(),
            epoch,
            self.s.to_self.clone(),
            self.s.fps,
        ) {
            Ok(rt) => rt,
            Err(e) => {
                tracing::warn!("session open failed: {}", e.msg);
                return Err(e);
            }
        };
        rt.open.replay = replay;
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
        self.abort_sweep(SweepFailure::SessionClosed, "the audio session reopened");
        self.end_recording(recording);
        self.stop_all_jobs();
        self.level = None;
        self.source = None;
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
        self.start_timing_job();
        self.start_session_levels();
    }

    pub(super) fn session_close(&mut self, client: &ClientId) {
        self.abort_sweep(SweepFailure::SessionClosed, "the audio session closed");
        self.end_recording(ac2_proto::model::RecordingEnd::SessionClosed);
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
        let recovering = self.recovery.take().is_some();
        let rt = self.session.take();
        let had = rt.is_some() || recovering;
        if let Some(rt) = rt {
            recovery::close_bounded(rt, recovery::CLOSE_BOUND);
        }
        if had {
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
            return Err(perr(ErrorCode::Invalid, "no open session"));
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
        self.wind_down(ac2_proto::model::RecordingEnd::SessionReopened);
        recovery::close_bounded(rt, recovery::CLOSE_BOUND);
        match Runtime::open(
            &*backend,
            &config,
            routes,
            self.s.limits(),
            epoch,
            self.s.to_self.clone(),
            self.s.fps,
        ) {
            Ok(mut rt) => {
                rt.open.replay = replay;
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
        .ok_or_else(|| {
            perr(
                ErrorCode::NotFound,
                format!("this daemon offers no {kind:?} backend"),
            )
        })
    }

    pub(super) fn backend_infos(&self) -> Vec<BackendInfo> {
        self.s
            .backends
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

    pub(super) fn close_preview(&mut self) {
        if let Some(p) = self.preview.take() {
            p.close();
            self.s.outbox.clear(b"session/preview");
        }
    }

    pub(super) fn session_preview(
        &mut self,
        kind: ac2_proto::model::BackendKind,
        device: ac2_proto::model::DeviceId,
    ) -> Result<ReplyBody, ProtoError> {
        if self.detecting.is_some() {
            // The burst holds the device; a preview asked for before the detection started
            // must not reopen it under the burst.
            return Err(perr(
                ErrorCode::Refused,
                "a loopback detection holds the device; the meters return with its result",
            ));
        }
        if let Some(p) = &mut self.preview
            && p.backend == kind
            && p.device == device
        {
            match p.dead(Instant::now()) {
                None => {
                    p.renew();
                    return Ok(ReplyBody::Preview(p.wire()));
                }
                Some(why) => {
                    tracing::warn!("preview of {kind:?} {:?}: {why}; reopening it", device.0);
                }
            }
        }
        let backend = self.backend_for(Some(kind))?;
        self.close_preview();
        let p = Preview::open(
            &*backend,
            kind,
            device,
            self.s.max_level,
            self.env_at(self.epoch()),
        )?;
        let wire = p.wire();
        self.preview = Some(p);
        Ok(ReplyBody::Preview(wire))
    }

    /// Checks a `session.detect_loopback` and starts it on its own thread.
    pub(super) fn start_detect(
        &mut self,
        client: &ClientId,
        token: LeaseToken,
        kind: ac2_proto::model::BackendKind,
        (input_device, output_device): (ac2_proto::model::DeviceId, ac2_proto::model::DeviceId),
        output: u16,
        level: Option<Dbfs>,
    ) -> Result<u64, ProtoError> {
        self.check_lease(Instant::now());
        self.lease_check(client, token)?;
        let Some(level) = level else {
            return Err(perr(
                ErrorCode::Refused,
                "type the burst level: loopback detection has no default level",
            ));
        };
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
        let g = &self.store.state().generator;
        if g.armed || g.firing {
            return Err(perr(
                ErrorCode::Refused,
                "the stimulus is armed: stop it before detecting the loopback",
            ));
        }
        if self.detecting.is_some() {
            return Err(perr(
                ErrorCode::Refused,
                "a loopback detection is already running",
            ));
        }
        let backend = self.backend_for(Some(kind))?;
        self.close_preview();
        let token = self.next_token;
        self.next_token += 1;
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
            client.0
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
            .map_err(|e| perr(ErrorCode::Internal, format!("cannot start detection: {e}")))?;
        Ok(token)
    }
}
