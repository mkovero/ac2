//! Client key directory: this client's CURVE keypair and the server keys it has pinned.
//!
//! Layout of the key directory (default [`default_key_dir`]):
//!
//! ```text
//! client.key      Z85 secret key (40 chars), mode 0600 on Unix
//! client.pub      Z85 public key (40 chars) — what the daemon operator authorizes
//! known_servers   one `<host> <Z85 server public key>` per line, `#` comments
//! ```
//!
//! A server key is pinned once (`ac2 auth pair`), after the operator compared its
//! [`fingerprint`] with the one the daemon shows. CURVE then refuses any server that cannot
//! prove it holds the pinned key's secret half.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use ac2_zmq::{CurveClient, KeyPair, PublicKey, SecretKey};

use crate::error::ClientError;

/// `$AC2_KEY_DIR`, else [`ac2_paths::client_key_dir`] (`keys` in the ac2 config directory,
/// next to the daemon's `server.key` and `authorized_clients`).
pub fn default_key_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("AC2_KEY_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    ac2_paths::client_key_dir()
}

/// Short, human-comparable fingerprint of a CURVE public key: the first 10 bytes of
/// SHA-256 over the 32 raw key bytes, as five dash-separated groups of four lowercase hex
/// digits (`1a2b-3c4d-5e6f-7a8b-9c0d`). The daemon shows the same for its own key.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint()
}

/// How an advertised daemon relates to this client's pinned keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinStatus {
    /// A key with the advertised fingerprint is pinned (under `host`).
    Paired {
        /// The host it was pinned under.
        host: String,
        /// The pinned key.
        key: PublicKey,
    },
    /// The daemon's host is pinned to a key with another fingerprint: a re-keyed daemon, or
    /// something impersonating it. Never connected to without a new, verified pairing.
    Mismatch {
        /// The pinned host.
        host: String,
        /// Fingerprint of the key pinned for it.
        pinned: String,
    },
    /// No pin matches.
    Unpaired,
}

/// A key directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyDir {
    dir: PathBuf,
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> ClientError + '_ {
    move |source| ClientError::Io {
        path: path.to_owned(),
        source,
    }
}

impl KeyDir {
    /// Key directory at `dir` (not created until something is written).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.dir
    }

    fn secret_path(&self) -> PathBuf {
        self.dir.join("client.key")
    }

    fn public_path(&self) -> PathBuf {
        self.dir.join("client.pub")
    }

    fn known_path(&self) -> PathBuf {
        self.dir.join("known_servers")
    }

    /// This client's keypair.
    pub fn client_keypair(&self) -> Result<KeyPair, ClientError> {
        let sp = self.secret_path();
        let pp = self.public_path();
        let secret = fs::read_to_string(&sp).map_err(io_err(&sp))?;
        let public = fs::read_to_string(&pp).map_err(io_err(&pp))?;
        let secret = SecretKey::from_z85(secret.trim())
            .map_err(|_| ClientError::Keys(format!("{}: not a Z85 key", sp.display())))?;
        let public = PublicKey::from_z85(public.trim())
            .map_err(|_| ClientError::Keys(format!("{}: not a Z85 key", pp.display())))?;
        Ok(KeyPair { public, secret })
    }

    /// This client's keypair, generating and storing one if none exists. Returns whether it
    /// was created.
    pub fn ensure_client_keypair(&self) -> Result<(KeyPair, bool), ClientError> {
        if self.secret_path().exists() {
            return Ok((self.client_keypair()?, false));
        }
        fs::create_dir_all(&self.dir).map_err(io_err(&self.dir))?;
        let kp = KeyPair::generate()?;
        write_file(&self.secret_path(), &kp.secret.to_z85(), true)?;
        write_file(&self.public_path(), &kp.public.to_z85(), false)?;
        Ok((kp, true))
    }

    /// Every pinned server key, in file order.
    pub fn known_servers(&self) -> Result<Vec<(String, PublicKey)>, ClientError> {
        let path = self.known_path();
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_err(&path)(e)),
        };
        let mut out = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut f = line.split_ascii_whitespace();
            let (Some(host), Some(key), None) = (f.next(), f.next(), f.next()) else {
                return Err(ClientError::Keys(format!(
                    "{} line {}: expected `<host> <Z85 key>`",
                    path.display(),
                    i + 1
                )));
            };
            let key = PublicKey::from_z85(key).map_err(|_| {
                ClientError::Keys(format!("{} line {}: not a Z85 key", path.display(), i + 1))
            })?;
            out.push((host.to_owned(), key));
        }
        Ok(out)
    }

    /// The pinned key of `host`.
    pub fn server_key(&self, host: &str) -> Result<PublicKey, ClientError> {
        self.known_servers()?
            .into_iter()
            .find(|(h, _)| h == host)
            .map(|(_, k)| k)
            .ok_or_else(|| {
                ClientError::Keys(format!(
                    "no pinned server key for {host:?} in {}; run `ac2 auth pair {host} --server-key <key>`",
                    self.known_path().display()
                ))
            })
    }

    /// Pins `key` for `host`, replacing an earlier pin. Returns the key it replaced.
    pub fn pin_server(&self, host: &str, key: PublicKey) -> Result<Option<PublicKey>, ClientError> {
        if host.is_empty() || host.chars().any(char::is_whitespace) {
            return Err(ClientError::Invalid(format!("host {host:?}")));
        }
        let mut all = self.known_servers()?;
        let old = all
            .iter()
            .position(|(h, _)| h == host)
            .map(|i| all.remove(i).1);
        all.push((host.to_owned(), key));
        let mut text = String::from("# ac2 pinned daemon keys: <host> <Z85 CURVE public key>\n");
        for (h, k) in &all {
            text.push_str(&format!("{h} {}\n", k.to_z85()));
        }
        fs::create_dir_all(&self.dir).map_err(io_err(&self.dir))?;
        write_file(&self.known_path(), &text, false)?;
        Ok(old)
    }

    /// How a daemon that advertises `fingerprint` (from mDNS, so untrusted) relates to the
    /// pins: `Paired` when a pinned key has that fingerprint, `Mismatch` when one of `hosts`
    /// (the names the daemon goes by) is pinned to a different key. Only a pinned key is
    /// ever used to connect; CURVE then proves the daemon holds it.
    pub fn pin_status(&self, fingerprint: &str, hosts: &[&str]) -> Result<PinStatus, ClientError> {
        let pins = self.known_servers()?;
        if let Some((host, key)) = pins.iter().find(|(_, k)| k.fingerprint() == fingerprint) {
            return Ok(PinStatus::Paired {
                host: host.clone(),
                key: *key,
            });
        }
        Ok(pins
            .iter()
            .find(|(h, _)| hosts.contains(&h.as_str()))
            .map_or(PinStatus::Unpaired, |(h, k)| PinStatus::Mismatch {
                host: h.clone(),
                pinned: k.fingerprint(),
            }))
    }

    /// CURVE client configuration for a daemon whose key was pinned (under any host name).
    pub fn curve_client_for_key(&self, server_key: PublicKey) -> Result<CurveClient, ClientError> {
        Ok(CurveClient {
            keys: self.client_keypair()?,
            server_key,
        })
    }

    /// CURVE client configuration for `host`: this client's keypair and the pinned key.
    pub fn curve_client(&self, host: &str) -> Result<CurveClient, ClientError> {
        Ok(CurveClient {
            keys: self.client_keypair()?,
            server_key: self.server_key(host)?,
        })
    }
}

fn write_file(path: &Path, text: &str, secret: bool) -> Result<(), ClientError> {
    let tmp = path.with_extension("tmp");
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(if secret { 0o600 } else { 0o644 });
    }
    #[cfg(not(unix))]
    let _ = secret;
    let mut f = opts.open(&tmp).map_err(io_err(&tmp))?;
    f.write_all(text.as_bytes()).map_err(io_err(&tmp))?;
    f.sync_all().map_err(io_err(&tmp))?;
    drop(f);
    fs::rename(&tmp, path).map_err(io_err(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keypair_and_pins() -> Result<(), ClientError> {
        let tmp = tempfile::tempdir().map_err(|e| ClientError::Keys(e.to_string()))?;
        let kd = KeyDir::new(tmp.path().join("keys"));
        assert!(kd.client_keypair().is_err());
        let (kp, created) = kd.ensure_client_keypair()?;
        assert!(created);
        let (again, created) = kd.ensure_client_keypair()?;
        assert!(!created);
        assert_eq!(kp, again);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(tmp.path().join("keys/client.key"))
                .map_err(|e| ClientError::Keys(e.to_string()))?
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let server = KeyPair::generate()?.public;
        assert!(kd.curve_client("rig").is_err());
        assert_eq!(kd.pin_server("rig", server)?, None);
        let other = KeyPair::generate()?.public;
        assert_eq!(kd.pin_server("rig", other)?, Some(server));
        let cc = kd.curve_client("rig")?;
        assert_eq!(cc.server_key, other);
        assert_eq!(kd.known_servers()?.len(), 1);
        Ok(())
    }

    #[test]
    fn pin_status_by_fingerprint_and_host() -> Result<(), ClientError> {
        let tmp = tempfile::tempdir().map_err(|e| ClientError::Keys(e.to_string()))?;
        let kd = KeyDir::new(tmp.path());
        let a = KeyPair::generate()?.public;
        let b = KeyPair::generate()?.public;
        assert_eq!(
            kd.pin_status(&a.fingerprint(), &["rig"])?,
            PinStatus::Unpaired
        );
        kd.pin_server("10.0.0.5", a)?;
        assert_eq!(
            kd.pin_status(&a.fingerprint(), &["rig.local", "10.0.0.9"])?,
            PinStatus::Paired {
                host: "10.0.0.5".into(),
                key: a
            }
        );
        assert_eq!(
            kd.pin_status(&b.fingerprint(), &["rig.local", "10.0.0.5"])?,
            PinStatus::Mismatch {
                host: "10.0.0.5".into(),
                pinned: a.fingerprint()
            }
        );
        assert_eq!(
            kd.pin_status(&b.fingerprint(), &["other"])?,
            PinStatus::Unpaired
        );
        Ok(())
    }

    #[test]
    fn client_keys_live_in_the_shared_config_dir() {
        // The daemon's server.key / authorized_clients and the client's keys must resolve
        // under one directory on every OS, or pairing instructions point to two places.
        if std::env::var_os("AC2_KEY_DIR").is_none() {
            assert_eq!(default_key_dir(), ac2_paths::config_dir().join("keys"));
        }
        assert_eq!(
            ac2_paths::server_key().parent(),
            Some(ac2_paths::config_dir().as_path())
        );
    }

    #[test]
    fn fingerprint_shape() {
        let fp = fingerprint(&PublicKey::from_bytes([0; 32]));
        // SHA-256 of 32 zero bytes starts 66687aadf862bd776c8f.
        assert_eq!(fp, "6668-7aad-f862-bd77-6c8f");
    }
}
