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
