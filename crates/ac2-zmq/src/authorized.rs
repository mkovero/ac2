//! Authorized client keys: the list the ZAP handler checks every CURVE handshake against.
//!
//! # File format
//!
//! UTF-8 text, one client per line:
//!
//! ```text
//! # ac2 authorized clients: <name> <Z85 CURVE public key> <operator | viewer | rig=<rig id>>
//! laptop   Yne@$w-vo<fVvi]a<NY6T1ed:M$fCG*[IaLV{hID operator
//! tablet.2 rq:rM>}U?@Lns47E1%kR.o@n%FcmmsL/@{H8]yf7 viewer
//! ```
//!
//! - Fields are separated by spaces or tabs; leading and trailing whitespace is ignored.
//! - A line whose first non-blank character is `#` is a comment; blank lines are ignored.
//!   (`#` is a Z85 character, so comments cannot follow a key on the same line.)
//! - `name`: 1–64 characters from `A–Z a–z 0–9 . _ -`. It becomes the connection's ZAP
//!   `User-Id`, i.e. the authenticated client identity the daemon binds leases and audit
//!   entries to.
//! - `key`: the client's 40-character Z85 CURVE public key.
//! - `role`, required: `operator` (drives the rig), `viewer` (reads it, and may stop the
//!   generator) or `rig=<16 hex digits>` (another rig's daemon, by its rig id, admitted to take
//!   audio rather than to control: `pupu <key> rig=00000000706f7075`). A line without one is
//!   malformed, so a lost or torn field locks that client out rather than granting it
//!   control. A file whose key lines all lack it is the shape before roles
//!   ([`is_before_roles`]).
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

/// What a listed key may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Drives the rig.
    Operator,
    /// Reads the rig; may stop the generator.
    Viewer,
    /// Another rig's daemon, with that rig's id: takes this rig's inputs.
    Rig(u64),
}

impl Role {
    /// The third field of its line.
    fn field(self) -> String {
        match self {
            Self::Operator => "operator".to_owned(),
            Self::Viewer => "viewer".to_owned(),
            Self::Rig(id) => format!("rig={id:016x}"),
        }
    }

    fn parse(field: &str) -> Option<Self> {
        match field {
            "operator" => Some(Self::Operator),
            "viewer" => Some(Self::Viewer),
            _ => field
                .strip_prefix("rig=")
                .filter(|h| h.len() == 16)
                .and_then(|h| u64::from_str_radix(h, 16).ok())
                .map(Self::Rig),
        }
    }
}

/// What a key that passed the handshake is admitted as now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// No longer listed under the name it connected with (removed, or the name now holds
    /// another key).
    Revoked,
    /// Listed with this role.
    Listed(Role),
}

/// Whether `text` is an authorized-clients file in the shape before roles: it lists keys and
/// no key line names a role. Such a file is converted, never read: every line of it would
/// be malformed, and reading it line by line would drop every client.
pub fn is_before_roles(text: &str) -> bool {
    let mut keys = 0usize;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_ascii_whitespace();
        match (fields.next(), fields.next(), fields.next()) {
            (Some(_), Some(_), None) => keys += 1,
            (Some(_), Some(_), Some(r)) if r.starts_with("rig=") => {}
            _ => return false,
        }
    }
    keys > 0
}

/// Named client public keys. Names and keys are each unique.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthorizedKeys {
    by_name: BTreeMap<String, PublicKey>,
    by_key: BTreeMap<PublicKey, String>,
    roles: BTreeMap<String, Role>,
}

impl AuthorizedKeys {
    /// An empty list (refuses every client).
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `key` under `name` with `role`.
    pub fn insert(&mut self, name: &str, key: PublicKey, role: Role) -> Result<(), KeyStoreError> {
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
        self.roles.insert(name.to_owned(), role);
        Ok(())
    }

    /// Removes `name`; returns its key if it was listed.
    pub fn remove(&mut self, name: &str) -> Option<PublicKey> {
        let key = self.by_name.remove(name)?;
        self.by_key.remove(&key);
        self.roles.remove(name);
        Some(key)
    }

    /// The role of `name`; `None` = not listed.
    pub fn role_of(&self, name: &str) -> Option<Role> {
        self.roles.get(name).copied()
    }

    /// The rig id `name` was admitted as; `None` = no rig (or not listed).
    pub fn rig_of(&self, name: &str) -> Option<u64> {
        match self.roles.get(name) {
            Some(Role::Rig(id)) => Some(*id),
            _ => None,
        }
    }

    /// The key listed under `name`.
    pub fn get(&self, name: &str) -> Option<&PublicKey> {
        self.by_name.get(name)
    }

    /// What a connection that passed the handshake as `key` under `name` is admitted as
    /// now. The name alone is not enough: a revoked name can later be given to a different
    /// key, and that must not revive a connection the old key still holds open.
    pub fn admission(&self, name: &str, key: &PublicKey) -> Admission {
        if self.by_name.get(name) != Some(key) {
            return Admission::Revoked;
        }
        self.roles
            .get(name)
            .map_or(Admission::Revoked, |r| Admission::Listed(*r))
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
            let (Some(name), Some(key), Some(role), None) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return Err(parse_err(
                    "expected `<name> <Z85 public key> <operator | viewer | rig=<rig id>>`"
                        .to_owned(),
                ));
            };
            let key = PublicKey::from_z85(key)
                .map_err(|_| parse_err(format!("{name}: not a 40-character Z85 key")))?;
            let role = Role::parse(role).ok_or_else(|| {
                parse_err(format!(
                    "{name}: expected `operator`, `viewer` or `rig=<16 hex digits>`"
                ))
            })?;
            keys.insert(name, key, role)
                .map_err(|e| parse_err(e.to_string()))?;
        }
        Ok(keys)
    }

    /// The file format described in the module docs.
    pub fn to_text(&self) -> String {
        let mut out = String::from(
            "# ac2 authorized clients: <name> <Z85 CURVE public key> <operator | viewer | rig=<rig id>>\n",
        );
        for (name, key) in self.iter() {
            out.push_str(name);
            out.push(' ');
            out.push_str(&key.to_z85());
            out.push(' ');
            out.push_str(&self.role_of(name).unwrap_or(Role::Viewer).field());
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
        keys.insert("laptop", a, Role::Operator)?;
        keys.insert("tablet.2", b, Role::Viewer)?;
        let mut back = AuthorizedKeys::parse(&keys.to_text())?;
        assert_eq!(back, keys);
        assert_eq!(back.name_of(&b), Some("tablet.2"));
        assert_eq!(back.get("laptop"), Some(&a));
        assert_eq!(back.remove("laptop"), Some(a));
        assert_eq!(back.name_of(&a), None);
        Ok(())
    }

    #[test]
    fn admission_follows_the_name_and_its_key() -> Result<(), KeyStoreError> {
        let (a, b) = (key(), key());
        let mut keys = AuthorizedKeys::new();
        keys.insert("pupu", a, Role::Rig(7))?;
        assert_eq!(keys.admission("pupu", &a), Admission::Listed(Role::Rig(7)));
        // The same name given to another key, as an app: the old key's connection is out.
        keys.remove("pupu");
        keys.insert("pupu", b, Role::Viewer)?;
        assert_eq!(keys.admission("pupu", &a), Admission::Revoked);
        assert_eq!(keys.admission("pupu", &b), Admission::Listed(Role::Viewer));
        keys.remove("pupu");
        assert_eq!(keys.admission("pupu", &b), Admission::Revoked);
        Ok(())
    }

    #[test]
    fn every_role_keeps_itself_through_the_file() -> Result<(), KeyStoreError> {
        let (a, v, r) = (key(), key(), key());
        let mut keys = AuthorizedKeys::new();
        keys.insert("laptop", a, Role::Operator)?;
        keys.insert("phone", v, Role::Viewer)?;
        keys.insert("pupu", r, Role::Rig(0x706f_7075))?;
        let text = keys.to_text();
        assert!(text.contains(" rig=00000000706f7075\n"), "{text}");
        assert!(text.contains(" operator\n"), "{text}");
        assert!(text.contains(" viewer\n"), "{text}");
        assert!(!is_before_roles(&text));
        let mut back = AuthorizedKeys::parse(&text)?;
        assert_eq!(back, keys);
        assert_eq!(back.rig_of("pupu"), Some(0x706f_7075));
        assert_eq!(back.rig_of("laptop"), None);
        assert_eq!(back.role_of("laptop"), Some(Role::Operator));
        assert_eq!(back.role_of("phone"), Some(Role::Viewer));
        back.remove("pupu");
        assert_eq!(back.rig_of("pupu"), None);
        let bad = format!("pupu {} rig=12", r.to_z85());
        assert!(AuthorizedKeys::parse(&bad).is_err());
        let bad = format!("pupu {} app", r.to_z85());
        assert!(AuthorizedKeys::parse(&bad).is_err());
        let bad = format!("pupu {} view", r.to_z85());
        assert!(AuthorizedKeys::parse(&bad).is_err());
        Ok(())
    }

    #[test]
    fn a_line_without_a_role_is_malformed_and_a_file_of_them_is_the_old_shape() {
        let (a, r) = (key(), key());
        let old = format!("# list\nlaptop {a}\npupu {r} rig=00000000706f7075\n");
        assert!(is_before_roles(&old));
        assert!(matches!(
            AuthorizedKeys::parse(&old),
            Err(KeyStoreError::Parse { line: 2, .. })
        ));
        // One line with a role makes it the new shape with one torn line, not the old one.
        let torn = format!("laptop {a} operator\nphone {r}\n");
        assert!(!is_before_roles(&torn));
        assert!(!is_before_roles("# only comments\n"));
        assert!(!is_before_roles(&format!(
            "pupu {r} rig=00000000706f7075\n"
        )));
    }

    #[test]
    fn parse_skips_comments_and_blank_lines() -> Result<(), KeyStoreError> {
        let k = key();
        let text = format!("\n  # comment\n\t laptop \t {k} \t operator  \n\n");
        let keys = AuthorizedKeys::parse(&text)?;
        assert_eq!(keys.name_of(&k), Some("laptop"));
        assert_eq!(keys.len(), 1);
        Ok(())
    }

    #[test]
    fn parse_rejects_malformed_lines_with_line_numbers() {
        let k = key();
        let cases = [
            format!("ok {k} viewer\nlaptop\n"),
            format!("ok {k} viewer\nlaptop {k} viewer extra\n"),
            "ok short-key viewer\n".to_owned(),
            format!("ok {k} viewer\nbad/name {} viewer\n", key()),
            format!("same {k} viewer\nsame {} viewer\n", key()),
            format!("one {k} viewer\ntwo {k} viewer\n"),
            format!("ok {k} viewer\nlaptop {}\n", key()),
        ];
        let lines = [2, 2, 1, 2, 2, 2, 2];
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
            keys.insert("", k, Role::Viewer),
            Err(KeyStoreError::InvalidName(_))
        ));
        assert!(matches!(
            keys.insert(&"x".repeat(MAX_NAME_LEN + 1), k, Role::Viewer),
            Err(KeyStoreError::InvalidName(_))
        ));
        assert!(keys.insert("a", k, Role::Viewer).is_ok());
        assert!(matches!(
            keys.insert("a", key(), Role::Viewer),
            Err(KeyStoreError::DuplicateName(_))
        ));
        assert!(matches!(
            keys.insert("b", k, Role::Viewer),
            Err(KeyStoreError::DuplicateKey { existing }) if existing == "a"
        ));
    }

    #[test]
    fn save_replaces_atomically_and_loads_back() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("authorized_clients");
        let mut keys = AuthorizedKeys::new();
        keys.insert("laptop", key(), Role::Operator)?;
        keys.save(&path)?;
        keys.insert("tablet", key(), Role::Viewer)?;
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
