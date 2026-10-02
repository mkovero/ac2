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
//!
//! The simulated rig starts ready to use: its session is opened (in 1–2, out 1, the loopback
//! out 1 → in 1 as the rig is wired) and a transfer measurement "demo" (ref 1 → meas 2) is
//! created and started. A daemon on real audio starts with no session: which interface and
//! channels to open is the operator's choice, made in the session dialog.

use std::fmt;

use ac2_client::Endpoints;
use ac2_proto::model::{
    DeviceSelector, LoopbackRoute, MeasConfig, MeasKind, SessionConfig, TransferConfig,
};

/// What an embedded daemon has when it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setup {
    /// No session: the operator opens one.
    Empty,
    /// The simulated rig's session open and the "demo" measurement running. Fake rig only.
    Demo,
}

/// The session the simulated rig opens: inputs 1–2, output 1, loopback out 1 → in 1.
pub fn demo_session() -> SessionConfig {
    SessionConfig {
        input_device: DeviceSelector::Default,
        output_device: DeviceSelector::Default,
        input_channels: vec![0, 1],
        output_channels: 1,
        sample_rate_hz: None,
        buffer_frames: None,
        loopback: Some(LoopbackRoute {
            output: 0,
            input: 0,
        }),
    }
}

/// The measurement the simulated rig starts: transfer ref 1 → meas 2 named "demo", with the
/// defaults of `ac2 meas new tf`.
pub fn demo_measurement() -> MeasConfig {
    MeasConfig {
        name: "demo".into(),
        kind: MeasKind::Transfer {
            config: TransferConfig::with_inputs(0, 1),
        },
    }
}

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
    /// The simulated rig's session or measurement could not be set up.
    Setup(String),
}

impl fmt::Display for EmbeddedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmbeddedError::Unavailable => f.write_str(
                "this build has no embedded daemon (feature `embedded`); start ac2d separately",
            ),
            EmbeddedError::Backend(e) => write!(f, "audio backend: {e}"),
            EmbeddedError::Start(e) => write!(f, "daemon: {e}"),
            EmbeddedError::Setup(e) => write!(f, "simulated rig setup: {e}"),
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

/// Starts the embedded daemon on `backend`: the simulated rig ready to measure
/// ([`Setup::Demo`]), real audio with no session ([`Setup::Empty`]).
pub fn start_embedded(backend: EmbeddedBackend) -> Result<Embedded, EmbeddedError> {
    let setup = match backend {
        EmbeddedBackend::Fake => Setup::Demo,
        EmbeddedBackend::Cpal | EmbeddedBackend::Jack => Setup::Empty,
    };
    start_embedded_with(backend, setup)
}

/// Starts the embedded daemon on `backend` with `setup`. A demo is refused on real audio:
/// nothing opens a real interface the operator did not choose.
#[cfg(feature = "embedded")]
pub fn start_embedded_with(
    backend: EmbeddedBackend,
    setup: Setup,
) -> Result<Embedded, EmbeddedError> {
    use ac2d::{BackendChoice, Daemon, DaemonConfig};
    if setup == Setup::Demo && backend != EmbeddedBackend::Fake {
        return Err(EmbeddedError::Setup(
            "the demo runs on the simulated rig only".into(),
        ));
    }
    let choice = match backend {
        EmbeddedBackend::Cpal => BackendChoice::Cpal,
        EmbeddedBackend::Jack => BackendChoice::Jack,
        EmbeddedBackend::Fake => BackendChoice::Fake,
    };
    let audio = ac2d::backends(choice).map_err(EmbeddedError::Backend)?;
    let (listen, dir) = listen();
    // The same global maximum as a stand-alone `ac2d` without `--max-level`.
    let mut config = DaemonConfig::new(std::sync::Arc::clone(&audio[0]), listen, -10.0);
    config.backends = audio;
    // Calibrations of real devices persist in the same store a stand-alone `ac2d` uses; a
    // simulated rig's stay in memory.
    config.cal_store = (backend != EmbeddedBackend::Fake).then(ac2_paths::cal_store);
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
    let e = Embedded {
        endpoints,
        backend,
        handle: Some(handle),
        dir,
    };
    if setup == Setup::Demo {
        // Dropping `e` on failure shuts the daemon down again.
        set_up_demo(e.endpoints())?;
    }
    Ok(e)
}

/// Opens the simulated rig's session and starts "demo", as a client of the daemon. Runs on
/// its own thread and runtime, so it works whether or not the caller is inside one.
#[cfg(feature = "embedded")]
fn set_up_demo(endpoints: Endpoints) -> Result<(), EmbeddedError> {
    use ac2_client::{Client, ClientConfig, expect_body};
    use ac2_proto::{Command, ReplyBody};
    const DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);
    let run = move || -> Result<(), String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        rt.block_on(async move {
            let work = async {
                let c =
                    Client::connect(ClientConfig::new(endpoints, "ac2-ui embedded setup")).await?;
                c.call(Command::SessionOpen {
                    config: demo_session(),
                })
                .await?;
                let r = c
                    .call(Command::MeasCreate {
                        config: demo_measurement(),
                    })
                    .await?;
                let m = expect_body!("meas.create", r, ReplyBody::Measurement(m) => m)?;
                c.call(Command::MeasStart { meas: m.id }).await?;
                Ok::<(), ac2_client::ClientError>(())
            };
            match tokio::time::timeout(DEADLINE, work).await {
                Ok(r) => r.map_err(|e| e.to_string()),
                Err(_) => Err("the daemon did not answer".into()),
            }
        })
    };
    std::thread::Builder::new()
        .name("ac2-ui embedded setup".into())
        .spawn(run)
        .map_err(|e| EmbeddedError::Setup(e.to_string()))?
        .join()
        .map_err(|_| EmbeddedError::Setup("setup thread panicked".into()))?
        .map_err(EmbeddedError::Setup)
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

/// Starts the embedded daemon on `backend` with `setup`.
#[cfg(not(feature = "embedded"))]
pub fn start_embedded_with(
    backend: EmbeddedBackend,
    setup: Setup,
) -> Result<Embedded, EmbeddedError> {
    let _ = (backend, setup);
    Err(EmbeddedError::Unavailable)
}
