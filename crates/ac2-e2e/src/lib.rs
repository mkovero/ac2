//! End-to-end tests of the app (`tests/it/`): the reducer and the link thread driven as the
//! app drives them, against real daemons on the simulated rig (never real audio). The app
//! starts `ac2d` as a process; here the daemon runs in the test process instead, on the
//! transport the app uses for the simulated rig (loopback TCP), so the tests need no binary.
#![forbid(unsafe_code)]

use ac2_client::{ClientConfig, Endpoints};
pub use ac2_ui::sim::Setup;

/// A daemon on the fake backend that keeps nothing, as `ac2d --backend fake --ephemeral`
/// runs it. Dropping it shuts it down.
pub struct SimDaemon {
    endpoints: Endpoints,
    handle: Option<ac2d::Handle>,
    /// The session directory it saves into unless `edit` gave it another; removed with it.
    _sessions: tempfile::TempDir,
}

impl SimDaemon {
    /// Settings for a client of this daemon named `name`.
    pub fn client_config(&self, name: impl Into<String>) -> ClientConfig {
        ClientConfig::new(self.endpoints.clone(), name)
    }

    /// Where it listens.
    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    /// The running daemon (its network status, its server key).
    pub fn handle(&self) -> Option<&ac2d::Handle> {
        self.handle.as_ref()
    }

    /// As the app describes the simulated rig's link.
    pub fn describe(&self) -> String {
        "simulated rig".into()
    }

    /// Shuts the daemon down now (as dropping it does).
    pub fn shutdown(mut self) {
        if let Some(h) = self.handle.take() {
            h.shutdown();
        }
    }
}

impl std::fmt::Debug for SimDaemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimDaemon")
            .field("endpoints", &self.endpoints)
            .finish_non_exhaustive()
    }
}

impl Drop for SimDaemon {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.shutdown();
        }
    }
}

/// The local pair of a daemon of a test, in `dir`: ipc, as every local pair is (a socket
/// directory closed to other users, made by the daemon).
pub fn local_pair(dir: &std::path::Path) -> ac2d::Listen {
    let ep = Endpoints::local_in(&dir.join("run"));
    ac2d::Listen::Local {
        ctrl: ep.ctrl,
        data: ep.data,
    }
}

/// Starts a simulated rig on an ipc local pair of its own with `setup` (the demo the app sets up, or no
/// session).
pub fn sim_daemon(setup: Setup) -> Result<SimDaemon, Box<dyn std::error::Error>> {
    sim_daemon_with(setup, |_| {})
}

/// [`sim_daemon`] with its configuration changed by `edit` before it starts: another name,
/// an identity kept in a file (a rig that keeps its id across a restart), a session
/// directory of the test's own (two rigs opening each other's sessions).
pub fn sim_daemon_with(
    setup: Setup,
    edit: impl FnOnce(&mut ac2d::DaemonConfig),
) -> Result<SimDaemon, Box<dyn std::error::Error>> {
    // A test that saves a session must not leave it in the platform directory, where the
    // next run (and the operator's own app) would find it; the sockets go beside it.
    let sessions = tempfile::tempdir()?;
    let listen = local_pair(sessions.path());
    let audio = ac2d::backends(ac2d::BackendChoice::Fake)?;
    // The same global maximum as a stand-alone `ac2d` without `--max-level`.
    let mut config = ac2d::DaemonConfig::new(std::sync::Arc::clone(&audio[0]), listen, -10.0);
    config.backends = audio;
    config.name = "Simulated rig".into();
    config.session_dir = sessions.path().to_owned();
    edit(&mut config);
    let handle = ac2d::Daemon::start(config)?;
    let d = SimDaemon {
        endpoints: Endpoints {
            ctrl: handle.ctrl_endpoint().to_owned(),
            data: handle.data_endpoint().to_owned(),
        },
        handle: Some(handle),
        _sessions: sessions,
    };
    if setup == Setup::Demo {
        // Dropping `d` on failure shuts the daemon down again.
        ac2_ui::sim::set_up_demo(d.client_config("ac2-e2e setup"))?;
    }
    Ok(d)
}
