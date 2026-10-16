//! Texts of the rig's Art-Net universe, shared by the Rig monitor settings page and the SPL
//! limit editor's cue rows: the target and universe as typed and back, a test frame's
//! levels, how sending fares and the channels the cues hold above 0 — in the words of
//! `ac2 artnet` and `ac2 rig status`.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use ac2_proto::model::{
    ARTNET_PORT, ArtnetTarget, DmxSet, PortAddress, Rig, dmx_list_text, parse_dmx_list,
};

/// The Rig monitor page's Art-Net rows.
pub const TARGET_TITLE: &str = "Art-Net target";
pub const UNIVERSE_TITLE: &str = "Universe";
pub const TEST_TITLE: &str = "Send DMX test";
/// The levels Send DMX test starts with.
pub const DEFAULT_TEST: &str = "1=255";
/// Under the target row.
pub const TARGET_NOTE: &str = "IPv4 address of a node or a broadcast address (2.255.255.255), \
                               port 6454 unless given; empty: off";
/// Under the universe row.
pub const UNIVERSE_NOTE: &str =
    "a number 0–32767 or net.sub.uni (0.0.1); Enter on either row applies both";
/// Under the test row.
pub const TEST_NOTE: &str = "Enter sends these levels, every other channel 0: the test frame \
                             holds 5 s, then the cue levels return";
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
            "{t:?}: type an IPv4 address with an optional port, like 2.255.255.255 or \
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

/// A test frame's levels as typed: at least one `channel=value`.
pub fn parse_test(text: &str) -> Result<Vec<DmxSet>, String> {
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

/// A cue state's levels in their field: `12=255 13=0`, empty for none.
pub fn cue_text(l: &[DmxSet]) -> String {
    dmx_list_text(l)
}

/// `Art-Net to 192.168.1.50:6454 universe 0.0.1 (1) · 12 frames sent · last error: …`;
/// off: `Art-Net off`.
pub fn status_line(rig: &Rig) -> String {
    let mut s = target_text(rig.artnet_target);
    if rig.artnet_target.is_some() || rig.artnet.sent > 0 {
        s.push_str(&format!(" · {}", sent_text(rig.artnet.sent)));
    }
    if let Some(e) = &rig.artnet.last_error {
        s.push_str(&format!(" · last error: {e}"));
    }
    s
}

fn sent_text(n: u64) -> String {
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
        rig.artnet.sent = 40;
        rig.artnet.last_error = Some("network unreachable".into());
        assert_eq!(
            status_line(&rig),
            "Art-Net to 192.168.1.50:6454 universe 0.0.1 (1) · 40 frames sent · last error: \
             network unreachable"
        );
        rig.artnet_target = None;
        rig.artnet.last_error = None;
        assert_eq!(status_line(&rig), "Art-Net off · 40 frames sent");
        rig.dmx = parse_dmx_list("12=255 14=40").expect("test value");
        assert_eq!(channels_line(&rig), "channels above 0: 12=255 14=40");
    }
}
