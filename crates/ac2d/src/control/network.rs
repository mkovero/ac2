//! The network listener as the control thread runs it: binding a new pair on a new context
//! for every attach, handing it to the I/O thread in band, watching the bound address and
//! retrying on a backoff whatever went wrong, so the network never needs an operator and
//! never stops the daemon.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ac2_proto::ProtoError;
use ac2_proto::model::{NetSaved, NetSetting, NotSavedReason, ServerSummary};
use ac2_zmq::{AuthorizedHandle, AuthorizedKeys, KeyPair, PublicKey};

use crate::authlog::{AuthLog, RefusedList};
use crate::config::{Listen, Mdns, NetworkSecurity};
use crate::damage::Damage;
use crate::io::{Attach, AttachQueue};
use crate::network::{
    Backoff, Host, Interfaces, NetBind, NetListen, NetPort, NetReason, NetStatus, NetworkChoice,
    bind_pair, check_port, resolve,
};
use crate::network_store::{NetSettings, NetStore};
use crate::outbox::Outbox;
use crate::remote::follow::PeerBrowse;
use crate::route::NetEpoch;
use crate::{StartError, keys};

use super::ControlMsg;

/// Shared with [`crate::Handle`]: what it reports of the network.
#[derive(Clone, Debug)]
pub(crate) struct NetView {
    pub(crate) status: Arc<Mutex<NetStatus>>,
    pub(crate) advertised_as: Arc<Mutex<Option<String>>>,
    pub(crate) server_key: Arc<Mutex<Option<PublicKey>>>,
    #[cfg(feature = "fault-injection")]
    pub(crate) fault: Arc<Mutex<Option<ac2_zmq::ZapFault>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl NetView {
    pub(crate) fn status(&self) -> NetStatus {
        lock(&self.status).clone()
    }

    pub(crate) fn advertised_as(&self) -> Option<String> {
        lock(&self.advertised_as).clone()
    }

    pub(crate) fn server_key(&self) -> Option<PublicKey> {
        *lock(&self.server_key)
    }
}

/// The authorized-clients list and where it is kept.
pub(crate) struct Authorized {
    pub(crate) handle: AuthorizedHandle,
    pub(crate) file: PathBuf,
    audit: Arc<AuthLog>,
}

/// What the network pair bound to, while attached.
struct Bound {
    epoch: NetEpoch,
    host: Host,
    /// Its `TAG_DETACH` was refused by a full pipe and is sent again: until it goes
    /// through the pair holds its port, and every rebind would find it in use.
    detaching: bool,
    /// The detach turns the network off: its peers are told so before it closes.
    off: bool,
}

/// What [`NetCtl::new`] needs.
pub(crate) struct NetSetup {
    pub(crate) choice: NetworkChoice,
    /// The local endpoints, which a network port must not take.
    pub(crate) local: Listen,
    pub(crate) files: Option<NetworkSecurity>,
    pub(crate) interfaces: Interfaces,
    pub(crate) retry: Backoff,
    pub(crate) mdns: Option<Mdns>,
    pub(crate) name: String,
    pub(crate) attach: AttachQueue,
    pub(crate) to_control: Sender<ControlMsg>,
}

/// The network's state on the control thread.
pub(crate) struct NetCtl {
    choice: NetworkChoice,
    local: Listen,
    /// The setting of a [`NetworkChoice::FromSettings`] network and where it is kept.
    settings: NetSettings,
    store: Option<NetStore>,
    files: Option<NetworkSecurity>,
    /// The server key pair, once one exists: outgoing rig links use it on or off.
    pub(crate) keys: Option<KeyPair>,
    /// `None` without key files (in-process daemons): no client keys at all.
    pub(crate) authorized: Option<Authorized>,
    pub(crate) refused: RefusedList,
    view: NetView,
    epoch: NetEpoch,
    bound: Option<Bound>,
    retry: Backoff,
    delay: Duration,
    due: Option<Instant>,
    interfaces: Interfaces,
    attach: AttachQueue,
    to_control: Sender<ControlMsg>,
    mdns: Option<Mdns>,
    name: String,
    advert: Option<ac2_discovery::Advertiser>,
    browse: Option<PeerBrowse>,
}

impl NetCtl {
    /// Loads the key files: the server key is created when the network is on, and only
    /// read when it is off (it then exists only once the network was on).
    pub(crate) fn new(s: NetSetup) -> Result<(Self, Vec<Damage>), StartError> {
        let mut damage = Vec::new();
        let (store, settings) = match &s.choice {
            NetworkChoice::FromSettings { store } => {
                let (store, settings, d) = NetStore::load(store.clone());
                damage.extend(d);
                (Some(store), settings)
            }
            NetworkChoice::Off | NetworkChoice::Fixed(_) => (None, NetSettings::default()),
        };
        let on = match &s.choice {
            NetworkChoice::Fixed(_) => true,
            NetworkChoice::FromSettings { .. } => settings.enabled,
            NetworkChoice::Off => false,
        };
        let mut keys = None;
        let mut authorized = None;
        let refused = match &s.files {
            Some(f) => {
                if on || f.server_key_file.exists() {
                    let (kp, d) = keys::load_or_create_server_keys(&f.server_key_file)
                        .map_err(StartError::Keys)?;
                    damage.extend(d);
                    keys = Some(kp);
                }
                let list = if on || f.authorized_clients_file.exists() {
                    let (a, d) = keys::load_or_create_authorized(&f.authorized_clients_file)
                        .map_err(StartError::Keys)?;
                    damage.extend(d);
                    a
                } else {
                    AuthorizedKeys::new()
                };
                if let Some(kp) = &keys {
                    tracing::info!(
                        "{} authorized client(s); server key {} (fingerprint {})",
                        list.len(),
                        kp.public.to_z85(),
                        kp.public.fingerprint()
                    );
                }
                let audit = Arc::new(AuthLog::new(&f.authorized_clients_file));
                let refused = audit.refused();
                authorized = Some(Authorized {
                    handle: AuthorizedHandle::new(list),
                    file: f.authorized_clients_file.clone(),
                    audit,
                });
                refused
            }
            None => RefusedList::default(),
        };
        let retry = s.retry;
        let server_key = keys.as_ref().map(|k| k.public);
        Ok((
            Self {
                choice: s.choice,
                local: s.local,
                settings,
                store,
                files: s.files,
                keys,
                authorized,
                refused,
                view: NetView {
                    status: Arc::new(Mutex::new(NetStatus::Off)),
                    advertised_as: Arc::new(Mutex::new(None)),
                    server_key: Arc::new(Mutex::new(server_key)),
                    #[cfg(feature = "fault-injection")]
                    fault: Arc::new(Mutex::new(None)),
                },
                epoch: NetEpoch(0),
                bound: None,
                retry,
                delay: retry.first,
                due: None,
                interfaces: s.interfaces,
                attach: s.attach,
                to_control: s.to_control,
                mdns: s.mdns,
                name: s.name,
                advert: None,
                browse: None,
            },
            damage,
        ))
    }

    /// The host's interfaces a network could bind, loopback left out.
    pub(crate) fn interfaces(&self) -> Vec<ac2_proto::model::NetInterface> {
        self.interfaces
            .list()
            .into_iter()
            .filter(|i| !i.addrs.is_empty() && !i.addrs.iter().all(std::net::IpAddr::is_loopback))
            .collect()
    }

    pub(crate) fn view(&self) -> NetView {
        self.view.clone()
    }

    pub(crate) fn server_key(&self) -> Option<PublicKey> {
        self.keys.as_ref().map(|k| k.public)
    }

    pub(crate) fn status(&self) -> NetStatus {
        self.view.status()
    }

    /// The setting when the network is on.
    pub(crate) fn listen(&self) -> Option<NetListen> {
        match &self.choice {
            NetworkChoice::Off => None,
            NetworkChoice::Fixed(l) => Some(l.clone()),
            NetworkChoice::FromSettings { .. } => self.settings.enabled.then(|| NetListen {
                bind: self.settings.bind.clone(),
                port: NetPort::Port(self.settings.port),
            }),
        }
    }

    /// The setting as `server.info` reports it.
    pub(crate) fn setting(&self) -> NetSetting {
        match (&self.choice, self.listen()) {
            (NetworkChoice::Fixed(l), _) => NetSetting::Fixed {
                listen: intended(l).0,
            },
            (_, Some(l)) => NetSetting::On {
                bind: l.bind,
                port: self.settings.port,
            },
            (_, None) => NetSetting::Off,
        }
    }

    /// Whether the setting outlives a restart.
    pub(crate) fn saved(&self) -> NetSaved {
        self.store.as_ref().map_or(
            NetSaved::NotSaved {
                reason: NotSavedReason::NoStore,
            },
            NetStore::saved_state,
        )
    }

    /// What `State.server` shows.
    pub(crate) fn summary(&self) -> ServerSummary {
        let status = self.status();
        ServerSummary {
            status: status.kind(),
            ctrl: match status {
                NetStatus::Listening { ctrl, .. } => Some(ctrl),
                _ => None,
            },
            port: self.listen().and_then(|l| match l.port {
                NetPort::Port(p) => Some(p),
                NetPort::Any => None,
            }),
            fingerprint: self.keys.as_ref().map(|k| k.public.fingerprint()),
        }
    }

    /// `server.network`: the new setting in force at once, kept, and the listener turned on,
    /// moved or off to match. `Ok(true)` when the server key was created by it.
    pub(crate) fn set(
        &mut self,
        enabled: bool,
        bind: Option<NetBind>,
        port: Option<u16>,
        outbox: &Outbox,
        now: Instant,
    ) -> Result<bool, ProtoError> {
        match &self.choice {
            NetworkChoice::Fixed(l) => {
                return Err(crate::util::refused(format!(
                    "the network is fixed by ac2d --listen {}; restart ac2d without --listen                      to change it",
                    intended(l).0
                )));
            }
            NetworkChoice::Off => {
                return Err(crate::util::perr(
                    ac2_proto::ErrorCode::Unsupported,
                    "this daemon serves in-process only: it has no network to turn on",
                ));
            }
            NetworkChoice::FromSettings { .. } => {}
        }
        let new = NetSettings {
            enabled,
            bind: bind.unwrap_or_else(|| self.settings.bind.clone()),
            port: port.unwrap_or(self.settings.port),
        };
        check_port(new.port, &self.local).map_err(|e| crate::util::invalid(e.0))?;
        if let NetBind::Interface { name } = &new.bind
            && (name.is_empty() || name.chars().any(char::is_control))
        {
            return Err(crate::util::invalid("an interface name must not be empty"));
        }
        let created = enabled && self.ensure_keys()?;
        let old = std::mem::replace(&mut self.settings, new.clone());
        if let Some(store) = &mut self.store {
            store.save(&new);
        }
        let moved = old.bind != new.bind || old.port != new.port;
        match (old.enabled, enabled) {
            (_, false) => {
                self.detach(outbox, true);
                self.set_status(NetStatus::Off);
                // A detach the pipe refused is sent again by the tick.
                self.due = self.bound.as_ref().map(|_| now);
            }
            (true, true) if !moved => {}
            (was_on, true) => {
                if was_on {
                    self.detach(outbox, false);
                }
                self.delay = self.retry.first;
                if self.bound.is_some() {
                    self.due = Some(now);
                } else {
                    self.attempt(outbox, now);
                }
            }
        }
        Ok(created)
    }

    /// Loads or creates the server key and the authorized-clients file before the first
    /// enable. `Ok(true)`: the key is new.
    fn ensure_keys(&mut self) -> Result<bool, ProtoError> {
        if self.keys.is_some() {
            return Ok(false);
        }
        let Some(f) = &self.files else {
            return Ok(false);
        };
        let fail = |e: keys::KeyFileError| crate::util::internal(e.to_string());
        let (kp, d) = keys::load_or_create_server_keys(&f.server_key_file).map_err(fail)?;
        d.iter().for_each(Damage::log);
        if let Some(a) = &self.authorized
            && !f.authorized_clients_file.exists()
        {
            let (list, d) =
                keys::load_or_create_authorized(&f.authorized_clients_file).map_err(fail)?;
            d.iter().for_each(Damage::log);
            a.handle.set(list);
        }
        tracing::info!(
            "server key {} (fingerprint {}) created",
            kp.public.to_z85(),
            kp.public.fingerprint()
        );
        *lock(&self.view.server_key) = Some(kp.public);
        self.keys = Some(kp);
        Ok(true)
    }

    pub(crate) fn advertised_as(&self) -> Option<String> {
        self.view.advertised_as()
    }

    fn set_status(&self, s: NetStatus) {
        *lock(&self.view.status) = s;
    }

    /// Whether the authorization a CURVE connection was granted (`key` under `name`) is gone.
    /// The name alone is not enough: a revoked name can later be given to a different key,
    /// and that must not revive a connection the old key still holds open.
    pub(crate) fn revoked(&self, name: &str, key: &PublicKey) -> bool {
        self.authorized
            .as_ref()
            .is_none_or(|a| a.handle.get().get(name) != Some(key))
    }

    /// Whether `name` is admitted as a rig rather than an app.
    pub(crate) fn is_rig(&self, name: &str) -> bool {
        self.authorized
            .as_ref()
            .is_some_and(|a| a.handle.get().rig_of(name).is_some())
    }

    /// When [`Self::tick`] has work.
    pub(crate) fn due(&self) -> Option<Instant> {
        self.due
    }

    /// The retry, the resent detach or the check of the bound address, when due.
    pub(crate) fn tick(&mut self, outbox: &Outbox, now: Instant) {
        if self.due.is_none_or(|d| d > now) {
            return;
        }
        self.due = None;
        let Some(b) = &self.bound else {
            self.attempt(outbox, now);
            return;
        };
        if b.detaching {
            let off = b.off;
            self.detach(outbox, off);
            // Bound again only after the port is let go; a new bind in between would only
            // find it in use.
            self.due = Some(now + self.retry.first);
            return;
        }
        let Some(l) = self.listen() else {
            return;
        };
        // Every address of the host is always there to listen on.
        if l.bind == NetBind::All {
            self.due = Some(now + self.retry.first);
            return;
        }
        // A listener on an address that went away silently accepts nothing; one bound
        // where the interface no longer resolves serves the wrong network.
        match resolve(&l.bind, &self.interfaces.list()) {
            Ok(h) if h == b.host => self.due = Some(now + self.retry.first),
            r => {
                let reason = r.err().unwrap_or(NetReason::AddressMissing);
                tracing::warn!("network: the bound address went away ({reason}); rebinding");
                self.set_status(NetStatus::Waiting { reason });
                self.detach(outbox, false);
                // The closing pair needs a moment to let go of the port.
                self.delay = self.retry.first;
                self.due = Some(now + self.retry.first);
            }
        }
    }

    /// Binds a new pair and attaches it; on failure `Waiting`, retried.
    pub(crate) fn attempt(&mut self, outbox: &Outbox, now: Instant) {
        if self.bound.is_some() {
            return;
        }
        let Some(l) = self.listen() else {
            self.set_status(NetStatus::Off);
            return;
        };
        let (Some(kp), Some(a)) = (&self.keys, &self.authorized) else {
            self.wait(NetReason::NoServerKey, now);
            return;
        };
        let host = match resolve(&l.bind, &self.interfaces.list()) {
            Ok(h) => h,
            Err(reason) => {
                self.wait(reason, now);
                return;
            }
        };
        match bind_pair(host, l.port, kp, &a.handle, Arc::clone(&a.audit)) {
            Ok(pair) => {
                self.epoch = NetEpoch(self.epoch.0 + 1);
                let epoch = self.epoch;
                let (ctrl, data) = (pair.ctrl_ep.clone(), pair.data_ep.clone());
                #[cfg(feature = "fault-injection")]
                let fault = pair.secure.zap_fault();
                self.attach.put(Attach { epoch, pair });
                if !outbox.attach(epoch) {
                    // Never served: taken back and closed, so its port is free for the retry.
                    if let Some(a) = self.attach.withdraw(epoch) {
                        crate::network::close(a.pair);
                    }
                    self.wait(NetReason::NotHandedOver, now);
                    return;
                }
                #[cfg(feature = "fault-injection")]
                {
                    *lock(&self.view.fault) = Some(fault);
                }
                tracing::info!("network: listening on ctrl {ctrl}, data {data}");
                crate::firewall::report(&ctrl, &data);
                self.bound = Some(Bound {
                    epoch,
                    host,
                    detaching: false,
                    off: false,
                });
                self.delay = self.retry.first;
                self.due = Some(now + self.retry.first);
                self.start_advert(host, &ctrl);
                self.set_status(NetStatus::Listening { ctrl, data });
            }
            Err(reason) => self.wait(reason, now),
        }
    }

    fn wait(&mut self, reason: NetReason, now: Instant) {
        tracing::warn!(
            "network not listening: {reason}; retrying in {} s",
            self.delay.as_secs_f64()
        );
        self.set_status(NetStatus::Waiting { reason });
        self.due = Some(now + self.delay);
        self.delay = self.retry.after(self.delay);
    }

    /// Detaches the pair in band, after everything queued before; a refused detach stays
    /// bound, marked, and is sent again at the next tick. `off`: the network is turned off,
    /// and the pair's peers are told so before it closes.
    fn detach(&mut self, outbox: &Outbox, off: bool) {
        self.stop_advert();
        let Some(b) = &mut self.bound else {
            return;
        };
        if outbox.detach(b.epoch, off) {
            self.bound = None;
        } else {
            b.detaching = true;
            b.off |= off;
        }
    }

    /// The I/O thread closed pair `epoch` (its ZAP handler died): `Failed`, retried with a
    /// new context.
    pub(crate) fn failed(&mut self, epoch: NetEpoch, reason: NetReason, now: Instant) {
        if self.bound.as_ref().is_none_or(|b| b.epoch != epoch) {
            return;
        }
        self.bound = None;
        self.stop_advert();
        tracing::error!(
            "network failed: {reason}; retrying in {} s",
            self.delay.as_secs_f64()
        );
        self.set_status(NetStatus::Failed { reason });
        self.due = Some(now + self.delay);
        self.delay = self.retry.after(self.delay);
    }

    fn start_advert(&mut self, host: Host, ctrl: &str) {
        let (Some(m), Some(kp)) = (&self.mdns, &self.keys) else {
            return;
        };
        if !m.advertise {
            return;
        }
        let bind = match host {
            Host::Any => ac2_discovery::Bind::Any,
            Host::Addr(a) => ac2_discovery::Bind::Addr(a),
        };
        self.advert = advertise(&self.name, &m.options, &kp.public, ctrl, &bind);
        *lock(&self.view.advertised_as) = self.advert.as_ref().map(|a| a.fullname().to_owned());
    }

    /// Its goodbye blocks up to about a second, which the control thread must not wait
    /// (leases expire in 1.5 s).
    fn stop_advert(&mut self) {
        *lock(&self.view.advertised_as) = None;
        if let Some(a) = self.advert.take() {
            let _ = std::thread::Builder::new()
                .name("ac2d-mdns-bye".into())
                .spawn(move || drop(a));
        }
    }

    /// The browse that follows paired rigs to new addresses runs whenever there is a key to
    /// link with and rigs to follow, whatever the listener does.
    pub(crate) fn sync_browse(&mut self, peers: bool) {
        let want = peers && self.keys.is_some();
        match (&self.browse, &self.mdns, want) {
            (None, Some(m), true) => {
                self.browse = PeerBrowse::start(m.options.clone(), self.to_control.clone())
                    .inspect_err(|e| tracing::warn!("mDNS browse not started: {e}"))
                    .ok();
            }
            (Some(_), _, false) => self.browse = None,
            _ => {}
        }
    }

    /// At shutdown: the advert's goodbye goes out before the sockets close, so browsers
    /// drop the rig at once.
    pub(crate) fn shutdown(&mut self) {
        *lock(&self.view.advertised_as) = None;
        self.advert = None;
        self.browse = None;
    }
}

/// Registers the mDNS advert of the network bound at `ctrl`. A failure is logged and
/// otherwise ignored: discovery is a convenience, the daemon serves paired clients without
/// it.
fn advertise(
    name: &str,
    options: &ac2_discovery::Options,
    key: &PublicKey,
    ctrl: &str,
    bind: &ac2_discovery::Bind,
) -> Option<ac2_discovery::Advertiser> {
    let port = ctrl
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse::<u16>().ok())?;
    let advert = ac2_discovery::Advert {
        name: ac2_discovery::instance_name(name),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        proto: ac2_proto::PROTO_VERSION,
        fingerprint: key.fingerprint(),
        server_key: key.to_z85(),
    };
    let report = |e: ac2_discovery::AdvertEvent| match e {
        ac2_discovery::AdvertEvent::Error(e) => {
            tracing::warn!("mDNS responder: {e}; the rig may not be discoverable");
        }
        ac2_discovery::AdvertEvent::AddressAdded(a) => {
            tracing::info!("mDNS: advertising on {a}");
        }
        ac2_discovery::AdvertEvent::AddressRemoved(a) => {
            tracing::info!("mDNS: {a} went away");
        }
        ac2_discovery::AdvertEvent::Renamed { from, to } => {
            tracing::warn!("mDNS: {from:?} is taken on this network; advertising as {to:?}");
        }
    };
    match ac2_discovery::Advertiser::start(&advert, port, bind, options, report) {
        Ok(adv) => {
            tracing::info!(
                "mDNS: advertising {:?} ({}) on port {port}",
                advert.name,
                advert.fingerprint
            );
            Some(adv)
        }
        Err(e) => {
            tracing::warn!(
                "{e}: the rig is not advertised, `ac2 discover` will not list it (clients can \
                 still connect with --remote <address>)"
            );
            None
        }
    }
}

/// The endpoints a setting names, for reports while it is not bound.
pub(crate) fn intended(l: &NetListen) -> (String, String) {
    let host = match &l.bind {
        crate::network::NetBind::All => "*".to_owned(),
        NetBind::Interface { name } => name.clone(),
        NetBind::Addr {
            addr: std::net::IpAddr::V6(a),
        } => format!("[{a}]"),
        NetBind::Addr { addr } => addr.to_string(),
    };
    match l.port {
        NetPort::Port(p) => (
            format!("tcp://{host}:{p}"),
            format!("tcp://{host}:{}", p + 1),
        ),
        NetPort::Any => (format!("tcp://{host}:*"), format!("tcp://{host}:*")),
    }
}
