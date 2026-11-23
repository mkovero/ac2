//! The Connection page of the Settings view: the link to the daemon, this client's key, the
//! network of a rig on this computer ([`super::network`]), and the authorized and refused
//! clients.

use ac2_proto::model::{NetBind, NetworkServer, ServerInfo, ServerSummary};
use ac2_scene::network::NetChange;

use super::network::NetPicks;
use super::pairing::Pairing;
use ac2_proto::units::RigId;

/// A line of the Connection page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnLine {
    /// Drop the link and connect again now.
    Reconnect,
    /// Add a rig (another daemon, or pair with one) in the connect dialog: the rigs held
    /// stay.
    ConnectOther,
    /// *Start ac2d when none is running* (←/→ or Enter).
    StartLocal,
    /// *Stop ac2d when the app quits* (←/→ or Enter).
    StopOnQuit,
    /// The network of a rig on this computer: on or off (Enter, ←/→).
    Network,
    /// Where it binds (←/→ picks, Enter applies or types an address).
    Interface,
    /// Its port (Enter types it).
    Port,
    /// An authorized client, by name.
    Authorized(String),
    /// A refused key (Z85), or a refused peer without CURVE (`None`).
    Refused(Option<String>, String),
    /// Authorize a key typed with its name.
    AddKey,
    /// A rig paired with this one, or half paired (Enter mends, Delete twice unpairs).
    Paired(RigId),
    /// *Pair with rig…*: Enter opens the pick of the rigs the app holds.
    PairWith,
    /// A rig in that pick (Enter pairs both ways).
    PairOffer(RigId),
    /// *Wait for remote inputs*: the session's link offset (Enter types it), while it has
    /// remote inputs.
    LinkOffset,
}

/// What the Connection page asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum ConnAction {
    Reconnect,
    ConnectOther,
    ToggleStartLocal,
    ToggleStopOnQuit,
    Authorize {
        name: String,
        key: String,
    },
    Revoke(String),
    /// `server.network`: `None` keeps what the rig has.
    Network {
        enabled: bool,
        bind: Option<NetBind>,
        port: Option<u16>,
        change: NetChange,
    },
    /// Reopen the session with this link offset, seconds (`None`: the least that works).
    LinkOffset(Option<f64>),
    /// Open *Pair with rig…*'s pick.
    PairPick,
    /// Pair this rig with that one, both ways.
    Pair(RigId),
    /// Unpair this rig and that one, both ways where the app holds it.
    Unpair(RigId),
}

/// A typed edit of the Connection page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnEdit {
    /// The name a refused key is authorized under.
    Name { key: String },
    /// `name key`, for a key not seen yet.
    NameAndKey,
    /// The link offset: a time or `default`.
    LinkOffset,
    /// An address of the rig's computer for the network.
    Address,
    /// The network's port.
    Port,
}

/// The Connection page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConnectionPage {
    pub focus: usize,
    /// The daemon's last `server.info` (or why it could not be asked).
    pub server: Option<Result<ServerInfo, String>>,
    /// When it was last asked for, `now_s`.
    pub asked_s: Option<f64>,
    pub edit: Option<(ConnEdit, String)>,
    /// The authorized client Delete was pressed on once; a second press revokes.
    pub confirm: Option<String>,
    pub notice: Option<String>,
    pub error: Option<String>,
    /// The open session takes remote inputs: the link offset line is listed.
    pub remote: bool,
    /// The network's interface and port as picked here, not yet in force.
    pub net: NetPicks,
    /// `State.server` when `server.info` was last asked: a change made anywhere (another
    /// app, the CLI, a retry in the daemon) asks again at once.
    pub seen: Option<ServerSummary>,
    /// *Paired rigs* and *Pair with rig…*'s pick.
    pub pairing: Pairing,
}

/// Seconds between `server.info` while the Connection page is shown: refused keys appear
/// while the operator watches for a new client to knock.
pub const SERVER_INFO_EVERY_S: f64 = 2.0;

/// The page's lines for `server` (the actions, then the network mode's keys, then the
/// paired rigs and *Pair with rig…*, then with remote inputs the link offset, as *Between
/// rigs* closes the page).
pub fn conn_lines(server: Option<&ServerInfo>, pairing: &Pairing, remote: bool) -> Vec<ConnLine> {
    let mut v = vec![
        ConnLine::Reconnect,
        ConnLine::ConnectOther,
        ConnLine::StartLocal,
        ConnLine::StopOnQuit,
    ];
    if server.and_then(super::network::switch).is_some() {
        v.extend([ConnLine::Network, ConnLine::Interface, ConnLine::Port]);
    }
    if let Some(ServerInfo {
        network:
            NetworkServer {
                server_key: Some(_),
                authorized,
                refused,
                ..
            },
        ..
    }) = server
    {
        // The apps, then the rigs admitted to take this rig's inputs: the page lists them
        // apart, in this order.
        v.extend(
            authorized
                .iter()
                .filter(|a| a.rig.is_none())
                .chain(authorized.iter().filter(|a| a.rig.is_some()))
                .map(|a| ConnLine::Authorized(a.name.clone())),
        );
        v.extend(
            refused
                .iter()
                .flatten()
                .map(|r| ConnLine::Refused(r.key.clone(), r.address.clone())),
        );
        v.push(ConnLine::AddKey);
    }
    v.extend(pairing.rows.iter().map(|r| ConnLine::Paired(r.rig)));
    v.push(ConnLine::PairWith);
    v.extend(
        pairing
            .offers
            .iter()
            .flatten()
            .map(|o| ConnLine::PairOffer(o.rig)),
    );
    if remote {
        v.push(ConnLine::LinkOffset);
    }
    v
}

impl ConnectionPage {
    pub(super) fn server_info(&self) -> Option<&ServerInfo> {
        self.server.as_ref().and_then(|r| r.as_ref().ok())
    }

    /// The lines as of the last answer.
    pub fn lines(&self) -> Vec<ConnLine> {
        conn_lines(self.server_info(), &self.pairing, self.remote)
    }

    /// The focused line (clamped: lines come and go with the daemon's answers).
    pub fn focused(&self) -> Option<ConnLine> {
        let l = self.lines();
        l.get(self.focus.min(l.len().saturating_sub(1))).cloned()
    }

    pub(super) fn clear(&mut self) {
        self.net.confirm_on = false;
        self.confirm = None;
        self.pairing.confirm = None;
        self.notice = None;
        self.error = None;
    }

    pub fn move_focus(&mut self, d: i32) {
        self.clear();
        self.edit = None;
        let n = self.lines().len() as i32;
        if n > 0 {
            self.focus = (self.focus.min(n as usize - 1) as i32 + d).rem_euclid(n) as usize;
        }
    }

    /// Enter: what the focused line does, or the typed edit's result.
    pub fn enter(&mut self) -> Option<ConnAction> {
        self.notice = None;
        self.error = None;
        if let Some((kind, text)) = self.edit.take() {
            let t = text.trim();
            return match kind {
                ConnEdit::Name { key } => {
                    if t.is_empty() || t.contains(char::is_whitespace) {
                        self.error = Some("type a name without spaces (e.g. tablet)".into());
                        self.edit = Some((ConnEdit::Name { key }, text));
                        None
                    } else {
                        Some(ConnAction::Authorize {
                            name: t.to_owned(),
                            key,
                        })
                    }
                }
                ConnEdit::LinkOffset => match ac2_scene::cross_rig::parse_link_offset(t) {
                    Ok(o) => Some(ConnAction::LinkOffset(o)),
                    Err(e) => {
                        self.error = Some(e);
                        self.edit = Some((ConnEdit::LinkOffset, text));
                        None
                    }
                },
                ConnEdit::Address | ConnEdit::Port => self.net_edit(kind, text),
                ConnEdit::NameAndKey => {
                    let mut parts = t.split_whitespace();
                    match (parts.next(), parts.next(), parts.next()) {
                        (Some(name), Some(key), None) => Some(ConnAction::Authorize {
                            name: name.to_owned(),
                            key: key.to_owned(),
                        }),
                        _ => {
                            self.error = Some(
                                "type the name, a space and the client's key (40 characters, as \
                                 `ac2 auth show` prints it)"
                                    .into(),
                            );
                            self.edit = Some((ConnEdit::NameAndKey, text));
                            None
                        }
                    }
                }
            };
        }
        match self.focused()? {
            ConnLine::Reconnect => Some(ConnAction::Reconnect),
            ConnLine::ConnectOther => Some(ConnAction::ConnectOther),
            ConnLine::StartLocal | ConnLine::StopOnQuit => self.toggle(1),
            l @ (ConnLine::Network | ConnLine::Interface | ConnLine::Port) => self.net_enter(&l),
            ConnLine::AddKey => {
                self.edit = Some((ConnEdit::NameAndKey, String::new()));
                None
            }
            ConnLine::Refused(..) => {
                self.start_authorize();
                None
            }
            ConnLine::Authorized(_) => {
                self.notice = Some("Delete twice revokes this client's key".into());
                None
            }
            ConnLine::LinkOffset => {
                self.edit = Some((ConnEdit::LinkOffset, String::new()));
                None
            }
            ConnLine::PairWith if self.pairing.offers.is_some() => {
                self.pairing.offers = None;
                None
            }
            ConnLine::PairWith => Some(ConnAction::PairPick),
            ConnLine::PairOffer(rig) => {
                let o = self.pairing.offer(rig)?;
                match &o.refusal {
                    Some(why) => {
                        self.error = Some(why.clone());
                        None
                    }
                    None => Some(ConnAction::Pair(rig)),
                }
            }
            ConnLine::Paired(rig) => {
                use ac2_scene::pairing as words;
                let r = self.pairing.row(rig)?;
                if r.halves.both_ways() {
                    self.notice = Some(words::unpair_hint(&r.name));
                    return None;
                }
                if !r.mend() {
                    self.notice = Some(words::cant_tell(&r.name, &r.halves));
                    return None;
                }
                match r.unreached {
                    None => Some(ConnAction::Pair(rig)),
                    Some(words::Unseen::NotConnected) => {
                        self.error =
                            Some(words::refusal(&r.name, &words::PairRefusal::NotConnected));
                        None
                    }
                    Some(_) => {
                        self.error = Some(words::not_held(&r.name));
                        None
                    }
                }
            }
        }
    }

    /// ←/→ (`d`) on an `ac2d` row: its choice flipped; on the network's switch as Enter; on
    /// the interface, the next choice; nothing elsewhere.
    pub fn toggle(&mut self, d: i32) -> Option<ConnAction> {
        match self.focused()? {
            ConnLine::StartLocal => Some(ConnAction::ToggleStartLocal),
            ConnLine::StopOnQuit => Some(ConnAction::ToggleStopOnQuit),
            ConnLine::Network => self.net_enter(&ConnLine::Network),
            ConnLine::Interface => {
                self.cycle_interface(d);
                None
            }
            _ => None,
        }
    }

    /// A on a refused key: types the name it is authorized under.
    pub fn start_authorize(&mut self) -> bool {
        self.clear();
        match self.focused() {
            Some(ConnLine::Refused(Some(key), _)) => {
                self.edit = Some((ConnEdit::Name { key }, String::new()));
                true
            }
            Some(ConnLine::Refused(None, _)) => {
                self.error = Some(
                    "that peer did not use CURVE: it has no key to authorize (an old or foreign \
                     client)"
                        .into(),
                );
                false
            }
            _ => {
                self.notice =
                    Some("A authorizes a refused key: move to one under Refused keys".into());
                false
            }
        }
    }

    /// Delete: revokes the focused authorized client, or unpairs the focused paired rig, on
    /// the second press.
    pub fn delete(&mut self) -> Option<ConnAction> {
        self.notice = None;
        self.error = None;
        let name = match self.focused() {
            Some(ConnLine::Authorized(name)) => name,
            Some(ConnLine::Paired(rig)) => {
                let r = self.pairing.row(rig)?;
                let ask = ac2_scene::pairing::unpair_confirm(&r.name, r.unreached, &r.halves);
                if self.pairing.confirm == Some(rig) {
                    self.pairing.confirm = None;
                    return Some(ConnAction::Unpair(rig));
                }
                self.pairing.confirm = Some(rig);
                self.notice = Some(ask);
                return None;
            }
            _ => {
                self.notice = Some(
                    "Delete revokes an authorized client or unpairs a paired rig: move to one"
                        .into(),
                );
                return None;
            }
        };
        if self.confirm.as_deref() == Some(name.as_str()) {
            self.confirm = None;
            return Some(ConnAction::Revoke(name));
        }
        self.confirm = Some(name.clone());
        self.notice = Some(format!(
            "Delete again revokes {name}: it is refused at once and cannot connect again"
        ));
        None
    }

    pub fn typing(&self) -> bool {
        self.edit.is_some()
    }

    pub fn type_text(&mut self, s: &str) {
        if let Some((_, t)) = &mut self.edit {
            t.push_str(s);
            self.error = None;
        }
    }

    pub fn backspace(&mut self) {
        if let Some((_, t)) = &mut self.edit {
            t.pop();
        }
    }

    /// What the edit line says before the typed text.
    pub fn edit_label(&self) -> Option<&'static str> {
        match &self.edit {
            Some((ConnEdit::Name { .. }, _)) => Some("Authorize this key as (name):"),
            Some((ConnEdit::NameAndKey, _)) => Some("Authorize a client (name, space, key):"),
            Some((ConnEdit::LinkOffset, _)) => Some(ac2_scene::cross_rig::LINK_OFFSET_PROMPT),
            Some((ConnEdit::Address, _)) => Some(ac2_scene::network::ADDRESS_PROMPT),
            Some((ConnEdit::Port, _)) => Some(ac2_scene::network::PORT_PROMPT),
            None => None,
        }
    }
}

/// This client's key, as the Connection page shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientKey {
    pub fingerprint: String,
    /// Z85, for the daemon operator to authorize.
    pub key: String,
}

/// The Connection page's "this app" lines: this app's build (`build`, as `ac2 --version`
/// prints it), what the link is, who this client is, its key.
pub fn link_lines(
    build: &str,
    rig: &str,
    target: &str,
    state: &str,
    server: Option<&str>,
    client_id: Option<&str>,
    key: Option<&ClientKey>,
) -> Vec<String> {
    let mut v = vec![
        format!("Version: ac2-ui {build}"),
        format!("Rig: {rig}"),
        format!("Daemon: {target} · {state}"),
    ];
    if let Some(s) = server {
        v.push(format!("Server software: {s}"));
    }
    if let Some(c) = client_id {
        v.push(format!("This client is {c} on the daemon"));
    }
    v.push(match key {
        Some(k) => format!(
            "This client's key: fingerprint {} · {} (what a rig's operator authorizes)",
            k.fingerprint, k.key
        ),
        None => "This client has no key yet: pairing with a rig (connect dialog) makes one; this \
                 computer's ac2d needs none"
            .into(),
    });
    v
}

/// The *Start ac2d when none is running* row.
pub fn start_local_text(on: bool) -> String {
    if on {
        "Start ac2d when none is running: on (←→)".into()
    } else {
        "Start ac2d when none is running: off: the app only connects, as to a service (←→)".into()
    }
}

/// The *Stop ac2d when the app quits* row; `forced` inside an AppImage.
pub fn stop_on_quit_text(on: bool, forced: bool) -> String {
    if forced {
        return "Stop ac2d when the app quits: always on: the AppImage's files go away when the \
                app quits"
            .into();
    }
    let state = if on { "on" } else { "off" };
    format!(
        "Stop ac2d when the app quits: {state} · the ac2d this app started; one started \
         otherwise keeps running · from the next start of ac2d (←→)"
    )
}

/// The Connection page's action lines (the `ac2d` rows: [`start_local_text`],
/// [`stop_on_quit_text`]).
pub fn conn_line_text(l: &ConnLine, server: Option<&ServerInfo>) -> String {
    match l {
        ConnLine::Reconnect => "Reconnect now (Enter)".into(),
        ConnLine::ConnectOther => {
            "Connect a rig (another daemon, or pair with one)… (Enter)".into()
        }
        ConnLine::Authorized(name) => server
            .and_then(|s| {
                s.network
                    .authorized
                    .iter()
                    .find(|a| &a.name == name)
                    .map(ac2_scene::rig::authorized_row)
            })
            .unwrap_or_else(|| name.clone()),
        // Drawn with what they need from the app (the prefs, the session, the page's picks:
        // [`ConnectionPage::net_line_text`]).
        ConnLine::Network
        | ConnLine::Interface
        | ConnLine::Port
        | ConnLine::Refused(..)
        | ConnLine::StartLocal
        | ConnLine::StopOnQuit
        | ConnLine::LinkOffset
        | ConnLine::Paired(_)
        | ConnLine::PairOffer(_) => String::new(),
        ConnLine::PairWith => ac2_scene::pairing::PAIR_WITH.into(),
        ConnLine::AddKey => "Authorize a client by its key… (Enter)".into(),
    }
}
