//! Network-mode key files: the server's CURVE key pair and the authorized-clients list.
//!
//! Both are written atomically (temporary file in the same directory, synced, renamed over
//! the target) and, on Unix, created with mode 0600 so the secret key is never readable by
//! other users even for an instant. An existing file that does not parse is an error and is
//! never overwritten.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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

/// Writes `contents` to `path` atomically; the file is created owner-only on Unix.
pub fn write_private_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = dir.join(format!(
        ".{}.tmp-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written?;
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    Ok(())
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

/// Loads the server key pair, generating and saving one if the file does not exist.
pub fn load_or_create_server_keys(path: &Path) -> Result<KeyPair, KeyFileError> {
    match fs::read_to_string(path) {
        Ok(text) => {
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
            parse_key_file(path, &text)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let kp = KeyPair::generate().map_err(|e| KeyFileError::Generate(e.to_string()))?;
            write_private_atomic(path, key_text(&kp).as_bytes()).map_err(io_err(path))?;
            tracing::info!(
                "generated server key pair {} (public {})",
                path.display(),
                kp.public.to_z85()
            );
            Ok(kp)
        }
        Err(e) => Err(io_err(path)(e)),
    }
}

/// Loads the authorized-clients list; a missing file is created empty (no client can
/// connect until one is added).
pub fn load_or_create_authorized(path: &Path) -> Result<AuthorizedKeys, KeyFileError> {
    match fs::metadata(path) {
        Ok(_) => AuthorizedKeys::load(path).map_err(KeyFileError::Authorized),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let keys = AuthorizedKeys::new();
            write_private_atomic(path, keys.to_text().as_bytes()).map_err(io_err(path))?;
            tracing::warn!(
                "{}: created an empty authorized-clients list; no client can connect until one is added",
                path.display()
            );
            Ok(keys)
        }
        Err(e) => Err(io_err(path)(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_keys_roundtrip_and_are_private() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("keys").join("server.key");
        let a = load_or_create_server_keys(&p).expect("create");
        let b = load_or_create_server_keys(&p).expect("load");
        assert_eq!(a.public, b.public);
        assert_eq!(a.secret, b.secret);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&p).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // A corrupt file is refused and left alone.
        fs::write(&p, "garbage").expect("write");
        assert!(load_or_create_server_keys(&p).is_err());
        assert_eq!(fs::read_to_string(&p).expect("read"), "garbage");
    }

    #[test]
    fn authorized_created_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("authorized_clients");
        assert!(load_or_create_authorized(&p).expect("create").is_empty());
        assert!(p.exists());
        assert!(load_or_create_authorized(&p).expect("load").is_empty());
    }
}
