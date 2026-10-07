//! The generator (Q6): stimulus lease, audit log, mute checks and `gen.set`.

use std::sync::Arc;
use std::time::Instant;

use ac2_audio::Gain;
use ac2_core::generator::{Generator as CoreGenerator, GeneratorConfig, dbfs_to_rms};
use ac2_proto::event::Change;
use ac2_proto::model::{
    GenAction, GenAudit, Generator, GeneratorDesired, Lease as WireLease, SweepFailure, SweepStatus,
};
use ac2_proto::units::{ClientId, LeaseToken, WallNs};
use ac2_proto::{ErrorCode, ErrorDetail, ProtoError, ReplyBody};

use crate::conv;
use crate::stimulus::LeasedSource;
use crate::util::{perr, perr_detail, random_u64, random_u128, wall_ns};

use super::{Control, Lease, SourceKey, gen_err, lease_required};

impl Control {
    pub(super) fn audit(&self, g: &mut Generator, action: GenAction, client: Option<&ClientId>) {
        tracing::info!(
            target: "ac2d::audit",
            "generator {action:?} by {}",
            client.map_or("daemon", |c| c.0.as_str())
        );
        g.last_action = Some(GenAudit {
            action,
            client: client.cloned(),
            at: WallNs(wall_ns()),
        });
    }

    pub(super) fn wire_lease(&self, token: LeaseToken) -> WireLease {
        WireLease {
            lease_token: token,
            expires_in_ms: u32::try_from(self.s.lease_expiry.as_millis()).unwrap_or(u32::MAX),
        }
    }

    pub(super) fn lease_check(
        &self,
        client: &ClientId,
        token: LeaseToken,
    ) -> Result<(), ProtoError> {
        match &self.lease {
            Some(l) if l.token == token && l.owner == *client && l.deadline > Instant::now() => {
                Ok(())
            }
            _ => Err(lease_required()),
        }
    }

    /// Fades the output out (the gate makes the source fade on its own as well).
    pub(super) fn stop_output(&mut self) {
        self.gate.close();
        if let Some(rt) = &self.session {
            rt.gen_handle.stop();
        }
        self.level = None;
        self.source = None;
    }

    pub(super) fn check_lease(&mut self, now: Instant) {
        if self.lease.as_ref().is_some_and(|l| l.deadline <= now) {
            let owner = self.lease.take().map(|l| l.owner);
            tracing::warn!(
                "stimulus lease of {} expired: output muted",
                owner.as_ref().map_or("?", |o| o.0.as_str())
            );
            self.abort_sweep(SweepFailure::LeaseExpired, "the stimulus lease expired");
            self.stop_output();
            let mut g = self.store.state().generator.clone();
            g.owner = None;
            g.armed = false;
            g.firing = false;
            self.audit(&mut g, GenAction::Expiry, owner.as_ref());
            self.commit(Change::Generator(g));
        }
    }

    /// The output path muted itself on an expired gate: whatever the control side believed,
    /// the stimulus is silent, so the state must not say firing. Disarms with an expiry
    /// audit, and the lease goes with it.
    pub(super) fn check_muted(&mut self) {
        if !self.gate.take_tripped() || !self.store.state().generator.firing {
            return;
        }
        let owner = self.lease.take().map(|l| l.owner);
        tracing::warn!(
            "stimulus lease of {} expired: output muted",
            owner.as_ref().map_or("?", |o| o.0.as_str())
        );
        self.abort_sweep(SweepFailure::LeaseExpired, "the stimulus lease expired");
        self.stop_output();
        let mut g = self.store.state().generator.clone();
        g.owner = None;
        g.armed = false;
        g.firing = false;
        self.audit(&mut g, GenAction::Expiry, owner.as_ref());
        self.commit(Change::Generator(g));
    }

    pub(super) fn gen_acquire(
        &mut self,
        client: &ClientId,
        force: bool,
    ) -> Result<ReplyBody, ProtoError> {
        self.check_lease(Instant::now());
        let mut g = self.store.state().generator.clone();
        let action = match &self.lease {
            Some(l) if l.owner != *client => {
                if !force {
                    return Err(perr_detail(
                        ErrorCode::LeaseHeld,
                        format!("{} holds the stimulus lease", l.owner.0),
                        ErrorDetail::LeaseHeld {
                            owner: l.owner.clone(),
                        },
                    ));
                }
                // Takeover stops and disarms first; the new owner arms and fires explicitly.
                self.abort_sweep(
                    SweepFailure::Stopped,
                    "another client took the stimulus over",
                );
                self.stop_output();
                g.armed = false;
                g.firing = false;
                GenAction::Force
            }
            _ => GenAction::Acquire,
        };
        let token = LeaseToken(random_u128());
        let deadline = Instant::now() + self.s.lease_expiry;
        self.lease = Some(Lease {
            token,
            owner: client.clone(),
            deadline,
        });
        if g.firing {
            self.gate.open_until(deadline);
        }
        g.owner = Some(client.clone());
        self.audit(&mut g, action, Some(client));
        self.commit(Change::Generator(g));
        Ok(ReplyBody::Lease(self.wire_lease(token)))
    }

    pub(super) fn gen_set(
        &mut self,
        client: &ClientId,
        token: LeaseToken,
        desired: GeneratorDesired,
    ) -> Result<ReplyBody, ProtoError> {
        self.lease_check(client, token)?;
        let st = &desired.settings;
        if desired.firing && !desired.armed {
            return Err(perr(ErrorCode::Refused, "firing requires armed"));
        }
        if !st.level.0.is_finite() {
            return Err(perr(ErrorCode::Invalid, "level must be finite"));
        }
        if st.level.0 > self.s.ceiling_dbfs {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "{:.1} dBFS is above the global maximum {:.1} dBFS",
                    st.level.0, self.s.ceiling_dbfs
                ),
            ));
        }
        for (i, o) in st.outputs.iter().enumerate() {
            if st.outputs[..i].contains(o) {
                return Err(perr(ErrorCode::Invalid, format!("output {o} listed twice")));
            }
        }
        if desired.armed && st.outputs.is_empty() {
            return Err(perr(ErrorCode::Invalid, "no output channels"));
        }
        let signal =
            conv::signal(st.signal).ok_or_else(|| perr(ErrorCode::Invalid, "invalid signal"))?;
        if let Some(rt) = &self.session
            && let Some(o) = st.outputs.iter().find(|o| **o >= rt.output_channels)
        {
            return Err(perr(
                ErrorCode::Invalid,
                format!("output {o} is not an output of the session"),
            ));
        }
        if desired.firing && self.session.is_none() {
            return Err(perr(ErrorCode::Refused, "no open session to emit on"));
        }
        if desired.firing && self.detecting.is_some() {
            return Err(perr(
                ErrorCode::Refused,
                "a loopback detection is playing its burst; fire once it is done",
            ));
        }
        if self
            .sweep
            .as_ref()
            .is_some_and(|s| matches!(s.run.status, SweepStatus::Playing { .. }))
        {
            return Err(perr(
                ErrorCode::Refused,
                "a sweep is playing: stop it (gen.stop) or let it finish",
            ));
        }

        let deadline = Instant::now() + self.s.lease_expiry;
        if let Some(l) = &mut self.lease {
            l.deadline = deadline;
        }
        let prev = self.store.state().generator.clone();

        // The stream carries every session output: arming routes the generator (and
        // connects the chosen outputs) without reopening it, so the session, its jobs and
        // every port connection stay as they are.
        if desired.armed
            && let Some(rt) = self.session.as_mut()
        {
            rt.set_routes(&st.outputs)?;
        }

        if desired.firing {
            let key = SourceKey {
                signal: st.signal,
                band: st.band,
            };
            let rt = self
                .session
                .as_mut()
                .ok_or_else(|| perr(ErrorCode::Refused, "no session"))?;
            if self.source != Some(key) || self.level.is_none() {
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
                if peak > f64::from(self.s.max_level.linear()) * (1.0 + 1e-9) {
                    return Err(perr(
                        ErrorCode::Refused,
                        "the signal's peak would exceed the output limit",
                    ));
                }
                let lc = g.level_control();
                // A fresh gate state for the new source; the old one fades on its own.
                self.gate.open_until(deadline);
                rt.gen_handle
                    .set_source(Box::new(LeasedSource::new(
                        Box::new(g),
                        Arc::clone(&self.gate),
                    )))
                    .map_err(|e| perr(ErrorCode::Internal, e.to_string()))?;
                self.level = Some(lc);
                self.source = Some(key);
            } else if let Some(lc) = &self.level {
                lc.set_level_dbfs(st.level.0).map_err(gen_err)?;
                self.gate.open_until(deadline);
            }
            rt.gen_handle.set_gain(Gain::UNITY);
            rt.gen_handle.start();
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
