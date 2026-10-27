//! The Connection page of the Settings view: the link to the daemon, this client's key, and
//! in network mode the authorized and refused clients.

use ac2_proto::model::{ServerInfo, ServerMode};

/// A line of the Connection page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnLine {
    /// Drop the link and connect again now.
    Reconnect,
    /// Add a rig (another daemon, or pair with one) in the connect dialog: the rigs held
    /// stay.
    ConnectOther,
    /// An authorized client, by name.
    Authorized(String),
    /// A refused key (Z85), or a refused peer without CURVE (`None`).
    Refused(Option<String>, String),
    /// Authorize a key typed with its name.
    AddKey,
}

/// What the Connection page asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnAction {
    Reconnect,
    ConnectOther,
    Authorize { name: String, key: String },
    Revoke(String),
}

/// A typed edit of the Connection page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnEdit {
    /// The name a refused key is authorized under.
    Name { key: String },
    /// `name key`, for a key not seen yet.
    NameAndKey,
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
}

/// Seconds between `server.info` while the Connection page is shown: refused keys appear
/// while the operator watches for a new client to knock.
pub const SERVER_INFO_EVERY_S: f64 = 2.0;

/// The page's lines for `server` (the actions, then the network mode's keys).
pub fn conn_lines(server: Option<&ServerInfo>) -> Vec<ConnLine> {
    let mut v = vec![ConnLine::Reconnect, ConnLine::ConnectOther];
    if let Some(ServerInfo {
        mode:
            ServerMode::Network {
                authorized,
                refused,
                ..
            },
        ..
    }) = server
    {
        v.extend(
            authorized
                .iter()
                .map(|a| ConnLine::Authorized(a.name.clone())),
        );
        v.extend(
            refused
                .iter()
                .map(|r| ConnLine::Refused(r.key.clone(), r.address.clone())),
        );
        v.push(ConnLine::AddKey);
    }
    v
}

impl ConnectionPage {
    fn server_info(&self) -> Option<&ServerInfo> {
        self.server.as_ref().and_then(|r| r.as_ref().ok())
    }

    /// The lines as of the last answer.
    pub fn lines(&self) -> Vec<ConnLine> {
        conn_lines(self.server_info())
    }

    /// The focused line (clamped: lines come and go with the daemon's answers).
    pub fn focused(&self) -> Option<ConnLine> {
        let l = self.lines();
        l.get(self.focus.min(l.len().saturating_sub(1))).cloned()
    }

    fn clear(&mut self) {
        self.confirm = None;
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

    /// Delete: revokes the focused authorized client on the second press.
    pub fn delete(&mut self) -> Option<ConnAction> {
        self.notice = None;
        self.error = None;
        let Some(ConnLine::Authorized(name)) = self.focused() else {
            self.notice = Some("Delete revokes an authorized client: move to one".into());
            return None;
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
        None => "This client has no key yet: pairing with a rig (connect dialog) makes one; local \
                 and embedded daemons need none"
            .into(),
    });
    v
}

/// The Connection page's action lines.
pub fn conn_line_text(l: &ConnLine, server: Option<&ServerInfo>) -> String {
    match l {
        ConnLine::Reconnect => "Reconnect now (Enter)".into(),
        ConnLine::ConnectOther => {
            "Connect a rig (another daemon, or pair with one)… (Enter)".into()
        }
        ConnLine::Authorized(name) => server
            .and_then(|s| match &s.mode {
                ServerMode::Network { authorized, .. } => authorized
                    .iter()
                    .find(|a| &a.name == name)
                    .map(ac2_scene::rig::authorized_row),
                _ => None,
            })
            .unwrap_or_else(|| name.clone()),
        ConnLine::Refused(..) => String::new(),
        ConnLine::AddKey => "Authorize a client by its key… (Enter)".into(),
    }
}
