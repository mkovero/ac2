//! systemd's service notification protocol (`sd_notify`): `READY=1` once a program serves,
//! `WATCHDOG=1` while it is healthy, `STOPPING=1` on the way out.
//!
//! A unit with `Type=notify` and `WatchdogSec=` restarts a program whose pings stop, so a
//! ping must come from the loop whose health it vouches for (the daemon's control loop,
//! the UI's frame), never from a side thread that would keep pinging over a hung loop.
//!
//! Linux only, and nothing at all unless systemd set `NOTIFY_SOCKET`: a desktop session,
//! macOS and Windows get a [`Notifier`] whose calls do nothing. `WATCHDOG_PID` is not
//! checked: each program is its unit's main process (a wrapper script `exec`s it), and
//! neither the daemon nor the UI starts other programs that could inherit the variables;
//! a wrapper that forks instead would need `NotifyAccess=all` and still be served.

use std::ffi::OsStr;
use std::io;
use std::time::{Duration, Instant};

/// The longest a program that pings from its frames may go without drawing while a
/// watchdog is set: frames that only happen on input or data would leave an idle, healthy
/// UI without pings.
pub const MAX_IDLE_FRAME: Duration = Duration::from_secs(1);

/// What a notification says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Startup is done; the program serves.
    Ready,
    /// The program is alive (resets the watchdog timer).
    Watchdog,
    /// An orderly shutdown has begun.
    Stopping,
}

impl State {
    /// The datagram's text.
    pub fn message(self) -> &'static str {
        match self {
            State::Ready => "READY=1",
            State::Watchdog => "WATCHDOG=1",
            State::Stopping => "STOPPING=1",
        }
    }
}

/// Sends the notifications, each ping at most once per [`Notifier::ping_every`].
#[derive(Debug, Default)]
pub struct Notifier {
    socket: Option<imp::Socket>,
    /// `WATCHDOG_USEC`; `None`: no watchdog, pings are not sent.
    watchdog: Option<Duration>,
    ready: bool,
    next_ping: Option<Instant>,
    /// A send failed; later failures are not reported again.
    failed: bool,
}

impl Notifier {
    /// One that never sends (tests, an embedded daemon, every OS but Linux).
    pub fn disabled() -> Self {
        Self::default()
    }

    /// From `NOTIFY_SOCKET` and `WATCHDOG_USEC`.
    pub fn from_env() -> Self {
        Self::from_vars(
            std::env::var_os("NOTIFY_SOCKET").as_deref(),
            std::env::var_os("WATCHDOG_USEC").as_deref(),
        )
    }

    /// From the two variables' values.
    pub fn from_vars(notify_socket: Option<&OsStr>, watchdog_usec: Option<&OsStr>) -> Self {
        let socket = notify_socket.and_then(imp::Socket::new);
        let watchdog = socket.as_ref().and(watchdog_usec).and_then(|v| {
            v.to_str()?
                .parse::<u64>()
                .ok()
                .filter(|&us| us > 0)
                .map(Duration::from_micros)
        });
        Self {
            socket,
            watchdog,
            ..Self::default()
        }
    }

    /// Whether systemd listens.
    pub fn enabled(&self) -> bool {
        self.socket.is_some()
    }

    /// The watchdog's timeout, when one is set.
    pub fn watchdog(&self) -> Option<Duration> {
        self.watchdog
    }

    /// How often pings go out: a quarter of the timeout, so that a ping that waits up to
    /// the same again for the next frame or tick still lands within half of it.
    pub fn ping_every(&self) -> Option<Duration> {
        self.watchdog.map(|w| w / 4)
    }

    /// How long the caller may go without a frame or tick: `None` without a watchdog.
    pub fn max_idle(&self) -> Option<Duration> {
        self.ping_every().map(|p| p.min(MAX_IDLE_FRAME))
    }

    /// `READY=1`, the first time only. Returns the error of the first failed send.
    pub fn ready(&mut self) -> Option<io::Error> {
        if self.ready || self.socket.is_none() {
            return None;
        }
        self.ready = true;
        self.send(State::Ready)
    }

    /// `WATCHDOG=1` if a watchdog is set and the last ping is [`Self::ping_every`] old.
    /// Returns the error of the first failed send.
    pub fn ping(&mut self, now: Instant) -> Option<io::Error> {
        let every = self.ping_every()?;
        if self.next_ping.is_some_and(|t| now < t) {
            return None;
        }
        self.next_ping = Some(now + every);
        self.send(State::Watchdog)
    }

    /// `STOPPING=1`. Returns the error of the first failed send.
    pub fn stopping(&mut self) -> Option<io::Error> {
        self.send(State::Stopping)
    }

    fn send(&mut self, state: State) -> Option<io::Error> {
        let socket = self.socket.as_ref()?;
        match socket.send(state.message().as_bytes()) {
            Ok(()) => None,
            Err(e) if !self.failed => {
                self.failed = true;
                Some(e)
            }
            Err(_) => None,
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::ffi::OsStr;
    use std::io;
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};

    #[derive(Debug)]
    pub(super) struct Socket {
        sock: UnixDatagram,
        addr: SocketAddr,
    }

    impl Socket {
        /// A path, or `@name` for the abstract namespace; `None` when unusable.
        pub(super) fn new(var: &OsStr) -> Option<Self> {
            let bytes = var.as_bytes();
            let addr = match bytes.split_first() {
                None => return None,
                Some((b'@', name)) => SocketAddr::from_abstract_name(name).ok()?,
                Some((b'/', _)) => SocketAddr::from_pathname(std::path::Path::new(var)).ok()?,
                // systemd also passes `vsock:` addresses (to a VM's host); not ours.
                Some(_) => return None,
            };
            let sock = UnixDatagram::unbound().ok()?;
            Some(Self { sock, addr })
        }

        pub(super) fn send(&self, msg: &[u8]) -> io::Result<()> {
            self.sock.send_to_addr(msg, &self.addr).map(|_| ())
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use std::ffi::OsStr;
    use std::io;

    #[derive(Debug)]
    pub(super) enum Socket {}

    impl Socket {
        pub(super) fn new(_: &OsStr) -> Option<Self> {
            None
        }

        pub(super) fn send(&self, _: &[u8]) -> io::Result<()> {
            match *self {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages() {
        assert_eq!(State::Ready.message(), "READY=1");
        assert_eq!(State::Watchdog.message(), "WATCHDOG=1");
        assert_eq!(State::Stopping.message(), "STOPPING=1");
    }

    #[test]
    fn without_notify_socket_nothing_is_sent() {
        let mut n = Notifier::from_vars(None, Some(OsStr::new("10000000")));
        assert!(!n.enabled());
        assert_eq!(n.watchdog(), None);
        assert_eq!(n.max_idle(), None);
        assert!(n.ready().is_none());
        assert!(n.ping(Instant::now()).is_none());
        assert!(n.stopping().is_none());
        let n = Notifier::from_vars(Some(OsStr::new("")), None);
        assert!(!n.enabled());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn notifications_reach_the_socket_and_pings_are_rate_limited() {
        use std::os::unix::net::UnixDatagram;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("notify");
        let rx = UnixDatagram::bind(&path).expect("bind");
        rx.set_nonblocking(true).expect("nonblocking");
        let recv = || {
            let mut buf = [0u8; 64];
            rx.recv(&mut buf)
                .ok()
                .map(|n| String::from_utf8_lossy(&buf[..n]).into_owned())
        };
        let mut n = Notifier::from_vars(Some(path.as_os_str()), Some(OsStr::new("8000000")));
        assert!(n.enabled());
        assert_eq!(n.watchdog(), Some(Duration::from_secs(8)));
        assert_eq!(n.ping_every(), Some(Duration::from_secs(2)));
        assert_eq!(n.max_idle(), Some(MAX_IDLE_FRAME));
        assert!(n.ready().is_none());
        assert!(n.ready().is_none());
        assert_eq!(recv().as_deref(), Some("READY=1"));
        assert_eq!(recv(), None, "READY only once");

        let t0 = Instant::now();
        assert!(n.ping(t0).is_none());
        assert!(n.ping(t0 + Duration::from_millis(16)).is_none());
        assert!(n.ping(t0 + Duration::from_millis(1999)).is_none());
        assert_eq!(recv().as_deref(), Some("WATCHDOG=1"));
        assert_eq!(recv(), None, "one ping per interval");
        assert!(n.ping(t0 + Duration::from_secs(2)).is_none());
        assert_eq!(recv().as_deref(), Some("WATCHDOG=1"));

        assert!(n.stopping().is_none());
        assert_eq!(recv().as_deref(), Some("STOPPING=1"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_short_watchdog_shortens_the_idle_frame_and_no_watchdog_no_pings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("notify");
        let rx = std::os::unix::net::UnixDatagram::bind(&path).expect("bind");
        rx.set_nonblocking(true).expect("nonblocking");
        let n = Notifier::from_vars(Some(path.as_os_str()), Some(OsStr::new("2000000")));
        assert_eq!(n.max_idle(), Some(Duration::from_millis(500)));
        let mut n = Notifier::from_vars(Some(path.as_os_str()), None);
        assert!(n.enabled());
        assert!(n.ping(Instant::now()).is_none());
        let mut buf = [0u8; 16];
        assert!(rx.recv(&mut buf).is_err(), "no watchdog, no ping");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_vanished_socket_reports_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("gone");
        let mut n = Notifier::from_vars(Some(path.as_os_str()), Some(OsStr::new("4000000")));
        assert!(n.ready().is_some());
        assert!(n.ping(Instant::now()).is_none());
    }
}
