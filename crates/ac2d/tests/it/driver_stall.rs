//! A driver slow to list or open its devices does not cost a stimulus lease: the holder's
//! refreshes are answered meanwhile, or the blocked time is not counted against it. A
//! request that needs a device a preview is still opening is refused within a client's
//! reply deadline, to be asked again.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ac2_audio::{AudioError, Backend, BackendKind as AudioKind, DeviceCaps, DuplexRequest};
use ac2_audio::{DuplexStream, FakeBackend};
use ac2_proto::model::{BackendKind, DeviceId, PreviewInput};
use ac2_proto::units::{Dbfs, LeaseToken};
use ac2_proto::{Command, ErrorCode, Request};
use ac2d::Daemon;

use common::*;

const EXPIRY: Duration = Duration::from_millis(1000);
/// Longer than the lease: a loop blocked this long would let it lapse.
const SLOW: Duration = Duration::from_millis(1800);

/// The fake rig whose driver takes [`SLOW`] to list or open its devices once `slow` is set.
#[derive(Debug)]
struct SlowDriver {
    inner: FakeBackend,
    knobs: Arc<Knobs>,
}

#[derive(Debug, Default)]
struct Knobs {
    slow: AtomicBool,
    /// Device listings so far.
    lists: AtomicU64,
}

impl SlowDriver {
    fn stall(&self) {
        if self.knobs.slow.load(Ordering::SeqCst) {
            std::thread::sleep(SLOW);
        }
    }
}

impl Backend for SlowDriver {
    fn kind(&self) -> AudioKind {
        self.inner.kind()
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
        self.knobs.lists.fetch_add(1, Ordering::SeqCst);
        self.stall();
        self.inner.enumerate()
    }

    fn probe(&self, device: &ac2_audio::DeviceSelector) -> ac2_audio::Presence {
        self.inner.probe(device)
    }

    fn open(&self, request: DuplexRequest) -> Result<DuplexStream, AudioError> {
        self.stall();
        self.inner.open(request)
    }

    fn control_panel(&self, device: &ac2_audio::DeviceSelector) -> Result<(), AudioError> {
        self.inner.control_panel(device)
    }
}

fn start(name: &str) -> (ac2d::Handle, Client, Client, LeaseToken, Arc<Knobs>) {
    init_log();
    let knobs = Arc::new(Knobs::default());
    let backend = SlowDriver {
        inner: manual_rig(),
        knobs: Arc::clone(&knobs),
    };
    let mut cfg = config(manual_rig(), inproc(name));
    cfg.backends = vec![Arc::new(backend)];
    cfg.lease_expiry = EXPIRY;
    let h = Daemon::start(cfg).unwrap();
    let mut a = Client::connect(h.context(), h.ctrl_endpoint());
    let b = Client::connect(h.context(), h.ctrl_endpoint());
    a.ok(Command::StreamOpen {
        config: stream(false),
    });
    let tok = a
        .ok(Command::GenAcquire { force: false })
        .lease()
        .lease_token;
    knobs.slow.store(true, Ordering::SeqCst);
    (h, a, b, tok, knobs)
}

/// A preview of the fake device.
fn preview(c: &mut Client) -> Request {
    Request::new(
        c.next_id(),
        Command::StreamPreview {
            backend: BackendKind::Fake,
            device: fake_dev(),
            inputs: Vec::<PreviewInput>::new(),
            pairs: Vec::new(),
        },
    )
}

fn fake_dev() -> DeviceId {
    DeviceId(ac2_audio::fake::FAKE_DEVICE_ID.into())
}

/// Within one try of a client's reply deadline.
const REPLY_DEADLINE: Duration = Duration::from_millis(1500);

/// Sends a preview from `b` and waits until the daemon is opening it on the slow driver.
fn preview_opening(b: &mut Client) -> Request {
    let p = preview(b);
    b.send(&p);
    std::thread::sleep(Duration::from_millis(200));
    p
}

/// The preview `p` answers once its device opened (listed, then opened, each [`SLOW`]):
/// refused, as it was closed meanwhile.
fn preview_refused(b: &Client, p: &Request) {
    let r = b.recv(Duration::from_secs(5)).expect("the preview reply");
    assert_eq!(r.id, p.id);
    assert_eq!(r.result.unwrap_err().code, ErrorCode::Refused);
}

/// Refreshes the lease every 300 ms over `span`, each answered promptly and accepted.
fn hold(a: &mut Client, tok: LeaseToken, span: Duration) {
    let end = Instant::now() + span;
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(300));
        let t = Instant::now();
        a.ok(Command::GenRefresh { lease_token: tok });
        assert!(
            t.elapsed() < Duration::from_millis(500),
            "a refresh waited {:?} behind the driver",
            t.elapsed()
        );
    }
}

#[test]
fn listing_or_previewing_on_a_slow_driver_answers_lease_refreshes() {
    let (h, mut a, mut b, tok, _slow) = start("slow-driver-list");

    let devices = Request::new(b.next_id(), Command::StreamDevices);
    b.send(&devices);
    hold(&mut a, tok, SLOW + Duration::from_millis(300));
    let r = b.recv(Duration::from_secs(5)).expect("the devices reply");
    assert_eq!(r.id, devices.id);
    r.result.unwrap();

    let preview = preview(&mut b);
    b.send(&preview);
    hold(&mut a, tok, SLOW + Duration::from_millis(300));
    let r = b.recv(Duration::from_secs(5)).expect("the preview reply");
    assert_eq!(r.id, preview.id);
    r.result.unwrap();
    drop(h);
}

#[test]
fn a_slow_stream_open_does_not_count_against_the_lease() {
    let (h, mut a, mut b, tok, _slow) = start("slow-driver-open");
    std::thread::sleep(Duration::from_millis(300));
    a.ok(Command::GenRefresh { lease_token: tok });
    // The loop is blocked opening the device for longer than the lease runs.
    b.ok(Command::StreamOpen {
        config: stream(false),
    });
    a.ok(Command::GenRefresh { lease_token: tok });
    drop(h);
}

#[test]
fn a_detection_while_a_preview_opens_is_refused_in_time_and_keeps_the_lease() {
    let (h, mut a, mut b, tok, _slow) = start("slow-driver-detect");
    let p = preview_opening(&mut b);
    a.ok(Command::GenRefresh { lease_token: tok });
    let t = Instant::now();
    let e = a
        .call(Command::StreamDetectLoopback {
            lease_token: tok,
            backend: BackendKind::Fake,
            input_device: fake_dev(),
            output_device: fake_dev(),
            output: 0,
            level: Dbfs(-30.0),
        })
        .unwrap_err();
    assert!(
        t.elapsed() < REPLY_DEADLINE,
        "answered after {:?}",
        t.elapsed()
    );
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    // The wait for the preview does not count against the lease.
    a.ok(Command::GenRefresh { lease_token: tok });
    hold(&mut a, tok, 2 * SLOW);
    preview_refused(&b, &p);
    drop(h);
}

#[test]
fn an_open_while_a_preview_opens_is_refused_in_time_and_keeps_the_lease() {
    let (h, mut a, mut b, tok, _slow) = start("slow-driver-open-preview");
    let p = preview_opening(&mut b);
    a.ok(Command::GenRefresh { lease_token: tok });
    let t = Instant::now();
    let e = a
        .call(Command::StreamOpen {
            config: stream(false),
        })
        .unwrap_err();
    assert!(
        t.elapsed() < REPLY_DEADLINE,
        "answered after {:?}",
        t.elapsed()
    );
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    a.ok(Command::GenRefresh { lease_token: tok });
    hold(&mut a, tok, 2 * SLOW);
    preview_refused(&b, &p);
    // Asked again once the preview has let the device go, the open goes through.
    a.ok(Command::StreamOpen {
        config: stream(false),
    });
    a.ok(Command::GenRefresh { lease_token: tok });
    drop(h);
}

#[test]
fn listings_asked_while_one_runs_wait_for_it() {
    let (h, _a, mut b, _tok, knobs) = start("slow-driver-lists");
    let before = knobs.lists.load(Ordering::SeqCst);
    let asks: Vec<Request> = (0..3)
        .map(|_| Request::new(b.next_id(), Command::StreamDevices))
        .collect();
    for r in &asks {
        b.send(r);
    }
    let mut answered = Vec::new();
    for _ in &asks {
        let r = b.recv(Duration::from_secs(5)).expect("a devices reply");
        r.result.unwrap();
        answered.push(r.id);
    }
    answered.sort();
    assert_eq!(answered, asks.iter().map(|r| r.id).collect::<Vec<_>>());
    assert_eq!(
        knobs.lists.load(Ordering::SeqCst) - before,
        1,
        "one listing answers every request made while it ran"
    );
    drop(h);
}
