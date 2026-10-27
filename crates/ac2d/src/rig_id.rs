//! The daemon's identity ([`RigId`]): made once, kept in a file, loaded at every start in
//! every mode. An app holding several rigs keys each by it, so it must outlive restarts,
//! renames and address changes, and two daemons must never share one. A file that cannot be
//! read is set aside and a new id made: a reconnecting app then sees a new rig, never takes
//! this one for another.

use std::io;
use std::path::Path;

use ac2_proto::model::StateFile;
use ac2_proto::units::RigId;

use crate::config::RigIdSource;
use crate::damage::Damage;

/// The id `source` names, and the damaged file set aside on the way, if any.
pub(crate) fn load(source: &RigIdSource) -> (RigId, Option<Damage>) {
    match source {
        RigIdSource::Ephemeral => (random(), None),
        RigIdSource::File(path) => load_file(path),
    }
}

fn load_file(path: &Path) -> (RigId, Option<Damage>) {
    let reason = match std::fs::read(path) {
        Ok(bytes) => match parse(&bytes) {
            Some(id) => return (id, None),
            None => "not 16 hex digits of a rig id other than 0".to_owned(),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let id = random();
            save(path, id);
            tracing::info!("made rig id {id} in {}", path.display());
            return (id, None);
        }
        Err(e) => crate::util::io_reason(&e),
    };
    let id = random();
    // A file that is not set aside would be replaced by the new one: keep the old id
    // unwritten instead, and this start's id lasts until the daemon stops.
    let set_aside_as = match ac2_paths::set_aside_damaged(path) {
        Ok(to) => {
            save(path, id);
            Some(to)
        }
        Err(e) => {
            tracing::error!(
                "{}: cannot set the damaged rig id aside ({}); this start's id {id} is not kept",
                path.display(),
                crate::util::io_reason(&e)
            );
            None
        }
    };
    let damage = Damage {
        kind: StateFile::RigId,
        path: path.to_owned(),
        set_aside_as,
        reason: format!("{reason}; made a new rig id {id}, apps see this rig as a new one"),
    };
    (id, Some(damage))
}

/// The file's form: 16 lowercase hex digits and a newline.
fn text(id: RigId) -> String {
    format!("{id}\n")
}

fn parse(bytes: &[u8]) -> Option<RigId> {
    let digits = bytes.strip_suffix(b"\n")?;
    let ok = digits.len() == 16
        && digits
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b));
    let n = u64::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()?;
    (ok && n != 0).then_some(RigId(n))
}

fn save(path: &Path, id: RigId) {
    if let Err(e) = ac2_paths::write_private_atomic(path, text(id).as_bytes()) {
        tracing::error!(
            "{}: cannot keep rig id {id} ({}); the next start makes another",
            path.display(),
            crate::util::io_reason(&e)
        );
    }
}

/// Zero is never an id: it reads as "none" in every language a client is written in.
fn random() -> RigId {
    loop {
        let n = crate::util::random_u64();
        if n != 0 {
            return RigId(n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_made_and_then_reused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rig-id");
        let (id, damage) = load(&RigIdSource::File(path.clone()));
        assert_eq!(damage, None);
        assert_ne!(id.0, 0);
        let text = std::fs::read_to_string(&path).expect("written");
        assert_eq!(text, format!("{:016x}\n", id.0));
        assert_eq!(load(&RigIdSource::File(path)), (id, None));
    }

    #[test]
    fn a_damaged_file_is_set_aside_and_a_new_id_kept() {
        for bad in [
            &b""[..],
            b"0123456789abcdef",
            b"0123456789ABCDEF\n",
            b"0000000000000000\n",
            b"0123456789abcde\n",
            b"\xff\xfe garbage",
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("rig-id");
            std::fs::write(&path, bad).expect("seed");
            let (id, damage) = load(&RigIdSource::File(path.clone()));
            let damage = damage.expect("reported");
            assert_eq!(damage.kind, StateFile::RigId);
            let aside = damage.set_aside_as.expect("set aside");
            assert_eq!(std::fs::read(&aside).expect("kept"), bad);
            assert!(damage.reason.contains("new rig id"), "{}", damage.reason);
            assert_eq!(load(&RigIdSource::File(path)), (id, None), "{bad:?}");
        }
    }

    #[test]
    fn the_file_form_reads_back() {
        let id = RigId(0x0123_4567_89ab_cdef);
        assert_eq!(text(id), "0123456789abcdef\n");
        assert_eq!(parse(text(id).as_bytes()), Some(id));
    }

    #[test]
    fn an_ephemeral_id_is_new_every_start_and_never_written() {
        let (a, da) = load(&RigIdSource::Ephemeral);
        let (b, db) = load(&RigIdSource::Ephemeral);
        assert_ne!(a, b);
        assert_ne!(a.0, 0);
        assert_eq!((da, db), (None, None));
    }
}
