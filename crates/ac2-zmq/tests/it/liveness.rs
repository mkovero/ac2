//! Connection liveness and queue options: the reconnect back-off is bounded, dead-peer
//! detection reaches TCP connections, and options read back.

use crate::common;

use std::net::TcpListener;
use std::time::Duration;

use ac2_zmq::{Context, Error, MonitorEvent, SocketType, TcpLiveness};
use common::*;

const LIVENESS: TcpLiveness = TcpLiveness {
    idle: Duration::from_secs(5),
    interval: Duration::from_secs(1),
    count: 3,
    max_unacked: Duration::from_secs(15),
};

#[test]
fn liveness_options_read_back() -> TestResult {
    let ctx = Context::new()?;
    let s = ctx.socket(SocketType::Dealer)?;
    assert_eq!(s.tcp_liveness()?, None);
    assert_eq!(s.reconnect_interval_max()?, None);
    s.set_tcp_liveness(Some(LIVENESS))?;
    assert_eq!(s.tcp_liveness()?, Some(LIVENESS));
    s.set_tcp_liveness(None)?;
    assert_eq!(s.tcp_liveness()?, None);
    s.set_reconnect_interval_max(Some(Duration::from_secs(3)))?;
    assert_eq!(s.reconnect_interval_max()?, Some(Duration::from_secs(3)));
    s.set_send_hwm(16)?;
    s.set_recv_hwm(8)?;
    assert_eq!((s.send_hwm()?, s.recv_hwm()?), (16, 8));

    for bad in [
        TcpLiveness {
            idle: Duration::from_millis(500),
            ..LIVENESS
        },
        TcpLiveness {
            interval: Duration::ZERO,
            ..LIVENESS
        },
        TcpLiveness {
            max_unacked: Duration::ZERO,
            ..LIVENESS
        },
    ] {
        assert!(matches!(
            s.set_tcp_liveness(Some(bad)),
            Err(Error::InvalidArgument(_))
        ));
    }
    Ok(())
}

#[test]
fn tcp_connections_with_liveness_work_both_ways() -> TestResult {
    // The kernel options are applied to the listener's accepted connections and to the
    // connecter's; a round trip shows libzmq accepted them on this platform.
    let ctx = Context::new()?;
    let router = ctx.socket(SocketType::Router)?;
    router.set_tcp_liveness(Some(LIVENESS))?;
    router.bind("tcp://127.0.0.1:*")?;
    let dealer = ctx.socket(SocketType::Dealer)?;
    dealer.set_tcp_liveness(Some(LIVENESS))?;
    dealer.connect(&router.last_endpoint()?)?;
    dealer.send(&[b"ping"])?;
    let m = router.recv_timeout(TIMEOUT)?.ok_or("no request")?;
    router.send(&[m.frames()[0].as_slice(), b"pong"])?;
    let r = dealer.recv_timeout(TIMEOUT)?.ok_or("no reply")?;
    assert_eq!(r.frames()[0], b"pong");
    Ok(())
}

#[test]
fn reconnect_backs_off_to_the_maximum() -> TestResult {
    // Nothing listens on this port: every attempt fails and the next waits longer.
    let l = TcpListener::bind("127.0.0.1:0")?;
    let ep = format!("tcp://{}", l.local_addr()?);
    drop(l);
    let ctx = Context::new()?;
    let s = ctx.socket(SocketType::Dealer)?;
    s.set_reconnect_interval(Duration::from_millis(20))?;
    s.set_reconnect_interval_max(Some(Duration::from_millis(160)))?;
    let mon = s.monitor()?;
    s.connect(&ep)?;
    let mut intervals = Vec::new();
    while intervals.len() < 6 {
        let Some(ev) = mon.wait_for(TIMEOUT, |e| {
            matches!(e, MonitorEvent::ConnectRetried { .. })
        })?
        else {
            return Err("no reconnect attempt".into());
        };
        if let MonitorEvent::ConnectRetried { interval } = ev.event {
            intervals.push(interval);
        }
    }
    assert!(
        intervals.windows(2).all(|w| w[1] >= w[0]),
        "back-off never shrinks: {intervals:?}"
    );
    assert!(
        intervals.iter().all(|i| *i <= Duration::from_millis(160)),
        "bounded by the maximum: {intervals:?}"
    );
    assert_eq!(intervals.last(), Some(&Duration::from_millis(160)));
    Ok(())
}

/// The liveness options as the kernel holds them for one TCP socket. libzmq accepting the
/// socket options proves nothing: its keepalive tuning is compiled in only when the build
/// defines the platform feature macros, and without them the connection is just as half-open
/// as before once the link drops.
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod kernel {
    use std::mem;
    use std::time::Duration;

    pub struct Ports {
        pub local: u16,
        pub peer: u16,
    }

    /// Local and peer TCP port of `fd`, or `None` if it is not a connected IPv4 TCP socket.
    pub fn tcp_ports(fd: libc::c_int) -> Option<Ports> {
        let port_of = |f: unsafe extern "C" fn(
            libc::c_int,
            *mut libc::sockaddr,
            *mut libc::socklen_t,
        ) -> libc::c_int| {
            let mut addr: libc::sockaddr_in = unsafe { mem::zeroed() };
            let mut len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
            let rc = unsafe { f(fd, (&raw mut addr).cast(), &mut len) };
            (rc == 0 && libc::c_int::from(addr.sin_family) == libc::AF_INET)
                .then(|| u16::from_be(addr.sin_port))
        };
        let mut sock_type: libc::c_int = 0;
        let mut len = mem::size_of::<libc::c_int>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&raw mut sock_type).cast(),
                &mut len,
            )
        };
        if rc != 0 || sock_type != libc::SOCK_STREAM {
            return None;
        }
        Some(Ports {
            local: port_of(libc::getsockname)?,
            peer: port_of(libc::getpeername)?,
        })
    }

    fn int_opt(fd: libc::c_int, level: libc::c_int, name: libc::c_int) -> libc::c_int {
        let mut v: libc::c_int = -1;
        let mut len = mem::size_of::<libc::c_int>() as libc::socklen_t;
        let rc = unsafe { libc::getsockopt(fd, level, name, (&raw mut v).cast(), &mut len) };
        assert_eq!(rc, 0, "getsockopt({level}, {name}) on fd {fd}");
        v
    }

    /// Every TCP socket of this process, by descriptor.
    pub fn tcp_fds() -> Vec<libc::c_int> {
        #[cfg(target_os = "linux")]
        let fds: Vec<libc::c_int> = std::fs::read_dir("/proc/self/fd")
            .expect("/proc/self/fd")
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
            .collect();
        #[cfg(not(target_os = "linux"))]
        let fds: Vec<libc::c_int> = (0..1024).collect();
        fds.into_iter()
            .filter(|&fd| tcp_ports(fd).is_some())
            .collect()
    }

    /// Asserts `fd` carries the probing schedule `l` the way the kernel will run it.
    pub fn assert_liveness(fd: libc::c_int, l: &ac2_zmq::TcpLiveness, what: &str) {
        let secs = |d: Duration| libc::c_int::try_from(d.as_secs()).expect("seconds fit a C int");
        assert_eq!(
            int_opt(fd, libc::SOL_SOCKET, libc::SO_KEEPALIVE),
            1,
            "{what}: SO_KEEPALIVE"
        );
        #[cfg(target_os = "linux")]
        let idle = libc::TCP_KEEPIDLE;
        #[cfg(target_os = "macos")]
        let idle = libc::TCP_KEEPALIVE;
        assert_eq!(
            int_opt(fd, libc::IPPROTO_TCP, idle),
            secs(l.idle),
            "{what}: keepalive idle"
        );
        assert_eq!(
            int_opt(fd, libc::IPPROTO_TCP, libc::TCP_KEEPINTVL),
            secs(l.interval),
            "{what}: TCP_KEEPINTVL"
        );
        assert_eq!(
            int_opt(fd, libc::IPPROTO_TCP, libc::TCP_KEEPCNT),
            libc::c_int::try_from(l.count).expect("count fits a C int"),
            "{what}: TCP_KEEPCNT"
        );
        #[cfg(target_os = "linux")]
        assert_eq!(
            int_opt(fd, libc::IPPROTO_TCP, libc::TCP_USER_TIMEOUT),
            libc::c_int::try_from(l.max_unacked.as_millis()).expect("milliseconds fit a C int"),
            "{what}: TCP_USER_TIMEOUT"
        );
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn liveness_reaches_the_kernel_on_both_ends() -> TestResult {
    // A SUB socket never sends, so an unacknowledged-send timeout can never fire on it; only
    // kernel keepalive probes notice that the far end went away without a FIN. The options
    // must therefore be on the descriptors themselves, not merely accepted by libzmq.
    let ctx = Context::new()?;
    let router = ctx.socket(SocketType::Router)?;
    router.set_tcp_liveness(Some(LIVENESS))?;
    router.bind("tcp://127.0.0.1:*")?;
    let endpoint = router.last_endpoint()?;
    let router_port: u16 = endpoint.rsplit(':').next().ok_or("port")?.parse()?;
    let dealer = ctx.socket(SocketType::Dealer)?;
    dealer.set_tcp_liveness(Some(LIVENESS))?;
    dealer.connect(&endpoint)?;
    dealer.send(&[b"ping"])?;
    let m = router.recv_timeout(TIMEOUT)?.ok_or("no request")?;
    router.send(&[m.frames()[0].as_slice(), b"pong"])?;
    dealer.recv_timeout(TIMEOUT)?.ok_or("no reply")?;

    // The descriptors belong to libzmq's I/O thread, but the table is per process.
    let mut connecter = None;
    let mut accepted = None;
    for fd in kernel::tcp_fds() {
        let ports = kernel::tcp_ports(fd).ok_or("ports")?;
        if ports.peer == router_port {
            connecter = Some(fd);
        } else if ports.local == router_port {
            accepted = Some(fd);
        }
    }
    let connecter = connecter.ok_or("no connected descriptor towards the router")?;
    let accepted = accepted.ok_or("no accepted descriptor on the router")?;
    kernel::assert_liveness(connecter, &LIVENESS, "connecter");
    kernel::assert_liveness(accepted, &LIVENESS, "accepted");
    Ok(())
}
