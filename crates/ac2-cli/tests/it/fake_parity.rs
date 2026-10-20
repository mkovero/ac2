//! The test-support fake daemon (`ac2_client::fake`) against the real one: the same
//! request sequence, sent raw over ctrl, must give the same error codes and the same rev
//! steps for request-id dedup, `expect_rev` and the stimulus lease. Timing is not
//! compared, only what a client can observe.
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use ac2_client::fake::{FakeDaemon, FakeOptions};
use ac2_proto::units::{Dbfs, LeaseToken, RequestId, Rev};
use ac2_proto::{Command, ErrorDetail, Reply, ReplyBody, Request, decode_reply, encode_request};
use ac2_zmq::{Context, Socket, SocketType};
use ac2d::{BackendChoice, Daemon, DaemonConfig, Listen};

const T: Duration = Duration::from_secs(5);
const LEASE: Duration = Duration::from_millis(400);

/// One raw ctrl connection (a client of its own: the lease is per connection).
struct Raw {
    sock: Socket,
    next: u64,
}

impl Raw {
    /// A connection whose own ids start after `base`.
    fn connect(ctx: &Context, ctrl: &str, base: u64) -> Self {
        let sock = ctx.socket(SocketType::Dealer).unwrap();
        sock.connect(ctrl).unwrap();
        Self { sock, next: base }
    }

    fn id(&mut self) -> RequestId {
        self.next += 1;
        RequestId(self.next)
    }

    /// Sends `req` and returns the reply bytes with its id.
    fn send(&self, req: &Request) -> Vec<u8> {
        self.sock.send(&[encode_request(req).unwrap()]).unwrap();
        loop {
            let m = self.sock.recv_timeout(T).unwrap().expect("a reply");
            let bytes = m.into_frames().remove(0);
            if decode_reply(&bytes).unwrap().id == req.id {
                return bytes;
            }
        }
    }

    fn call(&mut self, cmd: Command, expect_rev: Option<Rev>) -> Reply {
        let mut req = Request::new(self.id(), cmd);
        req.expect_rev = expect_rev;
        decode_reply(&self.send(&req)).unwrap()
    }

    fn rev(&mut self) -> Rev {
        match self.call(Command::StateSnapshot, None).result {
            Ok(ReplyBody::Snapshot(s)) => s.rev,
            other => panic!("state.snapshot: {other:?}"),
        }
    }
}

/// What a step showed: the reply kind or error code, and revs as steps from the start.
fn seen(r: &Reply, rev0: Rev) -> String {
    match &r.result {
        Ok(b) => ac2_client::body_name(b).to_owned(),
        Err(e) => {
            let detail = match &e.detail {
                Some(ErrorDetail::Conflict { rev }) => format!(" at +{}", rev.0 - rev0.0),
                Some(ErrorDetail::LeaseHeld { .. }) => " (held)".into(),
                Some(_) => " (detail)".into(),
                None => String::new(),
            };
            format!("{}{detail}", ac2_client::code_name(e.code))
        }
    }
}

fn token(r: &Reply) -> LeaseToken {
    match &r.result {
        Ok(ReplyBody::Lease(l)) => l.lease_token,
        other => panic!("gen.acquire: {other:?}"),
    }
}

fn ceiling(dbfs: f64) -> Command {
    // A lowering: allowed to any client, no lease, no confirmation.
    Command::GenCeiling {
        ceiling: Dbfs(dbfs),
        confirm_raise: false,
    }
}

/// The sequence, run against the daemon at `ctrl`.
fn script(ctrl: &str) -> Vec<(&'static str, String)> {
    let ctx = Context::new().unwrap();
    let (mut a, mut b) = (Raw::connect(&ctx, ctrl, 100), Raw::connect(&ctx, ctrl, 500));
    let mut out = Vec::new();
    let rev0 = a.rev();
    let step = |rev: Rev| format!("+{}", rev.0 - rev0.0);

    // Dedup: a resend of the same id gets the stored reply and executes nothing, even
    // when the resend carries another command.
    let req = Request::new(a.id(), ceiling(-20.0));
    let first = a.send(&req);
    out.push(("mutation", seen(&decode_reply(&first).unwrap(), rev0)));
    let after = a.rev();
    out.push(("rev after mutation", step(after)));
    out.push(("resend same bytes", (a.send(&req) == first).to_string()));
    let other = Request::new(req.id, ceiling(-30.0));
    out.push(("same id other cmd", (a.send(&other) == first).to_string()));
    out.push(("rev after resends", step(a.rev())));
    // Ids are per connection: b's request with a's id executes.
    let r = decode_reply(&b.send(&Request::new(req.id, ceiling(-21.0)))).unwrap();
    out.push(("same id other client", seen(&r, rev0)));

    // expect_rev: a stale rev refuses a mutation, a current one does not, a query ignores it.
    let r = a.call(ceiling(-22.0), Some(rev0));
    out.push(("stale expect_rev", seen(&r, rev0)));
    let now = a.rev();
    let r = a.call(ceiling(-23.0), Some(now));
    out.push(("current expect_rev", seen(&r, rev0)));
    let r = a.call(Command::SessionStatus, Some(rev0));
    out.push(("query, stale expect_rev", seen(&r, rev0)));

    // Lease: grant, refresh, held, force, the old holder's token, expiry.
    let bogus = LeaseToken(987_654);
    let r = a.call(Command::GenRefresh { lease_token: bogus }, None);
    out.push(("refresh unheld", seen(&r, rev0)));
    let r = a.call(Command::GenAcquire { force: false }, None);
    out.push(("acquire", seen(&r, rev0)));
    out.push(("rev after acquire", step(a.rev())));
    let ta = token(&r);
    if let Ok(ReplyBody::Lease(l)) = &r.result {
        out.push(("expires_in_ms", l.expires_in_ms.to_string()));
    }
    let r = a.call(Command::GenRefresh { lease_token: ta }, None);
    out.push(("refresh", seen(&r, rev0)));
    let r = a.call(Command::GenAcquire { force: false }, None);
    out.push(("re-acquire by holder", seen(&r, rev0)));
    let ta = match r.result {
        Ok(ReplyBody::Lease(l)) => l.lease_token,
        _ => ta,
    };
    let r = b.call(Command::GenAcquire { force: false }, None);
    out.push(("acquire held", seen(&r, rev0)));
    let r = b.call(Command::GenAcquire { force: true }, None);
    out.push(("acquire forced", seen(&r, rev0)));
    out.push(("rev after force", step(a.rev())));
    let tb = token(&r);
    let r = a.call(Command::GenRefresh { lease_token: ta }, None);
    out.push(("refresh taken over", seen(&r, rev0)));
    let r = a.call(Command::GenRelease { lease_token: ta }, None);
    out.push(("release taken over", seen(&r, rev0)));
    std::thread::sleep(LEASE * 3);
    let r = b.call(Command::GenRefresh { lease_token: tb }, None);
    out.push(("refresh expired", seen(&r, rev0)));
    out.push(("rev after expiry", step(a.rev())));
    let r = b.call(Command::GenRelease { lease_token: tb }, None);
    out.push(("release expired", seen(&r, rev0)));
    let r = a.call(Command::GenAcquire { force: false }, None);
    out.push(("acquire after expiry", seen(&r, rev0)));
    let r = a.call(
        Command::GenRelease {
            lease_token: token(&r),
        },
        None,
    );
    out.push(("release", seen(&r, rev0)));
    out.push(("rev at the end", step(a.rev())));
    out
}

#[test]
fn fake_daemon_matches_the_real_one_on_dedup_revs_and_lease() {
    let fake = FakeDaemon::start(FakeOptions {
        lease_expiry: LEASE,
        ..FakeOptions::default()
    })
    .unwrap();
    let faked = script(&fake.endpoints().ctrl);

    let backend = ac2d::backend(BackendChoice::Fake).unwrap();
    let listen = Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let mut cfg = DaemonConfig::new(backend, listen, -10.0);
    cfg.lease_expiry = LEASE;
    let h = Daemon::start(cfg).unwrap();
    let real = script(h.ctrl_endpoint());
    h.shutdown();

    let diff: Vec<String> = real
        .iter()
        .zip(&faked)
        .filter(|(r, f)| r != f)
        .map(|((step, r), (_, f))| format!("{step}: daemon {r}, fake {f}"))
        .collect();
    assert!(diff.is_empty(), "the fake differs:\n{}", diff.join("\n"));
    assert_eq!(real.len(), faked.len());
}
