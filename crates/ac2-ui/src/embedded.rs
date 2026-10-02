//! Embedded daemon: the desktop app hosting `ac2d` in-process (PLAN §4.1) so a laptop needs
//! no separate daemon. The UI talks to it through the same client and protocol as to any
//! other daemon; only the endpoints differ.
//!
//! Transport: the client opens its own ZeroMQ context, so `inproc://` (which needs the
//! daemon's context) is not used. On Unix the daemon listens on `ipc://` sockets in a
//! private per-process directory (mode 0700, removed on drop) — the same user boundary as
//! the local daemon, nothing on the network. On Windows it listens on `tcp://127.0.0.1`
//! with OS-assigned ports, like the local daemon there. The audio backend is always named
//! by the caller; the fake rig runs only when chosen.

use std::fmt;

use ac2_client::Endpoints;

/// Audio backend of the embedded daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedBackend {
    /// The platform audio API.
    Cpal,
    /// JACK (Linux, feature `jack`).
    Jack,
    /// The simulated rig of `ac2d --backend fake`; never real audio.
    Fake,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddedError {
    /// This build has no daemon to embed.
    Unavailable,
    /// The backend cannot be built.
    Backend(String),
    /// The daemon did not start.
    Start(String),
}

impl fmt::Display for EmbeddedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmbeddedError::Unavailable => f.write_str(
                "this build has no embedded daemon (feature `embedded`); start ac2d separately",
            ),
            EmbeddedError::Backend(e) => write!(f, "audio backend: {e}"),
            EmbeddedError::Start(e) => write!(f, "daemon: {e}"),
        }
    }
}

impl std::error::Error for EmbeddedError {}

/// A running embedded daemon. Dropping it shuts the daemon down (output faded, stream
/// closed) and removes its socket directory.
pub struct Embedded {
    endpoints: Endpoints,
    backend: EmbeddedBackend,
    #[cfg(feature = "embedded")]
    handle: Option<ac2d::Handle>,
    #[cfg(feature = "embedded")]
    dir: Option<std::path::PathBuf>,
}

impl fmt::Debug for Embedded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Embedded")
            .field("endpoints", &self.endpoints)
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

impl Embedded {
    /// Where the daemon listens.
    pub fn endpoints(&self) -> Endpoints {
        self.endpoints.clone()
    }

    /// `embedded daemon (fake rig)`, for the top bar.
    pub fn describe(&self) -> String {
        match self.backend {
            EmbeddedBackend::Cpal => "embedded daemon (cpal)".into(),
            EmbeddedBackend::Jack => "embedded daemon (jack)".into(),
            EmbeddedBackend::Fake => "embedded daemon (fake rig)".into(),
        }
    }
}

#[cfg(feature = "embedded")]
impl Drop for Embedded {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.shutdown();
        }
        if let Some(d) = self.dir.take() {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

/// Starts the embedded daemon on `backend`.
#[cfg(feature = "embedded")]
pub fn start_embedded(backend: EmbeddedBackend) -> Result<Embedded, EmbeddedError> {
    use ac2d::{BackendChoice, Daemon, DaemonConfig};
    let choice = match backend {
        EmbeddedBackend::Cpal => BackendChoice::Cpal,
        EmbeddedBackend::Jack => BackendChoice::Jack,
        EmbeddedBackend::Fake => BackendChoice::Fake,
    };
    let audio = ac2d::backend(choice).map_err(EmbeddedError::Backend)?;
    let (listen, dir) = listen();
    // The same global maximum as a stand-alone `ac2d` without `--max-level`.
    let mut config = DaemonConfig::new(audio, listen, -10.0);
    // Calibrations of real devices persist in the same store a stand-alone `ac2d` uses; a
    // simulated rig's stay in memory.
    config.cal_store = (backend != EmbeddedBackend::Fake).then(ac2d::default_cal_store);
    let handle = Daemon::start(config).map_err(|e| {
        if let Some(d) = &dir {
            let _ = std::fs::remove_dir_all(d);
        }
        EmbeddedError::Start(e.to_string())
    })?;
    let endpoints = Endpoints {
        ctrl: handle.ctrl_endpoint().to_owned(),
        data: handle.data_endpoint().to_owned(),
    };
    Ok(Embedded {
        endpoints,
        backend,
        handle: Some(handle),
        dir,
    })
}

#[cfg(all(feature = "embedded", unix))]
fn listen() -> (ac2d::Listen, Option<std::path::PathBuf>) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let dir = ac2d::runtime_dir().join(format!(
        "embedded-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let listen = ac2d::Listen::Local {
        ctrl: format!("ipc://{}", dir.join("ctrl.sock").display()),
        data: format!("ipc://{}", dir.join("data.sock").display()),
    };
    (listen, Some(dir))
}

#[cfg(all(feature = "embedded", not(unix)))]
fn listen() -> (ac2d::Listen, Option<std::path::PathBuf>) {
    let listen = ac2d::Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    (listen, None)
}

/// Starts the embedded daemon on `backend`.
#[cfg(not(feature = "embedded"))]
pub fn start_embedded(backend: EmbeddedBackend) -> Result<Embedded, EmbeddedError> {
    let _ = backend;
    Err(EmbeddedError::Unavailable)
}
