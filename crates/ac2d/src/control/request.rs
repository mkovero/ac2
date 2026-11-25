//! The wire side of the control thread: keepalive frames, request decoding, dedup and replies.

use std::sync::atomic::Ordering;
use std::time::Instant;

use ac2_proto::frame::{Frame, FrameData, FrameStamp, GenSummary, KaMeta, ProtectionFlags};
use ac2_proto::topic::Topic;
use ac2_proto::units::{RequestId, SampleIndex, WallNs};
use ac2_proto::{
    Command, CtrlError, ErrorCode, MAX_CTRL_BYTES, PROTO_VERSION, ProtoError, Reply, ReplyBody,
    decode_request, encode_reply, peek_envelope,
};

use ac2_proto::printable::printable;

use crate::dedup::{Admit, Retry};
use crate::health::StreamState;
use crate::refusals::LOGGED_ERROR_CHARS;
use crate::util::{internal, invalid, perr, wall_ns};

use super::{Control, PendingDetect, PendingFind, PendingReply, PreviewKey};
use crate::route::{Peer, Route};

impl Control {
    pub(super) fn send_ka(&mut self) {
        // Keepalives tell subscribers the daemon is alive; with nobody subscribed there is
        // no one to tell, and skipping them spares the I/O thread a wakeup each.
        if !self.s.interest.wants(&Topic::Ka.to_bytes()) {
            return;
        }
        self.ka.seq += 1;
        let stream = match (&self.stream, &self.audio_recovery) {
            (Some(rt), _) => StreamState::Running(&rt.health),
            (None, Some(_)) => StreamState::Recovering,
            (None, None) => StreamState::Closed,
        };
        self.health.tick(Instant::now(), stream);
        let st = self.store.state();
        let now = wall_ns();
        let latest = self
            .stream
            .as_ref()
            .map_or(0u64, |r| r.fanout.latest.load(Ordering::Acquire));
        let frame = Frame {
            stamp: FrameStamp {
                seq: self.ka.seq,
                audio_sample: SampleIndex(latest.saturating_sub(1)),
                stream_epoch: st.stream.epoch,
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
            // Answered, not dropped: the sender would otherwise retry it unread until it
            // gives up, with no word of why.
            Err(CtrlError::TooLarge {
                bytes,
                id: Some(id),
            }) => {
                tracing::warn!("ctrl request {} of {bytes} bytes refused: too large", id.0);
                self.send_reply(
                    route,
                    &Reply::new(
                        id,
                        Err(perr(
                            ErrorCode::TooLarge,
                            format!(
                                "the request is {bytes} bytes, over the {MAX_CTRL_BYTES}-byte \
                                 ctrl message limit; a file goes up in chunks \
                                 (file.upload_begin)"
                            ),
                        )),
                    ),
                );
                return;
            }
            Err(e) => {
                // The decoder's message quotes what it could not read: bounded and one line,
                // and warned about once per client.
                let e = printable(&e.to_string(), LOGGED_ERROR_CHARS);
                if self
                    .refusals
                    .first(peer.curve().map(|(_, k)| *k), "undecodable")
                {
                    tracing::warn!("undecodable ctrl message dropped: {e} (repeats at debug)");
                } else {
                    tracing::debug!("undecodable ctrl message dropped: {e}");
                }
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
            if self
                .refusals
                .first(peer.curve().map(|(_, k)| *k), "version")
            {
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
        // Before dedup admission: a refused request stores nothing, and a retry of it after
        // a promotion runs.
        let role = match self.role_gate(&peer, &req.cmd) {
            Ok(r) => r,
            Err(e) => {
                self.send_reply(route, &Reply::new(id, Err(e)));
                return;
            }
        };
        let client = peer;
        // Reserved before anything executes, the asynchronous branches below included: a
        // copy of a request still running waits for its reply rather than running it again.
        match self
            .dedup
            .admit(&client, req.id, route, now, retry(&req.cmd))
        {
            Admit::Execute => {
                if let Err(e) = self.viewer_transfer_gate(&client, role, &req.cmd) {
                    self.answer(route, &client, req.id, Err(e), now);
                    return;
                }
            }
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
        let result = match req.cmd {
            // The finder runs for a while on the job thread; the reply follows its result
            // and other clients are served meanwhile.
            // The burst and its capture take about a second on their own thread; the
            // reply follows the result.
            Command::StreamDetectLoopback {
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
                    Ok((token, cut)) => {
                        self.pending.detecting = Some(PendingDetect {
                            token,
                            reply: PendingReply {
                                route: route.clone(),
                                client,
                                id: req.id,
                            },
                            cut,
                        });
                        return;
                    }
                    Err(e) => Err(e),
                }
            }
            // Asking a driver for its devices, or opening one, can outlast a stimulus
            // lease's refresh margin: on their own threads, so a running sweep's refreshes
            // are answered meanwhile.
            Command::StreamDevices => match self.start_devices(PendingReply {
                route: route.clone(),
                client: client.clone(),
                id: req.id,
            }) {
                Ok(()) => return,
                Err(e) => Err(e),
            },
            Command::StreamPreview {
                backend,
                device,
                inputs,
                pairs,
            } => {
                let waiter = PendingReply {
                    route: route.clone(),
                    client: client.clone(),
                    id: req.id,
                };
                match self.start_preview(
                    PreviewKey {
                        backend,
                        device,
                        inputs,
                        pairs,
                    },
                    waiter,
                ) {
                    Ok(None) => return,
                    Ok(Some(body)) => Ok(body),
                    Err(e) => Err(e),
                }
            }
            // A driver's panel may be a modal dialog: the call runs on its own thread.
            Command::StreamControlPanel { backend, device } => {
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
                match self.start_file_job(route, &client, req.id, cmd, role) {
                    Ok(()) => return,
                    Err(e) => Err(e),
                }
            }
            cmd => self.execute(&client, cmd),
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
        let mut result = result;
        if client.curve().is_some() {
            super::redact::reply(&mut result);
        }
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

/// What a retry of `cmd` gets once it is answered ([`Command::is_read`]). Every command that
/// acts keeps its reply: one that commits, and one that asks the drivers for their devices,
/// opens a device or a driver's panel, sounds a burst, keeps the finder's result, opens or
/// feeds a transfer, files an upload as a session. A pure read (a page of a log, a chunk
/// of a download at its offset, a snapshot) runs again, and its reply, megabytes some of
/// them, is not held for the window.
fn retry(cmd: &Command) -> Retry {
    if cmd.is_read() {
        Retry::Runs
    } else {
        Retry::Stored
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
