//! The network rows of Settings › Connection: on a rig on this computer, its network on or
//! off, where it binds and its port. What the rig has comes from its `server.info`; what is
//! picked here waits for Enter (turning on, or moving a network that is on).

use ac2_proto::model::{
    NetBind, NetChoice, NetSetting, NetSettingView, NetToggle, NetUnavailable, ServerInfo,
};
use ac2_scene::network::{self as words, ChoiceFrom, IfaceRow, NetChange};

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
    /// Off; `kept`: where turning it on goes unless told otherwise (`None`: the daemon's
    /// default).
    Off {
        kept: Option<&'a NetChoice>,
    },
    On {
        choice: &'a NetChoice,
    },
}

/// Where the daemon turns its network on when nothing is kept or picked: every interface,
/// as `server.network` documents it, on the default port.
static DEFAULT_BIND: NetBind = NetBind::All;
const DEFAULT_PORT: u16 = ac2_client::endpoint::DEFAULT_PORT;

/// The network of `info` when this app may switch it: the daemon says so
/// ([`NetToggle::Allowed`]), and tells this client its setting in full.
pub fn switch(info: &ServerInfo) -> Option<Switch<'_>> {
    if info.toggle != NetToggle::Allowed {
        return None;
    }
    match &info.network.setting {
        NetSettingView::Host {
            setting: NetSetting::Off { kept, .. },
        } => Some(Switch::Off {
            kept: kept.as_ref(),
        }),
        NetSettingView::Host {
            setting: NetSetting::On { choice, .. },
        } => Some(Switch::On { choice }),
        _ => None,
    }
}

/// The network row when the page only reads it, and its tip: why, as the daemon's
/// [`NetToggle`] says. `None` when it is switched here.
pub fn read_only_row(info: &ServerInfo) -> Option<(String, &'static str)> {
    if switch(info).is_some() {
        return None;
    }
    let kind = match &info.network.setting {
        NetSettingView::Host { setting } => setting.kind(),
        NetSettingView::Peer { setting } => *setting,
    };
    Some(match info.toggle {
        NetToggle::Fixed => {
            let listen = match &info.network.setting {
                NetSettingView::Host {
                    setting: NetSetting::Fixed { listen },
                } => Some(listen.as_str()),
                _ => None,
            };
            (words::fixed_row(listen), words::FIXED_TIP)
        }
        NetToggle::Unavailable {
            reason: NetUnavailable::InProcess,
        } => (words::EMBEDDED_ROW.to_owned(), words::EMBEDDED_TIP),
        NetToggle::Unavailable {
            reason: NetUnavailable::NoKeyStore,
        } => (words::no_key_store_row(kind), words::NO_KEY_STORE_TIP),
        // Allowed, yet the setting is not told in full: read as a remote rig's.
        NetToggle::Unavailable {
            reason: NetUnavailable::NotLocal,
        }
        | NetToggle::Allowed => (words::peer_row(kind), words::PEER_TIP),
    })
}

impl ConnectionPage {
    fn net_info(&self) -> Option<(&ServerInfo, Switch<'_>)> {
        let info = self.server_info()?;
        Some((info, switch(info)?))
    }

    /// The interface row's choice: picked here, else the rig's in force or kept, else the
    /// daemon's default.
    fn iface_row(&self) -> Option<IfaceRow<'_>> {
        Some(match &self.net.iface {
            Some(IfacePick::TypeAddress) => IfaceRow::TypeAddress,
            Some(IfacePick::Bind(b)) => IfaceRow::Bind(b, ChoiceFrom::Picked),
            None => {
                let (b, from) = self.rig_bind()?;
                IfaceRow::Bind(b, from)
            }
        })
    }

    /// The rig's bind: in force, kept, else the daemon's default.
    fn rig_bind(&self) -> Option<(&NetBind, ChoiceFrom)> {
        Some(match self.net_info()?.1 {
            Switch::On { choice } => (&choice.bind, ChoiceFrom::InForce),
            Switch::Off { kept: Some(k) } => (&k.bind, ChoiceFrom::Kept),
            Switch::Off { kept: None } => (&DEFAULT_BIND, ChoiceFrom::Default),
        })
    }

    /// The bind picked here that differs from the rig's in force.
    fn picked_bind(&self) -> Option<NetBind> {
        match (&self.net.iface, self.net_info()?.1) {
            (Some(IfacePick::Bind(b)), Switch::On { choice }) if *b == choice.bind => None,
            (Some(IfacePick::Bind(b)), _) => Some(b.clone()),
            _ => None,
        }
    }

    /// The port row's port and where it comes from.
    fn shown_port(&self) -> Option<(u16, ChoiceFrom)> {
        Some(match (self.net.port, self.net_info()?.1) {
            (Some(p), _) => (p, ChoiceFrom::Picked),
            (None, Switch::On { choice }) => (choice.port, ChoiceFrom::InForce),
            (None, Switch::Off { kept: Some(k) }) => (k.port, ChoiceFrom::Kept),
            (None, Switch::Off { kept: None }) => (DEFAULT_PORT, ChoiceFrom::Default),
        })
    }

    /// The tip of a network line (`None` for any other).
    pub fn net_line_tip(&self, l: &ConnLine) -> Option<&'static str> {
        let (_, sw) = self.net_info()?;
        match l {
            ConnLine::Network => Some(words::network_tip(matches!(sw, Switch::On { .. }))),
            ConnLine::Interface => match self.iface_row()? {
                IfaceRow::Bind(_, from) => Some(words::choice_tip(from)),
                IfaceRow::TypeAddress => None,
            },
            ConnLine::Port => Some(words::choice_tip(self.shown_port()?.1)),
            _ => None,
        }
    }

    /// The text of a network line (empty for any other).
    pub fn net_line_text(&self, l: &ConnLine) -> String {
        let Some((info, sw)) = self.net_info() else {
            return String::new();
        };
        match l {
            ConnLine::Network => words::network_row(matches!(sw, Switch::On { .. })).to_owned(),
            ConnLine::Interface => self.iface_row().map_or_else(String::new, |r| {
                let pending = matches!(sw, Switch::On { .. }) && self.picked_bind().is_some();
                words::interface_row(r, &info.interfaces, pending)
            }),
            ConnLine::Port => self
                .shown_port()
                .map_or_else(String::new, |(p, from)| words::port_row(p, from)),
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
            Some(IfaceRow::Bind(b, _)) => Some(IfacePick::Bind(b.clone())),
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
                // An address still to be typed is no pick: the rig's bind goes on.
                let (bind, bind_from) = match self.iface_row()? {
                    IfaceRow::Bind(b, from) => (b, from),
                    IfaceRow::TypeAddress => self.rig_bind()?,
                };
                let (port, port_from) = self.shown_port()?;
                let default = bind_from == ChoiceFrom::Default && port_from == ChoiceFrom::Default;
                let notice = words::turn_on_confirm(bind, port, default, &info.interfaces);
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
        let (_, sw) = self.net_info()?;
        let on = matches!(sw, Switch::On { .. });
        let in_force = match sw {
            Switch::On { choice } => Some(choice.port),
            Switch::Off { .. } => None,
        };
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
            ConnEdit::Port => match words::parse_port(&text) {
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
