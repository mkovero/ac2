//! Texts of the rig's Art-Net universe, shared by the Rig monitor settings page and the SPL
//! limit editor's cue rows: the target and universe as typed and back, a test frame's
//! levels, how sending fares and the channels the cues hold above 0 — in the words of
//! `ac2 artnet` and `ac2 rig status`.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use ac2_proto::model::{
    ARTNET_PORT, ArtnetBindFailure, ArtnetFade, ArtnetSocket, ArtnetTarget, DmxEntry, DmxLevel,
    DmxOutput, DmxSet, PortAddress, Rig, dmx_list_text, parse_dmx_list,
};

/// The Rig monitor page's Art-Net rows.
pub const TARGET_TITLE: &str = "Art-Net target";
pub const UNIVERSE_TITLE: &str = "Art-Net universe";
pub const FADE_TITLE: &str = "DMX fade (s)";
pub const TEST_TITLE: &str = "Send DMX test";
/// The levels Send DMX test starts with.
pub const DEFAULT_TEST: &str = "1=255";
/// Under the target row.
pub const TARGET_NOTE: &str = "IPv4 address of a node, port 6454 unless given (a broadcast \
                               address works, but Art-Net 4 asks for unicast); empty: off";
/// Under the universe row.
pub const UNIVERSE_NOTE: &str =
    "a number 0–32767 or net.sub.uni (0.0.1); Enter on either row applies both";
/// Under the fade row.
pub const FADE_NOTE: &str = "seconds the lights take to move to a new cue state, evenly in \
                             brightness, 0–60; 0 or empty: they jump";
/// Under the test row.
pub const TEST_NOTE: &str = "Enter sends these levels to Art-Net and sACN, every other \
                             channel 0: the test frame holds 5 s, then the cue levels return";
/// A cue row's DMX line in the limit editor, under its OSC line.
pub const CUE_LINE: &str = "DMX";

/// The target's address as typed: empty (or `off`) sends nowhere; else an IPv4 address,
/// port [`ARTNET_PORT`] unless given.
pub fn parse_addr(text: &str) -> Result<Option<SocketAddr>, String> {
    let t = text.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("off") {
        return Ok(None);
    }
    if let Ok(ip) = t.parse::<Ipv4Addr>() {
        return Ok(Some(SocketAddrV4::new(ip, ARTNET_PORT).into()));
    }
    match t.parse::<SocketAddrV4>() {
        Ok(a) if a.port() != 0 => Ok(Some(a.into())),
        _ => Err(format!(
            "{t:?}: type an IPv4 address with an optional port, like 192.168.1.40 or \
             192.168.1.40:6454 (empty: off)"
        )),
    }
}

/// The target as typed on its two rows: `None` when the address is empty, else the address
/// with its universe (`N` or `net.sub.uni`), which then has to be given.
pub fn parse_target(addr: &str, universe: &str) -> Result<Option<ArtnetTarget>, String> {
    let Some(addr) = parse_addr(addr)? else {
        return Ok(None);
    };
    let u = universe.trim();
    if u.is_empty() {
        return Err("type the universe too, like 0.0.1 or 1".into());
    }
    let universe = u.parse::<PortAddress>().map_err(|e| e.to_string())?;
    Ok(Some(ArtnetTarget { addr, universe }))
}

/// The target on its two rows: the address and `net.sub.uni`, both empty when off.
pub fn target_texts(t: Option<ArtnetTarget>) -> (String, String) {
    t.map_or_else(Default::default, |t| {
        (t.addr.to_string(), t.universe.to_string())
    })
}

/// A universe as `ac2 artnet` says it, in both numberings: `0.0.1 (1)`.
pub fn universe_text(u: PortAddress) -> String {
    format!("{u} ({})", u.get())
}

/// The target as its toast and the status line say it.
pub fn target_text(t: Option<ArtnetTarget>) -> String {
    match t {
        Some(t) => format!(
            "Art-Net to {} universe {}",
            t.addr,
            universe_text(t.universe)
        ),
        None => "Art-Net off".to_owned(),
    }
}

/// The fade as typed: seconds 0–60, an `s` after the number allowed; empty (or `off`)
/// jumps.
pub fn parse_fade(text: &str) -> Result<ArtnetFade, String> {
    let t = text.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("off") {
        return Ok(ArtnetFade::SNAP);
    }
    let n = t.strip_suffix('s').unwrap_or(t).trim();
    let sec = n
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("{t:?}: type the fade in seconds, like 1 or 0.5 (0: jump)"))?;
    ArtnetFade::new(sec).map_err(|e| e.to_string())
}

/// The fade on its row: the seconds as a plain number (`1`, `0.25`).
pub fn fade_text(f: ArtnetFade) -> String {
    format!("{}", f.seconds())
}

/// The fade as its toast says it: `DMX fade 1.5 s`, or `DMX fade off` for 0. One fade
/// serves Art-Net and sACN.
pub fn fade_toast(f: ArtnetFade) -> String {
    if f.seconds() == 0.0 {
        "DMX fade off: cue changes jump".to_owned()
    } else {
        format!("DMX fade {} s", f.seconds())
    }
}

/// Port 6454 could not be bound: what the operator is told when setting a target, and what
/// the status line keeps saying.
pub fn not_conforming_text(from_port: u16, reason: ArtnetBindFailure) -> String {
    let why = match reason {
        ArtnetBindFailure::PortTaken => "another program holds it",
        ArtnetBindFailure::Refused => "the system refused it",
    };
    format!(
        "Art-Net sends from port {from_port}, not {ARTNET_PORT} ({why}): frames still go out, \
         but nodes that check the source port drop them, and consoles will not list the rig"
    )
}

/// How the Art-Net socket stands, for the status line: nothing while closed.
pub fn socket_text(s: &ArtnetSocket) -> Option<String> {
    match s {
        ArtnetSocket::Closed => None,
        ArtnetSocket::Conforming => Some(format!("from port {ARTNET_PORT}, answers ArtPoll")),
        ArtnetSocket::NotConforming {
            from_port,
            reason,
            os_error,
        } => Some(format!(
            "{} — {os_error}",
            not_conforming_text(*from_port, *reason)
        )),
        // The system's words are the status's last error.
        ArtnetSocket::CannotOpen { .. } => {
            Some("no Art-Net socket: nothing is sent; tried again every 30 s".to_owned())
        }
    }
}

/// Universe 0 set: what the reply and the status line say.
pub const UNIVERSE_ZERO: &str = "universe 0 is deprecated by Art-Net 4 (sACN has no universe \
                                 0), so a node may not take it: use 0.0.1 or above";

/// An `artnet.test` frame that one output did not send while the other did.
pub fn send_failed_text(output: DmxOutput, error: &str) -> String {
    let name = match output {
        DmxOutput::Artnet => "Art-Net",
        DmxOutput::Sacn => "sACN",
    };
    format!("the test frame did not go out on {name}: {error}")
}

/// Whether the Art-Net status line is one the operator must act on: a send error, a
/// socket off Art-Net's port or none, or universe 0.
pub fn status_warns(rig: &Rig) -> bool {
    rig.artnet.last_error.is_some()
        || socket_is_warning(&rig.artnet_socket)
        || rig.artnet_target.is_some_and(|t| t.universe.get() == 0)
}

/// Whether the socket's state is one the operator must act on.
pub fn socket_is_warning(s: &ArtnetSocket) -> bool {
    matches!(
        s,
        ArtnetSocket::NotConforming { .. } | ArtnetSocket::CannotOpen { .. }
    )
}

/// A test frame's levels as typed: at least one `channel=value`.
pub fn parse_test(text: &str) -> Result<Vec<DmxLevel>, String> {
    let l = parse_dmx_list(text).map_err(|e| e.to_string())?;
    if l.is_empty() {
        return Err(format!("type levels to send, like {DEFAULT_TEST}"));
    }
    Ok(l)
}

/// A cue state's levels as typed: empty holds none.
pub fn parse_cue(text: &str) -> Result<Vec<DmxSet>, String> {
    parse_dmx_list(text).map_err(|e| e.to_string())
}

/// Levels in their field: `12=255 13=0!`, empty for none.
pub fn cue_text<T: DmxEntry>(l: &[T]) -> String {
    dmx_list_text(l)
}

/// `Art-Net to 192.168.1.50:6454 universe 0.0.1 (1) · 12 frames sent · last error: …`;
/// off: `Art-Net off`.
pub fn status_line(rig: &Rig) -> String {
    let mut s = target_text(rig.artnet_target);
    if rig.artnet_target.is_some_and(|t| t.universe.get() == 0) {
        s.push_str(&format!(" · {UNIVERSE_ZERO}"));
    }
    if rig.artnet_target.is_some() || rig.artnet.sent > 0 {
        s.push_str(&format!(" · {}", sent_text(rig.artnet.sent)));
    }
    if let Some(t) = socket_text(&rig.artnet_socket) {
        s.push_str(&format!(" · {t}"));
    }
    if let Some(e) = &rig.artnet.last_error {
        s.push_str(&format!(" · last error: {e}"));
    }
    s
}

pub(crate) fn sent_text(n: u64) -> String {
    if n == 1 {
        "1 frame sent".into()
    } else {
        format!("{n} frames sent")
    }
}

/// The channels the cues' current states hold: `channels above 0: 12=255 14=40`, or
/// `all channels 0`.
pub fn channels_line(rig: &Rig) -> String {
    if rig.dmx.is_empty() {
        "all channels 0".to_owned()
    } else {
        format!("channels above 0: {}", dmx_list_text(&rig.dmx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::OutputStatus;

    #[test]
    fn the_target_is_an_ipv4_address_and_a_universe_or_off() {
        assert_eq!(parse_target("", "0.0.1"), Ok(None));
        assert_eq!(parse_target(" off ", ""), Ok(None));
        let t = parse_target("2.255.255.255", "0.0.1")
            .expect("test value")
            .expect("test value");
        assert_eq!(t.addr.to_string(), "2.255.255.255:6454");
        assert_eq!(t.universe.get(), 1);
        let t = parse_target("192.168.1.40:6455", "17")
            .expect("test value")
            .expect("test value");
        assert_eq!(
            target_texts(Some(t)),
            ("192.168.1.40:6455".into(), "0.1.1".into())
        );
        assert_eq!(target_texts(None), (String::new(), String::new()));
    }

    #[test]
    fn a_bad_target_or_universe_says_why() {
        for bad in ["node.local", "[fe80::1]:6454", "10.0.0.1:0", "10.0.0"] {
            let e = parse_target(bad, "1").expect_err(bad);
            assert!(e.contains("IPv4 address"), "{bad}: {e}");
        }
        assert_eq!(
            parse_target("10.0.0.1", " "),
            Err("type the universe too, like 0.0.1 or 1".into())
        );
        assert_eq!(
            parse_target("10.0.0.1", "0.16.0"),
            Err("\"0.16.0\": net is 0–127, sub-net 0–15, universe 0–15".into())
        );
        assert_eq!(
            parse_target("10.0.0.1", "40000"),
            Err("Art-Net port-address 40000 is above 32767".into())
        );
        assert!(
            parse_target("10.0.0.1", "one")
                .expect_err("a word")
                .contains("net.sub.uni")
        );
    }

    #[test]
    fn the_fade_is_seconds_0_to_60_or_off() {
        let secs = |t: &str| parse_fade(t).map(ArtnetFade::seconds);
        assert_eq!(secs("1.5"), Ok(1.5));
        assert_eq!(secs(" 2s "), Ok(2.0));
        assert_eq!(secs(""), Ok(0.0));
        assert_eq!(secs("off"), Ok(0.0));
        assert_eq!(secs("60"), Ok(60.0));
        assert_eq!(
            secs("61"),
            Err("Art-Net fade 61 s is outside 0–60 s".into())
        );
        assert_eq!(
            secs("-1"),
            Err("Art-Net fade -1 s is outside 0–60 s".into())
        );
        assert_eq!(
            secs("slow"),
            Err("\"slow\": type the fade in seconds, like 1 or 0.5 (0: jump)".into())
        );
        assert!(secs("inf").is_err() && secs("NaN").is_err());
        assert_eq!(fade_text(ArtnetFade::DEFAULT), "1");
        assert_eq!(fade_text(ArtnetFade::new(0.25).expect("valid")), "0.25");
        assert_eq!(
            fade_toast(ArtnetFade::new(0.25).expect("valid")),
            "DMX fade 0.25 s"
        );
        assert_eq!(
            fade_toast(ArtnetFade::SNAP),
            "DMX fade off: cue changes jump"
        );
    }

    #[test]
    fn levels_parse_as_typed_and_say_why_not() {
        assert_eq!(
            parse_test(" 12=255, 13=0 ").map(|l| cue_text(&l)),
            Ok("12=255 13=0".into())
        );
        assert_eq!(
            parse_test(""),
            Err("type levels to send, like 1=255".into())
        );
        assert_eq!(parse_cue(""), Ok(vec![]));
        assert_eq!(
            parse_cue("513=1"),
            Err("\"513=1\": the channel is a number 1–512".into())
        );
        assert_eq!(
            parse_cue("12=256"),
            Err("\"12=256\": the value is a number 0–255".into())
        );
        assert_eq!(
            parse_cue("12"),
            Err("\"12\": expected channel=value, e.g. 12=255".into())
        );
    }

    #[test]
    fn the_status_line_says_target_frames_and_error() {
        let mut rig = Rig::default();
        assert_eq!(status_line(&rig), "Art-Net off");
        assert_eq!(channels_line(&rig), "all channels 0");
        rig.artnet_target = parse_target("192.168.1.50", "0.0.1").expect("test value");
        rig.artnet = OutputStatus {
            sent: 1,
            last_error: None,
        };
        assert_eq!(
            status_line(&rig),
            "Art-Net to 192.168.1.50:6454 universe 0.0.1 (1) · 1 frame sent"
        );
        rig.artnet_socket = ArtnetSocket::Conforming;
        assert_eq!(
            status_line(&rig),
            "Art-Net to 192.168.1.50:6454 universe 0.0.1 (1) · 1 frame sent · from port 6454, \
             answers ArtPoll"
        );
        rig.artnet_socket = ArtnetSocket::NotConforming {
            from_port: 40000,
            reason: ArtnetBindFailure::PortTaken,
            os_error: "Address in use (os error 98)".into(),
        };
        assert_eq!(
            status_line(&rig),
            "Art-Net to 192.168.1.50:6454 universe 0.0.1 (1) · 1 frame sent · Art-Net sends \
             from port 40000, not 6454 (another program holds it): frames still go out, but \
             nodes that check the source port drop them, and consoles will not list the rig — \
             Address in use (os error 98)"
        );
        assert_eq!(
            not_conforming_text(51000, ArtnetBindFailure::Refused),
            "Art-Net sends from port 51000, not 6454 (the system refused it): frames still go \
             out, but nodes that check the source port drop them, and consoles will not list \
             the rig"
        );
        assert!(socket_is_warning(&rig.artnet_socket));
        rig.artnet_socket = ArtnetSocket::CannotOpen {
            os_error: "Too many open files (os error 24)".into(),
        };
        rig.artnet.last_error =
            Some("cannot open an Art-Net socket: Too many open files (os error 24)".into());
        assert_eq!(
            status_line(&rig),
            "Art-Net to 192.168.1.50:6454 universe 0.0.1 (1) · 1 frame sent · no Art-Net \
             socket: nothing is sent; tried again every 30 s · last error: cannot open an \
             Art-Net socket: Too many open files (os error 24)"
        );
        assert!(socket_is_warning(&rig.artnet_socket));
        rig.artnet.last_error = None;
        rig.artnet_socket = ArtnetSocket::Closed;
        assert!(!socket_is_warning(&rig.artnet_socket));
        rig.artnet.sent = 40;
        rig.artnet.last_error = Some("network unreachable".into());
        assert_eq!(
            status_line(&rig),
            "Art-Net to 192.168.1.50:6454 universe 0.0.1 (1) · 40 frames sent · last error: \
             network unreachable"
        );
        assert!(status_warns(&rig));
        rig.artnet.last_error = None;
        assert!(!status_warns(&rig));
        rig.artnet_target = parse_target("192.168.1.50", "0").expect("test value");
        assert_eq!(
            status_line(&rig),
            "Art-Net to 192.168.1.50:6454 universe 0.0.0 (0) · universe 0 is deprecated by \
             Art-Net 4 (sACN has no universe 0), so a node may not take it: use 0.0.1 or above \
             · 40 frames sent"
        );
        assert!(status_warns(&rig));
        rig.artnet_target = None;
        assert_eq!(status_line(&rig), "Art-Net off · 40 frames sent");
        rig.dmx = parse_dmx_list("12=255 14=40").expect("test value");
        assert_eq!(channels_line(&rig), "channels above 0: 12=255 14=40");
    }
}
