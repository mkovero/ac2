//! Settings › Connection's network rows: the switch, where it binds, its port, what it is
//! doing and whether the setting outlives a restart, worded from the daemon's typed
//! `server.info`; and what a rig that turned its network off reads as.

use std::net::IpAddr;

use ac2_proto::model::{
    LocalServer, NetBind, NetInterface, NetSaved, NetSettingKind, NetStatus, NetStatusView,
    NotSavedReason, PeerNetStatus,
};

use crate::rig::{reason_kind_text, reason_text};

/// The network row of a rig on this computer, which this app may switch. Off says what
/// still works, on says who turning it off cuts: neither asks to be confirmed, so the row
/// itself tells.
pub fn network_row(on: bool) -> String {
    if on {
        "Network: on · Enter turns it off: every app and rig connected to this rig over the \
         network is cut; this rig's own links to other rigs stay"
            .into()
    } else {
        "Network: off · this rig still pairs with, authorizes and reads other rigs; only \
         connections to it are off (Enter turns it on)"
            .into()
    }
}

/// The network row of a rig whose `ac2d` was started with `--listen` (how a rig host's
/// service runs it): the command line is the setting, so the page only reads it.
pub fn fixed_row(listen: &str) -> String {
    format!(
        "Network: on, set by --listen (systemd): {listen} · change it in the service's command \
         line"
    )
}

/// The network row of a daemon running inside its program: it has no network to turn on.
pub const EMBEDDED_ROW: &str =
    "Network: none · this daemon runs inside its program: no other program can connect";

/// The network row of a rig reached over the network: a network client may not switch it
/// (it would cut itself off, or open the rig where it was reached some other way).
pub fn peer_row(kind: NetSettingKind) -> String {
    let state = match kind {
        NetSettingKind::Off => "off",
        NetSettingKind::On => "on",
        NetSettingKind::Fixed => "on, set by --listen (systemd)",
    };
    format!("Network: {state} · change it on the rig's own computer")
}

/// What the interface row shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IfaceRow<'a> {
    /// Nothing picked while the network is off: the rig keeps the last choice, which a
    /// network that is off does not report.
    Kept,
    /// A bind, picked or in force.
    Bind(&'a NetBind),
    /// The choice that types an address.
    TypeAddress,
}

/// `all interfaces`, `eth0 (192.168.1.20, 2001:db8::20)`, `address 192.168.1.20 (eth0)`, with
/// the addresses as the daemon lists them; one this computer does not have now says so.
pub fn bind_text(bind: &NetBind, interfaces: &[NetInterface]) -> String {
    match bind {
        NetBind::All => "all interfaces".into(),
        NetBind::Interface { name } => match interfaces.iter().find(|i| &i.name == name) {
            None => format!("{name} (not on this computer now)"),
            Some(i) => {
                let mut what = if i.addrs.is_empty() {
                    "no address".to_owned()
                } else {
                    addrs_text(&i.addrs)
                };
                if !i.up {
                    what.push_str(", down");
                }
                format!("{name} ({what})")
            }
        },
        NetBind::Addr { addr } => match interfaces.iter().find(|i| i.addrs.contains(addr)) {
            Some(i) => format!("address {addr} ({})", i.name),
            // The rig lists no loopback interface: there is nothing to look it up in.
            None if addr.is_loopback() => format!("address {addr} (loopback)"),
            None => format!("address {addr} (not on this computer now)"),
        },
    }
}

fn addrs_text(addrs: &[IpAddr]) -> String {
    addrs
        .iter()
        .map(IpAddr::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The interface row. `pending`: the network is on elsewhere than the bind picked, and
/// Enter moves it.
pub fn interface_row(row: IfaceRow<'_>, interfaces: &[NetInterface], pending: bool) -> String {
    let what = match row {
        IfaceRow::Kept => "as last set on this rig".to_owned(),
        IfaceRow::Bind(b) => bind_text(b, interfaces),
        IfaceRow::TypeAddress => {
            return "Interface: an address… (Enter types it · ←→ picks)".into();
        }
    };
    if pending {
        format!("Interface: {what} · Enter moves the network here (←→ picks)")
    } else {
        format!("Interface: {what} (←→ picks)")
    }
}

/// The port row; `None`: nothing typed while the network is off, the rig keeps the last.
pub fn port_row(port: Option<u16>) -> String {
    match port {
        Some(p) => format!(
            "Port: {p} · data on {} (Enter types another)",
            u32::from(p) + 1
        ),
        None => "Port: as last set on this rig (Enter types one)".into(),
    }
}

/// Said when an interface or port is picked while the network is off.
pub const PICKED_FOR_ON: &str =
    "picked: the network uses it when turned on (Enter on the Network row)";

/// What the edit line says before a typed address.
pub const ADDRESS_PROMPT: &str = "Network address of this computer:";
/// What the edit line says before a typed port.
pub const PORT_PROMPT: &str = "Network port (data on the next one):";

/// The status line, live from the daemon: `Status: listening on 192.168.1.20:47820 …`,
/// `Status: waiting: the port is in use · retrying`, `Status: off`.
pub fn status_line(status: &NetStatusView) -> String {
    let why = match status {
        NetStatusView::Host {
            status: NetStatus::Waiting { reason } | NetStatus::Failed { reason },
        } => reason_text(reason),
        NetStatusView::Peer {
            status: PeerNetStatus::Waiting { reason } | PeerNetStatus::Failed { reason },
        } => reason_kind_text(*reason).to_owned(),
        _ => String::new(),
    };
    match status {
        NetStatusView::Host {
            status: NetStatus::Listening { ctrl, .. },
        }
        | NetStatusView::Peer {
            status: PeerNetStatus::Listening { ctrl, .. },
        } => format!(
            "Status: listening on {} (CURVE; authorized keys only)",
            endpoint_text(ctrl)
        ),
        NetStatusView::Host {
            status: NetStatus::Waiting { .. },
        }
        | NetStatusView::Peer {
            status: PeerNetStatus::Waiting { .. },
        } => format!("Status: waiting: {why} · retrying"),
        NetStatusView::Host {
            status: NetStatus::Failed { .. },
        }
        | NetStatusView::Peer {
            status: PeerNetStatus::Failed { .. },
        } => format!("Status: failed: {why} · retrying"),
        NetStatusView::Host {
            status: NetStatus::Off,
        }
        | NetStatusView::Peer {
            status: PeerNetStatus::Off,
        } => "Status: off".into(),
    }
}

/// `tcp://192.168.1.20:47820` as `192.168.1.20:47820`; bound to every interface, `all
/// interfaces, port 47820`. Anything else as it is.
fn endpoint_text(ctrl: &str) -> String {
    let Some((host, port)) = tcp_host_port(ctrl) else {
        return ctrl.to_owned();
    };
    match host {
        "*" | "0.0.0.0" | "::" => format!("all interfaces, port {port}"),
        h if h.contains(':') => format!("[{h}]:{port}"),
        h => format!("{h}:{port}"),
    }
}

fn tcp_host_port(endpoint: &str) -> Option<(&str, &str)> {
    let rest = endpoint.strip_prefix("tcp://")?;
    match rest.strip_prefix('[') {
        Some(v6) => {
            let (host, tail) = v6.split_once(']')?;
            Some((host, tail.strip_prefix(':')?))
        }
        None => rest.rsplit_once(':'),
    }
}

/// `Not saved: …` when the setting in force is lost at the daemon's next start.
pub fn not_saved_line(saved: &NetSaved) -> Option<String> {
    let NetSaved::NotSaved { reason } = saved else {
        return None;
    };
    let why = match reason {
        NotSavedReason::StoreReadOnly => "the settings file cannot be written".to_owned(),
        NotSavedReason::Unreadable { detail } => {
            format!(
                "the settings file could not be read at start ({detail}), so it is not written over"
            )
        }
        NotSavedReason::DamagedInPlace => {
            "the settings file is damaged and could not be set aside".to_owned()
        }
        NotSavedReason::NoStore => "this ac2d keeps no settings file".to_owned(),
        NotSavedReason::WriteFailed { detail } => {
            format!("writing the settings file failed ({detail})")
        }
    };
    Some(format!(
        "Not saved: {why} · ac2d starts without this setting next time"
    ))
}

/// The notice before the network goes on: where and on which port, and who can then
/// connect. `bind`/`port` `None`: the rig's last choice, which is not reported while off.
pub fn turn_on_confirm(
    bind: Option<&NetBind>,
    port: Option<u16>,
    interfaces: &[NetInterface],
) -> String {
    let at = match (bind, port) {
        (None, None) => "where it was last set on this rig".to_owned(),
        (Some(b), None) => format!("{}, the port last set", bind_text(b, interfaces)),
        (None, Some(p)) => format!(
            "the interface last set, port {p} (data {})",
            u32::from(p) + 1
        ),
        (Some(b), Some(p)) => format!(
            "{}, port {p} (data {})",
            bind_text(b, interfaces),
            u32::from(p) + 1
        ),
    };
    format!(
        "Enter again turns the network on · {at} · apps and rigs whose keys this rig authorized \
         can then connect"
    )
}

/// What a change of the network does, for its toast; and its failure, before the daemon's
/// words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetChange {
    On,
    Off,
    /// On, to another interface or port.
    Moved,
}

/// `network turned on` …
pub fn change_done(c: NetChange) -> &'static str {
    match c {
        NetChange::On => "network turned on",
        NetChange::Off => "network turned off",
        NetChange::Moved => "network moved",
    }
}

/// `cannot turn the network on` …
pub fn change_failed(c: NetChange) -> &'static str {
    match c {
        NetChange::On => "cannot turn the network on",
        NetChange::Off => "cannot turn the network off",
        NetChange::Moved => "cannot move the network",
    }
}

/// The ports a local pair on TCP holds (loopback, as on Windows): the network may not
/// take them.
pub fn local_tcp_ports(local: Option<&LocalServer>) -> Vec<u16> {
    match local {
        Some(LocalServer::Local { ctrl, data }) => [ctrl, data]
            .into_iter()
            .filter_map(|e| tcp_host_port(e)?.1.parse().ok())
            .collect(),
        _ => Vec::new(),
    }
}

/// A typed network port, checked as the daemon checks it: data takes the next port, so
/// 65535 has none; neither may be a port the local pair holds on TCP.
pub fn parse_port(text: &str, local_tcp: &[u16]) -> Result<u16, String> {
    let p = match text.trim().parse::<u16>() {
        Ok(p) if p != 0 && p != u16::MAX => p,
        _ => return Err("type a port from 1 to 65534 (data takes the next one)".into()),
    };
    if local_tcp.contains(&p) || local_tcp.contains(&(p + 1)) {
        let held = local_tcp
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(" and ");
        return Err(format!(
            "port {p} (data {}) would take TCP {held}, which apps on this computer reach ac2d \
             on (as on Windows): pick another port",
            p + 1
        ));
    }
    Ok(p)
}

/// A typed address of this computer.
pub fn parse_address(text: &str) -> Result<IpAddr, String> {
    text.trim()
        .parse()
        .map_err(|_| "type an IP address of this computer (e.g. 192.168.1.20)".into())
}

/// What a rig whose network was turned off reads as, after its name, in the drop-down, on
/// its panes and in the top bar: it went on purpose, so not as a fault.
pub const RIG_NETWORK_OFF: &str = "turned its network off · retrying";

/// The toast when the rig shown turns its network off.
pub fn network_off_toast(rig: &str) -> String {
    format!(
        "{rig} turned its network off: the app connects again once it is back on (Settings › \
         Connection on {rig}'s own computer)"
    )
}

#[cfg(test)]
#[path = "network_tests.rs"]
mod tests;
