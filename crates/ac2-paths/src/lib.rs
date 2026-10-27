//! Where ac2 keeps its files, shared by the daemon, the CLI and the UI so that every
//! program agrees on one location per kind of file.
//!
//! Two platform-native directories (via `directories::ProjectDirs`):
//!
//! | | Linux | macOS | Windows |
//! |---|---|---|---|
//! | config ([`config_dir`]) | `~/.config/ac2` | `~/Library/Application Support/ac2` | `%APPDATA%\ac2\config` |
//! | data ([`data_dir`]) | `~/.local/share/ac2` | `~/Library/Application Support/ac2` | `%APPDATA%\ac2\data` |
//! | logs ([`log_dir`]) | `~/.local/state/ac2` | `~/Library/Logs/ac2` | `%LOCALAPPDATA%\ac2\logs` |
//!
//! Config holds what belongs to this machine and user: the calibration store (it describes
//! the hardware, not a show), UI preferences, key bindings, the daemon's network keys. Data
//! holds the operator's documents: saved sessions and the daemon's autosave. `XDG_CONFIG_HOME` / `XDG_DATA_HOME` are
//! honoured on Linux; `AC2_CONFIG_DIR` and `AC2_SESSION_DIR` override for tests and
//! unusual setups.
//!
//! Logs are neither: they are disposable diagnostics of this machine, so each OS's own log
//! location ([`log_dir`]). With the `log` feature, [`log`] installs the shared tracing
//! subscriber (stderr plus a per-program file there).
#![forbid(unsafe_code)]

#[cfg(feature = "log")]
pub mod log;
pub mod notify;

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

fn project() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("", "", "ac2")
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
}

/// Per-user configuration directory: `$AC2_CONFIG_DIR`, else the platform config directory
/// (table above). Without a home directory: `./ac2-config`.
pub fn config_dir() -> PathBuf {
    env_dir("AC2_CONFIG_DIR").unwrap_or_else(|| {
        project().map_or_else(
            || PathBuf::from("ac2-config"),
            |d| d.config_dir().to_path_buf(),
        )
    })
}

/// Per-user data directory (table above). Without a home directory: `./ac2-data`.
pub fn data_dir() -> PathBuf {
    project().map_or_else(|| PathBuf::from("ac2-data"), |d| d.data_dir().to_path_buf())
}

/// Per-user log directory: `$AC2_LOG_DIR`, else the platform's place for logs (table above).
/// XDG puts logs under state (`$XDG_STATE_HOME`), not data, because they are not documents
/// worth backing up; macOS Console.app lists `~/Library/Logs`; Windows keeps logs out of the
/// roaming profile, in `%LOCALAPPDATA%`. Without a home directory: `./ac2-logs`.
pub fn log_dir() -> PathBuf {
    env_dir("AC2_LOG_DIR").unwrap_or_else(|| {
        directories::BaseDirs::new()
            .map_or_else(|| PathBuf::from("ac2-logs"), |b| platform_log_dir(&b))
    })
}

#[cfg(target_os = "macos")]
fn platform_log_dir(b: &directories::BaseDirs) -> PathBuf {
    b.home_dir().join("Library").join("Logs").join("ac2")
}

#[cfg(windows)]
fn platform_log_dir(b: &directories::BaseDirs) -> PathBuf {
    b.data_local_dir().join("ac2").join("logs")
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_log_dir(b: &directories::BaseDirs) -> PathBuf {
    b.state_dir()
        .map_or_else(
            || b.home_dir().join(".local").join("state"),
            Path::to_path_buf,
        )
        .join("ac2")
}

/// The daemon's calibration store: `<config>/calibrations.json`.
pub fn cal_store() -> PathBuf {
    config_dir().join("calibrations.json")
}

/// The daemon's identity, which apps key a rig by: `<config>/rig-id`.
pub fn rig_id() -> PathBuf {
    config_dir().join("rig-id")
}

/// The identity of the app's embedded daemon (real audio): `<config>/app-rig-id`, apart
/// from a stand-alone daemon's on the same machine, as both can be connected at once.
pub fn app_rig_id() -> PathBuf {
    config_dir().join("app-rig-id")
}

/// The daemon's rig settings (system max level, output labels): `<config>/rig.json`.
pub fn rig_settings() -> PathBuf {
    config_dir().join("rig.json")
}

/// The daemon's rig SPL meters' logs (a session directory holding only them, apart from
/// the autosave so that loading a session never ends them): `<data>/rig-logs`.
pub fn rig_log_dir() -> PathBuf {
    data_dir().join("rig-logs")
}

/// Saved sessions: `$AC2_SESSION_DIR`, else `<data>/sessions`.
pub fn session_dir() -> PathBuf {
    env_dir("AC2_SESSION_DIR").unwrap_or_else(|| data_dir().join("sessions"))
}

/// The daemon's autosave of measurements and traces (a session directory; its backup
/// `autosave.prev` beside it): `<data>/autosave`.
pub fn autosave_dir() -> PathBuf {
    data_dir().join("autosave")
}

/// The autosave of the app's embedded daemon (real audio): `<data>/autosave-app`, apart
/// from a stand-alone daemon's on the same machine.
pub fn app_autosave_dir() -> PathBuf {
    data_dir().join("autosave-app")
}

/// The daemon's raw capture files (`<name>.wav` + `<name>.ac2rec.json`):
/// `<data>/recordings`.
pub fn recording_dir() -> PathBuf {
    data_dir().join("recordings")
}

/// A network-mode daemon's CURVE key pair: `<config>/server.key`.
pub fn server_key() -> PathBuf {
    config_dir().join("server.key")
}

/// The clients a network-mode daemon accepts: `<config>/authorized_clients`.
pub fn authorized_clients() -> PathBuf {
    config_dir().join("authorized_clients")
}

/// A client's own CURVE key pair and the daemon keys it pinned (`ac2 auth pair`):
/// `<config>/keys/`.
pub fn client_key_dir() -> PathBuf {
    config_dir().join("keys")
}

/// The desktop UI's preferences (remembered stimulus outputs): `<config>/ui.toml`.
pub fn ui_prefs() -> PathBuf {
    config_dir().join("ui.toml")
}

/// The desktop UI's key binding overrides: `<config>/keys.toml`.
pub fn keymap() -> PathBuf {
    config_dir().join("keys.toml")
}

/// Writes `contents` to `path` atomically, creating the directory: a temporary file in the
/// same directory, flushed and synced, renamed over `path`, the directory synced. A crash
/// leaves the old file or the new one, never a mix. The file is owner-only on Unix.
pub fn write_private_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_atomic_mode(path, contents, true)
}

/// As [`write_private_atomic`], readable by others as the umask allows (public keys).
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_atomic_mode(path, contents, false)
}

fn write_atomic_mode(path: &Path, contents: &[u8], private: bool) -> io::Result<()> {
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
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        #[cfg(not(unix))]
        let _ = private;
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
    sync_dir(dir)
}

/// Syncs directory `dir`, so that a rename or a new file in it survives a power cut.
/// Nothing on Windows, where directories cannot be opened for sync (NTFS journals them).
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Moves a damaged file out of the way, keeping it for inspection: to
/// `<name>.damaged-<unix seconds>` beside it (`-2`, `-3`… when taken), the directory synced.
/// Returns where it went.
pub fn set_aside_damaged(path: &Path) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_string_lossy()
        .into_owned();
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut to = path.with_file_name(format!("{name}.damaged-{t}"));
    let mut n = 2;
    while to.exists() {
        to = path.with_file_name(format!("{name}.damaged-{t}-{n}"));
        n += 1;
    }
    fs::rename(path, &to)?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        // The file is moved either way; a failed sync only risks the rename being undone by
        // another power cut, which the next start sets aside again.
        let _ = sync_dir(dir);
    }
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_damaged_file_is_set_aside_with_a_time_and_never_replaced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("rig.json");
        fs::write(&p, b"{").expect("write");
        let a = set_aside_damaged(&p).expect("aside");
        assert!(!p.exists());
        let name = a.file_name().expect("name").to_string_lossy().into_owned();
        assert!(name.starts_with("rig.json.damaged-"), "{name}");
        fs::write(&p, b"[").expect("write");
        let b = set_aside_damaged(&p).expect("aside");
        assert_ne!(a, b);
        assert_eq!(fs::read(&a).expect("first"), b"{");
        assert_eq!(fs::read(&b).expect("second"), b"[");
    }

    #[test]
    fn files_live_in_their_directories() {
        let c = config_dir();
        assert_eq!(cal_store(), c.join("calibrations.json"));
        assert_eq!(ui_prefs(), c.join("ui.toml"));
        assert_eq!(keymap(), c.join("keys.toml"));
        assert_eq!(autosave_dir(), data_dir().join("autosave"));
        if std::env::var_os("AC2_SESSION_DIR").is_none() {
            assert_eq!(session_dir(), data_dir().join("sessions"));
        }
    }

    /// Logs go where each OS keeps them: XDG state on Linux, `~/Library/Logs` on macOS,
    /// the local (non-roaming) app data on Windows.
    #[test]
    fn log_dir_follows_the_platform() {
        if std::env::var_os("AC2_LOG_DIR").is_some() {
            return;
        }
        let d = log_dir();
        let b = directories::BaseDirs::new().expect("a home directory");
        #[cfg(target_os = "macos")]
        assert_eq!(d, b.home_dir().join("Library/Logs/ac2"));
        #[cfg(windows)]
        {
            assert_eq!(d, b.data_local_dir().join("ac2").join("logs"));
            assert!(d.ends_with("AppData\\Local\\ac2\\logs"), "{}", d.display());
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            assert_eq!(d, b.state_dir().expect("XDG state dir").join("ac2"));
            if std::env::var_os("XDG_STATE_HOME").is_none() {
                assert_eq!(d, b.home_dir().join(".local/state/ac2"));
            }
        }
    }

    /// Linux keeps the XDG layout: `~/.config/ac2` and `~/.local/share/ac2`.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_uses_xdg_directories() {
        if std::env::var_os("AC2_CONFIG_DIR").is_some() {
            return;
        }
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        let config = env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        let data = env_dir("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"));
        assert_eq!(config_dir(), config.join("ac2"));
        assert_eq!(data_dir(), data.join("ac2"));
    }

    /// macOS: both under Application Support (sessions in their own subdirectory).
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_uses_application_support() {
        if std::env::var_os("AC2_CONFIG_DIR").is_some() {
            return;
        }
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        let support = home.join("Library/Application Support/ac2");
        assert_eq!(config_dir(), support);
        assert_eq!(data_dir(), support);
    }

    /// Windows: roaming AppData, config and data apart.
    #[cfg(windows)]
    #[test]
    fn windows_uses_roaming_appdata() {
        if std::env::var_os("AC2_CONFIG_DIR").is_some() {
            return;
        }
        let c = config_dir();
        let d = data_dir();
        assert!(c.ends_with("ac2\\config"), "{}", c.display());
        assert!(d.ends_with("ac2\\data"), "{}", d.display());
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temporary_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("sub").join("f.toml");
        write_private_atomic(&p, b"one").expect("write");
        write_private_atomic(&p, b"two").expect("write");
        assert_eq!(fs::read(&p).expect("read"), b"two");
        let names: Vec<_> = fs::read_dir(p.parent().expect("dir"))
            .expect("list")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from("f.toml")]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&p).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
