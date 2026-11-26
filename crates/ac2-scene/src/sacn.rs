//! Texts of the rig's sACN (E1.31) output, beside Art-Net on the Rig monitor settings page:
//! the target, universe and priority as typed and back, and how sending fares — in the words
//! of `ac2 sacn` and `ac2 rig status`. The frame, fade and test frame are Art-Net's
//! ([`crate::artnet`]): both outputs carry the same universe.

use std::net::Ipv4Addr;

use ac2_proto::model::{Rig, SacnDest, SacnPriority, SacnTarget, SacnUniverse};

use crate::artnet::sent_text;

/// The Rig monitor page's sACN rows.
pub const TARGET_TITLE: &str = "sACN target";
pub const UNIVERSE_TITLE: &str = "sACN universe";
pub const PRIORITY_TITLE: &str = "sACN priority";
/// Under the target row.
pub const TARGET_NOTE: &str = "IPv4 address of a receiver, or multicast via this host's \
                               address on the lighting network (multicast via 10.0.0.4); \
                               empty: off";
/// Under the universe row.
pub const UNIVERSE_NOTE: &str = "a number 1–63999; Enter on any sACN row applies all three";
/// Under the priority row.
pub const PRIORITY_NOTE: &str =
    "0–200, a receiver takes the highest source; empty: 100, E1.31's default";

/// The destination as typed: empty (or `off`) sends nowhere; an IPv4 address is one
/// receiver; `multicast via <address>` (or `multicast <address>`) the universe's group, out
/// of the interface with that address.
pub fn parse_dest(text: &str) -> Result<Option<SacnDest>, String> {
    let t = text.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("off") {
        return Ok(None);
    }
    let bad = || {
        format!(
            "{t:?}: type a receiver's IPv4 address, like 192.168.1.40, or multicast via this \
             host's address, like multicast via 192.168.1.4 (empty: off)"
        )
    };
    let words: Vec<&str> = t.split_whitespace().collect();
    let dest = match words.as_slice() {
        [m] if m.eq_ignore_ascii_case("multicast") => {
            return Err(
                "type the interface too: multicast via this host's address on the lighting \
                 network, like multicast via 192.168.1.4"
                    .into(),
            );
        }
        [ip] => SacnDest::Unicast {
            addr: ip.parse().map_err(|_| bad())?,
        },
        [m, ip] | [m, "via", ip] if m.eq_ignore_ascii_case("multicast") => SacnDest::Multicast {
            interface: ip.parse().map_err(|_| bad())?,
        },
        _ => return Err(bad()),
    };
    check_dest(dest)?;
    Ok(Some(dest))
}

/// Refuses what the daemon would refuse, worded for the row.
fn check_dest(d: SacnDest) -> Result<(), String> {
    match d {
        SacnDest::Unicast { addr } if addr.is_multicast() => Err(format!(
            "{addr} is a multicast group: type multicast via this host's address instead"
        )),
        SacnDest::Unicast { addr } if addr.is_unspecified() || addr.is_broadcast() => {
            Err(format!("{addr} is no receiver: type a receiver's address"))
        }
        SacnDest::Multicast { interface } if interface.is_unspecified() => Err(format!(
            "{interface} is no interface: type this host's address on the lighting network"
        )),
        _ => Ok(()),
    }
}

/// The target as typed on its three rows: `None` when the target row is empty, else the
/// destination with its universe, which then has to be given, and its priority (empty: 100).
pub fn parse_target(
    dest: &str,
    universe: &str,
    priority: &str,
) -> Result<Option<SacnTarget>, String> {
    let Some(dest) = parse_dest(dest)? else {
        return Ok(None);
    };
    let u = universe.trim();
    if u.is_empty() {
        return Err("type the sACN universe too, a number 1–63999".into());
    }
    let universe = u.parse::<SacnUniverse>().map_err(|e| e.to_string())?;
    let p = priority.trim();
    let priority = if p.is_empty() {
        SacnPriority::DEFAULT
    } else {
        p.parse::<SacnPriority>().map_err(|e| e.to_string())?
    };
    Ok(Some(SacnTarget {
        dest,
        universe,
        priority,
    }))
}

/// The destination as its row and the status line say it.
pub fn dest_text(d: SacnDest) -> String {
    match d {
        SacnDest::Unicast { addr } => addr.to_string(),
        SacnDest::Multicast { interface } => format!("multicast via {interface}"),
    }
}

/// The target on its three rows; all empty when off.
pub fn target_texts(t: Option<SacnTarget>) -> (String, String, String) {
    t.map_or_else(Default::default, |t| {
        (
            dest_text(t.dest),
            t.universe.to_string(),
            t.priority.to_string(),
        )
    })
}

/// The group a multicast target sends to.
fn group(t: &SacnTarget) -> Option<Ipv4Addr> {
    matches!(t.dest, SacnDest::Multicast { .. }).then(|| t.universe.multicast_group())
}

/// The target as its toast and the status line say it: `sACN to 192.168.1.40 universe 1
/// priority 100`, a multicast one with its group.
pub fn target_text(t: Option<SacnTarget>) -> String {
    match t {
        Some(t) => {
            let to = match group(&t) {
                Some(g) => format!("{g} ({})", dest_text(t.dest)),
                None => dest_text(t.dest),
            };
            format!(
                "sACN to {to} universe {} priority {}",
                t.universe, t.priority
            )
        }
        None => "sACN off".to_owned(),
    }
}

/// `sACN to … · 12 frames sent · last error: …`; off: `sACN off`.
pub fn status_line(rig: &Rig) -> String {
    let mut s = target_text(rig.sacn_target);
    if rig.sacn_target.is_some() || rig.sacn.sent > 0 {
        s.push_str(&format!(" · {}", sent_text(rig.sacn.sent)));
    }
    if let Some(e) = &rig.sacn.last_error {
        s.push_str(&format!(" · last error: {e}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::OutputStatus;

    fn target(d: &str, u: &str, p: &str) -> SacnTarget {
        parse_target(d, u, p)
            .expect("test value")
            .expect("test value")
    }

    #[test]
    fn the_target_is_a_receiver_or_multicast_via_an_interface() {
        assert_eq!(parse_target("", "1", ""), Ok(None));
        assert_eq!(parse_target(" OFF ", "", ""), Ok(None));
        let t = target("192.168.1.40", "7", "");
        assert_eq!(
            t.dest,
            SacnDest::Unicast {
                addr: Ipv4Addr::new(192, 168, 1, 40)
            }
        );
        assert_eq!(t.priority, SacnPriority::DEFAULT);
        let m = target("multicast via 10.0.0.4", "258", "120");
        assert_eq!(
            target_texts(Some(m)),
            ("multicast via 10.0.0.4".into(), "258".into(), "120".into())
        );
        assert_eq!(target("Multicast 10.0.0.4", "258", "120"), m);
        assert_eq!(target_texts(None), Default::default());
    }

    #[test]
    fn a_bad_target_says_why() {
        for bad in ["rx.local", "10.0.0.1:5568", "multicast via", "via 10.0.0.4"] {
            let e = parse_target(bad, "1", "").expect_err(bad);
            assert!(e.contains("IPv4 address"), "{bad}: {e}");
        }
        assert!(
            parse_target("multicast", "1", "")
                .expect_err("no interface")
                .starts_with("type the interface too")
        );
        assert_eq!(
            parse_target("239.255.0.1", "1", ""),
            Err(
                "239.255.0.1 is a multicast group: type multicast via this host's address instead"
                    .into()
            )
        );
        assert_eq!(
            parse_target("255.255.255.255", "1", ""),
            Err("255.255.255.255 is no receiver: type a receiver's address".into())
        );
        assert_eq!(
            parse_target("multicast via 0.0.0.0", "1", ""),
            Err("0.0.0.0 is no interface: type this host's address on the lighting network".into())
        );
        assert_eq!(
            parse_target("10.0.0.1", " ", ""),
            Err("type the sACN universe too, a number 1–63999".into())
        );
        assert_eq!(
            parse_target("10.0.0.1", "0", ""),
            Err("sACN universe 0 is outside 1–63999".into())
        );
        assert_eq!(
            parse_target("10.0.0.1", "64000", ""),
            Err("sACN universe 64000 is outside 1–63999".into())
        );
        assert_eq!(
            parse_target("10.0.0.1", "1", "201"),
            Err("sACN priority 201 is outside 0–200".into())
        );
    }

    #[test]
    fn the_status_line_says_target_group_frames_and_error() {
        let mut rig = Rig::default();
        assert_eq!(status_line(&rig), "sACN off");
        rig.sacn_target = Some(target("multicast via 10.0.0.4", "258", ""));
        rig.sacn = OutputStatus {
            sent: 1,
            last_error: None,
        };
        assert_eq!(
            status_line(&rig),
            "sACN to 239.255.1.2 (multicast via 10.0.0.4) universe 258 priority 100 · 1 frame \
             sent"
        );
        rig.sacn_target = Some(target("192.168.1.40", "1", "150"));
        rig.sacn.sent = 9;
        rig.sacn.last_error = Some("network unreachable".into());
        assert_eq!(
            status_line(&rig),
            "sACN to 192.168.1.40 universe 1 priority 150 · 9 frames sent · last error: \
             network unreachable"
        );
    }
}
