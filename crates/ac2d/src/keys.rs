//! Network-mode key files: the server's CURVE key pair and the authorized-clients list.
//!
//! Both are written atomically (temporary file in the same directory, synced, renamed over
//! the target) and, on Unix, created with mode 0600 so the secret key is never readable by
//! other users even for an instant. A damaged file (torn by a power cut, or garbage) never
//! stops the daemon: a key pair that does not parse is set aside and a new pair generated
//! (clients must pin the new key), and authorized-clients lines that do not parse are
//! dropped, the original file set aside first.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::damage::Damage;

use ac2_proto::model::StateFile;
use ac2_zmq::{AuthorizedKeys, KeyPair, KeyStoreError, PublicKey, SecretKey};

/// A key file problem.
#[derive(Debug)]
pub enum KeyFileError {
    /// Reading or writing failed.
    Io {
        /// File.
        path: PathBuf,
        /// Cause.
        source: io::Error,
    },
    /// The file exists but is not a valid key file.
    Parse {
        /// File.
        path: PathBuf,
        /// What is wrong.
        reason: String,
    },
    /// Key generation failed.
    Generate(String),
    /// The authorized-clients list is malformed.
    Authorized(KeyStoreError),
}

impl fmt::Display for KeyFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Parse { path, reason } => write!(f, "{}: {reason}", path.display()),
            Self::Generate(e) => write!(f, "key generation failed: {e}"),
            Self::Authorized(e) => write!(f, "authorized clients: {e}"),
        }
    }
}

impl std::error::Error for KeyFileError {}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> KeyFileError + '_ {
    move |source| KeyFileError::Io {
        path: path.to_owned(),
        source,
    }
}

const KEY_HEADER: &str = "# ac2d server CURVE key pair; keep this file private";

fn key_text(kp: &KeyPair) -> String {
    format!(
        "{KEY_HEADER}\npublic {}\nsecret {}\n",
        kp.public.to_z85(),
        kp.secret.to_z85()
    )
}

fn parse_key_file(path: &Path, text: &str) -> Result<KeyPair, KeyFileError> {
    let bad = |reason: &str| KeyFileError::Parse {
        path: path.to_owned(),
        reason: reason.to_owned(),
    };
    let mut public = None;
    let mut secret = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.split_once(' ') {
            Some(("public", k)) if public.is_none() => {
                public = Some(PublicKey::from_z85(k.trim()).map_err(|_| bad("bad public key"))?);
            }
            Some(("secret", k)) if secret.is_none() => {
                secret = Some(SecretKey::from_z85(k.trim()).map_err(|_| bad("bad secret key"))?);
            }
            _ => return Err(bad("expected `public <z85>` and `secret <z85>` lines")),
        }
    }
    match (public, secret) {
        (Some(public), Some(secret)) => Ok(KeyPair { public, secret }),
        _ => Err(bad("missing public or secret key")),
    }
}

/// Loads the server key pair, generating and saving one if the file does not exist or is
/// damaged (the damaged file set aside and reported).
pub(crate) fn load_or_create_server_keys(
    path: &Path,
) -> Result<(KeyPair, Option<Damage>), KeyFileError> {
    let mut damage = None;
    match fs::read(path) {
        Ok(bytes) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(m) = fs::metadata(path)
                    && m.permissions().mode() & 0o077 != 0
                {
                    tracing::warn!(
                        "{}: server secret key is readable by other users (chmod 600)",
                        path.display()
                    );
                }
            }
            let parsed = match String::from_utf8(bytes) {
                Ok(text) => parse_key_file(path, &text),
                Err(_) => Err(KeyFileError::Parse {
                    path: path.to_owned(),
                    reason: "not text".to_owned(),
                }),
            };
            match parsed {
                Ok(kp) => return Ok((kp, None)),
                Err(KeyFileError::Parse { reason, .. }) => {
                    let to = ac2_paths::set_aside_damaged(path).map_err(io_err(path))?;
                    damage = Some(Damage {
                        kind: StateFile::ServerKey,
                        path: path.to_owned(),
                        set_aside_as: Some(to),
                        reason: format!(
                            "{reason}; generated a new key pair, clients must pin the new key"
                        ),
                    });
                }
                Err(e) => return Err(e),
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(path)(e)),
    }
    let kp = KeyPair::generate().map_err(|e| KeyFileError::Generate(e.to_string()))?;
    ac2_paths::write_private_atomic(path, key_text(&kp).as_bytes()).map_err(io_err(path))?;
    tracing::info!(
        "generated server key pair {} (public {})",
        path.display(),
        kp.public.to_z85()
    );
    if let Some(d) = &damage {
        d.log();
    }
    Ok((kp, damage))
}

/// Parses the authorized-clients list line by line, keeping every line that parses: a torn
/// last line must not lock out every client. The reasons of the dropped lines come back.
fn parse_authorized_lenient(text: &str) -> (AuthorizedKeys, Vec<String>) {
    let mut keys = AuthorizedKeys::new();
    let mut dropped = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let one = AuthorizedKeys::parse(line).and_then(|one| {
            for (name, key) in one.iter() {
                // Each line keeps its role: an admission read back as anything else
                // leaves the pairing refused, or a viewer in control, after a restart.
                if let Some(role) = one.role_of(name) {
                    keys.insert(name, *key, role)?;
                }
            }
            Ok(())
        });
        if let Err(e) = one {
            dropped.push(format!("line {}: {e}", i + 1));
        }
    }
    (keys, dropped)
}

/// Why the authorized-clients file at `path` cannot be trusted to say who controls the
/// rig: whoever can write it can make any key an operator. `None` when it is the daemon
/// user's own regular file that no one else may write, or when it does not exist.
fn not_trusted(path: &Path) -> io::Result<Option<&'static str>> {
    let m = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if !m.file_type().is_file() {
        return Ok(Some("not a regular file (a link, a directory or a device)"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if m.uid() != ac2_paths::socket_dir::effective_uid() {
            return Ok(Some("owned by another user"));
        }
        if m.mode() & 0o022 != 0 {
            return Ok(Some("writable by its group or by other users"));
        }
    }
    Ok(None)
}

/// Loads the authorized-clients list; a missing file is created empty (no client can
/// connect until one is added). Lines that do not parse are dropped and the file rewritten
/// without them, the original set aside and reported. A list whose key lines name no role is
/// set aside whole and an empty one started: guessing a role for its keys would either lock
/// out every operator or give every viewer control, so the person who adds them decides.
pub(crate) fn load_or_create_authorized(
    path: &Path,
) -> Result<(AuthorizedKeys, Option<Damage>), KeyFileError> {
    if let Some(why) = not_trusted(path).map_err(io_err(path))? {
        // Left as it is: the daemon has no business rewriting a file another user can
        // write, or following a link to wherever it points.
        let d = Damage {
            kind: StateFile::AuthorizedClients,
            path: path.to_owned(),
            set_aside_as: None,
            reason: format!(
                "{why}: no network client is admitted until it is a regular file of the \
                 daemon's user that only that user may write (chmod 600)"
            ),
        };
        d.log();
        return Ok((AuthorizedKeys::new(), Some(d)));
    }
    match fs::read(path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            if ac2_zmq::is_roleless(&text) {
                let to = ac2_paths::set_aside_damaged(path).map_err(io_err(path))?;
                let keys = AuthorizedKeys::new();
                ac2_paths::write_private_atomic(path, keys.to_text().as_bytes())
                    .map_err(io_err(path))?;
                let d = Damage {
                    kind: StateFile::AuthorizedClients,
                    path: path.to_owned(),
                    reason: format!(
                        "its lines name a client and a key but no role, and every line needs \
                         one: `<name> <key> operator`, `<name> <key> viewer` or \
                         `<name> <key> rig=<rig id>`; give each line of {} its role and put it \
                         back with the daemon stopped (or add the clients again with \
                         `ac2 auth`); no network client can connect until then",
                        to.display()
                    ),
                    set_aside_as: Some(to),
                };
                d.log();
                return Ok((keys, Some(d)));
            }
            let (keys, dropped) = parse_authorized_lenient(&text);
            let not_utf8 = matches!(text, std::borrow::Cow::Owned(_));
            if dropped.is_empty() && !not_utf8 {
                return Ok((keys, None));
            }
            let to = ac2_paths::set_aside_damaged(path).map_err(io_err(path))?;
            ac2_paths::write_private_atomic(path, keys.to_text().as_bytes())
                .map_err(io_err(path))?;
            let mut reason = if dropped.is_empty() {
                "not valid UTF-8".to_owned()
            } else {
                format!("dropped {} line(s): {}", dropped.len(), dropped.join("; "))
            };
            reason.push_str(&format!("; kept {} client(s)", keys.len()));
            let d = Damage {
                kind: StateFile::AuthorizedClients,
                path: path.to_owned(),
                set_aside_as: Some(to),
                reason,
            };
            d.log();
            Ok((keys, Some(d)))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let keys = AuthorizedKeys::new();
            ac2_paths::write_private_atomic(path, keys.to_text().as_bytes())
                .map_err(io_err(path))?;
            tracing::warn!(
                "{}: created an empty authorized-clients list; no client can connect until one is added",
                path.display()
            );
            Ok((keys, None))
        }
        Err(e) => Err(io_err(path)(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_zmq::Role;

    #[test]
    fn server_keys_roundtrip_and_are_private() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("keys").join("server.key");
        let (a, none) = load_or_create_server_keys(&p).expect("create");
        assert!(none.is_none());
        let (b, none) = load_or_create_server_keys(&p).expect("load");
        assert!(none.is_none());
        assert_eq!(a.public, b.public);
        assert_eq!(a.secret, b.secret);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&p).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // A damaged file (garbage, torn, empty) is set aside and a new pair generated.
        for bad in [&b"garbage"[..], b"", b"public abc", b"\xff\0\0"] {
            fs::write(&p, bad).expect("write");
            let (c, d) = load_or_create_server_keys(&p).expect("recover");
            let d = d.expect("damage reported");
            assert_eq!(d.kind, StateFile::ServerKey);
            assert_ne!(c.public, a.public);
            let aside = d.set_aside_as.expect("set aside");
            assert_eq!(fs::read(&aside).expect("kept"), bad);
            fs::remove_file(aside).expect("rm");
            let (again, none) = load_or_create_server_keys(&p).expect("reload");
            assert_eq!(again.public, c.public);
            assert!(none.is_none());
        }
    }

    #[test]
    fn authorized_drops_only_the_bad_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("authorized_clients");
        let kp = KeyPair::generate().expect("gen");
        let good = format!("# list\nlaptop {} viewer\n", kp.public.to_z85());
        fs::write(&p, format!("{good}phone abc operator\ntorn")).expect("write");
        let (keys, d) = load_or_create_authorized(&p).expect("recover");
        assert_eq!(keys.len(), 1);
        let d = d.expect("damage");
        assert_eq!(d.kind, StateFile::AuthorizedClients);
        assert!(d.reason.contains("dropped 2 line(s)"), "{}", d.reason);
        assert!(d.set_aside_as.expect("aside").exists());
        let (again, none) = load_or_create_authorized(&p).expect("reload");
        assert_eq!(again.len(), 1);
        assert!(none.is_none());

        fs::write(&p, b"\0\0\0\0").expect("nul");
        let (keys, d) = load_or_create_authorized(&p).expect("recover nul");
        assert!(keys.is_empty());
        assert!(d.is_some());
    }

    #[test]
    fn every_role_is_itself_after_a_reload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("authorized_clients");
        let app = KeyPair::generate().expect("gen");
        let view = KeyPair::generate().expect("gen");
        let rig = KeyPair::generate().expect("gen");
        let mut keys = AuthorizedKeys::new();
        keys.insert("laptop", app.public, Role::Operator)
            .expect("app");
        keys.insert("phone", view.public, Role::Viewer)
            .expect("viewer");
        keys.insert("rig-pupu", rig.public, Role::Rig(0x26b1_c286_0000_0001))
            .expect("rig");
        fs::write(&p, keys.to_text()).expect("write");
        let (back, none) = load_or_create_authorized(&p).expect("load");
        assert!(none.is_none());
        assert_eq!(back.rig_of("rig-pupu"), Some(0x26b1_c286_0000_0001));
        assert_eq!(back.role_of("laptop"), Some(Role::Operator));
        assert_eq!(back.role_of("phone"), Some(Role::Viewer));
        // Also through the path that rewrites a damaged file.
        fs::write(&p, format!("{}torn", keys.to_text())).expect("torn");
        let (back, d) = load_or_create_authorized(&p).expect("recover");
        assert!(d.is_some());
        assert_eq!(back.rig_of("rig-pupu"), Some(0x26b1_c286_0000_0001));
        assert_eq!(back.role_of("phone"), Some(Role::Viewer));
        let (again, _) = load_or_create_authorized(&p).expect("reload");
        assert_eq!(again.rig_of("rig-pupu"), Some(0x26b1_c286_0000_0001));
    }

    #[test]
    fn a_list_without_roles_is_set_aside_whole_saying_what_a_line_needs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("authorized_clients");
        let app = KeyPair::generate().expect("gen");
        let old = format!("# old\nlaptop {}\n", app.public.to_z85());
        fs::write(&p, &old).expect("write");
        let (keys, d) = load_or_create_authorized(&p).expect("load");
        assert!(keys.is_empty());
        let d = d.expect("damage");
        assert!(d.reason.contains("no role"), "{}", d.reason);
        assert!(d.reason.contains("`<name> <key> operator`"), "{}", d.reason);
        assert_eq!(
            fs::read_to_string(d.set_aside_as.expect("aside")).expect("kept"),
            old
        );
        let (again, none) = load_or_create_authorized(&p).expect("reload");
        assert!(again.is_empty() && none.is_none());
    }

    #[test]
    fn authorized_created_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("authorized_clients");
        assert!(load_or_create_authorized(&p).expect("create").0.is_empty());
        assert!(p.exists());
        assert!(load_or_create_authorized(&p).expect("load").0.is_empty());
    }

    /// A list another user could have written is not read: no client is admitted, the
    /// damage says why, and the file is left as it is.
    #[cfg(unix)]
    #[test]
    fn an_untrusted_list_admits_no_one() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("authorized_clients");
        let mut keys = AuthorizedKeys::new();
        let kp = ac2_zmq::KeyPair::generate().expect("key");
        keys.insert("laptop", kp.public, Role::Operator)
            .expect("insert");
        keys.save(&p).expect("save");
        assert_eq!(load_or_create_authorized(&p).expect("load").0.len(), 1);

        fs::set_permissions(&p, fs::Permissions::from_mode(0o620)).expect("chmod");
        let (none, d) = load_or_create_authorized(&p).expect("refused, not an error");
        assert!(none.is_empty());
        let d = d.expect("damage");
        assert!(
            d.reason.contains("writable") && d.set_aside_as.is_none(),
            "{}",
            d.reason
        );
        assert_eq!(
            fs::metadata(&p).expect("left").permissions().mode() & 0o777,
            0o620
        );

        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).expect("chmod");
        let link = dir.path().join("linked");
        std::os::unix::fs::symlink(&p, &link).expect("link");
        let (none, d) = load_or_create_authorized(&link).expect("refused");
        assert!(none.is_empty() && d.is_some_and(|d| d.reason.contains("regular file")));
    }
}
