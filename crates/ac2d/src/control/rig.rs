//! The rig's settings and server features: the system max level (`gen.ceiling`), the
//! output labels (`session.outputs`) and who may connect (`server.*`).
//!
//! The system max level protects ears and speakers, so the asymmetry is deliberate: any
//! client may lower it and that applies at once, while raising it takes an explicit
//! confirmation and a silent generator, never goes above the bound the daemon was started
//! with, and leaves an audit line naming the client.

use ac2_proto::event::Change;
use ac2_proto::model::{
    AuthorizedClient, GenAction, LocalServer, NetBind, NetSettingView, NetStatusView,
    NetworkServer, OutputSetup, RefusedKey, ServerInfo, SweepFailure, check_output_label,
};
use ac2_proto::units::{Dbfs, RigId, WallNs};
use ac2_proto::{ErrorCode, ProtoError, ReplyBody};
use ac2_zmq::PublicKey;

use super::Control;
use crate::rig::{RigSettings, upsert_outputs};
use crate::route::Peer;
use crate::util::{internal, invalid, missing, perr, refused};

/// How the daemon serves clients, as `server.*` reports and changes it.
pub(crate) struct Server {
    pub(crate) local: LocalSetup,
    pub(crate) network: super::network::NetCtl,
}

/// The local pair.
pub(crate) enum LocalSetup {
    /// In-process only.
    Embedded,
    /// This machine only.
    Local {
        /// Ctrl endpoint as bound.
        ctrl: String,
        /// Data endpoint as bound.
        data: String,
    },
}

fn network_only() -> ProtoError {
    perr(
        ErrorCode::Unsupported,
        "this daemon keeps no client keys (the operating system's user is the boundary)",
    )
}

impl Control {
    /// `gen.ceiling`.
    pub(super) fn gen_ceiling(
        &mut self,
        client: &Peer,
        ceiling: Dbfs,
        confirm_raise: bool,
    ) -> Result<ReplyBody, ProtoError> {
        let new = ceiling.0;
        let bound = self.s.ceiling_bound;
        let old = self.s.ceiling_dbfs;
        if !new.is_finite() {
            return Err(invalid("the level must be finite"));
        }
        if new > bound {
            return Err(invalid(format!(
                "{new:.1} dBFS is above this rig's bound {bound:.1} dBFS (ac2d --max-level; \
                 only a restart with a higher --max-level raises the bound)"
            )));
        }
        if new == old {
            return Ok(ReplyBody::Generator(self.store.state().generator.clone()));
        }
        let peak = crate::stimulus::peak_limit(new).map_err(|e| invalid(e.to_string()))?;
        let raising = new > old;
        if raising {
            if !confirm_raise {
                return Err(refused(format!(
                    "raising the system max level from {old:.1} to {new:.1} dBFS needs an \
                     explicit confirmation"
                )));
            }
            let g = &self.store.state().generator;
            let sweeping = self.sweep.as_ref().is_some_and(|s| s.run.active());
            if g.armed || g.firing || sweeping || self.pending.detecting.is_some() {
                return Err(refused(
                    "the stimulus is armed or playing: stop it before raising the system max \
                     level",
                ));
            }
        }
        // Kept first: a level that would quietly come back different after a restart is
        // worse than a refused change.
        self.rig.settings.persist(&RigSettings {
            ceiling_dbfs: Some(new),
            ..self.rig_settings_now()
        })?;
        self.out.ceiling_set = Some(new);
        self.s.ceiling_dbfs = new;
        self.s.max_level = peak;
        if let Some(rt) = &self.session {
            rt.gen_handle.set_max_level(peak);
        }
        let mut g = self.store.state().generator.clone();
        if !raising {
            let sweep_above = self
                .sweep
                .as_ref()
                .is_some_and(|s| s.run.active() && s.run.level.0 > new)
                || self
                    .dynamics
                    .as_ref()
                    .is_some_and(|d| d.run.active() && d.run.bursts.iter().any(|b| b.0 > new));
            if sweep_above {
                self.abort_runs(
                    SweepFailure::Stopped,
                    "the system max level was lowered below the run's level",
                );
            }
            let above = g.settings.as_ref().is_some_and(|s| s.level.0 > new);
            if (g.armed || g.firing) && (above || sweep_above) {
                // Stopped, not turned down: the owner's level and the measurement's level
                // must never silently disagree; re-arming at a level within the new maximum
                // is the owner's explicit step.
                self.stop_output();
                g.armed = false;
                g.firing = false;
                self.audit(&mut g, GenAction::Stop, Some(client));
                self.commit(Change::Generator(g.clone()));
            }
        }
        tracing::warn!(
            target: "ac2d::audit",
            "system max level {} from {old:.1} to {new:.1} dBFS by {} (bound {bound:.1} dBFS)",
            if raising { "raised" } else { "lowered" },
            client.id().0
        );
        g.ceiling = ceiling;
        self.audit(
            &mut g,
            if raising {
                GenAction::CeilingRaised
            } else {
                GenAction::CeilingLowered
            },
            Some(client),
        );
        self.commit(Change::Generator(g.clone()));
        Ok(ReplyBody::Generator(g))
    }

    /// `session.outputs`.
    pub(super) fn session_outputs(
        &mut self,
        rows: Vec<OutputSetup>,
    ) -> Result<ReplyBody, ProtoError> {
        for (i, r) in rows.iter().enumerate() {
            let n = u32::from(r.channel) + 1;
            if rows[..i].iter().any(|o| o.channel == r.channel) {
                return Err(invalid(format!("output {n} listed twice")));
            }
            if let Some(l) = &r.label {
                check_output_label(l).map_err(|e| invalid(format!("output {n}: {e}")))?;
            }
        }
        let all = upsert_outputs(&self.store.state().outputs, &rows);
        if all != self.store.state().outputs {
            self.rig.settings.persist(&RigSettings {
                outputs: all.clone(),
                ..self.rig_settings_now()
            })?;
            self.commit(Change::Outputs(all));
        }
        Ok(ReplyBody::Outputs(self.store.state().outputs.clone()))
    }

    /// `server.info`. A network client gets no local endpoints and no interface list: host
    /// detail it has no use for.
    pub(super) fn server_info(&self, peer: &Peer) -> ServerInfo {
        let n = &self.s.server.network;
        let local = peer.curve().is_none();
        let key = n.server_key();
        let (authorized, refused) = match &n.authorized {
            Some(a) => {
                let keys = a.handle.get();
                let authorized = keys
                    .iter()
                    .map(|(name, k)| AuthorizedClient {
                        name: name.to_owned(),
                        key: k.to_z85(),
                        fingerprint: k.fingerprint(),
                        rig: keys.rig_of(name).map(RigId),
                    })
                    .collect();
                let refused = n
                    .refused
                    .snapshot()
                    .into_iter()
                    .map(|r| RefusedKey {
                        key: r.key.map(|k| k.to_z85()),
                        fingerprint: r.key.map(|k| k.fingerprint()),
                        address: r.address,
                        count: r.count,
                        last_at: WallNs(r.last_at_ns),
                    })
                    .collect();
                (authorized, local.then_some(refused))
            }
            None => (Vec::new(), local.then(Vec::new)),
        };
        ServerInfo {
            local: local.then(|| match &self.s.server.local {
                LocalSetup::Embedded => LocalServer::Embedded,
                LocalSetup::Local { ctrl, data } => LocalServer::Local {
                    ctrl: ctrl.clone(),
                    data: data.clone(),
                },
            }),
            network: NetworkServer {
                setting: if local {
                    NetSettingView::Host {
                        setting: n.setting(),
                    }
                } else {
                    NetSettingView::Peer {
                        setting: n.setting().kind(),
                    }
                },
                status: if local {
                    NetStatusView::Host { status: n.status() }
                } else {
                    NetStatusView::Peer {
                        status: (&n.status()).into(),
                    }
                },
                server_key: key.map(|k| k.to_z85()),
                fingerprint: key.map(|k| k.fingerprint()),
                advertised_as: n.advertised_as(),
                authorized,
                refused,
            },
            interfaces: if local { n.interfaces() } else { Vec::new() },
            toggle: n.toggle_for(local),
            recording_dir: self
                .s
                .recording_dir
                .as_ref()
                .map(|d| d.display().to_string()),
        }
    }

    /// `server.network`: only from this host, whose user already holds every other power;
    /// a network client could otherwise cut itself and everyone else off, or open the rig
    /// on a network it reached some other way.
    pub(super) fn server_network(
        &mut self,
        client: &Peer,
        enabled: bool,
        bind: Option<NetBind>,
        port: Option<u16>,
    ) -> Result<ReplyBody, ProtoError> {
        let had_key = self.s.server.network.server_key().is_some();
        let r = self.s.server.network.set(
            client.curve().is_none(),
            enabled,
            bind,
            port,
            &self.s.outbox,
            std::time::Instant::now(),
        );
        // A key made by a change refused after it is still this rig's key from now on.
        if !had_key && self.s.server.network.server_key().is_some() {
            self.keys_created();
        }
        self.server_sync();
        r?;
        tracing::warn!(
            target: "ac2d::audit",
            "network {} by {}",
            if enabled { "turned on" } else { "turned off" },
            client.id().0
        );
        Ok(ReplyBody::Server(self.server_info(client)))
    }

    /// `State.server` follows the network, whatever moved it (a request, a retry, a failure).
    pub(super) fn server_sync(&mut self) {
        if !self.s.server.network.take_changed() {
            return;
        }
        let now = self.s.server.network.summary();
        if self.store.state().server != now {
            self.commit(ac2_proto::event::Change::Server(now));
        }
    }

    /// `server.authorize`.
    pub(super) fn server_authorize(
        &mut self,
        client: &Peer,
        name: &str,
        key: &str,
    ) -> Result<ReplyBody, ProtoError> {
        self.admit(client, name, key, None)
    }

    /// `auth.admit_rig`: the other rig's key, admitted to take this rig's inputs only.
    pub(super) fn admit_rig(
        &mut self,
        client: &Peer,
        name: &str,
        key: &str,
        rig: RigId,
    ) -> Result<ReplyBody, ProtoError> {
        if rig == self.s.rig_id {
            return Err(invalid("that is this rig's own identity"));
        }
        self.admit(client, name, key, Some(rig))
    }

    fn admit(
        &mut self,
        client: &Peer,
        name: &str,
        key: &str,
        rig: Option<RigId>,
    ) -> Result<ReplyBody, ProtoError> {
        let Some(n) = &self.s.server.network.authorized else {
            return Err(network_only());
        };
        // Only a connection on the local pair is local; a name that reads like a local
        // client's id would still mislead the logs and every app that shows the lease.
        let local = ac2_proto::units::ClientId::local("").0;
        if name.starts_with(&local) {
            return Err(refused(format!(
                "{name:?}: names starting {local:?} are kept for local clients"
            )));
        }
        let key = PublicKey::from_z85(key.trim()).map_err(|_| {
            invalid("not a client key: expected the 40 Z85 characters the client shows")
        })?;
        let mut keys = n.handle.get();
        // A name given again replaces its key and kind: its old connections are dropped
        // below, as a revoke would.
        let old_rig = keys.rig_of(name);
        let old = keys.remove(name);
        match rig {
            Some(r) => keys.insert_rig(name, key, r.0),
            None => keys.insert(name, key),
        }
        .map_err(|e| invalid(e.to_string()))?;
        keys.save(&n.file)
            .map_err(|e| internal(format!("cannot write {}: {e}", n.file.display())))?;
        n.handle.set(keys);
        self.s.server.network.refused.forget(&key);
        // The same key given another kind keeps its connections but not what they hold:
        // an app made a rig must lose its state subscriptions, a rig made an app its audio.
        if let Some(o) = old
            && (o != key || old_rig.map(RigId) != rig)
        {
            self.s.outbox.revoke(o.as_bytes());
        }
        let kind = if rig.is_some() { "rig" } else { "client" };
        tracing::warn!(
            target: "ac2d::audit",
            "{kind} {name:?} (fingerprint {}) authorized by {}",
            key.fingerprint(),
            client.id().0
        );
        Ok(ReplyBody::Server(self.server_info(client)))
    }

    /// `server.revoke`.
    pub(super) fn server_revoke(
        &mut self,
        client: &Peer,
        name: &str,
    ) -> Result<ReplyBody, ProtoError> {
        let Some(n) = &self.s.server.network.authorized else {
            return Err(network_only());
        };
        if client.curve().is_some_and(|(own, _)| own == name) {
            return Err(refused(
                "a client cannot revoke its own key (that would lock this operator out); \
                 revoke it from another client or edit the authorized-clients file",
            ));
        }
        let mut keys = n.handle.get();
        let Some(key) = keys.remove(name) else {
            return Err(missing(format!("no authorized client {name:?}")));
        };
        keys.save(&n.file)
            .map_err(|e| internal(format!("cannot write {}: {e}", n.file.display())))?;
        n.handle.set(keys);
        // Its data connection, audio included, ends now rather than at its next request.
        self.s.outbox.revoke(key.as_bytes());
        self.dedup.forget_name(name);
        tracing::warn!(
            target: "ac2d::audit",
            "client {name:?} (fingerprint {}) revoked by {}",
            key.fingerprint(),
            client.id().0
        );
        Ok(ReplyBody::Server(self.server_info(client)))
    }
}
