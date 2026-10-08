//! The wire side of the control thread: keepalive frames, request decoding, dedup and replies.

use std::sync::atomic::Ordering;
use std::time::Instant;

use ac2_proto::frame::{Frame, FrameData, FrameStamp, GenSummary, KaMeta, ProtectionFlags};
use ac2_proto::topic::Topic;
use ac2_proto::units::{ClientId, RequestId, SampleIndex, WallNs};
use ac2_proto::{
    Command, ErrorCode, PROTO_VERSION, ProtoError, Reply, ReplyBody, decode_request, encode_reply,
    peek_envelope,
};

use crate::util::{hex, perr, wall_ns};

use super::{Control, PendingFind, PendingReply, mutation_conflict};

impl Control {
    pub(super) fn send_ka(&mut self) {
        // Keepalives tell subscribers the daemon is alive; with nobody subscribed there is
        // no one to tell, and skipping them spares the I/O thread a wakeup each.
        if !self.s.interest.wants(&Topic::Ka.to_bytes()) {
            return;
        }
        self.ka_seq += 1;
        let st = self.store.state();
        let now = wall_ns();
        let latest = self
            .session
            .as_ref()
            .map_or(0u64, |r| r.fanout.latest.load(Ordering::Acquire));
        let frame = Frame {
            stamp: FrameStamp {
                seq: self.ka_seq,
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
                    armed: st.generator.armed,
                    firing: st.generator.firing,
                },
            }),
        };
        match ac2_proto::encode_frame(&frame) {
            Ok(parts) => self.s.outbox.ka(parts),
            Err(e) => tracing::error!("keepalive not encodable: {e}"),
        }
    }

    pub(super) fn on_request(
        &mut self,
        routing_id: &[u8],
        user_id: Option<String>,
        payload: &[u8],
    ) {
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
            tracing::warn!("refused a client at protocol version {:?}", env.v);
            self.send_reply(routing_id, &Reply::version_refusal(id, env.v));
            return;
        }
        let req = match decode_request(payload) {
            Ok(r) => r,
            Err(e) => {
                self.send_reply(
                    routing_id,
                    &Reply::new(id, Err(perr(ErrorCode::Invalid, e.to_string()))),
                );
                return;
            }
        };
        if let Some(name) = &user_id
            && self.s.server.revoked(name)
        {
            // The handshake check refuses it from now on; a connection already up must
            // not keep acting on the rig until it drops.
            tracing::warn!(target: "ac2d::auth", "request from revoked client {name:?} refused");
            self.send_reply(
                routing_id,
                &Reply::new(
                    id,
                    Err(perr(
                        ErrorCode::Refused,
                        format!("the key of client {name:?} was revoked on this rig"),
                    )),
                ),
            );
            return;
        }
        let client = ClientId(user_id.unwrap_or_else(|| format!("local-{}", hex(routing_id))));
        if let Some(stored) = self.dedup.get(&client, req.id, now) {
            let stored = stored.to_vec();
            tracing::debug!("{} retried request {}; stored reply", client.0, req.id.0);
            self.s.outbox.reply(routing_id, &stored);
            return;
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
                            self.detecting = Some((
                                token,
                                PendingReply {
                                    routing_id: routing_id.to_vec(),
                                    client,
                                    id: req.id,
                                },
                            ));
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
                        self.pending_finds.insert(
                            token,
                            PendingFind {
                                routing_id: routing_id.to_vec(),
                                client,
                                id: req.id,
                                meas,
                            },
                        );
                        return;
                    }
                    Err(e) => Err(e),
                },
                cmd => self.execute(&client, cmd),
            },
        };
        self.answer(routing_id, &client, req.id, result, now);
    }

    /// Sends a reply and remembers it for retries of `id`.
    pub(super) fn answer(
        &mut self,
        routing_id: &[u8],
        client: &ClientId,
        id: RequestId,
        result: Result<ReplyBody, ProtoError>,
        now: Instant,
    ) {
        let reply = Reply::new(id, result);
        let bytes = match encode_reply(&reply) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!("reply not encodable: {e}");
                match encode_reply(&Reply::new(
                    id,
                    Err(perr(ErrorCode::Internal, e.to_string())),
                )) {
                    Ok(b) => b,
                    Err(_) => return,
                }
            }
        };
        self.dedup.insert(client, id, bytes.clone(), now);
        self.s.outbox.reply(routing_id, &bytes);
    }

    pub(super) fn send_reply(&self, routing_id: &[u8], r: &Reply) {
        match encode_reply(r) {
            Ok(b) => self.s.outbox.reply(routing_id, &b),
            Err(e) => tracing::error!("reply not encodable: {e}"),
        }
    }
}
