//! mDNS / DNS-SD for network-mode ac2 daemons (PLAN.md §6.4).
//!
//! A daemon in network mode advertises one `_ac2._tcp` service: its ctrl port, a human name,
//! the daemon version, protocol version, public CURVE server key and its fingerprint.
//! Clients browse for those adverts to list rigs on the local network.
//!
//! Discovery never establishes trust. Anyone on the network can send an advert with any
//! name and any fingerprint. A client connects to a discovered rig only with a server key it
//! pinned after the operator verified its fingerprint on the daemon host. CURVE refuses a
//! server that cannot prove it holds that key's secret half. A discovered public key may
//! prefill pairing, but must never replace a pinned key without operator verification.
//!
//! Layers: [`Advert`] / [`Rig`] / [`RigTable`] are plain data with the TXT encoding and the
//! browse-result bookkeeping (tested without a network); [`Advertiser`] and [`Browser`] are
//! thin wrappers over `mdns-sd`, whose responder runs on its own thread.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use mdns_sd::{DaemonEvent, DaemonStatus, IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};

mod query;

pub use query::{InterfaceQuery, query_interfaces};

/// DNS-SD service type of an ac2 daemon's ctrl socket (the data socket is ctrl + 1).
pub const SERVICE_TYPE: &str = "_ac2._tcp.local.";
/// The mDNS port (RFC 6762).
pub const MDNS_PORT: u16 = 5353;
/// Version of the TXT record layout below (`txtvers`, RFC 6763 §6.7).
pub const TXT_VERSION: &str = "2";
/// Longest instance name we advertise (a DNS label is at most 63 bytes).
pub const MAX_NAME_LEN: usize = 63;

/// What a daemon advertises in its TXT record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advert {
    /// Human name of the rig (the DNS-SD instance name).
    pub name: String,
    /// Daemon version (`CARGO_PKG_VERSION` of `ac2d`).
    pub version: String,
    /// Protocol version the daemon speaks (`ac2_proto::PROTO_VERSION`).
    pub proto: u16,
    /// Fingerprint of the daemon's CURVE server key (`PublicKey::fingerprint`).
    pub fingerprint: String,
    /// Z85-encoded public server key. Discovery supplies it, never grants trust.
    pub server_key: String,
}

/// A TXT record that is not an ac2 advert we understand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxtError {
    /// A required key is absent.
    Missing(&'static str),
    /// `txtvers` is not [`TXT_VERSION`].
    Version(String),
    /// A value does not parse.
    Invalid {
        /// The key.
        key: &'static str,
        /// The value received.
        value: String,
    },
}

impl fmt::Display for TxtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(k) => write!(f, "TXT record has no `{k}`"),
            Self::Version(v) => write!(
                f,
                "TXT layout version {v:?} (this build reads {TXT_VERSION})"
            ),
            Self::Invalid { key, value } => write!(f, "TXT `{key}` = {value:?} is not valid"),
        }
    }
}

impl std::error::Error for TxtError {}

/// Shape of a fingerprint: five groups of four lowercase hex digits.
fn is_fingerprint(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .all(|g| g.len() == 4 && g.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

impl Advert {
    /// TXT key/value pairs, in a fixed order.
    pub fn txt(&self) -> Vec<(&'static str, String)> {
        vec![
            ("txtvers", TXT_VERSION.to_owned()),
            ("name", self.name.clone()),
            ("v", self.version.clone()),
            ("proto", self.proto.to_string()),
            ("fp", self.fingerprint.clone()),
            ("key", self.server_key.clone()),
        ]
    }

    /// Reads an advert from TXT values looked up by key.
    pub fn from_txt(get: impl Fn(&str) -> Option<String>) -> Result<Self, TxtError> {
        let need = |k: &'static str| get(k).ok_or(TxtError::Missing(k));
        let txtvers = need("txtvers")?;
        if txtvers != TXT_VERSION {
            return Err(TxtError::Version(txtvers));
        }
        let proto = need("proto")?;
        let proto = proto.parse().map_err(|_| TxtError::Invalid {
            key: "proto",
            value: proto,
        })?;
        let fingerprint = need("fp")?;
        if !is_fingerprint(&fingerprint) {
            return Err(TxtError::Invalid {
                key: "fp",
                value: fingerprint,
            });
        }
        let server_key = need("key")?;
        const Z85: &str =
            "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?&<>()[]{}@%$#";
        if server_key.len() != 40 || !server_key.bytes().all(|b| Z85.as_bytes().contains(&b)) {
            return Err(TxtError::Invalid {
                key: "key",
                value: server_key,
            });
        }
        Ok(Self {
            name: need("name")?,
            version: need("v")?,
            proto,
            fingerprint,
            server_key,
        })
    }
}

/// A resolved advert: where the rig is and what it says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rig {
    /// DNS-SD full name (`<instance>._ac2._tcp.local.`), unique on the link.
    pub instance: String,
    /// The TXT contents.
    pub advert: Advert,
    /// mDNS host name without the trailing dot (`foh-rig.local`).
    pub host: String,
    /// Addresses, IPv4 first, then routable IPv6, then link-local IPv6.
    pub addresses: Vec<IpAddr>,
    /// Ctrl port; data is `port + 1`.
    pub port: u16,
}

fn addr_rank(a: &IpAddr) -> u8 {
    match a {
        IpAddr::V4(v) if v.is_loopback() => 1,
        IpAddr::V4(_) => 0,
        // Link-local IPv6 needs a scope id that a `tcp://` endpoint cannot carry portably.
        IpAddr::V6(v) if (v.segments()[0] & 0xffc0) == 0xfe80 => 3,
        IpAddr::V6(_) => 2,
    }
}

impl Rig {
    /// Builds a rig from the parts of a resolved service.
    pub fn from_parts(
        instance: &str,
        host: &str,
        port: u16,
        mut addresses: Vec<IpAddr>,
        get: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, TxtError> {
        let advert = Advert::from_txt(get)?;
        addresses.sort_by_key(|a| (addr_rank(a), *a));
        addresses.dedup();
        Ok(Self {
            instance: instance.to_owned(),
            advert,
            host: host.trim_end_matches('.').to_owned(),
            addresses,
            port,
        })
    }

    /// The host to connect to: the best address, or the mDNS host name when none resolved.
    /// IPv6 is returned without brackets, like `RemoteAddr::host`.
    pub fn connect_host(&self) -> String {
        self.addresses
            .first()
            .filter(|a| addr_rank(a) < 3)
            .map_or_else(|| self.host.clone(), IpAddr::to_string)
    }
}

/// One browse result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    /// A rig was resolved (or re-resolved with new data).
    Resolved(Rig),
    /// A rig went away (goodbye packet or expired record).
    Removed {
        /// Full name.
        instance: String,
    },
    /// An `_ac2._tcp` service whose TXT record we cannot read.
    Invalid {
        /// Full name.
        instance: String,
        /// Why.
        error: TxtError,
    },
}

/// The current set of rigs, keyed by instance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RigTable {
    rigs: BTreeMap<String, Rig>,
    invalid: BTreeMap<String, TxtError>,
}

impl RigTable {
    /// Applies an update; `true` when the visible set changed.
    pub fn apply(&mut self, u: Update) -> bool {
        match u {
            Update::Resolved(r) => {
                self.invalid.remove(&r.instance);
                let same = self.rigs.get(&r.instance) == Some(&r);
                self.rigs.insert(r.instance.clone(), r);
                !same
            }
            Update::Removed { instance } => {
                self.invalid.remove(&instance);
                self.rigs.remove(&instance).is_some()
            }
            Update::Invalid { instance, error } => {
                let had = self.rigs.remove(&instance).is_some();
                self.invalid.insert(instance, error);
                had
            }
        }
    }

    /// Rigs, ordered by name then instance.
    pub fn rigs(&self) -> Vec<&Rig> {
        let mut v: Vec<&Rig> = self.rigs.values().collect();
        v.sort_by(|a, b| (&a.advert.name, &a.instance).cmp(&(&b.advert.name, &b.instance)));
        v
    }

    /// Services that answered with a TXT record we could not read.
    pub fn invalid(&self) -> impl Iterator<Item = (&str, &TxtError)> {
        self.invalid.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Number of rigs.
    pub fn len(&self) -> usize {
        self.rigs.len()
    }

    /// No rigs.
    pub fn is_empty(&self) -> bool {
        self.rigs.is_empty()
    }
}

/// mDNS settings. Production uses the defaults; tests use a private port on loopback so they
/// neither depend on nor disturb the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// UDP port of the mDNS traffic.
    pub mdns_port: u16,
    /// Restrict to 127.0.0.1.
    pub loopback_only: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mdns_port: MDNS_PORT,
            loopback_only: false,
        }
    }
}

/// mDNS failure.
#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mDNS: {}", self.0)
    }
}

impl std::error::Error for Error {}

fn merr(e: mdns_sd::Error) -> Error {
    Error(e.to_string())
}

fn responder(opts: &Options) -> Result<ServiceDaemon, Error> {
    let d = ServiceDaemon::new_with_port(opts.mdns_port).map_err(merr)?;
    if opts.loopback_only {
        d.disable_interface(IfKind::All).map_err(merr)?;
        d.enable_interface(IfKind::LoopbackV4).map_err(merr)?;
    } else {
        // Loopback answers would list this machine's rigs twice (127.0.0.1 and the LAN
        // address) and are of no use to another host.
        d.disable_interface(vec![IfKind::LoopbackV4, IfKind::LoopbackV6])
            .map_err(merr)?;
    }
    Ok(d)
}

/// This machine's host name, reduced to a DNS label (`A–Z a–z 0–9 -`).
pub fn local_host_label() -> String {
    let raw = gethostname::gethostname().to_string_lossy().into_owned();
    let first = raw.split('.').next().unwrap_or_default();
    let label = instance_name(first).replace(' ', "-");
    if label.is_empty() {
        "ac2".into()
    } else {
        label
    }
}

/// Default rig name: `ac2 on <host>`.
pub fn default_rig_name() -> String {
    instance_name(&format!("ac2 on {}", local_host_label()))
}

/// An instance name as advertised: printable, no dots (they would read as label separators
/// to many browsers), at most [`MAX_NAME_LEN`] bytes on a character boundary.
pub fn instance_name(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars() {
        let c = if c.is_control() || c == '.' || c == '\\' {
            '-'
        } else {
            c
        };
        if out.len() + c.len_utf8() > MAX_NAME_LEN {
            break;
        }
        out.push(c);
    }
    out
}

/// Which addresses an advert names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bind {
    /// The daemon listens on every interface: advertise every current address (kept up to
    /// date as interfaces come and go).
    Any,
    /// The daemon listens on one address.
    Addr(IpAddr),
    /// The daemon listens on one interface by name (`tcp://eth0:47820`).
    Interface(String),
}

impl Bind {
    /// From the host part of a `tcp://host:port` endpoint.
    pub fn from_listen_host(host: &str) -> Self {
        let h = host.trim_start_matches('[').trim_end_matches(']');
        match h {
            "*" | "0.0.0.0" | "::" => Self::Any,
            _ => h
                .parse()
                .map_or_else(|_| Self::Interface(h.to_owned()), Self::Addr),
        }
    }
}

/// Something the responder reports while an advert runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdvertEvent {
    /// The responder failed at something (binding, joining the group, sending).
    Error(String),
    /// An address the advert names appeared on the host.
    AddressAdded(IpAddr),
    /// An address went away.
    AddressRemoved(IpAddr),
    /// Another host claimed the name; the responder renamed the advert.
    Renamed {
        /// The name asked for.
        from: String,
        /// The name now in use.
        to: String,
    },
}

/// A registered advert. Dropping it sends the goodbye and stops the responder.
pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl fmt::Debug for Advertiser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Advertiser")
            .field("fullname", &self.fullname)
            .finish_non_exhaustive()
    }
}

impl Advertiser {
    /// Registers `advert` for a ctrl socket on `port`. The responder runs on its own thread
    /// for as long as the returned value lives; `report` hears what it runs into (from that
    /// thread), so a responder that cannot bind or send never fails silently.
    ///
    /// Fails when the responder is not running once the advert is registered.
    pub fn start(
        advert: &Advert,
        port: u16,
        bind: &Bind,
        opts: &Options,
        mut report: impl FnMut(AdvertEvent) + Send + 'static,
    ) -> Result<Self, Error> {
        let daemon = responder(opts)?;
        let events = daemon.monitor().map_err(merr)?;
        std::thread::Builder::new()
            .name("ac2-mdns-monitor".into())
            .spawn(move || {
                // Ends when the responder shuts down and drops its side.
                while let Ok(e) = events.recv() {
                    let e = match e {
                        DaemonEvent::Error(e) => AdvertEvent::Error(e.to_string()),
                        DaemonEvent::IpAdd(a) => AdvertEvent::AddressAdded(a),
                        DaemonEvent::IpDel(a) => AdvertEvent::AddressRemoved(a),
                        DaemonEvent::NameChange(c) => AdvertEvent::Renamed {
                            from: c.original,
                            to: c.new_name,
                        },
                        _ => continue,
                    };
                    report(e);
                }
            })
            .map_err(|e| Error(format!("cannot start the monitor thread: {e}")))?;
        let host = format!("{}.local.", local_host_label());
        let name = instance_name(&advert.name);
        if name.is_empty() {
            return Err(Error("empty rig name".into()));
        }
        let txt = advert.txt();
        let info = match bind {
            Bind::Addr(ip) => ServiceInfo::new(SERVICE_TYPE, &name, &host, *ip, port, &txt[..]),
            Bind::Any | Bind::Interface(_) => {
                ServiceInfo::new(SERVICE_TYPE, &name, &host, "", port, &txt[..])
                    .map(ServiceInfo::enable_addr_auto)
            }
        }
        .map_err(merr)?;
        let info = if opts.loopback_only {
            let mut i = info;
            i.set_interfaces(vec![IfKind::LoopbackV4]);
            i
        } else if let Bind::Interface(n) = bind {
            let mut i = info;
            i.set_interfaces(vec![IfKind::Name(n.clone())]);
            i
        } else {
            info
        };
        let fullname = info.get_fullname().to_owned();
        daemon.register(info).map_err(merr)?;
        let status = daemon
            .status()
            .map_err(merr)?
            .recv_timeout(Duration::from_secs(2))
            .map_err(|e| Error(format!("the responder does not answer: {e}")))?;
        if !matches!(status, DaemonStatus::Running) {
            return Err(Error(format!("the responder is not running: {status:?}")));
        }
        Ok(Self { daemon, fullname })
    }

    /// The registered full name.
    pub fn fullname(&self) -> &str {
        &self.fullname
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        // Goodbye packets let browsers drop the rig at once instead of after the TTL.
        if let Ok(rx) = self.daemon.unregister(&self.fullname) {
            let _ = rx.recv_timeout(Duration::from_millis(500));
        }
        if let Ok(rx) = self.daemon.shutdown() {
            let _ = rx.recv_timeout(Duration::from_millis(500));
        }
    }
}

/// A running browse for `_ac2._tcp`. Dropping it stops the responder.
///
/// Besides the responder's own queries, the browser asks on every interface itself, with
/// the RFC 6762 §5.2 back-off (0, 1, 3 s): an interface the responder leaves out (one that
/// shares a subnet with another, one not flagged running) still gets the question, and the
/// answers come back to the responder like any other. Answers are merged per instance.
pub struct Browser {
    daemon: ServiceDaemon,
    rx: mdns_sd::Receiver<ServiceEvent>,
    stop: Option<mpsc::Sender<()>>,
    queried: mpsc::Receiver<Vec<InterfaceQuery>>,
}

impl fmt::Debug for Browser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Browser").finish_non_exhaustive()
    }
}

fn update_from(e: ServiceEvent) -> Option<Update> {
    match e {
        ServiceEvent::ServiceResolved(s) => {
            let addrs = s
                .get_addresses()
                .iter()
                .map(mdns_sd::ScopedIp::to_ip_addr)
                .collect();
            let props = s.get_properties();
            let get = |k: &str| props.get_property_val_str(k).map(str::to_owned);
            Some(
                match Rig::from_parts(s.get_fullname(), s.get_hostname(), s.get_port(), addrs, get)
                {
                    Ok(r) => Update::Resolved(r),
                    Err(error) => Update::Invalid {
                        instance: s.get_fullname().to_owned(),
                        error,
                    },
                },
            )
        }
        ServiceEvent::ServiceRemoved(_, instance) => Some(Update::Removed { instance }),
        _ => None,
    }
}

impl Browser {
    /// Starts browsing.
    pub fn start(opts: &Options) -> Result<Self, Error> {
        let daemon = responder(opts)?;
        let rx = daemon.browse(SERVICE_TYPE).map_err(merr)?;
        let (stop, stopped) = mpsc::channel::<()>();
        let (tell, queried) = mpsc::channel();
        let o = opts.clone();
        std::thread::Builder::new()
            .name("ac2-mdns-query".into())
            .spawn(move || {
                for wait in [
                    Duration::ZERO,
                    Duration::from_secs(1),
                    Duration::from_secs(2),
                ] {
                    match stopped.recv_timeout(wait) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        _ => return,
                    }
                    if tell.send(query_interfaces(&o)).is_err() {
                        return;
                    }
                }
            })
            .map_err(|e| Error(format!("cannot start the query thread: {e}")))?;
        Ok(Self {
            daemon,
            rx,
            stop: Some(stop),
            queried,
        })
    }

    /// The interfaces asked so far, each once (by name and address), with any failure.
    pub fn queried(&self, seen: &mut Vec<InterfaceQuery>) {
        for round in self.queried.try_iter() {
            for q in round {
                match seen
                    .iter_mut()
                    .find(|s| s.name == q.name && s.addr == q.addr)
                {
                    // A later success clears an earlier failure.
                    Some(s) if s.error.is_some() => *s = q,
                    Some(_) => {}
                    None => seen.push(q),
                }
            }
        }
    }

    /// Everything that arrived since the last call, without blocking.
    pub fn poll(&self) -> Vec<Update> {
        self.rx.try_iter().filter_map(update_from).collect()
    }

    /// Waits up to `timeout` for the next update.
    pub fn next(&self, timeout: Duration) -> Option<Update> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            match self.rx.recv_timeout(left) {
                Ok(e) => {
                    if let Some(u) = update_from(e) {
                        return Some(u);
                    }
                }
                Err(_) => return None,
            }
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        drop(self.stop.take());
        let _ = self.daemon.stop_browse(SERVICE_TYPE);
        if let Ok(rx) = self.daemon.shutdown() {
            let _ = rx.recv_timeout(Duration::from_millis(500));
        }
    }
}

/// What a browse found, and where it asked.
#[derive(Clone, Debug, Default)]
pub struct Discovery {
    /// The rigs that answered, one per instance.
    pub table: RigTable,
    /// The interfaces the browser asked on itself.
    pub queried: Vec<InterfaceQuery>,
}

/// Browses for `window` and returns what was found.
pub fn discover(window: Duration, opts: &Options) -> Result<Discovery, Error> {
    let b = Browser::start(opts)?;
    let mut found = Discovery::default();
    let deadline = Instant::now() + window;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match b.next(left) {
            Some(u) => {
                found.table.apply(u);
            }
            None => break,
        }
    }
    b.queried(&mut found.queried);
    Ok(found)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn advert() -> Advert {
        Advert {
            name: "FOH rig".into(),
            version: "1.0.0".into(),
            proto: 1,
            fingerprint: "1a2b-3c4d-5e6f-7a8b-9c0d".into(),
            server_key: "0".repeat(40),
        }
    }

    fn txt_map(a: &Advert) -> HashMap<String, String> {
        a.txt()
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect()
    }

    #[test]
    fn txt_roundtrip() {
        let m = txt_map(&advert());
        let back = Advert::from_txt(|k| m.get(k).cloned());
        assert_eq!(back, Ok(advert()));
        // Every pair fits a TXT string (key=value ≤ 255 bytes).
        assert!(
            advert()
                .txt()
                .iter()
                .all(|(k, v)| k.len() + 1 + v.len() <= 255)
        );
    }

    #[test]
    fn txt_rejects_what_it_cannot_read() {
        let base = txt_map(&advert());
        let with = |k: &str, v: Option<&str>| {
            let mut m = base.clone();
            match v {
                Some(v) => m.insert(k.to_owned(), v.to_owned()),
                None => m.remove(k),
            };
            Advert::from_txt(move |k| m.get(k).cloned())
        };
        assert_eq!(with("fp", None), Err(TxtError::Missing("fp")));
        assert_eq!(with("key", None), Err(TxtError::Missing("key")));
        for bad in [
            "".to_owned(),
            "0".repeat(39),
            "0".repeat(41),
            "_".repeat(40),
        ] {
            assert!(matches!(
                with("key", Some(&bad)),
                Err(TxtError::Invalid { key: "key", .. })
            ));
        }
        assert_eq!(
            with("txtvers", Some("99")),
            Err(TxtError::Version("99".into()))
        );
        assert!(matches!(
            with("proto", Some("one")),
            Err(TxtError::Invalid { key: "proto", .. })
        ));
        for bad in [
            "",
            "1a2b",
            "1A2B-3c4d-5e6f-7a8b-9c0d",
            "1a2b-3c4d-5e6f-7a8b-9c0dx",
        ] {
            assert!(
                matches!(
                    with("fp", Some(bad)),
                    Err(TxtError::Invalid { key: "fp", .. })
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn rig_prefers_ipv4_and_skips_link_local() {
        let m = txt_map(&advert());
        let get = |k: &str| m.get(k).cloned();
        let addrs: Vec<IpAddr> = ["fe80::1", "2001:db8::5", "10.0.0.7", "10.0.0.7"]
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect();
        let r = Rig::from_parts("FOH rig._ac2._tcp.local.", "foh.local.", 47820, addrs, get)
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.host, "foh.local");
        assert_eq!(r.addresses.len(), 3);
        assert_eq!(r.connect_host(), "10.0.0.7");
        let only_ll = Rig {
            addresses: vec!["fe80::1".parse().unwrap_or(IpAddr::from([0u8; 4]))],
            ..r.clone()
        };
        assert_eq!(only_ll.connect_host(), "foh.local");
        let v6 = Rig {
            addresses: vec!["2001:db8::5".parse().unwrap_or(IpAddr::from([0u8; 4]))],
            ..r
        };
        assert_eq!(v6.connect_host(), "2001:db8::5");
    }

    #[test]
    fn table_tracks_resolve_update_remove() {
        let m = txt_map(&advert());
        let rig = |port| {
            Rig::from_parts("a._ac2._tcp.local.", "a.local.", port, vec![], |k| {
                m.get(k).cloned()
            })
            .unwrap_or_else(|e| panic!("{e}"))
        };
        let mut t = RigTable::default();
        assert!(t.apply(Update::Resolved(rig(1))));
        assert!(!t.apply(Update::Resolved(rig(1))));
        assert!(t.apply(Update::Resolved(rig(2))));
        assert_eq!(t.len(), 1);
        assert!(t.apply(Update::Invalid {
            instance: "a._ac2._tcp.local.".into(),
            error: TxtError::Missing("fp"),
        }));
        assert!(t.is_empty());
        assert_eq!(t.invalid().count(), 1);
        assert!(t.apply(Update::Resolved(rig(2))));
        assert_eq!(t.invalid().count(), 0);
        assert!(t.apply(Update::Removed {
            instance: "a._ac2._tcp.local.".into()
        }));
        assert!(!t.apply(Update::Removed {
            instance: "a._ac2._tcp.local.".into()
        }));
    }

    #[test]
    fn names_and_binds() {
        assert_eq!(instance_name("  my.rig\\x\n"), "my-rig-x");
        assert!(instance_name(&"ä".repeat(100)).len() <= MAX_NAME_LEN);
        assert!(!local_host_label().is_empty());
        assert_eq!(Bind::from_listen_host("0.0.0.0"), Bind::Any);
        assert_eq!(Bind::from_listen_host("*"), Bind::Any);
        assert_eq!(Bind::from_listen_host("[::]"), Bind::Any);
        assert_eq!(
            Bind::from_listen_host("10.1.2.3"),
            Bind::Addr("10.1.2.3".parse().unwrap_or(IpAddr::from([0u8; 4])))
        );
        assert_eq!(
            Bind::from_listen_host("eth0"),
            Bind::Interface("eth0".into())
        );
    }
}
