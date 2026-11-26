//! The rig's settings and server features: the system max level (`gen.ceiling`), the
//! output labels (`stream.outputs`) and who may connect (`server.*`).
//!
//! The system max level protects ears and speakers, so the asymmetry is deliberate: any
//! client may lower it and that applies at once, while raising it takes an explicit
//! confirmation and a silent generator, never goes above the bound the daemon was started
//! with, and leaves an audit line naming the client.

use ac2_proto::event::Change;
use ac2_proto::model::{
    AuthorizedClient, ClientRole, GenAction, LocalServer, NetBind, NetSettingView, NetStatusView,
    NetworkServer, OutputSetup, Recordings, RefusedKey, ServerInfo, SweepFailure,
    check_output_label,
};
use ac2_proto::units::{Dbfs, RigId, WallNs};
use ac2_proto::{ErrorCode, ProtoError, ReplyBody};
use ac2_zmq::{PublicKey, Role};

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
            // The level in force, kept where a restart reads it if the file has none yet
            // (its start could not write it): a later start then never takes a higher bound.
            if self.rig.settings.stored_ceiling() != Some(new) {
                self.rig.settings.store_level(&RigSettings {
                    ceiling_dbfs: Some(new),
                    ..self.rig_settings_now()
                })?;
            }
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
        self.rig.settings.store_level(&RigSettings {
            ceiling_dbfs: Some(new),
            ..self.rig_settings_now()
        })?;
        self.s.ceiling_dbfs = new;
        self.s.max_level = peak;
        if let Some(rt) = &self.stream {
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
            target: crate::AUDIT_TARGET,
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

    /// `stream.outputs`.
    pub(super) fn stream_outputs(
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
    /// detail it has no use for. A viewer is told its own entry only (other clients' keys
    /// and roles are an operator's business) and not where recordings lie.
    pub(super) fn server_info(&self, peer: &Peer) -> ServerInfo {
        let n = &self.s.server.network;
        let local = peer.curve().is_none();
        let viewer = self.peer_role(peer) != Some(ClientRole::Operator);
        let key = n.server_key();
        let (authorized, refused) = match &n.authorized {
            Some(a) => {
                let keys = a.handle.get();
                let authorized = keys
                    .iter()
                    .filter(|(name, _)| {
                        !viewer || peer.curve().is_some_and(|(own, _)| own == *name)
                    })
                    .map(|(name, k)| AuthorizedClient {
                        name: name.to_owned(),
                        key: k.to_z85(),
                        fingerprint: k.fingerprint(),
                        role: keys
                            .role_of(name)
                            .map_or(ClientRole::Viewer, super::network::client_role),
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
            recordings: match &self.s.recording_dir {
                None => Recordings::Off,
                Some(_) if viewer => Recordings::Kept,
                Some(d) => Recordings::Dir {
                    dir: d.display().to_string(),
                },
            },
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
            self.start_links();
        }
        self.server_sync();
        if r? {
            self.drop_network_lease();
        }
        tracing::warn!(
            target: crate::AUDIT_TARGET,
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

    /// `auth.authorize`: an operator or a viewer, never a rig (`auth.admit_rig`), and never
    /// over a name admitted as a rig, so a rig key never becomes an app by accident.
    pub(super) fn server_authorize(
        &mut self,
        client: &Peer,
        name: &str,
        key: &str,
        role: ClientRole,
    ) -> Result<ReplyBody, ProtoError> {
        let role = match role {
            ClientRole::Operator => Role::Operator,
            ClientRole::Viewer => Role::Viewer,
            ClientRole::Rig { .. } => {
                return Err(invalid(
                    "a rig is admitted with auth.admit_rig, which pins its identity",
                ));
            }
        };
        if let Some(n) = &self.s.server.network.authorized
            && n.handle.get().rig_of(name).is_some()
        {
            return Err(invalid(format!(
                "{name:?} is admitted as a rig: revoke it with auth.revoke_rig first"
            )));
        }
        self.admit(client, name, key, role)
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
        // Admitted over an app's name, the rig's key would replace the app's: an app is
        // revoked by name first, on purpose.
        if let Some(n) = &self.s.server.network.authorized
            && n.handle
                .get()
                .role_of(name)
                .is_some_and(|r| !matches!(r, Role::Rig(_)))
        {
            return Err(invalid(format!(
                "{name:?} is an app's name: revoke it with auth.revoke first, or admit the \
                 rig under another name"
            )));
        }
        self.admit(client, name, key, Role::Rig(rig.0))
    }

    fn admit(
        &mut self,
        client: &Peer,
        name: &str,
        key: &str,
        role: Role,
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
        // A client that changes its own entry (another key, another role) is cut off at
        // once and cannot put it back: only the same key and role again may pass.
        if let Some((own, own_key)) = client.curve()
            && own == name
            && (*own_key != key || n.handle.get().role_of(name) != Some(role))
        {
            return Err(refused(
                "a client cannot change its own key or role (that would lock this operator \
                 out); change it from another client or edit the authorized-clients file",
            ));
        }
        let mut keys = n.handle.get();
        // A name given again replaces its key and role: its old connections are dropped
        // below, as a revoke would.
        let old_role = keys.role_of(name);
        let old = keys.remove(name);
        keys.insert(name, key, role)
            .map_err(|e| invalid(e.to_string()))?;
        keys.save(&n.file)
            .map_err(|e| internal(format!("cannot write {}: {e}", n.file.display())))?;
        n.handle.set(keys);
        self.s.server.network.refused.forget(&key);
        // The same key given another role keeps none of what it held: an app made a rig
        // must lose its state subscriptions, a rig made an app its audio, an operator made
        // a viewer its lease and transfers.
        if let Some(o) = old
            && (o != key || old_role != Some(role))
        {
            self.take_down(name, &o);
        }
        let kind = match role {
            Role::Rig(_) => "rig",
            Role::Operator => "operator",
            Role::Viewer => "viewer",
        };
        tracing::warn!(
            target: crate::AUDIT_TARGET,
            "{kind} {name:?} (fingerprint {}) authorized by {}",
            key.fingerprint(),
            client.id().0
        );
        Ok(ReplyBody::Server(self.server_info(client)))
    }

    /// `auth.revoke_rig`: every key admitted as rig `rig`, by its identity. The entries are
    /// picked by their rig kind, never by name, so an app the operator authorized since
    /// under that name keeps its key.
    pub(super) fn auth_revoke_rig(
        &mut self,
        client: &Peer,
        rig: RigId,
    ) -> Result<ReplyBody, ProtoError> {
        let Some(n) = &self.s.server.network.authorized else {
            return Err(network_only());
        };
        let mut keys = n.handle.get();
        let names: Vec<String> = keys
            .iter()
            .filter(|(name, _)| keys.rig_of(name) == Some(rig.0))
            .map(|(name, _)| name.to_owned())
            .collect();
        if names.is_empty() {
            return Err(missing(format!("no rig {rig} admitted")));
        }
        let gone: Vec<PublicKey> = names.iter().filter_map(|name| keys.remove(name)).collect();
        keys.save(&n.file)
            .map_err(|e| internal(format!("cannot write {}: {e}", n.file.display())))?;
        n.handle.set(keys);
        for (name, key) in names.iter().zip(&gone) {
            self.take_down(name, key);
            tracing::warn!(
                target: crate::AUDIT_TARGET,
                "rig {name:?} (fingerprint {}) revoked by {}",
                key.fingerprint(),
                client.id().0
            );
        }
        Ok(ReplyBody::Server(self.server_info(client)))
    }

    /// `auth.revoke`.
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
        self.take_down(name, &key);
        tracing::warn!(
            target: crate::AUDIT_TARGET,
            "client {name:?} (fingerprint {}) revoked by {}",
            key.fingerprint(),
            client.id().0
        );
        Ok(ReplyBody::Server(self.server_info(client)))
    }
}
