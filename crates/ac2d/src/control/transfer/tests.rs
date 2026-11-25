#![allow(clippy::unwrap_used)]
use super::*;

use ac2_proto::ErrorCode;

#[test]
fn file_part_takes_the_last_component() {
    assert_eq!(
        file_part("/run/media/foh/STICK/show.ac2session"),
        "show.ac2session"
    );
    assert_eq!(file_part("C:\\shows\\fri\\"), "fri");
    assert_eq!(file_part("show"), "show");
}

fn download(dir: &Path, n: u32) -> Kind {
    let mut spool = Spool::create(dir.join(format!("down-{n}.part"))).unwrap();
    spool.append(&[1, 2, 3]).unwrap();
    Kind::Download {
        spool,
        working: None,
    }
}

fn add(t: &mut Transfers, dir: &Path, client: &Peer) -> Result<TransferId, ProtoError> {
    let id = t.admit(client)?;
    t.insert(id, client, false, download(dir, id.0));
    Ok(id)
}

#[test]
fn transfers_are_bounded_per_peer_and_per_daemon_and_take_their_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut t = Transfers::default();
    let a = Peer::local(b"a");
    let ids: Vec<_> = (0..MAX_PEER_TRANSFERS)
        .map(|_| add(&mut t, dir.path(), &a).unwrap())
        .collect();
    let e = add(&mut t, dir.path(), &a).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);
    assert!(e.msg.contains(&MAX_PEER_TRANSFERS.to_string()), "{}", e.msg);
    for i in 0..MAX_TRANSFERS - MAX_PEER_TRANSFERS {
        add(&mut t, dir.path(), &Peer::local(&[b'c', i as u8])).unwrap();
    }
    let e = add(&mut t, dir.path(), &Peer::local(b"z")).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);
    assert!(e.msg.contains(&MAX_TRANSFERS.to_string()), "{}", e.msg);
    let b = Peer::local(b"b");
    assert_eq!(
        t.get(&b, ids[0]).map(|_| ()).map_err(|e| e.code),
        Err(ErrorCode::NotFound)
    );
    let file = dir.path().join(format!("down-{}.part", ids[0].0));
    assert!(file.exists());
    if let Some(old) = Instant::now().checked_sub(IDLE) {
        t.map.get_mut(&ids[0]).unwrap().last = old;
        t.prune(Instant::now());
        assert!(t.get(&a, ids[0]).is_err());
        assert!(!file.exists(), "an idle transfer's file goes with it");
        assert!(add(&mut t, dir.path(), &a).is_ok());
    }
}

/// Touched again and again, a transfer still ends at its lifetime.
#[test]
fn a_touched_transfer_ends_at_its_lifetime() {
    let dir = tempfile::tempdir().unwrap();
    let mut t = Transfers::default();
    let a = Peer::local(b"a");
    let id = add(&mut t, dir.path(), &a).unwrap();
    assert!(t.get(&a, id).is_ok(), "fresh, and touched now");
    if let Some(born) = Instant::now().checked_sub(TransferLimits::LIFETIME) {
        t.map.get_mut(&id).unwrap().born = born;
        assert!(t.get(&a, id).is_err(), "at its lifetime though touched");
    }
}
