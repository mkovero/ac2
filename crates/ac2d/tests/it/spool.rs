//! Uploads spooled to disk: a client holds two transfers and the daemon eight; an upload
//! is refused when the disk cannot take it and keep its reserve, and dropped when the disk
//! fills under it; `file.upload_abort` ends one; a chunk sent again after its reply was lost
//! is acknowledged; a `trace.import` the client must answer with other options keeps its
//! upload. However an upload ends, its spool file goes with it, and a restart leaves
//! nothing in the staging directory.
//!
//! The disk's free space is the daemon's injected [`ac2d::FreeSpace`]: no test fills a disk.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ac2_proto::model::*;
use ac2_proto::units::{Blob, TransferId};
use ac2_proto::{
    Command, ErrorCode, ErrorDetail, ImportProblem, ProtoError, ReplyBody, TraceImportArgs,
};
use ac2d::{DaemonConfig, FreeSpace, Handle};
use common::*;

/// A daemon whose disk reports `free` bytes free, keeping 1000 bytes for uploads and 100
/// for packing.
fn start(
    name: &str,
    dir: &Path,
    free: &Arc<AtomicU64>,
    tweak: impl FnOnce(&mut DaemonConfig),
) -> Handle {
    init_log();
    let mut cfg = config(manual_rig(), inproc(&unique(name)));
    cfg.session_dir = Some(dir.join("sessions"));
    let f = Arc::clone(free);
    cfg.transfers.free_space = FreeSpace::with(move |_| Ok(f.load(Ordering::Relaxed)));
    cfg.transfers.reserve = 1000;
    cfg.transfers.pack_reserve = 100;
    tweak(&mut cfg);
    ac2d::Daemon::start(cfg).unwrap()
}

fn plenty() -> Arc<AtomicU64> {
    Arc::new(AtomicU64::new(1 << 40))
}

fn staging(dir: &Path) -> PathBuf {
    dir.join("sessions/.incoming")
}

/// The spool files in the staging directory.
fn spooled(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(staging(dir))
        .map(|rd| {
            rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn begin(c: &mut Client, data: &[u8]) -> Result<TransferId, ProtoError> {
    c.call(Command::FileUploadBegin {
        file: "/home/foh/upload.bin".into(),
        bytes: data.len() as u64,
        sha256: ac2_archive::sha256_hex(data),
    })
    .map(|r| match r {
        ReplyBody::Transfer(t) => t.transfer,
        other => panic!("{other:?}"),
    })
}

fn chunk(
    c: &mut Client,
    t: TransferId,
    offset: usize,
    data: &[u8],
) -> Result<ReplyBody, ProtoError> {
    c.call(Command::FileUploadChunk {
        transfer: t,
        offset: offset as u64,
        data: Blob(data.to_vec()),
    })
}

fn abort(c: &mut Client, t: TransferId) -> Result<ReplyBody, ProtoError> {
    c.call(Command::FileUploadAbort { transfer: t })
}

#[track_caller]
fn code(r: Result<impl std::fmt::Debug, ProtoError>, want: ErrorCode) -> ProtoError {
    let e = r.expect_err("refused");
    assert_eq!(e.code, want, "{e:?}");
    e
}

#[test]
fn an_upload_is_spooled_aborted_and_retried_chunk_by_chunk() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("spool-abort", dir.path(), &plenty(), |_| {});
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    let data = b"some bytes of an upload".to_vec();
    let t = begin(c, &data).unwrap();
    assert_eq!(spooled(dir.path()), [format!("up-{t}.part")]);
    chunk(c, t, 0, &data[..10]).unwrap();
    // Sent again after its reply was lost: the same bytes are acknowledged, others not.
    chunk(c, t, 0, &data[..10]).unwrap();
    code(chunk(c, t, 0, b"other byte"), ErrorCode::Invalid);
    assert_eq!(
        std::fs::read(staging(dir.path()).join(format!("up-{t}.part"))).unwrap(),
        &data[..10]
    );
    // Another client can neither use nor end it.
    let (mut other, _s2) = connect(&h, &[]);
    code(abort(&mut other, t), ErrorCode::NotFound);
    assert!(matches!(abort(c, t), Ok(ReplyBody::Ack { .. })));
    assert_eq!(spooled(dir.path()), Vec::<String>::new());
    code(chunk(c, t, 10, &data[10..]), ErrorCode::NotFound);
    code(abort(c, t), ErrorCode::NotFound);

    // A download is ended with file.download_end, not aborted.
    let d = c.transfer(Command::FileDownloadBegin { session: None });
    code(abort(c, d.transfer), ErrorCode::Invalid);
    c.ok(Command::FileDownloadEnd {
        transfer: d.transfer,
        saved_as: None,
    });

    // A damaged upload goes when its last chunk shows it.
    let t = c
        .transfer(Command::FileUploadBegin {
            file: "x".into(),
            bytes: data.len() as u64,
            sha256: "0".repeat(64),
        })
        .transfer;
    let e = code(chunk(c, t, 0, &data), ErrorCode::Invalid);
    assert!(e.msg.contains("checksum"), "{e:?}");
    assert_eq!(spooled(dir.path()), Vec::<String>::new());
    h.shutdown();
}

/// A chunk whose offset and length overflow is refused like any out-of-order chunk; the
/// upload goes on.
#[test]
fn a_chunk_offset_that_overflows_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("spool-overflow", dir.path(), &plenty(), |_| {});
    let (mut c, _sub) = connect(&h, &[]);
    let data = b"some bytes of an upload".to_vec();
    let t = begin(&mut c, &data).unwrap();
    chunk(&mut c, t, 0, &data[..10]).unwrap();
    for offset in [u64::MAX, u64::MAX - 5, u64::MAX - 12] {
        let r = c.call(Command::FileUploadChunk {
            transfer: t,
            offset,
            data: Blob(data[10..].to_vec()),
        });
        code(r, ErrorCode::Invalid);
    }
    chunk(&mut c, t, 10, &data[10..]).unwrap();
    h.shutdown();
}

/// An empty upload is complete at once: its sum is checked as it begins.
#[test]
fn an_empty_upload_with_a_wrong_sum_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("spool-empty", dir.path(), &plenty(), |_| {});
    let (mut c, _sub) = connect(&h, &[]);
    let e = code(
        c.call(Command::FileUploadBegin {
            file: "empty".into(),
            bytes: 0,
            sha256: "0".repeat(64),
        }),
        ErrorCode::Invalid,
    );
    assert!(e.msg.contains("checksum"), "{e:?}");
    assert_eq!(spooled(dir.path()), Vec::<String>::new());
    begin(&mut c, &[]).unwrap();
    h.shutdown();
}

#[test]
fn a_client_holds_two_transfers_and_the_daemon_eight() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("spool-quota", dir.path(), &plenty(), |_| {});
    let data = b"x".to_vec();
    let mut clients: Vec<_> = (0..5).map(|_| connect(&h, &[])).collect();
    let (c, _) = &mut clients[0];
    let first = begin(c, &data).unwrap();
    begin(c, &data).unwrap();
    let e = code(begin(c, &data), ErrorCode::Refused);
    assert!(e.msg.contains("holds 2 transfers"), "{e:?}");
    // Ending one makes room for another.
    abort(c, first).unwrap();
    begin(c, &data).unwrap();
    for (c, _) in &mut clients[1..4] {
        begin(c, &data).unwrap();
        begin(c, &data).unwrap();
    }
    let (c, _) = &mut clients[4];
    let e = code(begin(c, &data), ErrorCode::Refused);
    assert!(e.msg.contains("8 transfers"), "{e:?}");
    assert_eq!(spooled(dir.path()).len(), 8);
    h.shutdown();
}

#[test]
fn an_upload_the_disk_cannot_take_is_refused_or_dropped_and_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let free = Arc::new(AtomicU64::new(1500));
    let h = start("spool-full", dir.path(), &free, |_| {});
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    let data = vec![7u8; 400];
    // 400 bytes and the 1000 kept fit in 1500; another 400 do not beside the first.
    let t = begin(c, &data).unwrap();
    let e = code(begin(c, &data), ErrorCode::Refused);
    assert!(e.msg.contains("no room"), "{e:?}");
    free.store(1 << 30, Ordering::Relaxed);
    let e = code(
        c.call(Command::FileUploadBegin {
            file: "x".into(),
            bytes: 1 << 30,
            sha256: "0".repeat(64),
        }),
        ErrorCode::Refused,
    );
    assert!(e.msg.contains("no room"), "{e:?}");
    assert_eq!(spooled(dir.path()), [format!("up-{t}.part")]);

    // The disk fills meanwhile: the next chunk drops the upload, its file with it.
    chunk(c, t, 0, &data[..200]).unwrap();
    free.store(250, Ordering::Relaxed);
    let e = code(chunk(c, t, 200, &data[200..]), ErrorCode::Refused);
    assert!(e.msg.contains("dropped"), "{e:?}");
    assert_eq!(spooled(dir.path()), Vec::<String>::new());
    code(chunk(c, t, 200, &data[200..]), ErrorCode::NotFound);

    // A download is packed only with room for it and the reserve.
    free.store(150, Ordering::Relaxed);
    let e = code(
        c.call(Command::FileDownloadBegin { session: None }),
        ErrorCode::Refused,
    );
    assert!(e.msg.contains("no room"), "{e:?}");
    assert_eq!(spooled(dir.path()), Vec::<String>::new());
    h.shutdown();
}

#[test]
fn an_upload_ends_at_its_lifetime_with_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("spool-life", dir.path(), &plenty(), |c| {
        c.transfers.lifetime = Duration::from_millis(300);
    });
    let (mut c, _sub) = connect(&h, &[]);
    let data = b"abc".to_vec();
    let t = begin(&mut c, &data).unwrap();
    let began = Instant::now();
    loop {
        match chunk(&mut c, t, 0, &data[..1]) {
            Ok(_)
            | Err(ProtoError {
                code: ErrorCode::Invalid,
                ..
            }) => assert!(began.elapsed() < Duration::from_secs(10), "still held"),
            Err(e) => {
                assert_eq!(e.code, ErrorCode::NotFound, "{e:?}");
                break;
            }
        }
        std::thread::yield_now();
    }
    assert_eq!(spooled(dir.path()), Vec::<String>::new());
    h.shutdown();
}

/// An upload nobody touches again goes at its lifetime with its spool file, though no
/// client sends another transfer command.
#[test]
fn an_untouched_upload_goes_at_its_lifetime_without_another_command() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("spool-untouched", dir.path(), &plenty(), |c| {
        c.transfers.lifetime = Duration::from_millis(300);
    });
    let (mut c, _sub) = connect(&h, &[]);
    let t = begin(&mut c, b"abc").unwrap();
    assert_eq!(spooled(dir.path()), [format!("up-{t}.part")]);
    let began = Instant::now();
    while !spooled(dir.path()).is_empty() {
        assert!(began.elapsed() < Duration::from_secs(10), "still spooled");
        std::thread::sleep(Duration::from_millis(50));
    }
    h.shutdown();
}

#[test]
fn a_restart_leaves_nothing_in_the_staging_directory() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("spool-restart", dir.path(), &plenty(), |_| {});
    let (mut c, sub) = connect(&h, &[]);
    let data = b"half an upload".to_vec();
    let t = begin(&mut c, &data).unwrap();
    chunk(&mut c, t, 0, &data[..4]).unwrap();
    drop((c, sub));
    h.shutdown();
    // What a power cut would leave: a spool file, a half unpacked upload, a session half
    // deleted.
    let s = staging(dir.path());
    std::fs::write(s.join("up-9.part"), b"x").unwrap();
    std::fs::create_dir_all(s.join("upload-1-1/traces")).unwrap();
    std::fs::create_dir_all(s.join("deleted-1-2")).unwrap();
    let h = start("spool-restart-2", dir.path(), &plenty(), |_| {});
    let (mut c, _sub) = connect(&h, &[]);
    c.ok(Command::ServerInfo);
    assert_eq!(spooled(dir.path()), Vec::<String>::new());
    h.shutdown();
}

/// A power cut while `file.import` replaced a session, between parking the old one and
/// renaming the new one in: at the next start the old one is put back. One parked whose
/// name was taken again (the new one landed) goes with the staging.
#[test]
fn a_session_half_replaced_by_an_import_is_put_back_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("spool-replaced", dir.path(), &plenty(), |_| {});
    let (mut c, sub) = connect(&h, &[]);
    for name in ["show", "kept", "older"] {
        c.ok(Command::FileSave {
            session: SessionRef::Name { name: name.into() },
            overwrite: false,
        });
    }
    drop((c, sub));
    h.shutdown();
    let sessions = dir.path().join("sessions");
    let s = staging(dir.path());
    std::fs::create_dir_all(s.join("replaced-1-3")).unwrap();
    std::fs::rename(sessions.join("show"), s.join("replaced-1-3/show")).unwrap();
    std::fs::create_dir_all(s.join("replaced-1-4")).unwrap();
    std::fs::rename(sessions.join("older"), s.join("replaced-1-4/kept")).unwrap();
    let kept = std::fs::read(sessions.join("kept/session.json")).unwrap();

    let h = start("spool-replaced-2", dir.path(), &plenty(), |_| {});
    let (mut c, _sub) = connect(&h, &[]);
    let ReplyBody::Sessions(l) = c.ok(Command::FileList) else {
        panic!("a session list")
    };
    let mut names: Vec<_> = l.into_iter().map(|s| s.name).collect();
    names.sort();
    assert_eq!(names, ["kept", "show"]);
    assert_eq!(
        std::fs::read(sessions.join("kept/session.json")).unwrap(),
        kept
    );
    assert_eq!(spooled(dir.path()), Vec::<String>::new());
    h.shutdown();
}

fn two_channel_ir() -> Vec<u8> {
    let mut inter = vec![0.0f32; 2 * 256];
    inter[2 * 5] = 1.0;
    inter[2 * 20 + 1] = 0.5;
    ac2_traces::raw::wav::to_bytes(2, 48_000, &inter)
}

fn import_ir(c: &mut Client, t: TransferId, channel: Option<u16>) -> Result<ReplyBody, ProtoError> {
    c.call(Command::TraceImport(TraceImportArgs {
        transfer: t,
        format: ImportFormat::Auto,
        role: ImportRole::Trace,
        ir: Some(IrImport {
            channel,
            ..IrImport::default()
        }),
        owner: TraceOwner::Imported,
    }))
}

fn upload(c: &mut Client, data: &[u8]) -> TransferId {
    let t = begin(c, data).unwrap();
    for (i, part) in data.chunks(1 << 20).enumerate() {
        chunk(c, t, i << 20, part).unwrap();
    }
    t
}

/// A `pick_channel` or `bad_options` refusal keeps the upload: the client answers on the
/// same transfer, or ends it.
#[test]
fn a_trace_import_asking_for_options_keeps_its_upload() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("spool-import", dir.path(), &plenty(), |_| {});
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    let wav = two_channel_ir();
    let t = upload(c, &wav);
    let e = code(import_ir(c, t, None), ErrorCode::Invalid);
    assert!(
        matches!(
            e.detail,
            Some(ErrorDetail::Import {
                problem: ImportProblem::PickChannel,
                ..
            })
        ),
        "{e:?}"
    );
    code(import_ir(c, t, Some(7)), ErrorCode::Invalid);
    assert_eq!(spooled(dir.path()).len(), 1);
    let ReplyBody::Trace(m) = import_ir(c, t, Some(1)).unwrap() else {
        panic!("a trace")
    };
    assert!(!m.edit.name.is_empty());
    // Used up once imported.
    code(abort(c, t), ErrorCode::NotFound);
    assert_eq!(spooled(dir.path()), Vec::<String>::new());

    // Given up on: ended by the client.
    let t = upload(c, &wav);
    code(import_ir(c, t, None), ErrorCode::Invalid);
    abort(c, t).unwrap();
    assert_eq!(spooled(dir.path()), Vec::<String>::new());

    // Any other refusal uses it up.
    let t = upload(c, b"not a trace file at all");
    let e = code(
        c.call(Command::TraceImport(TraceImportArgs {
            transfer: t,
            format: ImportFormat::Auto,
            role: ImportRole::Trace,
            ir: None,
            owner: TraceOwner::Imported,
        })),
        ErrorCode::Invalid,
    );
    assert!(
        !matches!(
            e.detail,
            Some(ErrorDetail::Import {
                problem: ImportProblem::PickChannel | ImportProblem::BadOptions,
                ..
            })
        ),
        "{e:?}"
    );
    code(abort(c, t), ErrorCode::NotFound);
    assert_eq!(spooled(dir.path()), Vec::<String>::new());
    h.shutdown();
}
