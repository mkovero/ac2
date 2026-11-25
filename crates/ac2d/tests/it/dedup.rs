//! Request-id dedup across asynchronous commands: a copy of a request still running attaches
//! to it and gets its reply; nothing executes twice.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ac2_audio::{AudioError, Backend, BackendKind as AudioKind, DeviceCaps, DuplexRequest};
use ac2_audio::{DuplexStream, FakeBackend};
use ac2_proto::model::{BackendKind, DeviceId};
use ac2_proto::{Command, ReplyBody, Request};
use ac2d::{Daemon, DaemonConfig};

use common::*;

const T: Duration = Duration::from_secs(5);

/// The fake rig whose control panel stays open, like a modal driver dialog, until the test
/// closes it, and counts how often it was opened.
#[derive(Debug)]
struct HeldPanel {
    inner: FakeBackend,
    opened: Arc<AtomicUsize>,
    close: Mutex<Receiver<()>>,
}

impl Backend for HeldPanel {
    fn kind(&self) -> AudioKind {
        self.inner.kind()
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
        self.inner.enumerate()
    }

    fn probe(&self, device: &ac2_audio::DeviceSelector) -> ac2_audio::Presence {
        self.inner.probe(device)
    }

    fn open(&self, request: DuplexRequest) -> Result<DuplexStream, AudioError> {
        self.inner.open(request)
    }

    fn control_panel(&self, device: &ac2_audio::DeviceSelector) -> Result<(), AudioError> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        // A panel the test never closes still ends, so a failing run does not hang.
        let _ = self.close.lock().unwrap().recv_timeout(T);
        self.inner.control_panel(device)
    }
}

fn held_panel() -> (ac2d::Handle, Arc<AtomicUsize>, Sender<()>) {
    init_log();
    let opened = Arc::new(AtomicUsize::new(0));
    let (close, rx) = channel();
    let backend = HeldPanel {
        inner: manual_rig(),
        opened: Arc::clone(&opened),
        close: Mutex::new(rx),
    };
    let h = Daemon::start(DaemonConfig::new(
        Arc::new(backend),
        inproc("dedup-panel"),
        -10.0,
    ))
    .unwrap();
    (h, opened, close)
}

fn panel() -> Command {
    Command::StreamControlPanel {
        backend: BackendKind::Fake,
        device: DeviceId(ac2_audio::fake::FAKE_DEVICE_ID.into()),
    }
}

/// Every request sent before this one has been taken in: the control thread handles a
/// connection's requests in order.
fn barrier(c: &mut Client) {
    c.ok(Command::StreamStatus);
}

#[test]
fn a_retry_of_a_running_panel_request_attaches_to_it() {
    let (h, opened, close) = held_panel();
    let (mut c, _s) = connect(&h, &[]);
    // A command that commits nothing but acts (a driver's panel) is deduplicated all the
    // same.
    let req = Request::new(c.next_id(), panel());
    c.send(&req);
    c.send(&req);
    barrier(&mut c);
    // Both copies are in while the panel is still open.
    assert!(c.recv(Duration::from_millis(100)).is_none(), "no reply yet");

    close.send(()).unwrap();
    let first = c.recv_raw(T).unwrap();
    let second = c.recv_raw(T).unwrap();
    assert_eq!(first, second, "both copies get the one reply");
    let reply = ac2_proto::decode_reply(&first).unwrap();
    assert_eq!(reply.id, req.id);
    assert!(
        matches!(reply.result, Ok(ReplyBody::Ack { .. })),
        "{reply:?}"
    );
    assert_eq!(
        opened.load(Ordering::SeqCst),
        1,
        "one panel for both copies"
    );

    // After completion: the stored reply, the panel not opened again.
    c.send(&req);
    assert_eq!(c.recv_raw(T).unwrap(), first);
    assert_eq!(opened.load(Ordering::SeqCst), 1);

    // A new id opens it again.
    let again = Request::new(c.next_id(), panel());
    c.send(&again);
    barrier(&mut c);
    close.send(()).unwrap();
    assert_eq!(c.recv(T).unwrap().id, again.id);
    assert_eq!(opened.load(Ordering::SeqCst), 2);
    h.shutdown();
}

#[test]
fn ids_are_per_connection_for_local_clients() {
    let (h, opened, close) = held_panel();
    let (mut a, _sa) = connect(&h, &[]);
    let (mut b, _sb) = connect(&h, &[]);
    let req = Request::new(a.next_id(), panel());
    a.send(&req);
    barrier(&mut a);
    b.send(&req);
    barrier(&mut b);
    // The panel thread is busy with the first; the second connection's own request queues
    // behind it on the backend, not on the first's reply.
    close.send(()).unwrap();
    close.send(()).unwrap();
    assert_eq!(a.recv(T).unwrap().id, req.id);
    assert_eq!(b.recv(T).unwrap().id, req.id);
    assert_eq!(opened.load(Ordering::SeqCst), 2);
    h.shutdown();
}

/// A read is not held for the dedup window: its retry after the answer runs again and sees
/// the state as it is now, while a mutation's retry still gets its stored reply.
#[test]
fn a_retried_read_runs_again() {
    init_log();
    let h = Daemon::start(config(manual_rig(), inproc("dedup-read"))).unwrap();
    let (mut c, _s) = connect(&h, &[]);
    let rev = |r: ac2_proto::Reply| match r.result {
        Ok(ReplyBody::Snapshot(s)) => s.rev,
        other => panic!("{other:?}"),
    };
    let read = Request::new(c.next_id(), Command::StateSnapshot);
    let before = rev(c.call_req(read.clone()));

    let create = Request::new(
        c.next_id(),
        Command::MeasCreate {
            config: transfer("tf"),
        },
    );
    let created = c.call_req(create.clone());
    assert!(created.result.is_ok(), "{created:?}");

    let after = rev(c.call_req(read));
    assert!(
        after > before,
        "the retried read was answered from {before:?}"
    );

    c.send(&create);
    let again = c.recv(T).unwrap();
    assert_eq!(again, created, "a mutation's retry gets its stored reply");
    assert_eq!(c.state().measurements.len(), 1);
    h.shutdown();
}

/// A repeated `file.upload_chunk` id gets its stored reply: run again, the chunk would be
/// out of order (or appended twice).
#[test]
fn a_retried_upload_chunk_gets_its_stored_reply() {
    use ac2_proto::units::Blob;
    init_log();
    let h = Daemon::start(config(manual_rig(), inproc("dedup-chunk"))).unwrap();
    let (mut c, _s) = connect(&h, &[]);
    let bytes = b"not an archive, only bytes".to_vec();
    let t = c.transfer(Command::FileUploadBegin {
        file: "x.ac2session".into(),
        bytes: bytes.len() as u64,
        sha256: ac2_archive::sha256_hex(&bytes),
    });
    let chunk = Request::new(
        c.next_id(),
        Command::FileUploadChunk {
            transfer: t.transfer,
            offset: 0,
            data: Blob(bytes[..8].to_vec()),
        },
    );
    let first = c.call_req(chunk.clone());
    assert!(first.result.is_ok(), "{first:?}");
    c.send(&chunk);
    let again = c.recv(T).unwrap();
    assert_eq!(again, first, "the retry ran again");
    // The next chunk is expected right after the first, once.
    c.ok(Command::FileUploadChunk {
        transfer: t.transfer,
        offset: 8,
        data: Blob(bytes[8..].to_vec()),
    });
    h.shutdown();
}
