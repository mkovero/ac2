//! The generator (Q6): stimulus lease, audit log, mute checks and `gen.set`.

use std::sync::Arc;
use std::time::Instant;

use ac2_audio::{Gain, GeneratorHandle, OutputState};
use ac2_core::generator::{Generator as CoreGenerator, GeneratorConfig, dbfs_to_rms};
use ac2_proto::event::Change;
use ac2_proto::model::MeasKind;
use ac2_proto::model::{
    GenAction, GenAudit, Generator, GeneratorDesired, Lease as WireLease, Signal, SweepFailure,
    SweepStatus,
};
use ac2_proto::units::{LeaseToken, WallNs};
use ac2_proto::{ErrorCode, ErrorDetail, ProtoError, ReplyBody};

use crate::conv;
use crate::stimulus::LeasedSource;
use crate::util::{internal, invalid, perr_detail, random_u64, random_u128, refused, wall_ns};

use super::{Control, Lease, SourceKey, gen_err, lease_required};
use crate::route::Peer;

/// A client as the audit names it: its id and the pair it came in on, since a network
/// key's name may read like a local client's id.
fn describe(p: &Peer) -> String {
    match p.curve() {
        None => format!("{} (local)", p.id().0),
        Some((name, key)) => format!("{name} (network, key {})", key.fingerprint()),
    }
}
use crate::jobs::JobCmd;
use crate::jobs::finder::GeneratorEdge;

/// Starts `h` at unity gain; says whether it starts sounding (it was silent or fading out).
pub(super) fn start_output(h: &GeneratorHandle) -> bool {
    let starts = !matches!(h.state(), OutputState::Active | OutputState::FadingIn);
    h.set_gain(Gain::UNITY);
    h.start();
    starts
}

/// The frequency (Hz) of the sine the generator is firing; `None` when it is silent or
/// plays anything else (noise, a sweep).
pub(super) fn firing_sine(g: &Generator) -> Option<f64> {
    match g.settings.as_ref().filter(|_| g.firing)?.signal {
        Signal::Sine { freq } => Some(freq.0),
        Signal::White | Signal::Pink | Signal::PeriodicPink { .. } | Signal::Ess { .. } => None,
    }
}

impl Control {
    pub(super) fn audit(&self, g: &mut Generator, action: GenAction, client: Option<&Peer>) {
        tracing::info!(
            target: "ac2d::audit",
            "generator {action:?} by {}",
            client.map_or_else(|| "daemon".to_owned(), describe)
        );
        g.last_action = Some(GenAudit {
            action,
            client: client.map(|c| c.id().clone()),
            at: WallNs(wall_ns()),
        });
    }

    pub(super) fn wire_lease(&self, token: LeaseToken) -> WireLease {
        WireLease {
            lease_token: token,
            expires_in_ms: u32::try_from(self.s.lease_expiry.as_millis()).unwrap_or(u32::MAX),
        }
    }

    pub(super) fn lease_check(&self, client: &Peer, token: LeaseToken) -> Result<(), ProtoError> {
        match &self.out.lease {
            Some(l) if l.token == token && l.owner == *client && l.deadline > Instant::now() => {
                Ok(())
            }
            _ => Err(lease_required()),
        }
    }

    /// Fades the output out (the gate makes the source fade on its own as well).
    pub(super) fn stop_output(&mut self) {
        self.out.gate.close();
        let sounding = self
            .stream
            .as_ref()
            .is_some_and(|rt| rt.gen_handle.state() != OutputState::Silent);
        if let Some(rt) = &self.stream {
            rt.gen_handle.stop();
        }
        if sounding {
            self.generator_edge(GeneratorEdge::Stopped);
        }
        self.out.level = None;
        self.out.source = None;
    }

    /// Tells every transfer job the generator started or stopped: it places the edge in the
    /// audio a delay find reads.
    pub(super) fn generator_edge(&self, edge: GeneratorEdge) {
        for (id, j) in &self.jobs {
            if self
                .meas(*id)
                .is_ok_and(|m| matches!(m.config.kind, MeasKind::Transfer { .. }))
            {
                j.send(JobCmd::GeneratorEdge(edge));
            }
        }
    }

    /// Tells every spectrum job which sine the generator fires now, when that changed.
    /// Called on every commit of the generator's state, the one place every start, stop,
    /// expiry, recovery and sweep passes: the state says firing exactly while the output
    /// sounds. Only spectrum jobs read a sine.
    pub(super) fn sync_tone(&mut self) {
        let tone = firing_sine(&self.store.state().generator);
        if tone == self.out.tone {
            return;
        }
        self.out.tone = tone;
        for (id, j) in &self.jobs {
            if self
                .meas(*id)
                .is_ok_and(|m| matches!(m.config.kind, MeasKind::Spectrum { .. }))
            {
                j.send(JobCmd::Tone(tone));
            }
        }
    }

    pub(super) fn check_lease(&mut self, now: Instant) {
        if self.out.lease.as_ref().is_some_and(|l| l.deadline <= now) {
            self.end_lease("expired");
        }
    }

    /// The control loop was blocked since `since` in a backend call (opening a stream):
    /// the lease holder's refreshes waited unanswered meanwhile, which is no sign it walked
    /// away, so that time does not count against its lease. Only the lease moves: every
    /// call that blocks closes or replaces the stream the gate guards, so no stimulus plays
    /// on through the excused time.
    pub(super) fn excuse_blocked(&mut self, since: Instant) {
        if let Some(l) = &mut self.out.lease {
            l.deadline += since.elapsed();
        }
    }

    /// The network went off, moved or failed: a network client holding the lease can no
    /// longer refresh or stop it, so its stimulus must not play on for the rest of the lease.
    pub(super) fn drop_network_lease(&mut self) {
        if self.out.lease.as_ref().is_some_and(|l| !l.owner.is_local()) {
            self.end_lease("ended with the network");
        }
    }

    /// Client key `key` was revoked: its connections are cut, so a lease it holds ends
    /// now for the same reason.
    pub(super) fn drop_lease_of_key(&mut self, key: &ac2_zmq::PublicKey) {
        if self
            .out
            .lease
            .as_ref()
            .is_some_and(|l| l.owner.curve().is_some_and(|(_, k)| k == key))
        {
            self.end_lease("ended with its key revoked");
        }
    }

    /// Takes the lease away from its holder: output faded out, disarmed, audited as an
    /// expiry of that owner's lease.
    fn end_lease(&mut self, why: &str) {
        let owner = self.out.lease.take().map(|l| l.owner);
        tracing::warn!(
            "stimulus lease of {} {why}: output muted",
            owner.as_ref().map_or_else(|| "?".to_owned(), describe)
        );
        self.abort_runs(SweepFailure::LeaseExpired, "the stimulus lease expired");
        self.stop_output();
        let mut g = self.store.state().generator.clone();
        g.owner = None;
        g.owner_local = false;
        g.armed = false;
        g.firing = false;
        self.audit(&mut g, GenAction::Expiry, owner.as_ref());
        self.commit(Change::Generator(g));
    }

    /// The output path muted itself on an expired gate: whatever the control side believed,
    /// the stimulus is silent, so the state must not say firing. Disarms with an expiry
    /// audit, and the lease goes with it.
    pub(super) fn check_muted(&mut self) {
        if !self.out.gate.take_tripped() || !self.store.state().generator.firing {
            return;
        }
        let owner = self.out.lease.take().map(|l| l.owner);
        tracing::warn!(
            "stimulus lease of {} expired: output muted",
            owner.as_ref().map_or_else(|| "?".to_owned(), describe)
        );
        self.abort_runs(SweepFailure::LeaseExpired, "the stimulus lease expired");
        self.stop_output();
        let mut g = self.store.state().generator.clone();
        g.owner = None;
        g.owner_local = false;
        g.armed = false;
        g.firing = false;
        self.audit(&mut g, GenAction::Expiry, owner.as_ref());
        self.commit(Change::Generator(g));
    }

    pub(super) fn gen_acquire(
        &mut self,
        client: &Peer,
        force: bool,
    ) -> Result<ReplyBody, ProtoError> {
        self.check_lease(Instant::now());
        let mut g = self.store.state().generator.clone();
        // Ownership is bound to the authenticated key, but a lease belongs to one client
        // instance: the one holding its token. An acquisition carries no token, so it is
        // always a new instance (a reconnect, or another process with the same key) and
        // must never inherit armed or firing output it did not set. Whatever the old lease
        // drove fades out and disarms first; the new holder arms and fires explicitly.
        let action = match &self.out.lease {
            Some(l) if l.owner != *client && !force => {
                return Err(perr_detail(
                    ErrorCode::LeaseHeld,
                    format!("{} holds the stimulus lease", l.owner.id().0),
                    ErrorDetail::LeaseHeld {
                        owner: l.owner.id().clone(),
                    },
                ));
            }
            Some(l) if l.owner != *client => GenAction::Force,
            _ => GenAction::Acquire,
        };
        if self.out.lease.is_some() || g.armed || g.firing {
            let why = if action == GenAction::Force {
                "another client took the stimulus over"
            } else {
                "the stimulus lease was acquired again"
            };
            self.abort_runs(SweepFailure::Stopped, why);
            self.stop_output();
            g.armed = false;
            g.firing = false;
        }
        let token = LeaseToken(random_u128());
        let deadline = Instant::now() + self.s.lease_expiry;
        self.out.lease = Some(Lease {
            token,
            owner: client.clone(),
            deadline,
        });
        g.owner = Some(client.id().clone());
        g.owner_local = client.is_local();
        self.audit(&mut g, action, Some(client));
        self.commit(Change::Generator(g));
        Ok(ReplyBody::Lease(self.wire_lease(token)))
    }

    pub(super) fn gen_set(
        &mut self,
        client: &Peer,
        token: LeaseToken,
        desired: GeneratorDesired,
    ) -> Result<ReplyBody, ProtoError> {
        self.lease_check(client, token)?;
        let st = &desired.settings;
        if desired.firing && !desired.armed {
            return Err(refused("firing requires armed"));
        }
        if !st.level.0.is_finite() {
            return Err(invalid("level must be finite"));
        }
        if st.level.0 > self.s.ceiling_dbfs {
            return Err(refused(format!(
                "{:.1} dBFS is above the global maximum {:.1} dBFS",
                st.level.0, self.s.ceiling_dbfs
            )));
        }
        for (i, o) in st.outputs.iter().enumerate() {
            if st.outputs[..i].contains(o) {
                return Err(invalid(format!("output {o} listed twice")));
            }
        }
        if desired.armed && st.outputs.is_empty() {
            return Err(invalid("no output channels"));
        }
        let signal = conv::signal(st.signal).ok_or_else(|| invalid("invalid signal"))?;
        if let Some(rt) = &self.stream
            && let Some(o) = st.outputs.iter().find(|o| **o >= rt.output_channels)
        {
            return Err(invalid(format!(
                "output {o} is not an output of the stream"
            )));
        }
        if desired.firing && self.stream.is_none() {
            return Err(refused("no open stream to emit on"));
        }
        if desired.firing && self.pending.detecting.is_some() {
            return Err(refused(
                "a loopback detection is playing its burst; fire once it is done",
            ));
        }
        if self
            .sweep
            .as_ref()
            .is_some_and(|s| matches!(s.run.status, SweepStatus::Playing { .. }))
        {
            return Err(refused(
                "a sweep is playing: stop it (gen.stop) or let it finish",
            ));
        }
        if self.dynamics.as_ref().is_some_and(|d| d.run.active()) {
            return Err(refused(
                "a dynamics run is playing: stop it (gen.stop) or let it finish",
            ));
        }

        let deadline = Instant::now() + self.s.lease_expiry;
        if let Some(l) = &mut self.out.lease {
            l.deadline = deadline;
        }
        let prev = self.store.state().generator.clone();

        // The stream carries every stream output: arming routes the generator (and
        // connects the chosen outputs) without reopening it, so the stream, its jobs and
        // every port connection stay as they are.
        if desired.armed
            && let Some(rt) = self.stream.as_mut()
        {
            rt.set_routes(&st.outputs)?;
        }

        if desired.firing {
            let key = SourceKey {
                signal: st.signal,
                band: st.band,
            };
            let rt = self.stream.as_mut().ok_or_else(|| refused("no stream"))?;
            if self.out.source != Some(key) || self.out.level.is_none() {
                let g = CoreGenerator::new(&GeneratorConfig {
                    signal,
                    sample_rate: f64::from(rt.sample_rate),
                    seed: random_u64(),
                    band: conv::band_limit(st.band),
                    level_dbfs: st.level.0,
                    ceiling_dbfs: self.s.ceiling_dbfs,
                })
                .map_err(gen_err)?;
                let peak = dbfs_to_rms(st.level.0) * g.crest_factor();
                if !self.s.max_level.admits_peak(peak) {
                    return Err(refused("the signal's peak would exceed the output limit"));
                }
                let lc = g.level_control();
                // A fresh gate state for the new source; the old one fades on its own.
                self.out.gate.open_until(deadline);
                rt.gen_handle
                    .set_source(Box::new(LeasedSource::new(
                        Box::new(g),
                        Arc::clone(&self.out.gate),
                    )))
                    .map_err(|e| internal(e.to_string()))?;
                self.out.level = Some(lc);
                self.out.source = Some(key);
            } else if let Some(lc) = &self.out.level {
                lc.set_level_dbfs(st.level.0).map_err(gen_err)?;
                self.out.gate.open_until(deadline);
            }
            let starts = start_output(&rt.gen_handle);
            if starts {
                self.generator_edge(GeneratorEdge::Started);
            }
        } else {
            self.stop_output();
        }

        let mut g = self.store.state().generator.clone();
        let action = if desired.firing && !prev.firing {
            GenAction::Fire
        } else if desired.armed && !prev.armed {
            GenAction::Arm
        } else {
            GenAction::Set
        };
        g.armed = desired.armed;
        g.firing = desired.firing;
        g.settings = Some(desired.settings.clone());
        self.audit(&mut g, action, Some(client));
        self.commit(Change::Generator(g.clone()));
        Ok(ReplyBody::Generator(g))
    }
}
