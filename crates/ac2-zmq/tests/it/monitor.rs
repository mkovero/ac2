//! Socket monitors never stall the context's I/O thread: libzmq reports events from it with a
//! blocking send, so a report to a reader that is gone would stop every socket of the context.

use crate::common;

use std::io::ErrorKind;
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use ac2_zmq::{Context, MonitorEvent, Socket, SocketType};
use common::*;

/// A TCP port whose accept queue is full: the kernel drops the SYNs of every further connect,
/// which stays in progress, as one to a host that is gone does (or, on some platforms, one
/// to a closed port, retried for seconds before it is refused).
struct Unanswering {
    endpoint: String,
    _listener: socket2::Socket,
    _queued: Vec<TcpStream>,
}

/// `None` (with the reason printed) where the OS refuses a connect to a full queue instead.
fn unanswering() -> R<Option<Unanswering>> {
    use socket2::{Domain, Socket, Type};
    let l = Socket::new(Domain::IPV4, Type::STREAM, None)?;
    l.bind(&SocketAddr::from(([127, 0, 0, 1], 0)).into())?;
    // Each OS sizes the queue from the backlog its own way: it is filled until a connect
    // goes unanswered.
    l.listen(1)?;
    let addr = l.local_addr()?.as_socket().ok_or("not an inet address")?;
    let mut queued = Vec::new();
    for _ in 0..1024 {
        match TcpStream::connect_timeout(&addr, Duration::from_millis(200)) {
            Ok(c) => queued.push(c),
            Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
                return Ok(Some(Unanswering {
                    endpoint: format!("tcp://{addr}"),
                    _listener: l,
                    _queued: queued,
                }));
            }
            Err(e) => {
                eprintln!("skipped: a connect to a full accept queue is refused here ({e})");
                return Ok(None);
            }
        }
    }
    Err("the accept queue never filled".into())
}

/// A round trip through `router`, which only the context's I/O thread can carry.
fn carried(router: &Socket, d: &Socket) -> TestResult {
    d.send(&[b"ping"])?;
    router
        .recv_timeout(Duration::from_secs(3))?
        .map(drop)
        .ok_or_else(|| "the context's I/O thread stalled".into())
}

/// The context's I/O thread still carries TCP traffic.
fn io_thread_answers(ctx: &Context) -> TestResult {
    let router = ctx.socket(SocketType::Router)?;
    router.bind("tcp://127.0.0.1:*")?;
    carried(&router, &dealer(ctx, &router.last_endpoint()?, None)?)
}

/// Runs `f` on a context of its own on a thread of its own: a stalled I/O thread also stalls
/// the context's teardown, which must not keep the test from reporting it.
fn on_own_context(f: impl FnOnce(&Context) -> TestResult + Send + 'static) -> TestResult {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let ctx = match Context::new() {
            Ok(c) => c,
            Err(e) => return drop(tx.send(Err(e.to_string()))),
        };
        let _ = tx.send(f(&ctx).map_err(|e| e.to_string()));
    });
    match rx.recv_timeout(TIMEOUT) {
        Ok(r) => r.map_err(Into::into),
        Err(mpsc::RecvTimeoutError::Timeout) => Err("the context's I/O thread stalled".into()),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err("the test's thread panicked".into()),
    }
}

/// A socket closed with a connect in progress, its monitor dropped `monitor_first` or right
/// after it: libzmq's I/O thread then reports the connect's close.
fn closed_mid_connect(monitor_first: bool) -> TestResult {
    let Some(gone) = unanswering()? else {
        return Ok(());
    };
    let ep = gone.endpoint.clone();
    on_own_context(move |ctx| {
        let s = ctx.socket(SocketType::Dealer)?;
        let mon = s.monitor()?;
        s.connect(&ep)?;
        mon.wait_for(TIMEOUT, |e| *e == MonitorEvent::ConnectDelayed)?
            .ok_or("no connect in progress")?;
        if let Some(e) = mon.wait_for(Duration::from_millis(200), |e| {
            matches!(
                e,
                MonitorEvent::Connected | MonitorEvent::ConnectRetried { .. }
            )
        })? {
            // Nothing to close mid-connect then: the test has no case here.
            eprintln!(
                "skipped: the connect did not stay in progress ({:?})",
                e.event
            );
            return Ok(());
        }
        if monitor_first {
            drop(mon);
            // libzmq's reaper closes the reader in the background: closed before the
            // socket, the report of the connect's close has nobody to go to.
            std::thread::sleep(Duration::from_millis(200));
            drop(s);
        } else {
            // The order a struct holding both drops them in.
            drop(s);
            drop(mon);
        }
        // The reaper hands the close to the I/O thread in the background too: the probe
        // goes after it, not before.
        std::thread::sleep(Duration::from_millis(200));
        io_thread_answers(ctx)
    })
}

#[test]
fn a_socket_closed_mid_connect_after_its_monitor_leaves_the_io_thread_free() -> TestResult {
    closed_mid_connect(true)
}

#[test]
fn a_socket_closed_mid_connect_before_its_monitor_leaves_the_io_thread_free() -> TestResult {
    closed_mid_connect(false)
}

#[test]
fn a_socket_outliving_its_monitor_leaves_the_io_thread_free() -> TestResult {
    on_own_context(|ctx| {
        let router = ctx.socket(SocketType::Router)?;
        router.bind("tcp://127.0.0.1:*")?;
        let s = ctx.socket(SocketType::Dealer)?;
        drop(s.monitor()?);
        // Every event of this connection goes to a monitor nobody reads now.
        s.connect(&router.last_endpoint()?)?;
        carried(&router, &s)?;
        io_thread_answers(ctx)
    })
}

#[test]
fn a_replaced_monitor_reports_nothing_more_and_the_new_one_does() -> TestResult {
    on_own_context(|ctx| {
        let router = ctx.socket(SocketType::Router)?;
        router.bind("tcp://127.0.0.1:*")?;
        let s = ctx.socket(SocketType::Dealer)?;
        let old = s.monitor()?;
        let new = s.monitor()?;
        s.connect(&router.last_endpoint()?)?;
        new.wait_for(TIMEOUT, |e| *e == MonitorEvent::Connected)?
            .ok_or("the new monitor saw no connection")?;
        if old
            .wait_for(Duration::from_millis(200), |e| {
                *e == MonitorEvent::Connected
            })?
            .is_some()
        {
            return Err("the replaced monitor still reports".into());
        }
        io_thread_answers(ctx)
    })
}
