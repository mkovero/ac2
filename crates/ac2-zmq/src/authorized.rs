//! Authorized client keys: the list the ZAP handler checks every CURVE handshake against.
//!
//! # File format
//!
//! UTF-8 text, one client per line:
//!
//! ```text
//! # ac2 authorized clients: <name> <Z85 CURVE public key>
//! laptop   Yne@$w-vo<fVvi]a<NY6T1ed:M$fCG*[IaLV{hID
//! tablet.2 rq:rM>}U?@Lns47E1%kR.o@n%FcmmsL/@{H8]yf7
//! ```
//!
//! - Fields are separated by spaces or tabs; leading and trailing whitespace is ignored.
//! - A line whose first non-blank character is `#` is a comment; blank lines are ignored.
//!   (`#` is a Z85 character, so comments cannot follow a key on the same line.)
//! - `name`: 1–64 characters from `A–Z a–z 0–9 . _ -`. It becomes the connection's ZAP
//!   `User-Id`, i.e. the authenticated client identity the daemon binds leases and audit
//!   entries to.
//! - `key`: the client's 40-character Z85 CURVE public key.
//! - Names and keys are unique. Any malformed line rejects the whole file (with its line
//!   number): a half-read list must never silently drop a key or authorize a wrong one.
//!
//! [`AuthorizedKeys::save`] writes atomically (temporary file in the same directory, flushed
//! to disk, then renamed over the target), so a crash leaves either the old or the new list.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::curve::PublicKey;

/// Longest accepted client name.
pub const MAX_NAME_LEN: usize = 64;

/// Why a key list could not be read, written or changed.
#[derive(Debug)]
#[non_exhaustive]
pub enum KeyStoreError {
    /// Reading or writing the file failed.
    Io {
        /// File involved.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// A line of the file is malformed.
    Parse {
        /// 1-based line number.
        line: usize,
        /// What is wrong with it.
        reason: String,
    },
    /// The name is empty, too long or has characters outside `A–Z a–z 0–9 . _ -`.
    InvalidName(String),
    /// The name is already in the list.
    DuplicateName(String),
    /// The key is already in the list, under the given name.
    DuplicateKey {
        /// Name the key is already listed under.
        existing: String,
    },
}

impl fmt::Display for KeyStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Parse { line, reason } => write!(f, "line {line}: {reason}"),
            Self::InvalidName(n) => write!(
                f,
                "invalid client name {n:?}: use 1-{MAX_NAME_LEN} of A-Z a-z 0-9 . _ -"
            ),
            Self::DuplicateName(n) => write!(f, "client name {n:?} is already authorized"),
            Self::DuplicateKey { existing } => {
                write!(f, "key is already authorized as {existing:?}")
            }
        }
    }
}

impl std::error::Error for KeyStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn valid_name(name: &str) -> bool {
    (1..=MAX_NAME_LEN).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Named client public keys. Names and keys are each unique.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthorizedKeys {
    by_name: BTreeMap<String, PublicKey>,
    by_key: BTreeMap<PublicKey, String>,
}

impl AuthorizedKeys {
    /// An empty list (refuses every client).
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `key` under `name`.
    pub fn insert(&mut self, name: &str, key: PublicKey) -> Result<(), KeyStoreError> {
        if !valid_name(name) {
            return Err(KeyStoreError::InvalidName(name.to_owned()));
        }
        if self.by_name.contains_key(name) {
            return Err(KeyStoreError::DuplicateName(name.to_owned()));
        }
        if let Some(existing) = self.by_key.get(&key) {
            return Err(KeyStoreError::DuplicateKey {
                existing: existing.clone(),
            });
        }
        self.by_name.insert(name.to_owned(), key);
        self.by_key.insert(key, name.to_owned());
        Ok(())
    }

    /// Removes `name`; returns its key if it was listed.
    pub fn remove(&mut self, name: &str) -> Option<PublicKey> {
        let key = self.by_name.remove(name)?;
        self.by_key.remove(&key);
        Some(key)
    }

    /// The key listed under `name`.
    pub fn get(&self, name: &str) -> Option<&PublicKey> {
        self.by_name.get(name)
    }

    /// The name `key` is listed under; `None` = not authorized.
    pub fn name_of(&self, key: &PublicKey) -> Option<&str> {
        self.by_key.get(key).map(String::as_str)
    }

    /// `(name, key)` pairs ordered by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &PublicKey)> {
        self.by_name.iter().map(|(n, k)| (n.as_str(), k))
    }

    /// Number of authorized clients.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether no client is authorized.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Parses the file format described in the module docs.
    pub fn parse(text: &str) -> Result<Self, KeyStoreError> {
        let mut keys = Self::new();
        for (i, line) in text.lines().enumerate() {
            let line_no = i + 1;
            let parse_err = |reason: String| KeyStoreError::Parse {
                line: line_no,
                reason,
            };
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut fields = line.split_ascii_whitespace();
            let (Some(name), Some(key), None) = (fields.next(), fields.next(), fields.next())
            else {
                return Err(parse_err("expected `<name> <Z85 public key>`".to_owned()));
            };
            let key = PublicKey::from_z85(key)
                .map_err(|_| parse_err(format!("{name}: not a 40-character Z85 key")))?;
            keys.insert(name, key)
                .map_err(|e| parse_err(e.to_string()))?;
        }
        Ok(keys)
    }

    /// The file format described in the module docs.
    pub fn to_text(&self) -> String {
        let mut out = String::from("# ac2 authorized clients: <name> <Z85 CURVE public key>\n");
        for (name, key) in self.iter() {
            out.push_str(name);
            out.push(' ');
            out.push_str(&key.to_z85());
            out.push('\n');
        }
        out
    }

    /// Reads and parses `path`. A missing file is an error; the caller decides whether that
    /// means "no clients".
    pub fn load(path: &Path) -> Result<Self, KeyStoreError> {
        let text = fs::read_to_string(path).map_err(|source| KeyStoreError::Io {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&text)
    }

    /// Writes the list to `path` atomically (see the module docs).
    pub fn save(&self, path: &Path) -> Result<(), KeyStoreError> {
        write_atomic(path, self.to_text().as_bytes()).map_err(|source| KeyStoreError::Io {
            path: path.to_owned(),
            source,
        })
    }
}

fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    // Same directory, so the rename cannot cross filesystems.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = dir.join(format!(
        ".{}.tmp-{}-{}",
        file_name.to_string_lossy(),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
        drop(f);
        // Replaces an existing target on every platform (MoveFileEx with REPLACE_EXISTING on
        // Windows).
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written?;
    // Persist the rename itself; directories cannot be opened for sync on Windows.
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KeyPair;

    fn key() -> PublicKey {
        KeyPair::generate().expect("keypair").public
    }

    #[test]
    fn text_roundtrip_and_lookup() -> Result<(), KeyStoreError> {
        let (a, b) = (key(), key());
        let mut keys = AuthorizedKeys::new();
        keys.insert("laptop", a)?;
        keys.insert("tablet.2", b)?;
        let mut back = AuthorizedKeys::parse(&keys.to_text())?;
        assert_eq!(back, keys);
        assert_eq!(back.name_of(&b), Some("tablet.2"));
        assert_eq!(back.get("laptop"), Some(&a));
        assert_eq!(back.remove("laptop"), Some(a));
        assert_eq!(back.name_of(&a), None);
        Ok(())
    }

    #[test]
    fn parse_skips_comments_and_blank_lines() -> Result<(), KeyStoreError> {
        let k = key();
        let text = format!("\n  # comment\n\t laptop \t {k}  \n\n");
        let keys = AuthorizedKeys::parse(&text)?;
        assert_eq!(keys.name_of(&k), Some("laptop"));
        assert_eq!(keys.len(), 1);
        Ok(())
    }

    #[test]
    fn parse_rejects_malformed_lines_with_line_numbers() {
        let k = key();
        let cases = [
            format!("ok {k}\nlaptop\n"),
            format!("ok {k}\nlaptop {k} extra\n"),
            "ok short-key\n".to_owned(),
            format!("ok {k}\nbad/name {}\n", key()),
            format!("same {k}\nsame {}\n", key()),
            format!("one {k}\ntwo {k}\n"),
        ];
        let lines = [2, 2, 1, 2, 2, 2];
        for (text, want) in cases.iter().zip(lines) {
            match AuthorizedKeys::parse(text) {
                Err(KeyStoreError::Parse { line, .. }) => assert_eq!(line, want, "{text}"),
                other => panic!("{text:?} parsed as {other:?}"),
            }
        }
    }

    #[test]
    fn insert_validates_names_and_uniqueness() {
        let mut keys = AuthorizedKeys::new();
        let k = key();
        assert!(matches!(
            keys.insert("", k),
            Err(KeyStoreError::InvalidName(_))
        ));
        assert!(matches!(
            keys.insert(&"x".repeat(MAX_NAME_LEN + 1), k),
            Err(KeyStoreError::InvalidName(_))
        ));
        assert!(keys.insert("a", k).is_ok());
        assert!(matches!(
            keys.insert("a", key()),
            Err(KeyStoreError::DuplicateName(_))
        ));
        assert!(matches!(
            keys.insert("b", k),
            Err(KeyStoreError::DuplicateKey { existing }) if existing == "a"
        ));
    }

    #[test]
    fn save_replaces_atomically_and_loads_back() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("authorized_clients");
        let mut keys = AuthorizedKeys::new();
        keys.insert("laptop", key())?;
        keys.save(&path)?;
        keys.insert("tablet", key())?;
        keys.save(&path)?;
        assert_eq!(AuthorizedKeys::load(&path)?, keys);
        // Only the target remains: no temporary file left behind.
        let names: Vec<_> = fs::read_dir(dir.path())?
            .map(|e| e.map(|e| e.file_name()))
            .collect::<Result<_, _>>()?;
        assert_eq!(names, vec![std::ffi::OsString::from("authorized_clients")]);
        Ok(())
    }

    #[test]
    fn load_missing_file_is_an_io_error() {
        let r = AuthorizedKeys::load(Path::new("/nonexistent/ac2/authorized_clients"));
        assert!(matches!(r, Err(KeyStoreError::Io { .. })));
    }
}
