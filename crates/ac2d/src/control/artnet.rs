//! Art-Net cues: the rig's one DMX universe, made from the cue states the OSC table keeps
//! (each channel at the highest level any current state lists, 0 elsewhere) and sent to the
//! rig's Art-Net target.
//!
//! A frame that changes is sent at once; the current frame is sent again every
//! [`KEEPALIVE`] while a target is set, because Art-Net nodes drop a source that has been
//! silent for about 4 s (and with it, under HTP merge, the light it held).
//!
//! A changed cue frame fades in over the rig's `artnet_fade`: every channel moves from the
//! level last output to its new level in perceived-brightness steps, a frame every
//! [`FADE_STEP`]. A test frame, the cue frame coming back after it and the shutdown frame
//! jump: they are for looking at a fixture or for leaving it settled.

use std::time::{Duration, Instant};

use ac2_proto::event::Change;
use ac2_proto::model::{ArtnetFade, ArtnetTarget, DMX_CHANNELS, DmxSet, OutputStatus, dmx_htp};
use ac2_proto::units::{Rev, Seconds};
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

/// How often a fade sends a frame: DMX512 refreshes a full universe about 44 times a
/// second, so steps any closer would never reach the fixture.
pub(crate) const FADE_STEP: Duration = Duration::from_millis(25);

/// Display gamma: lamps and LED drivers map DMX levels to light roughly linearly, while the
/// eye's brightness response is close to a 1/2.2 power of the light. Equal steps of
/// `(v/255)^(1/GAMMA)` therefore look like equal steps of brightness, where a linear
/// ramp of `v` would rush through the dark end and crawl at the top.
const GAMMA: f64 = 2.2;

/// The level `k` (0–1) of the way from `from` to `to` in perceived brightness; exact at
/// both ends and monotone between.
pub(crate) fn fade_level(from: u8, to: u8, k: f64) -> u8 {
    if k <= 0.0 {
        return from;
    }
    if k >= 1.0 || from == to {
        return to;
    }
    let p = |v: u8| (f64::from(v) / 255.0).powf(1.0 / GAMMA);
    let (p0, p1) = (p(from), p(to));
    let v = 255.0 * (p0 + (p1 - p0) * k).powf(GAMMA);
    // In range by construction; the clamp only guards the float rounding at the ends.
    v.round().clamp(0.0, 255.0) as u8
}

/// A fade in progress: from the frame output when it began to the cue frame.
#[derive(Debug, Clone, Copy)]
struct Fade {
    from: [u8; DMX_CHANNELS],
    to: [u8; DMX_CHANNELS],
    start: Instant,
    length: Duration,
}

impl Fade {
    /// The frame at `now`, and whether the fade has ended.
    fn at(&self, now: Instant) -> ([u8; DMX_CHANNELS], bool) {
        let t = now.saturating_duration_since(self.start);
        if t >= self.length {
            return (self.to, true);
        }
        let k = t.as_secs_f64() / self.length.as_secs_f64();
        let mut f = [0u8; DMX_CHANNELS];
        for (i, v) in f.iter_mut().enumerate() {
            *v = fade_level(self.from[i], self.to[i], k);
        }
        (f, false)
    }
}

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
    /// The frame last sent was a test frame: the cue frame comes back at once.
    after_test: bool,
    /// The fade in progress.
    fade: Option<Fade>,
    /// When the next frame goes out: the next fade step, else the keepalive.
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
            after_test: false,
            fade: None,
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
        let next = if self.fade.is_some() {
            FADE_STEP
        } else {
            KEEPALIVE
        };
        self.next_keepalive = Some(now + next);
        r
    }

    /// Sends `frame` at once, ending any fade.
    fn snap(
        &mut self,
        target: ArtnetTarget,
        frame: [u8; DMX_CHANNELS],
        now: Instant,
    ) -> Result<(), String> {
        self.fade = None;
        self.send(target, frame, now)
    }

    /// The frame being output now: the fade's present step, else the frame last sent.
    fn output(&self, now: Instant) -> Option<[u8; DMX_CHANNELS]> {
        match &self.fade {
            Some(f) => Some(f.at(now).0),
            None => self.sent,
        }
    }

    /// Where the output is going: the fade's end, else the frame last sent.
    fn goal(&self) -> Option<[u8; DMX_CHANNELS]> {
        self.fade.map(|f| f.to).or(self.sent)
    }

    /// Moves the output to the cue frame: a fade of `length` from what is output now, or a
    /// jump when there is no fade, nothing has been sent yet or a test frame is out.
    fn go_to_cue(&mut self, target: ArtnetTarget, length: Duration, now: Instant) {
        let to = self.cue_frame;
        let from = self.output(now);
        let after_test = std::mem::take(&mut self.after_test);
        match from {
            Some(from) if !length.is_zero() && !after_test && from != to => {
                self.fade = Some(Fade {
                    from,
                    to,
                    start: now,
                    length,
                });
                // The first step goes out one step from now: at `now` the fade is still
                // at the frame already on the wire.
                self.next_keepalive = Some(now + FADE_STEP);
            }
            _ => {
                // Counted and kept in the status; nothing else to do with a lost datagram.
                let _ = self.snap(target, to, now);
            }
        }
    }
}

impl Control {
    /// Takes the cue frame the current states make and sends it when it changed (or the
    /// target has not had a frame yet). A changed cue frame ends a test hold at once.
    pub(super) fn artnet_sync(&mut self, now: Instant) {
        let frame = self.rig.osc.frame();
        let u = &mut self.rig.artnet;
        if frame != u.cue_frame {
            u.cue_frame = frame;
            u.test_until = None;
        }
        if u.test_until.is_some_and(|t| t <= now) {
            u.test_until = None;
        }
        let rig = &self.store.state().rig;
        let Some(target) = rig.artnet_target else {
            u.sent = None;
            u.fade = None;
            u.after_test = false;
            u.next_keepalive = None;
            u.test_until = None;
            return;
        };
        if u.test_until.is_none() && u.goal() != Some(u.cue_frame) {
            u.go_to_cue(target, rig.artnet_fade.duration(), now);
        }
    }

    /// The next fade step or the keepalive, when due. The status commit stays throttled
    /// ([`status_due`]): a fade's steps count like keepalives.
    pub(super) fn artnet_tick(&mut self, now: Instant) {
        let Some(target) = self.store.state().rig.artnet_target else {
            return;
        };
        let u = &mut self.rig.artnet;
        if u.next_keepalive.is_some_and(|t| t <= now) {
            let f = match u.fade {
                Some(fade) => {
                    let (f, ended) = fade.at(now);
                    if ended {
                        u.fade = None;
                    }
                    f
                }
                None => u.sent.unwrap_or(u.cue_frame),
            };
            let _ = u.send(target, f, now);
            self.commit_artnet_status(false);
        }
    }

    /// When the control loop must wake for a fade step, the keepalive or the end of a test
    /// hold.
    pub(super) fn artnet_wake(&self) -> Option<Instant> {
        match (self.rig.artnet.next_keepalive, self.rig.artnet.test_until) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Clean shutdown: one frame of every cue's `ok` levels.
    pub(super) fn artnet_shutdown(&mut self) {
        if let Some(target) = self.store.state().rig.artnet_target {
            let f = self.rig.osc.ok_frame();
            let _ = self.rig.artnet.snap(target, f, Instant::now());
        }
    }

    /// Commits `rig.artnet` when [`status_due`]; `now_too` for an operator's command.
    fn commit_artnet_status(&mut self, now_too: bool) {
        let now = Instant::now();
        if status_due(
            &self.store.state().rig.artnet,
            self.rig.artnet.status(),
            self.rig.artnet.status_at,
            now_too,
            now,
        ) {
            self.rig.artnet.status_at = Some(now);
            let mut rig = self.store.state().rig.clone();
            rig.artnet = self.rig.artnet.status().clone();
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
        self.rig.settings.persist(&RigSettings {
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
        self.rig.artnet.sent = None;
        self.rig.artnet.fade = None;
        self.rig.artnet.test_until = None;
        self.artnet_sync(Instant::now());
        self.commit_artnet_status(true);
        Ok(ReplyBody::Ack { rev })
    }

    /// `artnet.fade`: applies from the next change of the cue frame; a fade in progress
    /// keeps its length.
    pub(super) fn artnet_fade(
        &mut self,
        base: Rev,
        seconds: Seconds,
    ) -> Result<ReplyBody, ProtoError> {
        self.rig_base(base)?;
        let fade =
            ArtnetFade::new(seconds.0).map_err(|e| perr(ErrorCode::Invalid, e.to_string()))?;
        if self.store.state().rig.artnet_fade == fade {
            return Ok(ReplyBody::Ack {
                rev: self.store.rev(),
            });
        }
        self.rig.settings.persist(&RigSettings {
            artnet_fade: fade,
            ..self.rig_settings_now()
        })?;
        tracing::info!("Art-Net fade {} s", fade.seconds());
        let rev = self.commit_rig(|r| r.artnet_fade = fade);
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
        let r = self.rig.artnet.snap(target, frame, now);
        self.rig.artnet.test_until = Some(now + TEST_HOLD);
        self.rig.artnet.after_test = true;
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
    fn a_fade_is_exact_at_its_ends_and_monotone() {
        for (a, b) in [
            (0u8, 255u8),
            (255, 0),
            (10, 200),
            (37, 38),
            (128, 128),
            (255, 1),
        ] {
            assert_eq!(fade_level(a, b, 0.0), a);
            assert_eq!(fade_level(a, b, 1.0), b);
            assert_eq!(fade_level(a, b, 1.5), b);
            let mut prev = a;
            for i in 1..=1000 {
                let v = fade_level(a, b, f64::from(i) / 1000.0);
                assert!(
                    if a <= b { v >= prev } else { v <= prev },
                    "{a}→{b} at {i}: {prev} then {v}"
                );
                prev = v;
            }
            assert_eq!(prev, b);
        }
        // Halfway in brightness is well below halfway in level: 255·0.5^2.2.
        assert_eq!(fade_level(0, 255, 0.5), 55);
    }

    #[test]
    fn a_fade_steps_to_its_end_and_retargets_from_the_output() {
        let target = ArtnetTarget {
            addr: "127.0.0.1:9".parse().expect("test value"),
            universe: ac2_proto::model::PortAddress::new(0).expect("test value"),
        };
        let t0 = Instant::now();
        let mut u = Universe::default();
        // Nothing sent yet: the first frame jumps.
        u.cue_frame[0] = 200;
        u.go_to_cue(target, Duration::from_secs(1), t0);
        assert!(u.fade.is_none());
        assert_eq!(u.sent.map(|f| f[0]), Some(200));
        // A change fades from what is out to the new frame.
        u.cue_frame[0] = 0;
        u.cue_frame[1] = 255;
        u.go_to_cue(target, Duration::from_secs(1), t0);
        let fade = u.fade.expect("fading");
        assert_eq!(u.goal(), Some(u.cue_frame));
        assert_eq!(u.next_keepalive, Some(t0 + FADE_STEP));
        let (mid, ended) = fade.at(t0 + Duration::from_millis(500));
        assert!(!ended);
        assert!(mid[0] > 0 && mid[0] < 200 && mid[1] > 0 && mid[1] < 255);
        assert_eq!(fade.at(t0 + Duration::from_secs(1)), (u.cue_frame, true));
        // A change mid-fade starts from the frame output then, for a whole new fade.
        let t1 = t0 + Duration::from_millis(500);
        u.cue_frame[0] = 255;
        u.go_to_cue(target, Duration::from_secs(1), t1);
        let fade = u.fade.expect("fading");
        assert_eq!((fade.from, fade.start), (mid, t1));
        assert_eq!(fade.at(t1).0, mid);
        assert!(!fade.at(t1 + Duration::from_millis(999)).1);
        // No fade time: the next change jumps and ends the fade.
        u.cue_frame[1] = 0;
        u.go_to_cue(target, Duration::ZERO, t1);
        assert!(u.fade.is_none());
        assert_eq!(u.sent, Some(u.cue_frame));
        assert_eq!(u.next_keepalive, Some(t1 + KEEPALIVE));
        // The cue frame coming back after a test frame jumps too.
        u.snap(target, [9; DMX_CHANNELS], t1).expect("loopback");
        u.after_test = true;
        u.go_to_cue(target, Duration::from_secs(1), t1);
        assert!(u.fade.is_none() && !u.after_test);
        assert_eq!(u.sent, Some(u.cue_frame));
    }

    #[test]
    fn only_ipv4_hosts_and_broadcasts_with_a_port() {
        assert!(check_addr("2.255.255.255:6454".parse().expect("test value")).is_ok());
        assert!(check_addr("192.168.1.30:6454".parse().expect("test value")).is_ok());
        assert!(check_addr("0.0.0.0:6454".parse().expect("test value")).is_err());
        assert!(check_addr("192.168.1.30:0".parse().expect("test value")).is_err());
        assert!(check_addr("[fe80::1]:6454".parse().expect("test value")).is_err());
    }
}
