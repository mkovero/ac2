//! Art-Net cues: the rig's one DMX universe, made from the cue states the OSC table keeps
//! (each channel at the highest level any current state lists, 0 elsewhere) and sent to the
//! rig's Art-Net target.
//!
//! A frame that changes is sent at once; the current frame is sent again every
//! [`KEEPALIVE`] while a target is set, because Art-Net nodes drop a source that has been
//! silent for about 4 s (and with it, under HTP merge, the light it held).

use std::time::{Duration, Instant};

use ac2_proto::event::Change;
use ac2_proto::model::{ArtnetTarget, DMX_CHANNELS, DmxSet, OutputStatus, dmx_htp};
use ac2_proto::units::Rev;
use ac2_proto::{ErrorCode, ProtoError, ReplyBody};

use super::Control;
use super::osc::{REPEAT, status_due};
use crate::artnet::ArtnetSender;
use crate::rig::RigSettings;
use crate::util::perr;

/// How often the current frame is sent again.
pub(crate) const KEEPALIVE: Duration = REPEAT;

/// How long an `artnet.test` frame holds before the cue frame comes back: long enough to
/// walk to the fixture and look.
pub(crate) const TEST_HOLD: Duration = Duration::from_secs(5);

/// Why `a` cannot be an Art-Net target.
pub(crate) fn check_addr(a: std::net::SocketAddr) -> Result<(), String> {
    if !a.is_ipv4() {
        return Err(format!("{a}: Art-Net is IPv4 only"));
    }
    if a.ip().is_unspecified() || a.port() == 0 {
        return Err(format!(
            "{a} is not an address to send to: give a node's IP (or a broadcast address) and a port"
        ));
    }
    Ok(())
}

/// The frame last sent, the test hold and the sender.
#[derive(Debug)]
pub(crate) struct Universe {
    sender: ArtnetSender,
    /// The cue frame as last made, to tell when it changes.
    cue_frame: [u8; DMX_CHANNELS],
    /// The frame last sent (cue or test); `None`: nothing since the target was set.
    sent: Option<[u8; DMX_CHANNELS]>,
    /// While a test frame holds.
    test_until: Option<Instant>,
    next_keepalive: Option<Instant>,
    /// When `rig.artnet` was last committed for the sent counter alone.
    pub(super) status_at: Option<Instant>,
}

impl Default for Universe {
    fn default() -> Self {
        Self {
            sender: ArtnetSender::default(),
            cue_frame: [0; DMX_CHANNELS],
            sent: None,
            test_until: None,
            next_keepalive: None,
            status_at: None,
        }
    }
}

impl Universe {
    pub(crate) fn status(&self) -> &OutputStatus {
        self.sender.status()
    }

    fn send(
        &mut self,
        target: ArtnetTarget,
        frame: [u8; DMX_CHANNELS],
        now: Instant,
    ) -> Result<(), String> {
        let r = self.sender.send(target.addr, target.universe, &frame);
        self.sent = Some(frame);
        self.next_keepalive = Some(now + KEEPALIVE);
        r
    }
}

impl Control {
    /// Takes the cue frame the current states make and sends it when it changed (or the
    /// target has not had a frame yet). A changed cue frame ends a test hold at once.
    pub(super) fn artnet_sync(&mut self, now: Instant) {
        let frame = self.osc.frame();
        let u = &mut self.artnet;
        if frame != u.cue_frame {
            u.cue_frame = frame;
            u.test_until = None;
        }
        if u.test_until.is_some_and(|t| t <= now) {
            u.test_until = None;
        }
        let Some(target) = self.store.state().rig.artnet_target else {
            u.sent = None;
            u.next_keepalive = None;
            u.test_until = None;
            return;
        };
        if u.test_until.is_none() && u.sent != Some(u.cue_frame) {
            let f = u.cue_frame;
            // Counted and kept in the status; nothing else to do with a lost datagram.
            let _ = u.send(target, f, now);
        }
    }

    /// The keepalive, when due: the frame last sent, again.
    pub(super) fn artnet_tick(&mut self, now: Instant) {
        let Some(target) = self.store.state().rig.artnet_target else {
            return;
        };
        let u = &mut self.artnet;
        if u.next_keepalive.is_some_and(|t| t <= now) {
            let f = u.sent.unwrap_or(u.cue_frame);
            let _ = u.send(target, f, now);
            self.commit_artnet_status(false);
        }
    }

    /// When the control loop must wake for the keepalive or the end of a test hold.
    pub(super) fn artnet_wake(&self) -> Option<Instant> {
        match (self.artnet.next_keepalive, self.artnet.test_until) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Clean shutdown: one frame of every cue's `ok` levels.
    pub(super) fn artnet_shutdown(&mut self) {
        if let Some(target) = self.store.state().rig.artnet_target {
            let f = self.osc.ok_frame();
            let _ = self.artnet.send(target, f, Instant::now());
        }
    }

    /// Commits `rig.artnet` when [`status_due`]; `now_too` for an operator's command.
    fn commit_artnet_status(&mut self, now_too: bool) {
        let now = Instant::now();
        if status_due(
            &self.store.state().rig.artnet,
            self.artnet.status(),
            self.artnet.status_at,
            now_too,
            now,
        ) {
            self.artnet.status_at = Some(now);
            let mut rig = self.store.state().rig.clone();
            rig.artnet = self.artnet.status().clone();
            self.commit(Change::Rig(rig));
        }
    }

    /// `artnet.target`.
    pub(super) fn artnet_target(
        &mut self,
        base: Rev,
        target: Option<ArtnetTarget>,
    ) -> Result<ReplyBody, ProtoError> {
        self.rig_base(base)?;
        if let Some(t) = target {
            check_addr(t.addr).map_err(|e| perr(ErrorCode::Invalid, e))?;
        }
        if self.store.state().rig.artnet_target == target {
            return Ok(ReplyBody::Ack {
                rev: self.store.rev(),
            });
        }
        // Kept first: a target that would come back different after a restart would send
        // the lights' levels somewhere the operator did not choose.
        self.rig.persist(&RigSettings {
            artnet_target: target,
            ..self.rig_settings_now()
        })?;
        tracing::info!(
            "Art-Net target {}",
            target.map_or_else(
                || "off".to_owned(),
                |t| format!("{} universe {} ({})", t.addr, t.universe, t.universe.get())
            )
        );
        let rev = self.commit_rig(|r| r.artnet_target = target);
        // The new receiver gets the current frame now, not at the next keepalive.
        self.artnet.sent = None;
        self.artnet.test_until = None;
        self.artnet_sync(Instant::now());
        self.commit_artnet_status(true);
        Ok(ReplyBody::Ack { rev })
    }

    /// `artnet.test`.
    pub(super) fn artnet_test(&mut self, channels: &[DmxSet]) -> Result<ReplyBody, ProtoError> {
        let Some(target) = self.store.state().rig.artnet_target else {
            return Err(perr(
                ErrorCode::Invalid,
                "no Art-Net target is set (artnet.target)",
            ));
        };
        if let Some((_, d)) = channels
            .iter()
            .enumerate()
            .find(|(i, d)| channels[..*i].iter().any(|x| x.channel == d.channel))
        {
            return Err(perr(
                ErrorCode::Invalid,
                format!("channel {} is listed twice", d.channel),
            ));
        }
        let now = Instant::now();
        let frame = dmx_htp([channels]);
        let r = self.artnet.send(target, frame, now);
        self.artnet.test_until = Some(now + TEST_HOLD);
        self.commit_artnet_status(true);
        r.map_err(|e| perr(ErrorCode::Internal, e))?;
        Ok(ReplyBody::Ack {
            rev: self.store.rev(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ipv4_hosts_and_broadcasts_with_a_port() {
        assert!(check_addr("2.255.255.255:6454".parse().expect("test value")).is_ok());
        assert!(check_addr("192.168.1.30:6454".parse().expect("test value")).is_ok());
        assert!(check_addr("0.0.0.0:6454".parse().expect("test value")).is_err());
        assert!(check_addr("192.168.1.30:0".parse().expect("test value")).is_err());
        assert!(check_addr("[fe80::1]:6454".parse().expect("test value")).is_err());
    }
}
