//! The connect dialog: choose the local daemon, an embedded one, or a rig found over mDNS.
//!
//! Discovered rigs are only names until paired. A rig is selectable when a pinned key has
//! the fingerprint it advertises; the link then connects with that pinned key and CURVE
//! proves the daemon holds it, so a forged advert gets nowhere. Pairing from here takes the
//! daemon's full key, typed or pasted by the operator from the daemon host, and refuses a key
//! whose fingerprint differs from the advert's (a typo, or an advert that is not that
//! daemon's). It then shows this client's key for the daemon's `authorized_clients`.

use ac2_client::{ClientConfig, Endpoints, KeyDir, PinStatus, RemoteAddr};
use ac2_discovery::{Browser, Options, Rig, RigTable};
use ac2_zmq::PublicKey;
use eframe::egui;

use crate::conn::Target;
use crate::embedded::EmbeddedBackend;
use crate::theme::Chrome;

/// What the operator picked.
#[derive(Clone, Debug)]
pub enum Choice {
    /// A daemon to connect to as is.
    Target(Box<Target>),
    /// Host a daemon in this process.
    Embedded(EmbeddedBackend),
}

/// One row of the dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// The per-user local daemon (`ac2 daemon start`).
    Local { running: bool },
    /// An embedded daemon on a backend.
    Embedded(EmbeddedBackend),
    /// A discovered rig and how it relates to the pinned keys.
    Rig { rig: Box<Rig>, status: PinStatus },
}

impl Entry {
    /// Connectable now.
    pub fn enabled(&self) -> bool {
        match self {
            Entry::Local { .. } | Entry::Embedded(_) => true,
            Entry::Rig { rig, status } => {
                matches!(status, PinStatus::Paired { .. })
                    && rig.advert.proto == ac2_proto::PROTO_VERSION
            }
        }
    }

    pub fn title(&self) -> String {
        match self {
            Entry::Local { .. } => "Local daemon".into(),
            Entry::Embedded(EmbeddedBackend::Cpal) => "This computer's audio".into(),
            Entry::Embedded(EmbeddedBackend::Jack) => "This computer's audio (JACK)".into(),
            Entry::Embedded(EmbeddedBackend::Fake) => "Simulated rig (no audio)".into(),
            Entry::Rig { rig, .. } => rig.advert.name.clone(),
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Entry::Local { running: true } => "ac2d is running for this user".into(),
            Entry::Local { running: false } => {
                "not running; start it with `ac2 daemon start`".into()
            }
            Entry::Embedded(EmbeddedBackend::Fake) => {
                "a daemon in this app with a built-in loopback + speaker model, for trying ac2"
                    .into()
            }
            Entry::Embedded(EmbeddedBackend::Jack) => {
                "a daemon inside this app on JACK (JACK2 or PipeWire's); closes with the window"
                    .into()
            }
            Entry::Embedded(_) => "a daemon inside this app; closes with the window".into(),
            Entry::Rig { rig, status } => {
                let mut s = format!(
                    "{}:{}  ·  ac2d {}  ·  key {}",
                    rig.connect_host(),
                    rig.port,
                    rig.advert.version,
                    rig.advert.fingerprint
                );
                if rig.advert.proto != ac2_proto::PROTO_VERSION {
                    s.push_str(&format!(
                        "  ·  protocol {} (this app speaks {})",
                        rig.advert.proto,
                        ac2_proto::PROTO_VERSION
                    ));
                }
                s.push_str(match status {
                    PinStatus::Paired { .. } => "  ·  paired",
                    PinStatus::Mismatch { .. } => "  ·  KEY CHANGED: pair again only if expected",
                    PinStatus::Unpaired => "  ·  not paired",
                });
                s
            }
        }
    }
}

/// Outcome of a pairing attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairOutcome {
    /// Pinned. `authorized_line` goes into the daemon's `authorized_clients`.
    Pinned {
        host: String,
        authorized_line: String,
    },
    Refused(String),
}

/// Whether the local daemon looks up: its socket file (Unix) or its ctrl port (Windows).
pub fn local_daemon_running() -> bool {
    #[cfg(unix)]
    {
        ac2_client::endpoint::runtime_dir()
            .join("ctrl.sock")
            .exists()
    }
    #[cfg(not(unix))]
    {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], ac2_client::endpoint::DEFAULT_PORT));
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(150)).is_ok()
    }
}

pub struct ConnectDialog {
    key_dir: KeyDir,
    client_name: String,
    backends: Vec<EmbeddedBackend>,
    local_running: bool,
    browser: Option<Browser>,
    pub browse_error: Option<String>,
    pub table: RigTable,
    pub selected: usize,
    /// Pairing form: the rig instance it is open for, the key text, the last outcome.
    pair: Option<(String, String, Option<PairOutcome>)>,
    /// Shown under the list (embedded start failed, …).
    pub error: Option<String>,
}

impl std::fmt::Debug for ConnectDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectDialog")
            .field("rigs", &self.table.len())
            .finish_non_exhaustive()
    }
}

impl ConnectDialog {
    /// `mdns: None` lists no network rigs (tests, or mDNS unwanted).
    pub fn new(
        key_dir: KeyDir,
        client_name: impl Into<String>,
        backends: Vec<EmbeddedBackend>,
        local_running: bool,
        mdns: Option<&Options>,
    ) -> Self {
        let (browser, browse_error) = match mdns.map(Browser::start) {
            Some(Ok(b)) => (Some(b), None),
            Some(Err(e)) => (None, Some(e.to_string())),
            None => (None, None),
        };
        let selected = usize::from(!local_running && !backends.is_empty());
        Self {
            key_dir,
            client_name: client_name.into(),
            backends,
            local_running,
            browser,
            browse_error,
            table: RigTable::default(),
            // Preselect what works without further steps: the running local daemon, else
            // this computer's audio in an embedded daemon.
            selected,
            pair: None,
            error: None,
        }
    }

    /// The name sent in `hello`.
    pub fn client_name(&self) -> String {
        self.client_name.clone()
    }

    /// Applies browse results; `true` when the list changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(b) = &self.browser {
            for u in b.poll() {
                changed |= self.table.apply(u);
            }
        }
        changed
    }

    pub fn entries(&self) -> Vec<Entry> {
        let mut v = vec![Entry::Local {
            running: self.local_running,
        }];
        v.extend(self.backends.iter().copied().map(Entry::Embedded));
        for r in self.table.rigs() {
            let host = r.connect_host();
            let status = self
                .key_dir
                .pin_status(&r.advert.fingerprint, &[r.host.as_str(), host.as_str()])
                .unwrap_or(PinStatus::Unpaired);
            v.push(Entry::Rig {
                rig: Box::new(r.clone()),
                status,
            });
        }
        v
    }

    /// The connection for `e`, or why it cannot be used.
    pub fn choice(&self, e: &Entry) -> Result<Choice, String> {
        match e {
            Entry::Local { .. } => Ok(Choice::Target(Box::new(Target {
                config: ClientConfig::local(self.client_name.clone()),
                describe: "local daemon".into(),
            }))),
            Entry::Embedded(b) => Ok(Choice::Embedded(*b)),
            Entry::Rig { rig, status } => {
                let PinStatus::Paired { key, .. } = status else {
                    return Err(format!("{} is not paired", rig.advert.name));
                };
                if rig.advert.proto != ac2_proto::PROTO_VERSION {
                    return Err(format!(
                        "{} speaks protocol {}; this app speaks {}",
                        rig.advert.name,
                        rig.advert.proto,
                        ac2_proto::PROTO_VERSION
                    ));
                }
                let addr = RemoteAddr {
                    host: rig.connect_host(),
                    port: rig.port,
                };
                let mut config =
                    ClientConfig::new(Endpoints::remote(&addr), self.client_name.clone());
                config.curve = Some(
                    self.key_dir
                        .curve_client_for_key(*key)
                        .map_err(|e| e.to_string())?,
                );
                Ok(Choice::Target(Box::new(Target {
                    config,
                    describe: format!("{} ({addr})", rig.advert.name),
                })))
            }
        }
    }

    /// Pins `key_text` for `rig` after checking it has the advertised fingerprint.
    pub fn pair(&self, rig: &Rig, key_text: &str) -> PairOutcome {
        let Ok(key) = PublicKey::from_z85(key_text.trim()) else {
            return PairOutcome::Refused(
                "not a daemon key: expected the 40-character key the daemon prints".into(),
            );
        };
        if key.fingerprint() != rig.advert.fingerprint {
            return PairOutcome::Refused(format!(
                "this key's fingerprint is {}, the rig advertises {}: check the key on the \
                 daemon host; never pair with a key whose fingerprint you have not compared",
                key.fingerprint(),
                rig.advert.fingerprint
            ));
        }
        let host = rig.connect_host();
        let r = self
            .key_dir
            .ensure_client_keypair()
            .and_then(|(kp, _)| self.key_dir.pin_server(&host, key).map(|_| kp));
        match r {
            Ok(kp) => {
                let name = ac2_discovery::local_host_label();
                PairOutcome::Pinned {
                    host,
                    authorized_line: format!("{name} {}", kp.public.to_z85()),
                }
            }
            Err(e) => PairOutcome::Refused(e.to_string()),
        }
    }

    /// Draws the dialog; returns the operator's choice.
    pub fn show(&mut self, ctx: &egui::Context, ch: &Chrome) -> Option<Choice> {
        if self.poll() {
            ctx.request_repaint();
        }
        // Adverts arrive on mDNS's thread; look again soon while the dialog is open.
        ctx.request_repaint_after(std::time::Duration::from_millis(300));
        let entries = self.entries();
        self.selected = self.selected.min(entries.len().saturating_sub(1));
        let mut picked = None;
        egui::Modal::new(egui::Id::new("ac2-connect")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading("Connect to a rig");
            ui.label(
                egui::RichText::new(
                    "Pick where measurements run. Rigs on this network appear as they answer.",
                )
                .color(ch.dim),
            );
            ui.add_space(8.0);
            for (i, e) in entries.iter().enumerate() {
                let sel = i == self.selected;
                let title = egui::RichText::new(e.title()).strong().color(if e.enabled() {
                    ch.text
                } else {
                    ch.dim
                });
                let r = ui.add(egui::Button::selectable(sel, title).min_size(egui::vec2(540.0, 0.0)));
                if r.clicked() {
                    self.selected = i;
                }
                if r.double_clicked() && e.enabled() {
                    picked = Some(i);
                }
                ui.label(egui::RichText::new(e.detail()).small().color(ch.dim));
                if let Entry::Rig { rig, status } = e
                    && sel
                    && !matches!(status, PinStatus::Paired { .. })
                {
                    self.pair_form(ui, ch, rig);
                }
                ui.add_space(4.0);
            }
            if self.table.is_empty() {
                let msg = match &self.browse_error {
                    Some(e) => format!("Network discovery is unavailable ({e})."),
                    None if self.browser.is_some() => {
                        "Looking for rigs… (a daemon advertises in network mode: ac2d --listen tcp://0.0.0.0)".into()
                    }
                    None => "Network discovery is off.".into(),
                };
                ui.label(egui::RichText::new(msg).small().color(ch.dim));
            }
            if let Some(err) = &self.error {
                ui.colored_label(ch.fault, err);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let can = entries.get(self.selected).is_some_and(Entry::enabled);
                if ui.add_enabled(can, egui::Button::new("Connect")).clicked() {
                    picked = Some(self.selected);
                }
            });
        });
        let i = picked?;
        let e = entries.get(i)?;
        match self.choice(e) {
            Ok(c) => Some(c),
            Err(msg) => {
                self.error = Some(msg);
                None
            }
        }
    }

    fn pair_form(&mut self, ui: &mut egui::Ui, ch: &Chrome, rig: &Rig) {
        let open_for_this = self
            .pair
            .as_ref()
            .is_some_and(|(inst, _, _)| *inst == rig.instance);
        if !open_for_this {
            self.pair = Some((rig.instance.clone(), String::new(), None));
        }
        let Some((_, text, _)) = self.pair.as_mut() else {
            return;
        };
        ui.label(
            egui::RichText::new(format!(
                "Pair: on the daemon host, read its key and fingerprint (the ac2d log prints \
                 both). Check the fingerprint is {} and paste the key:",
                rig.advert.fingerprint
            ))
            .small(),
        );
        let mut submit = false;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .desired_width(400.0)
                    .hint_text("daemon key (40 characters)"),
            );
            submit = ui.button("Pair").clicked();
        });
        if submit {
            let key = text.clone();
            let o = self.pair(rig, &key);
            if let Some((_, _, out)) = self.pair.as_mut() {
                *out = Some(o);
            }
        }
        match self.pair.as_ref().and_then(|p| p.2.as_ref()) {
            Some(PairOutcome::Pinned {
                host,
                authorized_line,
            }) => {
                ui.colored_label(ch.ok, format!("Pinned the daemon key for {host}."));
                ui.label(
                    egui::RichText::new(
                        "Now authorize this computer: add this line to authorized_clients on the daemon host, then connect.",
                    )
                    .small(),
                );
                let mut line = authorized_line.clone();
                ui.add(egui::TextEdit::singleline(&mut line).desired_width(540.0));
            }
            Some(PairOutcome::Refused(why)) => {
                ui.colored_label(ch.fault, why);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use ac2_zmq::KeyPair;

    use super::*;

    fn rig(name: &str, key: &PublicKey, addr: &str) -> Rig {
        let fp = key.fingerprint();
        let z85 = key.to_z85();
        let txt = move |k: &str| -> Option<String> {
            match k {
                "txtvers" => Some(ac2_discovery::TXT_VERSION.into()),
                "name" => Some(name.to_owned()),
                "v" => Some("1.0.0".into()),
                "proto" => Some(ac2_proto::PROTO_VERSION.to_string()),
                "fp" => Some(fp.clone()),
                "key" => Some(z85.clone()),
                _ => None,
            }
        };
        let addrs: Vec<IpAddr> = addr.parse().into_iter().collect();
        Rig::from_parts(
            &format!("{name}._ac2._tcp.local."),
            "rig.local.",
            47_820,
            addrs,
            txt,
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    fn dialog(dir: &std::path::Path) -> ConnectDialog {
        ConnectDialog::new(
            KeyDir::new(dir),
            "test",
            vec![EmbeddedBackend::Fake],
            false,
            None,
        )
    }

    #[test]
    fn unpaired_rig_is_listed_but_not_connectable_until_paired() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let mut d = dialog(tmp.path());
        let server = KeyPair::generate().unwrap_or_else(|e| panic!("{e}"));
        let r = rig("FOH", &server.public, "10.0.0.5");
        d.table.apply(ac2_discovery::Update::Resolved(r.clone()));
        let e = d.entries();
        assert_eq!(e.len(), 3);
        assert!(e[0].enabled() && e[1].enabled());
        assert!(!e[2].enabled());
        assert!(d.choice(&e[2]).is_err());

        // A key with another fingerprint is refused and pins nothing.
        let other = KeyPair::generate().unwrap_or_else(|e| panic!("{e}"));
        assert!(matches!(
            d.pair(&r, &other.public.to_z85()),
            PairOutcome::Refused(_)
        ));
        assert!(matches!(d.pair(&r, "garbage"), PairOutcome::Refused(_)));
        assert!(!d.entries()[2].enabled());

        // The right key pins under the address and yields the authorized_clients line.
        match d.pair(&r, &server.public.to_z85()) {
            PairOutcome::Pinned {
                host,
                authorized_line,
            } => {
                assert_eq!(host, "10.0.0.5");
                let kp = KeyDir::new(tmp.path())
                    .client_keypair()
                    .unwrap_or_else(|e| panic!("{e}"));
                assert!(authorized_line.ends_with(&kp.public.to_z85()));
            }
            other => panic!("{other:?}"),
        }
        let e = d.entries();
        assert!(e[2].enabled());
        match d.choice(&e[2]) {
            Ok(Choice::Target(t)) => {
                assert_eq!(t.config.endpoints.ctrl, "tcp://10.0.0.5:47820");
                let curve = t.config.curve.unwrap_or_else(|| panic!("CURVE"));
                assert_eq!(curve.server_key, server.public);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rekeyed_rig_shows_mismatch() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let d0 = dialog(tmp.path());
        let old = KeyPair::generate().unwrap_or_else(|e| panic!("{e}"));
        let new = KeyPair::generate().unwrap_or_else(|e| panic!("{e}"));
        KeyDir::new(tmp.path())
            .pin_server("10.0.0.5", old.public)
            .unwrap_or_else(|e| panic!("{e}"));
        let mut d = d0;
        d.table.apply(ac2_discovery::Update::Resolved(rig(
            "FOH",
            &new.public,
            "10.0.0.5",
        )));
        let e = d.entries();
        assert!(matches!(
            &e[2],
            Entry::Rig {
                status: PinStatus::Mismatch { .. },
                ..
            }
        ));
        assert!(!e[2].enabled());
        assert!(e[2].detail().contains("KEY CHANGED"));
    }
}
