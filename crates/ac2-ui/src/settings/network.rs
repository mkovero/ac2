//! The network rows of Settings › Connection: on a rig on this computer, its network on or
//! off, where it binds and its port. What the rig has comes from its `server.info`; what is
//! picked here waits for Enter (turning on, or moving a network that is on).

use ac2_proto::model::{LocalServer, NetBind, NetSetting, NetSettingView, ServerInfo};
use ac2_scene::network::{self as words, IfaceRow, NetChange};

use super::connection::{ConnAction, ConnEdit, ConnLine, ConnectionPage};

/// A choice of the interface row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IfacePick {
    Bind(NetBind),
    /// The row's last choice: Enter types an address.
    TypeAddress,
}

/// What the page picked for the network, not yet in force.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetPicks {
    pub iface: Option<IfacePick>,
    pub port: Option<u16>,
    /// Enter was pressed once on the switch while off: a second press turns it on.
    pub confirm_on: bool,
}

/// The network as this app may switch it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch<'a> {
    Off,
    On { bind: &'a NetBind, port: u16 },
}

/// The network of `info` when this app may switch it: the rig's own daemon on this computer
/// (a client on its local pair is told the setting in full), set from the settings, not
/// fixed by `--listen` and not inside its program.
pub fn switch(info: &ServerInfo) -> Option<Switch<'_>> {
    if !matches!(info.local, Some(LocalServer::Local { .. })) {
        return None;
    }
    match &info.network.setting {
        NetSettingView::Host {
            setting: NetSetting::Off { .. },
        } => Some(Switch::Off),
        NetSettingView::Host {
            setting: NetSetting::On { bind, port, .. },
        } => Some(Switch::On { bind, port: *port }),
        _ => None,
    }
}

/// The network row when the page only reads it: fixed by `--listen`, a rig reached over the
/// network, a daemon inside its program. `None` when it is switched here.
pub fn read_only_row(info: &ServerInfo) -> Option<String> {
    if switch(info).is_some() {
        return None;
    }
    Some(match (&info.local, &info.network.setting) {
        (Some(LocalServer::Embedded), _) => words::EMBEDDED_ROW.to_owned(),
        (
            _,
            NetSettingView::Host {
                setting: NetSetting::Fixed { listen },
            },
        ) => words::fixed_row(listen),
        (_, NetSettingView::Host { setting }) => words::peer_row(setting.kind()),
        (_, NetSettingView::Peer { setting }) => words::peer_row(*setting),
    })
}

impl ConnectionPage {
    fn net_info(&self) -> Option<(&ServerInfo, Switch<'_>)> {
        let info = self.server_info()?;
        Some((info, switch(info)?))
    }

    /// The interface row's choice: picked here, else the rig's while on.
    fn iface_row(&self) -> Option<IfaceRow<'_>> {
        let (_, sw) = self.net_info()?;
        Some(match (&self.net.iface, sw) {
            (Some(IfacePick::TypeAddress), _) => IfaceRow::TypeAddress,
            (Some(IfacePick::Bind(b)), _) => IfaceRow::Bind(b),
            (None, Switch::On { bind, .. }) => IfaceRow::Bind(bind),
            (None, Switch::Off) => IfaceRow::Kept,
        })
    }

    /// The bind picked here that differs from the rig's.
    fn picked_bind(&self) -> Option<NetBind> {
        match (&self.net.iface, self.net_info()?.1) {
            (Some(IfacePick::Bind(b)), Switch::On { bind, .. }) if b == bind => None,
            (Some(IfacePick::Bind(b)), _) => Some(b.clone()),
            _ => None,
        }
    }

    fn shown_port(&self) -> Option<u16> {
        match (self.net.port, self.net_info()?.1) {
            (Some(p), _) => Some(p),
            (None, Switch::On { port, .. }) => Some(port),
            (None, Switch::Off) => None,
        }
    }

    /// The text of a network line (empty for any other).
    pub fn net_line_text(&self, l: &ConnLine) -> String {
        let Some((info, sw)) = self.net_info() else {
            return String::new();
        };
        match l {
            ConnLine::Network => words::network_row(matches!(sw, Switch::On { .. })),
            ConnLine::Interface => self.iface_row().map_or_else(String::new, |r| {
                let pending = matches!(sw, Switch::On { .. }) && self.picked_bind().is_some();
                words::interface_row(r, &info.interfaces, pending)
            }),
            ConnLine::Port => words::port_row(self.shown_port()),
            _ => String::new(),
        }
    }

    /// ←/→ on the interface row: all interfaces, each the rig lists, an address typed
    /// earlier, then the choice that types one.
    pub(super) fn cycle_interface(&mut self, d: i32) {
        self.clear();
        let Some((info, _)) = self.net_info() else {
            return;
        };
        let mut choices: Vec<IfacePick> = std::iter::once(NetBind::All)
            .chain(info.interfaces.iter().map(|i| NetBind::Interface {
                name: i.name.clone(),
            }))
            .map(IfacePick::Bind)
            .collect();
        let current = match self.iface_row() {
            Some(IfaceRow::Bind(b)) => Some(IfacePick::Bind(b.clone())),
            Some(IfaceRow::TypeAddress) => Some(IfacePick::TypeAddress),
            _ => None,
        };
        if let Some(c @ IfacePick::Bind(_)) = &current
            && !choices.contains(c)
        {
            choices.push(c.clone());
        }
        choices.push(IfacePick::TypeAddress);
        let n = choices.len() as i32;
        let at = current.and_then(|c| choices.iter().position(|x| *x == c));
        let next = match at {
            Some(i) => (i as i32 + d).rem_euclid(n),
            None if d < 0 => n - 1,
            None => 0,
        };
        self.net.iface = Some(choices[next as usize].clone());
    }

    /// Enter on a network line.
    pub(super) fn net_enter(&mut self, l: &ConnLine) -> Option<ConnAction> {
        let on = matches!(self.net_info()?.1, Switch::On { .. });
        match (l, on) {
            (ConnLine::Network, true) => {
                self.clear();
                Some(ConnAction::Network {
                    enabled: false,
                    bind: None,
                    port: None,
                    change: NetChange::Off,
                })
            }
            (ConnLine::Network, false) if self.net.confirm_on => {
                self.clear();
                Some(ConnAction::Network {
                    enabled: true,
                    bind: self.picked_bind(),
                    port: self.net.port,
                    change: NetChange::On,
                })
            }
            (ConnLine::Network, false) => {
                let info = self.server_info()?;
                let bind = self.picked_bind();
                let notice = words::turn_on_confirm(bind.as_ref(), self.net.port, &info.interfaces);
                self.clear();
                self.net.confirm_on = true;
                self.notice = Some(notice);
                None
            }
            (ConnLine::Interface, _) if self.net.iface == Some(IfacePick::TypeAddress) => {
                self.edit = Some((ConnEdit::Address, String::new()));
                None
            }
            (ConnLine::Interface, true) => self.picked_bind().map(|b| apply(Some(b), None)),
            (ConnLine::Interface, false) => {
                self.notice = Some(words::PICKED_FOR_ON.into());
                None
            }
            (ConnLine::Port, _) => {
                self.edit = Some((ConnEdit::Port, String::new()));
                None
            }
            _ => None,
        }
    }

    /// Enter on a typed address or port: checked here as the daemon checks it, then picked;
    /// on a network that is on, applied.
    pub(super) fn net_edit(&mut self, kind: ConnEdit, text: String) -> Option<ConnAction> {
        let (info, sw) = self.net_info()?;
        let on = matches!(sw, Switch::On { .. });
        let in_force = match sw {
            Switch::On { port, .. } => Some(port),
            Switch::Off => None,
        };
        let local = words::local_tcp_ports(info.local.as_ref());
        match kind {
            ConnEdit::Address => match words::parse_address(&text) {
                Ok(addr) => {
                    let bind = NetBind::Addr { addr };
                    self.net.iface = Some(IfacePick::Bind(bind.clone()));
                    if on {
                        return self.picked_bind().map(|b| apply(Some(b), None));
                    }
                    self.notice = Some(words::PICKED_FOR_ON.into());
                    None
                }
                Err(e) => {
                    self.error = Some(e);
                    self.edit = Some((kind, text));
                    None
                }
            },
            ConnEdit::Port => match words::parse_port(&text, &local) {
                Ok(p) => {
                    self.net.port = Some(p);
                    if on {
                        return (in_force != Some(p)).then(|| apply(None, Some(p)));
                    }
                    self.notice = Some(words::PICKED_FOR_ON.into());
                    None
                }
                Err(e) => {
                    self.error = Some(e);
                    self.edit = Some((kind, text));
                    None
                }
            },
            _ => None,
        }
    }
}

/// A network that is on moved to `bind`/`port` (`None`: as it is).
fn apply(bind: Option<NetBind>, port: Option<u16>) -> ConnAction {
    ConnAction::Network {
        enabled: true,
        bind,
        port,
        change: NetChange::Moved,
    }
}

#[cfg(test)]
#[path = "network_tests.rs"]
mod tests;
