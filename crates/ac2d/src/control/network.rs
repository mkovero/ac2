//! The network listener as the control thread runs it: binding a new pair on a new context
//! for every attach, handing it to the I/O thread in band, watching the bound address and
//! retrying on a backoff whatever went wrong, so the network never needs an operator and
//! never stops the daemon.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ac2_zmq::{AuthorizedHandle, AuthorizedKeys, KeyPair, PublicKey};

use crate::authlog::{AuthLog, RefusedList};
use crate::config::{Mdns, NetworkSecurity};
use crate::damage::Damage;
use crate::io::Attach;
use crate::network::{
    Backoff, Host, Interfaces, NetListen, NetPort, NetReason, NetStatus, NetworkChoice, bind_pair,
    resolve,
};
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
}

/// What [`NetCtl::new`] needs.
pub(crate) struct NetSetup {
    pub(crate) choice: NetworkChoice,
    pub(crate) files: Option<NetworkSecurity>,
    pub(crate) interfaces: Interfaces,
    pub(crate) retry: Backoff,
    pub(crate) mdns: Option<Mdns>,
    pub(crate) name: String,
    pub(crate) attach: Sender<Attach>,
    pub(crate) to_control: Sender<ControlMsg>,
}

/// The network's state on the control thread.
pub(crate) struct NetCtl {
    choice: NetworkChoice,
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
    attach: Sender<Attach>,
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
        let on = matches!(s.choice, NetworkChoice::Fixed(_));
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
        Ok((
            Self {
                choice: s.choice,
                keys,
                authorized,
                refused,
                view: NetView {
                    status: Arc::new(Mutex::new(NetStatus::Off)),
                    advertised_as: Arc::new(Mutex::new(None)),
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
    pub(crate) fn listen(&self) -> Option<&NetListen> {
        match &self.choice {
            NetworkChoice::Off => None,
            NetworkChoice::Fixed(l) => Some(l),
        }
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

    /// The retry or the check of the bound address, when due.
    pub(crate) fn tick(&mut self, outbox: &Outbox, now: Instant) {
        if self.due.is_none_or(|d| d > now) {
            return;
        }
        self.due = None;
        let Some(b) = &self.bound else {
            self.attempt(outbox, now);
            return;
        };
        let Some(l) = self.listen() else {
            return;
        };
        // A listener on an address that went away silently accepts nothing; one bound
        // where the interface no longer resolves serves the wrong network.
        match resolve(&l.bind, &self.interfaces.list()) {
            Ok(h) if h == b.host => self.due = Some(now + self.retry.first),
            r => {
                let reason = r.err().unwrap_or(NetReason::AddressMissing);
                tracing::warn!("network: the bound address went away ({reason}); rebinding");
                self.detach(outbox);
                self.delay = self.retry.first;
                self.attempt(outbox, now);
            }
        }
    }

    /// Binds a new pair and attaches it; on failure `Waiting`, retried.
    pub(crate) fn attempt(&mut self, outbox: &Outbox, now: Instant) {
        let Some(l) = self.listen().cloned() else {
            self.set_status(NetStatus::Off);
            return;
        };
        let (Some(kp), Some(a)) = (&self.keys, &self.authorized) else {
            self.wait(
                NetReason::Other {
                    detail: "no server key".into(),
                },
                now,
            );
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
                {
                    *lock(&self.view.fault) = Some(pair.secure.zap_fault());
                }
                if self.attach.send(Attach { epoch, pair }).is_err() {
                    return;
                }
                outbox.attach(epoch);
                tracing::info!("network: listening on ctrl {ctrl}, data {data}");
                crate::firewall::report(&ctrl, &data);
                self.bound = Some(Bound { epoch, host });
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

    /// Detaches the pair in band, after everything queued before.
    fn detach(&mut self, outbox: &Outbox) {
        if let Some(b) = self.bound.take() {
            outbox.detach(b.epoch);
        }
        self.stop_advert();
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
        crate::network::NetBind::Interface(i) => i.clone(),
        crate::network::NetBind::Addr(std::net::IpAddr::V6(a)) => format!("[{a}]"),
        crate::network::NetBind::Addr(a) => a.to_string(),
    };
    match l.port {
        NetPort::Port(p) => (
            format!("tcp://{host}:{p}"),
            format!("tcp://{host}:{}", p + 1),
        ),
        NetPort::Any => (format!("tcp://{host}:*"), format!("tcp://{host}:*")),
    }
}
