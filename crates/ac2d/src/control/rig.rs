//! The rig's settings and server features: the system max level (`gen.ceiling`), the
//! output labels (`session.outputs`) and who may connect (`server.*`).
//!
//! The system max level protects ears and speakers, so the asymmetry is deliberate: any
//! client may lower it and that applies at once, while raising it takes an explicit
//! confirmation and a silent generator, never goes above the bound the daemon was started
//! with, and leaves an audit line naming the client.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ac2_proto::event::Change;
use ac2_proto::model::{
    AuthorizedClient, GenAction, OutputSetup, RefusedKey, ServerInfo, ServerMode, SweepFailure,
    check_output_label,
};
use ac2_proto::units::{ClientId, Dbfs, WallNs};
use ac2_proto::{ErrorCode, ProtoError, ReplyBody};
use ac2_zmq::{AuthorizedHandle, PublicKey};

use super::Control;
use crate::authlog::RefusedList;
use crate::rig::{RigSettings, upsert_outputs};
use crate::util::perr;

/// How the daemon serves clients, as `server.*` reports and changes it.
pub(crate) enum ServerSetup {
    /// In-process only.
    Embedded,
    /// This machine only.
    Local {
        /// Ctrl endpoint as bound.
        ctrl: String,
    },
    /// Network mode (CURVE).
    Network(Box<NetworkSetup>),
}

/// What a network-mode daemon reports and changes.
pub(crate) struct NetworkSetup {
    pub(crate) ctrl: String,
    pub(crate) data: String,
    pub(crate) server_key: PublicKey,
    /// The keys the handshake check uses.
    pub(crate) authorized: AuthorizedHandle,
    /// Where they are kept.
    pub(crate) authorized_file: PathBuf,
    pub(crate) refused: RefusedList,
    /// The mDNS name, once the advert is up.
    pub(crate) advertised_as: Arc<Mutex<Option<String>>>,
}

impl ServerSetup {
    /// Whether `name` (a CURVE client's authorized name) is no longer authorized.
    pub(crate) fn revoked(&self, name: &str) -> bool {
        match self {
            Self::Network(n) => n.authorized.get().get(name).is_none(),
            _ => false,
        }
    }
}

fn network_only() -> ProtoError {
    perr(
        ErrorCode::Unsupported,
        "this daemon is not in network mode: there are no client keys (the operating system's \
         user is the boundary)",
    )
}

impl Control {
    /// `gen.ceiling`.
    pub(super) fn gen_ceiling(
        &mut self,
        client: &ClientId,
        ceiling: Dbfs,
        confirm_raise: bool,
    ) -> Result<ReplyBody, ProtoError> {
        let new = ceiling.0;
        let bound = self.s.ceiling_bound;
        let old = self.s.ceiling_dbfs;
        if !new.is_finite() {
            return Err(perr(ErrorCode::Invalid, "the level must be finite"));
        }
        if new > bound {
            return Err(perr(
                ErrorCode::Invalid,
                format!(
                    "{new:.1} dBFS is above this rig's bound {bound:.1} dBFS (ac2d --max-level; \
                     only a restart with a higher --max-level raises the bound)"
                ),
            ));
        }
        if new == old {
            return Ok(ReplyBody::Generator(self.store.state().generator.clone()));
        }
        let peak = crate::stimulus::peak_limit(new)
            .map_err(|e| perr(ErrorCode::Invalid, e.to_string()))?;
        let raising = new > old;
        if raising {
            if !confirm_raise {
                return Err(perr(
                    ErrorCode::Refused,
                    format!(
                        "raising the system max level from {old:.1} to {new:.1} dBFS needs an \
                         explicit confirmation"
                    ),
                ));
            }
            let g = &self.store.state().generator;
            let sweeping = self.sweep.as_ref().is_some_and(|s| s.run.active());
            if g.armed || g.firing || sweeping || self.detecting.is_some() {
                return Err(perr(
                    ErrorCode::Refused,
                    "the stimulus is armed or playing: stop it before raising the system max \
                     level",
                ));
            }
        }
        // Kept first: a level that would quietly come back different after a restart is
        // worse than a refused change.
        self.rig.persist(&RigSettings {
            ceiling_dbfs: Some(new),
            outputs: self.store.state().outputs.clone(),
        })?;
        self.ceiling_set = Some(new);
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
                .is_some_and(|s| s.run.active() && s.run.level.0 > new);
            if sweep_above {
                self.abort_sweep(
                    SweepFailure::Stopped,
                    "the system max level was lowered below the sweep's level",
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
            client.0
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
                return Err(perr(ErrorCode::Invalid, format!("output {n} listed twice")));
            }
            if let Some(l) = &r.label {
                check_output_label(l)
                    .map_err(|e| perr(ErrorCode::Invalid, format!("output {n}: {e}")))?;
            }
        }
        let all = upsert_outputs(&self.store.state().outputs, &rows);
        if all != self.store.state().outputs {
            self.rig.persist(&RigSettings {
                ceiling_dbfs: self.ceiling_set,
                outputs: all.clone(),
            })?;
            self.commit(Change::Outputs(all));
        }
        Ok(ReplyBody::Outputs(self.store.state().outputs.clone()))
    }

    /// `server.info`.
    pub(super) fn server_info(&self) -> ServerInfo {
        let mode = match &self.s.server {
            ServerSetup::Embedded => ServerMode::Embedded,
            ServerSetup::Local { ctrl } => ServerMode::Local { ctrl: ctrl.clone() },
            ServerSetup::Network(n) => ServerMode::Network {
                ctrl: n.ctrl.clone(),
                data: n.data.clone(),
                server_key: n.server_key.to_z85(),
                fingerprint: n.server_key.fingerprint(),
                advertised_as: n
                    .advertised_as
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
                authorized: n
                    .authorized
                    .get()
                    .iter()
                    .map(|(name, k)| AuthorizedClient {
                        name: name.to_owned(),
                        key: k.to_z85(),
                        fingerprint: k.fingerprint(),
                    })
                    .collect(),
                refused: n
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
                    .collect(),
            },
        };
        ServerInfo {
            mode,
            recording_dir: self
                .s
                .recording_dir
                .as_ref()
                .map(|d| d.display().to_string()),
        }
    }

    /// `server.authorize`.
    pub(super) fn server_authorize(
        &mut self,
        client: &ClientId,
        name: &str,
        key: &str,
    ) -> Result<ReplyBody, ProtoError> {
        let ServerSetup::Network(n) = &self.s.server else {
            return Err(network_only());
        };
        let key = PublicKey::from_z85(key.trim()).map_err(|_| {
            perr(
                ErrorCode::Invalid,
                "not a client key: expected the 40 Z85 characters the client shows",
            )
        })?;
        let mut keys = n.authorized.get();
        keys.insert(name, key)
            .map_err(|e| perr(ErrorCode::Invalid, e.to_string()))?;
        keys.save(&n.authorized_file).map_err(|e| {
            perr(
                ErrorCode::Internal,
                format!("cannot write {}: {e}", n.authorized_file.display()),
            )
        })?;
        n.authorized.set(keys);
        n.refused.forget(&key);
        tracing::warn!(
            target: "ac2d::audit",
            "client {name:?} (fingerprint {}) authorized by {}",
            key.fingerprint(),
            client.0
        );
        Ok(ReplyBody::Server(self.server_info()))
    }

    /// `server.revoke`.
    pub(super) fn server_revoke(
        &mut self,
        client: &ClientId,
        name: &str,
    ) -> Result<ReplyBody, ProtoError> {
        let ServerSetup::Network(n) = &self.s.server else {
            return Err(network_only());
        };
        if client.0 == name {
            return Err(perr(
                ErrorCode::Refused,
                "a client cannot revoke its own key (that would lock this operator out); \
                 revoke it from another client or edit the authorized-clients file",
            ));
        }
        let mut keys = n.authorized.get();
        let Some(key) = keys.remove(name) else {
            return Err(perr(
                ErrorCode::NotFound,
                format!("no authorized client {name:?}"),
            ));
        };
        keys.save(&n.authorized_file).map_err(|e| {
            perr(
                ErrorCode::Internal,
                format!("cannot write {}: {e}", n.authorized_file.display()),
            )
        })?;
        n.authorized.set(keys);
        tracing::warn!(
            target: "ac2d::audit",
            "client {name:?} (fingerprint {}) revoked by {}",
            key.fingerprint(),
            client.0
        );
        Ok(ReplyBody::Server(self.server_info()))
    }
}
