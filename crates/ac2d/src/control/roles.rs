//! What each connection may do: its role, the refusal of a command outside it, and what a
//! key loses when its role is taken down.

use ac2_proto::model::ClientRole;
use ac2_proto::{Command, ErrorCode, ErrorDetail, ProtoError};
use ac2_zmq::PublicKey;

use crate::route::Peer;
use crate::util::{perr_detail, refused};

use super::Control;
use super::transfer::MAX_VIEWER_TRANSFERS;

/// The refusal of a command a viewer may not send.
pub(super) fn view_only(op: &str) -> ProtoError {
    perr_detail(
        ErrorCode::Refused,
        format!("{op}: this client is authorized as a viewer on this rig; it may only look"),
        ErrorDetail::ViewOnly,
    )
}

impl Control {
    /// The role `peer` has now: the local pair is always an operator (the OS user is the
    /// boundary); a network key has the role it is listed with, `None` once revoked.
    pub(super) fn peer_role(&self, peer: &Peer) -> Option<ClientRole> {
        match peer.curve() {
            None => Some(ClientRole::Operator),
            Some((name, key)) => self.s.server.network.role(name, key),
        }
    }

    /// Refuses `cmd` from `peer` unless its role may send it. Checked before dedup
    /// admission and before anything runs, so a refused request stores and changes nothing.
    pub(super) fn role_gate(
        &mut self,
        peer: &Peer,
        cmd: &Command,
    ) -> Result<ClientRole, ProtoError> {
        let Some(role) = self.peer_role(peer) else {
            let name = peer.curve().map_or("", |(n, _)| n);
            // The handshake check refuses it from now on; a connection already up must
            // not keep acting on the rig until it drops. Said once per key: the connection
            // may keep sending.
            if self
                .refusals
                .first(peer.curve().map(|(_, k)| *k), "revoked")
            {
                tracing::warn!(
                    target: "ac2d::auth",
                    "request from revoked client {name:?} refused (repeats at debug)"
                );
            } else {
                tracing::debug!(target: "ac2d::auth", "request from revoked client {name:?}");
            }
            return Err(refused(format!(
                "the key of client {name:?} was revoked on this rig"
            )));
        };
        if cmd.allowed(&role) {
            return Ok(role);
        }
        let name = peer.id().0.as_str();
        Err(match role {
            ClientRole::Viewer => view_only(cmd.name()),
            // Only a rig reads the source state: a rig whose key was admitted as an app
            // would otherwise see its link up and its audio subscriptions refused without a
            // word.
            ClientRole::Operator => refused(format!(
                "{name:?} is not admitted as a rig: admit it with auth.admit_rig"
            )),
            // A rig admitted to take this rig's inputs says hello and reads the source
            // state; it is no operator and never drives this rig.
            ClientRole::Rig { .. } => refused(format!(
                "{name:?} is admitted as a rig: it may only take inputs"
            )),
        })
    }

    /// A viewer key holds one transfer at a time, its download still being packed
    /// included, and viewers together hold at most [`MAX_VIEWER_TRANSFERS`]: transfers are
    /// capped for the whole daemon, and viewers must not be able to crowd an operator's
    /// out.
    pub(super) fn viewer_transfer_gate(
        &mut self,
        peer: &Peer,
        role: ClientRole,
        cmd: &Command,
    ) -> Result<(), ProtoError> {
        let opens = matches!(
            cmd,
            Command::TraceExport { .. } | Command::FileDownloadBegin { .. }
        );
        let Some((_, key)) = peer.curve() else {
            return Ok(());
        };
        if role != ClientRole::Viewer || !opens {
            return Ok(());
        }
        let own =
            self.transfers.held_by_viewers(Some(key)) + self.file_jobs.viewer_downloads(Some(key));
        if own > 0 {
            return Err(refused(
                "a viewer holds one transfer at a time: finish or abandon the one under way",
            ));
        }
        let all = self.transfers.held_by_viewers(None) + self.file_jobs.viewer_downloads(None);
        if all >= MAX_VIEWER_TRANSFERS {
            return Err(refused(format!(
                "viewers hold {all} transfers, all they may hold together; try again when \
                 one is done"
            )));
        }
        Ok(())
    }

    /// `key`'s role was taken down (revoked, or demoted from operator): what an operator
    /// holds goes with it now, not at its next request. Its generator lease ends (output
    /// fades out, its sweep or dynamics run aborted) and its transfers are dropped; its
    /// connections are dropped by the caller, and it learns its role from `welcome` again.
    pub(super) fn take_down(&mut self, name: &str, key: &PublicKey) {
        self.s.outbox.revoke(key.as_bytes());
        self.dedup.forget_name(name);
        self.drop_lease_of_key(key);
        self.transfers.forget_key(key);
        // What it began as an operator and is still running commits nothing and hands
        // nothing out: file jobs stop before they change or pack a session (and are
        // refused when they report), a loopback burst is cut with its fade.
        self.file_jobs.cancel_key(key);
        self.cut_detect_of_key(name, key);
    }

    /// `Ok` while `client` is still an operator: an operator's command answered after its
    /// role was taken down hands nothing out and commits nothing.
    pub(super) fn still_operator(&self, client: &Peer, op: &str) -> Result<(), ProtoError> {
        if self.peer_role(client) == Some(ClientRole::Operator) {
            Ok(())
        } else {
            Err(refused(format!(
                "this client's role changed while {op} ran; nothing was done or handed out"
            )))
        }
    }

    /// Cuts the loopback detection `key` started, if one is sounding: its burst fades out
    /// as a lease taken away does.
    fn cut_detect_of_key(&mut self, name: &str, key: &PublicKey) {
        let Some(d) = &self.pending.detecting else {
            return;
        };
        if d.reply.client.curve().is_some_and(|(_, k)| k == key) {
            d.cut.set();
            tracing::warn!(
                target: crate::AUDIT_TARGET,
                "loopback detection of {name:?} cut: its role was taken down"
            );
        }
    }
}
