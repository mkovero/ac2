//! A driver slow to list or open its devices does not cost a stimulus lease: the holder's
//! refreshes are answered meanwhile, or the blocked time is not counted against it.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ac2_audio::{AudioError, Backend, BackendKind as AudioKind, DeviceCaps, DuplexRequest};
use ac2_audio::{DuplexStream, FakeBackend};
use ac2_proto::model::{BackendKind, DeviceId, PreviewInput};
use ac2_proto::units::LeaseToken;
use ac2_proto::{Command, Request};
use ac2d::Daemon;

use common::*;

const EXPIRY: Duration = Duration::from_millis(1000);
/// Longer than the lease: a loop blocked this long would let it lapse.
const SLOW: Duration = Duration::from_millis(1800);

/// The fake rig whose driver takes [`SLOW`] to list or open its devices once `slow` is set.
#[derive(Debug)]
struct SlowDriver {
    inner: FakeBackend,
    slow: Arc<AtomicBool>,
}

impl SlowDriver {
    fn stall(&self) {
        if self.slow.load(Ordering::SeqCst) {
            std::thread::sleep(SLOW);
        }
    }
}

impl Backend for SlowDriver {
    fn kind(&self) -> AudioKind {
        self.inner.kind()
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
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

fn start(name: &str) -> (ac2d::Handle, Client, Client, LeaseToken, Arc<AtomicBool>) {
    init_log();
    let slow = Arc::new(AtomicBool::new(false));
    let backend = SlowDriver {
        inner: manual_rig(),
        slow: Arc::clone(&slow),
    };
    let mut cfg = config(manual_rig(), inproc(name));
    cfg.backends = vec![Arc::new(backend)];
    cfg.lease_expiry = EXPIRY;
    let h = Daemon::start(cfg).unwrap();
    let mut a = Client::connect(h.context(), h.ctrl_endpoint());
    let b = Client::connect(h.context(), h.ctrl_endpoint());
    a.ok(Command::SessionOpen {
        config: session(false),
    });
    let tok = a
        .ok(Command::GenAcquire { force: false })
        .lease()
        .lease_token;
    slow.store(true, Ordering::SeqCst);
    (h, a, b, tok, slow)
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

    let devices = Request::new(b.next_id(), Command::SessionDevices);
    b.send(&devices);
    hold(&mut a, tok, SLOW + Duration::from_millis(300));
    let r = b.recv(Duration::from_secs(5)).expect("the devices reply");
    assert_eq!(r.id, devices.id);
    r.result.unwrap();

    let preview = Request::new(
        b.next_id(),
        Command::SessionPreview {
            backend: BackendKind::Fake,
            device: DeviceId(ac2_audio::fake::FAKE_DEVICE_ID.into()),
            inputs: Vec::<PreviewInput>::new(),
            pairs: Vec::new(),
        },
    );
    b.send(&preview);
    hold(&mut a, tok, SLOW + Duration::from_millis(300));
    let r = b.recv(Duration::from_secs(5)).expect("the preview reply");
    assert_eq!(r.id, preview.id);
    r.result.unwrap();
    drop(h);
}

#[test]
fn a_slow_session_open_does_not_count_against_the_lease() {
    let (h, mut a, mut b, tok, _slow) = start("slow-driver-open");
    std::thread::sleep(Duration::from_millis(300));
    a.ok(Command::GenRefresh { lease_token: tok });
    // The loop is blocked opening the device for longer than the lease runs.
    b.ok(Command::SessionOpen {
        config: session(false),
    });
    a.ok(Command::GenRefresh { lease_token: tok });
    drop(h);
}
