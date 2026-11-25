//! Settings › Connection's network rows: the switch, where it binds, its port, what it is
//! doing and whether the setting outlives a restart, worded from the daemon's typed
//! `server.info`; and what a rig that turned its network off reads as.

use std::net::IpAddr;

use ac2_proto::model::{
    NetBind, NetInterface, NetSaved, NetSettingKind, NetStatus, NetStatusView, NotSavedReason,
    PeerNetStatus,
};

use crate::rig::{reason_kind_text, reason_text};

/// The network row of a rig on this computer, which this app may switch. Off acts at once,
/// so the row itself says what turning it off cuts; what turning it on opens is said by the
/// confirmation that turning on asks for ([`turn_on_confirm`]).
pub fn network_row(on: bool) -> &'static str {
    if on {
        "Network: on · Enter turns it off: cuts apps and rigs connected to it"
    } else {
        "Network: off · Enter turns it on"
    }
}

/// The network row's tip: what on and off mean for this rig's own links and for those to it.
pub fn network_tip(on: bool) -> &'static str {
    if on {
        "turning it off cuts every app and rig connected to this rig over the network; this \
         rig's own links to other rigs stay"
    } else {
        "while off, this rig still pairs with, authorizes and reads other rigs; only \
         connections to it are off. Turned on, apps and rigs whose keys this rig authorized \
         can connect to it"
    }
}

/// The network row of a rig whose `ac2d` was started with `--listen` (how a rig host's
/// service runs it): the command line is the setting, so the page only reads it.
pub fn fixed_row(listen: Option<&str>) -> String {
    match listen {
        Some(l) => format!(
            "Network: on, set by --listen (systemd): {l} · change it in the service's command line"
        ),
        None => "Network: on, set by --listen (systemd) · change it in the service's command line"
            .into(),
    }
}

/// The fixed row's tip.
pub const FIXED_TIP: &str =
    "ac2d's command line is the setting for as long as it runs: this page only reads it";

/// The network row of a daemon running inside its program: it has no network to turn on.
pub const EMBEDDED_ROW: &str = "Network: none · this daemon runs inside its program";

/// The in-process row's tip.
pub const EMBEDDED_TIP: &str = "it serves its own program only: no other program can connect, so it has no network to \
     turn on";

/// The network row of a daemon that keeps no files: without a server key there is nothing
/// to serve CURVE with, so its network cannot go on.
pub fn no_key_store_row(kind: NetSettingKind) -> String {
    format!(
        "Network: {} · cannot be turned on: no config directory for a server key",
        state_word(kind)
    )
}

/// The no-key-store row's tip.
pub const NO_KEY_STORE_TIP: &str = "this ac2d runs without a config directory, so it has nowhere to keep the server key the \
     network needs: its network cannot be turned on";

/// The network row of a rig reached over the network: a network client may not switch it
/// (it would cut itself off, or open the rig where it was reached some other way).
pub fn peer_row(kind: NetSettingKind) -> String {
    format!(
        "Network: {} · change it on the rig's own computer",
        state_word(kind)
    )
}

/// The remote row's tip.
pub const PEER_TIP: &str = "only an app on the rig's own computer may turn its network on or \
     off: from here it could cut this app off";

fn state_word(kind: NetSettingKind) -> &'static str {
    match kind {
        NetSettingKind::Off => "off",
        NetSettingKind::On => "on",
        NetSettingKind::Fixed => "on, set by --listen (systemd)",
    }
}

/// Where the interface or port a row shows comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceFrom {
    /// The network is on there.
    InForce,
    /// Picked on the page, not yet in force.
    Picked,
    /// Kept by the rig from when its network was last on: turning on uses it.
    Kept,
    /// The daemon's default: its network has never been on.
    Default,
}

/// The interface and port rows' tip.
pub fn choice_tip(from: ChoiceFrom) -> &'static str {
    match from {
        ChoiceFrom::InForce => "where the network is on now; another picked here moves it",
        ChoiceFrom::Picked => "picked here: used when the network is turned on, or moved",
        ChoiceFrom::Kept => {
            "kept from when the network was last on: turning it on uses it unless another is \
             picked here"
        }
        ChoiceFrom::Default => {
            "the daemon's default, as its network has not been on yet: turning it on uses it \
             unless another is picked here"
        }
    }
}

/// What the interface row shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IfaceRow<'a> {
    /// A bind, and where it comes from.
    Bind(&'a NetBind, ChoiceFrom),
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
        IfaceRow::Bind(b, from) => default_marked(bind_text(b, interfaces), from),
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

fn default_marked(what: String, from: ChoiceFrom) -> String {
    match from {
        ChoiceFrom::Default => format!("{what}, the default"),
        _ => what,
    }
}

/// The port row.
pub fn port_row(port: u16, from: ChoiceFrom) -> String {
    format!(
        "Port: {} · data on {} (Enter types another)",
        default_marked(port.to_string(), from),
        u32::from(port) + 1
    )
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

/// The notice before the network goes on: where and on which port (`default`: the
/// daemon's, nothing kept or picked), and who can then connect.
pub fn turn_on_confirm(
    bind: &NetBind,
    port: u16,
    default: bool,
    interfaces: &[NetInterface],
) -> String {
    let mut at = format!(
        "{}, port {port} (data {})",
        bind_text(bind, interfaces),
        u32::from(port) + 1
    );
    if default {
        at.push_str(", the default");
    }
    format!(
        "Enter again turns the network on at {at} · apps and rigs whose keys this rig \
         authorized can then connect"
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

/// A typed network port, checked as the daemon checks it: data takes the next port, so
/// 65535 has none. The local pair is ipc, so no port clashes with it.
pub fn parse_port(text: &str) -> Result<u16, String> {
    match text.trim().parse::<u16>() {
        Ok(p) if p != 0 && p != u16::MAX => Ok(p),
        _ => Err("type a port from 1 to 65534 (data takes the next one)".into()),
    }
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
