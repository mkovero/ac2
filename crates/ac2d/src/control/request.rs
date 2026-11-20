//! The wire side of the control thread: keepalive frames, request decoding, dedup and replies.

use std::sync::atomic::Ordering;
use std::time::Instant;

use ac2_proto::frame::{Frame, FrameData, FrameStamp, GenSummary, KaMeta, ProtectionFlags};
use ac2_proto::topic::Topic;
use ac2_proto::units::{RequestId, SampleIndex, WallNs};
use ac2_proto::{
    Command, ErrorCode, MAX_CTRL_BYTES, PROTO_VERSION, ProtoError, Reply, ReplyBody,
    decode_request, encode_reply, peek_envelope,
};

use crate::dedup::Admit;
use crate::health::SessionStream;
use crate::util::{internal, invalid, perr, refused, wall_ns};

use super::{Control, PendingFind, PendingReply, mutation_conflict};
use crate::route::{Peer, Route};

impl Control {
    pub(super) fn send_ka(&mut self) {
        // Keepalives tell subscribers the daemon is alive; with nobody subscribed there is
        // no one to tell, and skipping them spares the I/O thread a wakeup each.
        if !self.s.interest.wants(&Topic::Ka.to_bytes()) {
            return;
        }
        self.ka.seq += 1;
        let stream = match (&self.session, &self.audio_recovery) {
            (Some(rt), _) => SessionStream::Running(&rt.health),
            (None, Some(_)) => SessionStream::Recovering,
            (None, None) => SessionStream::Closed,
        };
        self.health.tick(Instant::now(), stream);
        let st = self.store.state();
        let now = wall_ns();
        let latest = self
            .session
            .as_ref()
            .map_or(0u64, |r| r.fanout.latest.load(Ordering::Acquire));
        let frame = Frame {
            stamp: FrameStamp {
                seq: self.ka.seq,
                audio_sample: SampleIndex(latest.saturating_sub(1)),
                session_epoch: st.session.epoch,
                daemon_incarnation: self.s.incarnation,
                config_rev: self.store.rev(),
                config_applied_at: SampleIndex(0),
                capture_wall_ns: WallNs(now),
                grid_id: None,
                protection: ProtectionFlags::NONE,
            },
            data: FrameData::Ka(KaMeta {
                rev: self.store.rev(),
                daemon_wall_ns: WallNs(now),
                timing: st.timing.state,
                generator: GenSummary {
                    owner: st.generator.owner.clone(),
                    owner_local: st.generator.owner_local,
                    armed: st.generator.armed,
                    firing: st.generator.firing,
                },
                health: self.health.snapshot(),
            }),
        };
        match ac2_proto::encode_frame(&frame) {
            Ok(parts) => self.s.outbox.ka(parts),
            Err(e) => tracing::error!("keepalive not encodable: {e}"),
        }
    }

    pub(super) fn on_request(&mut self, route: &Route, peer: Peer, payload: &[u8]) {
        let now = Instant::now();
        let env = match peek_envelope(payload) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("undecodable ctrl message dropped: {e}");
                return;
            }
        };
        let id = env.id.unwrap_or(RequestId(0));
        if env.v != Some(PROTO_VERSION) {
            let why = match env.v {
                Some(v) => format!("protocol version {v}"),
                None => "no protocol version".to_owned(),
            };
            let who = peer.curve().map_or("a local client", |(name, _)| name);
            if self.refusals.first(peer.curve().map(|(_, k)| *k), &why) {
                tracing::warn!(
                    "refused {who} at {why}, this build speaks {PROTO_VERSION} (repeats at debug)"
                );
            } else {
                tracing::debug!("refused {who} at {why}");
            }
            self.send_reply(route, &Reply::version_refusal(id, env.v));
            return;
        }
        let req = match decode_request(payload) {
            Ok(r) => r,
            Err(e) => {
                self.send_reply(route, &Reply::new(id, Err(invalid(e.to_string()))));
                return;
            }
        };
        if let Some((name, key)) = peer.curve()
            && self.s.server.network.revoked(name, key)
        {
            // The handshake check refuses it from now on; a connection already up must
            // not keep acting on the rig until it drops.
            tracing::warn!(target: "ac2d::auth", "request from revoked client {name:?} refused");
            self.send_reply(
                route,
                &Reply::new(
                    id,
                    Err(refused(format!(
                        "the key of client {name:?} was revoked on this rig"
                    ))),
                ),
            );
            return;
        }
        // A rig admitted to take this rig's inputs says hello and reads the source state;
        // it is no operator and never drives this rig.
        if let Some((name, _)) = peer.curve()
            && self.s.server.network.is_rig(name)
            && !matches!(req.cmd, Command::Hello { .. } | Command::SourceState)
        {
            self.send_reply(
                route,
                &Reply::new(
                    id,
                    Err(refused(format!(
                        "{name:?} is admitted as a rig: it may only take inputs"
                    ))),
                ),
            );
            return;
        }
        // Only a rig reads the source state: a rig whose key was admitted as an app would
        // otherwise see its link up and its audio subscriptions refused without a word.
        if let Some((name, _)) = peer.curve()
            && matches!(req.cmd, Command::SourceState)
            && !self.s.server.network.is_rig(name)
        {
            self.send_reply(
                route,
                &Reply::new(
                    id,
                    Err(refused(format!(
                        "{name:?} is not admitted as a rig: admit it with auth.admit_rig"
                    ))),
                ),
            );
            return;
        }
        let client = peer;
        // Reserved before anything executes, the asynchronous branches below included: a
        // copy of a request still running waits for its reply rather than running it again.
        match self.dedup.admit(&client, req.id, route, now) {
            Admit::Execute => {}
            Admit::Attached => {
                tracing::debug!("{} retried running request {}", client.id().0, req.id.0);
                return;
            }
            Admit::Stored(stored) => {
                tracing::debug!(
                    "{} retried request {}; stored reply",
                    client.id().0,
                    req.id.0
                );
                self.s.outbox.reply(route, &stored);
                return;
            }
        }
        let result = match req.expect_rev {
            Some(r) if req.cmd.is_mutation() && r != self.store.rev() => {
                Err(mutation_conflict(self.store.rev()))
            }
            _ => match req.cmd {
                // The finder runs for a while on the job thread; the reply follows its result
                // and other clients are served meanwhile.
                // The burst and its capture take about a second on their own thread; the
                // reply follows the result.
                Command::SessionDetectLoopback {
                    lease_token,
                    backend,
                    input_device,
                    output_device,
                    output,
                    level,
                } => {
                    match self.start_detect(
                        &client,
                        lease_token,
                        backend,
                        (input_device, output_device),
                        output,
                        level,
                    ) {
                        Ok(token) => {
                            self.pending.detecting = Some((
                                token,
                                PendingReply {
                                    route: route.clone(),
                                    client,
                                    id: req.id,
                                },
                            ));
                            return;
                        }
                        Err(e) => Err(e),
                    }
                }
                // A driver's panel may be a modal dialog: the call runs on its own thread.
                Command::SessionControlPanel { backend, device } => {
                    match self.start_panel(backend, device) {
                        Ok(token) => {
                            self.pending.panels.insert(
                                token,
                                PendingReply {
                                    route: route.clone(),
                                    client,
                                    id: req.id,
                                },
                            );
                            return;
                        }
                        Err(e) => Err(e),
                    }
                }
                Command::DelayFind {
                    meas,
                    band,
                    observation,
                } => match self.start_find(meas, band, observation) {
                    Ok(token) => {
                        self.pending.finds.insert(
                            token,
                            PendingFind {
                                route: route.clone(),
                                client,
                                id: req.id,
                                meas,
                            },
                        );
                        return;
                    }
                    Err(e) => Err(e),
                },
                // Session file work runs on a worker; the reply follows its result.
                cmd @ (Command::FileSave { .. }
                | Command::FileLoad { .. }
                | Command::FileNew { .. }
                | Command::FileRecover
                | Command::FileSalvage { .. }
                | Command::FileList
                | Command::FileImport { .. }
                | Command::FileDownloadBegin { .. }) => {
                    match self.start_file_job(route, &client, req.id, cmd) {
                        Ok(()) => return,
                        Err(e) => Err(e),
                    }
                }
                cmd => self.execute(&client, cmd),
            },
        };
        self.answer(route, &client, req.id, result, now);
    }

    /// Answers request `id`, received on `route`: sends the reply to it and to every copy
    /// of the request that arrived while it ran, and remembers it for later retries.
    pub(super) fn answer(
        &mut self,
        route: &Route,
        client: &Peer,
        id: RequestId,
        result: Result<ReplyBody, ProtoError>,
        now: Instant,
    ) {
        let reply = Reply::new(id, result).with_warnings(std::mem::take(&mut self.reply_warnings));
        let Some(bytes) = encode_bounded(&reply) else {
            return;
        };
        let owed = self.dedup.complete(client, id, route, &bytes, now);
        if owed.is_empty() {
            self.s.outbox.reply(route, &bytes);
        }
        for r in &owed {
            self.s.outbox.reply(r, &bytes);
        }
    }

    pub(super) fn send_reply(&self, route: &Route, r: &Reply) {
        if let Some(b) = encode_bounded(r) {
            self.s.outbox.reply(route, &b);
        }
    }
}

/// `r` encoded, or in its place an error reply to the same id: `internal` when it cannot be
/// encoded, `too_large` when it is over [`MAX_CTRL_BYTES`] (the client would discard it
/// unread and wait out its deadline). `None` only when not even the error encodes.
fn encode_bounded(r: &Reply) -> Option<Vec<u8>> {
    let error = match encode_reply(r) {
        Ok(b) if b.len() <= MAX_CTRL_BYTES => return Some(b),
        Ok(b) => {
            tracing::warn!("reply to request {} is {} bytes; refused", r.id.0, b.len());
            perr(
                ErrorCode::TooLarge,
                format!(
                    "the reply is {:.1} MiB, over the {} MiB limit of one message",
                    b.len() as f64 / f64::from(1 << 20),
                    MAX_CTRL_BYTES >> 20
                ),
            )
        }
        Err(e) => {
            tracing::error!("reply not encodable: {e}");
            internal(e.to_string())
        }
    };
    encode_reply(&Reply::new(r.id, Err(error))).ok()
}

#[cfg(test)]
mod tests {
    use ac2_proto::units::Blob;
    use ac2_proto::{ErrorCode, decode_reply};

    use super::*;

    /// A reply over the ctrl limit reaches the client as `too_large` to the same id, never
    /// as a message the client would discard unread; one at the limit goes as it is.
    #[test]
    fn an_oversize_reply_is_answered_too_large() {
        let id = RequestId(17);
        let big = Reply::new(id, Ok(ReplyBody::Chunk(Blob(vec![7; MAX_CTRL_BYTES]))));
        let b = encode_bounded(&big).expect("encoded");
        assert!(b.len() <= MAX_CTRL_BYTES);
        let r = decode_reply(&b).expect("decodes");
        assert_eq!(r.id, id);
        let e = r.result.expect_err("refused");
        assert_eq!(e.code, ErrorCode::TooLarge);
        assert!(e.msg.contains("4 MiB"), "{}", e.msg);

        let fits = Reply::new(id, Ok(ReplyBody::Chunk(Blob(vec![7; 1 << 20]))));
        let r = decode_reply(&encode_bounded(&fits).expect("encoded")).expect("decodes");
        assert_eq!(r.result, Ok(ReplyBody::Chunk(Blob(vec![7; 1 << 20]))));
    }
}
