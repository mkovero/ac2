//! This computer's `ac2d`, started by the app: found beside the app's executable, started
//! when none serves, owned through its stdin pipe, stopped in order, watched while it runs.
//!
//! - **Which binary**: the `ac2d` beside this executable only, never one on `PATH`: the wire
//!   has no compatibility between versions, so another build must not be picked up.
//! - **One per user**: every `ac2d` but an ephemeral one takes an instance lock; one that
//!   loses it exits [`ANOTHER_RUNNING`] and the app reaches the holder (its endpoints file,
//!   [`ac2_client::endpoint::LockHolder`]) without owning it.
//! - **Ownership**: an owned child holds the read end of a pipe the app writes nothing to and
//!   runs with `--stop-on-stdin-eof`. Its end (the app dropping it, quitting or crashing on
//!   any OS) stops the daemon through its orderly shutdown: the stimulus faded, the session
//!   closed, the autosave flushed. A child started without (Settings › Connection *Stop
//!   ac2d when the app quits* off) is detached and outlives the app.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ac2_client::Endpoints;
use ac2_proto::model::BackendKind;

/// `ac2d`'s exit code when another `ac2d` runs for this user (it holds the instance lock).
pub const ANOTHER_RUNNING: i32 = 3;

/// How long an owned daemon has to stop once its pipe is dropped before it is killed: its
/// shutdown fades the output and flushes the autosave, which takes well under this.
pub const STOP_WAIT: Duration = Duration::from_secs(10);

/// How long the simulated rig has to say where it listens.
const LISTEN_WAIT: Duration = Duration::from_secs(10);

/// The name `ac2d` has on this OS.
pub fn exe_name() -> String {
    format!("ac2d{}", std::env::consts::EXE_SUFFIX)
}

/// The `ac2d` beside the executable at `app`.
pub fn beside(app: &Path) -> PathBuf {
    app.with_file_name(exe_name())
}

/// The `ac2d` beside this executable, or why there is none (naming where it was looked
/// for).
pub fn find_ac2d() -> Result<PathBuf, String> {
    let app = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    let p = beside(&app);
    if p.is_file() {
        Ok(p)
    } else {
        Err(format!(
            "no ac2d at {}: the app starts the ac2d installed beside it",
            p.display()
        ))
    }
}

/// Inside an AppImage the app's files go away when it quits, the `ac2d` beside it with
/// them: a child that outlived the app would run from a mount that is gone, so the app
/// always stops it.
pub fn stop_forced() -> bool {
    std::env::var_os("APPIMAGE").is_some_and(|v| !v.is_empty())
}

/// The name `ac2d --backend` takes for `b`.
pub const fn backend_arg(b: BackendKind) -> &'static str {
    match b {
        BackendKind::Jack => "jack",
        BackendKind::CoreAudio => "coreaudio",
        BackendKind::Asio => "asio",
        BackendKind::Fake => "fake",
        BackendKind::Replay => "replay",
    }
}

/// Which `ac2d` a child is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// This computer's, on the default local endpoints.
    Local,
    /// The simulated rig: the fake backend, nothing kept, loopback port 0.
    Simulated,
}

/// An `ac2d` the app started.
#[derive(Debug)]
pub struct Spawned {
    child: Child,
    /// The pipe the child stops at the end of; `None`: detached, never stopped by the app.
    stdin: Option<ChildStdin>,
    kind: Kind,
    log: PathBuf,
}

impl Spawned {
    /// The app stops it (it holds the pipe and the child still runs).
    pub fn owned(&self) -> bool {
        self.stdin.is_some()
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Where it logs.
    pub fn log(&self) -> &Path {
        &self.log
    }

    /// Its exit status once it ended; `None` while it runs.
    pub fn exited(&mut self) -> Option<ExitStatus> {
        match self.child.try_wait() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("ac2d (pid {}): cannot wait: {e}", self.child.id());
                None
            }
        }
    }

    /// Drops the pipe: an owned child begins its orderly stop. Waited for with
    /// [`Stopping::finish`].
    pub fn release(mut self) -> Stopping {
        let owned = self.stdin.take().is_some();
        Stopping {
            child: self.child,
            owned,
            since: Instant::now(),
        }
    }
}

/// A child whose pipe was dropped.
#[derive(Debug)]
pub struct Stopping {
    child: Child,
    owned: bool,
    since: Instant,
}

/// How a child given up by the app ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// It stopped on its own after its pipe closed.
    Stopped,
    /// It did not stop in time and was killed.
    Killed,
    /// Detached: left running.
    LeftRunning,
}

impl Stopping {
    /// Waits until `deadline` for it to end, then kills it. A detached child is left alone.
    pub fn finish(mut self, deadline: Instant) -> Stop {
        if !self.owned {
            return Stop::LeftRunning;
        }
        let pid = self.child.id();
        loop {
            match self.child.try_wait() {
                Ok(Some(st)) => {
                    tracing::info!(
                        "ac2d (pid {pid}) stopped ({}) {:.1} s after its pipe closed",
                        exit_text(st),
                        self.since.elapsed().as_secs_f64()
                    );
                    return Stop::Stopped;
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) | Err(_) => break,
            }
        }
        tracing::warn!(
            "ac2d (pid {pid}) did not stop within {:.1} s of its pipe closing: killed",
            deadline.saturating_duration_since(self.since).as_secs_f64()
        );
        let _ = self.child.kill();
        let _ = self.child.wait();
        Stop::Killed
    }

    /// [`Self::finish`] [`STOP_WAIT`] after the pipe was dropped.
    pub fn finish_in_time(self) -> Stop {
        let deadline = self.since + STOP_WAIT;
        self.finish(deadline)
    }
}

/// Stops every owned child at once: all pipes dropped first, then each waited for within
/// [`STOP_WAIT`] of that, so several stop in parallel.
pub fn stop_all(children: Vec<Spawned>) {
    let stopping: Vec<Stopping> = children.into_iter().map(Spawned::release).collect();
    for s in stopping {
        s.finish_in_time();
    }
}

/// Where the children still running when the window closes are handed over, so they stop
/// after the window is gone ([`AtExit::stop`], from `main`).
#[derive(Clone, Debug, Default)]
pub struct AtExit(Arc<Mutex<Vec<Spawned>>>);

impl AtExit {
    pub fn hand_over(&self, children: Vec<Spawned>) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(children);
    }

    /// Stops what was handed over ([`stop_all`]).
    pub fn stop(&self) {
        let v = std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner));
        stop_all(v);
    }
}

/// `exit 3`, `killed by signal 9`.
pub fn exit_text(st: ExitStatus) -> String {
    if let Some(c) = st.code() {
        return format!("exit {c}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(s) = st.signal() {
            return format!("killed by signal {s}");
        }
    }
    st.to_string()
}

/// What a child's end means for the rig it was started for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    /// Another `ac2d` runs for this user: the app reaches that one, not its own.
    LostLock,
    /// It ended before it served: the error, for the connect dialog.
    AtStart(String),
    /// It ended while serving: the notice. The link retries; nothing starts it again, as a
    /// driver fault would loop holding the interface.
    Stopped(String),
}

/// What `st` means for a child that `served` (its rig's link connected once) and logs to
/// `log`.
pub fn ended(st: ExitStatus, served: bool, log: &Path) -> Ended {
    if st.code() == Some(ANOTHER_RUNNING) {
        Ended::LostLock
    } else if served {
        Ended::Stopped(format!(
            "ac2d stopped ({}); its log: {}",
            exit_text(st),
            log.display()
        ))
    } else {
        Ended::AtStart(format!(
            "ac2d exited at start ({}); its log: {}",
            exit_text(st),
            log.display()
        ))
    }
}

/// The command for `exe` with nothing inherited the daemon must not see.
fn command(exe: &Path) -> Command {
    let mut cmd = Command::new(exe);
    // With stderr null, a journal stream inherited from an app started by a unit would make
    // the daemon skip its own log file and log nowhere.
    cmd.env_remove("JOURNAL_STREAM").stderr(Stdio::null());
    cmd
}

/// An owned child: the pipe on stdin, its own process group on Unix (a terminal's Ctrl-C
/// reaches the app, whose end then ends the child), and its own hidden console on Windows:
/// both programs are console programs, and a child sharing the app's console would be
/// killed with it on `CTRL_CLOSE_EVENT` before any orderly stop.
fn own(cmd: &mut Command) {
    cmd.arg("--stop-on-stdin-eof").stdin(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

/// A detached child, as `ac2 daemon start` starts one: it outlives the app.
fn detach(cmd: &mut Command) {
    cmd.stdin(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
}

fn start(mut cmd: Command, exe: &Path, kind: Kind, log: PathBuf) -> Result<Spawned, String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
    let stdin = child.stdin.take();
    tracing::info!(
        "started {} (pid {}, {})",
        exe.display(),
        child.id(),
        if stdin.is_some() {
            "stops with the app"
        } else {
            "keeps running"
        }
    );
    Ok(Spawned {
        child,
        stdin,
        kind,
        log,
    })
}

/// Starts this computer's `ac2d` at `exe` on its platform backend (or `backend`), owned
/// when `owned`.
pub fn spawn_local(
    exe: &Path,
    backend: Option<BackendKind>,
    owned: bool,
) -> Result<Spawned, String> {
    let mut cmd = command(exe);
    if let Some(b) = backend {
        cmd.args(["--backend", backend_arg(b)]);
    }
    cmd.stdout(Stdio::null());
    if owned {
        own(&mut cmd);
    } else {
        detach(&mut cmd);
    }
    start(cmd, exe, Kind::Local, ac2_paths::log_dir().join("ac2d.log"))
}

/// Starts the simulated rig at `exe` (always owned: it holds no interface and its
/// measurements live only in it) and reads where it listens from its stdout.
pub fn spawn_simulated(exe: &Path) -> Result<(Spawned, Endpoints), String> {
    let mut cmd = command(exe);
    cmd.args([
        "--backend",
        "fake",
        "--ephemeral",
        "--name",
        "Simulated rig",
        "--ctrl",
        "tcp://127.0.0.1:0",
        "--data",
        "tcp://127.0.0.1:0",
        "--print-endpoints",
    ])
    .stdout(Stdio::piped());
    own(&mut cmd);
    let mut s = start(
        cmd,
        exe,
        Kind::Simulated,
        ac2_paths::log_dir().join("ac2d-sim.log"),
    )?;
    let stdout = s.child.stdout.take();
    match stdout.map(read_endpoints) {
        Some(Ok(ep)) => Ok((s, ep)),
        Some(Err(e)) => {
            let log = s.log.clone();
            let _ = s.child.kill();
            let _ = s.child.wait();
            Err(format!("simulated rig: {e}; its log: {}", log.display()))
        }
        None => Err("simulated rig: no stdout".into()),
    }
}

/// The endpoints line from `out`, within [`LISTEN_WAIT`]. The rest is read to its end on
/// the same thread, so a later line never finds the pipe closed.
fn read_endpoints(out: impl std::io::Read + Send + 'static) -> Result<Endpoints, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("ac2-ui ac2d stdout".into())
        .spawn(move || {
            let mut tx = Some(tx);
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if let Some(ep) = ac2_client::endpoint::parse_listening(&line)
                    && let Some(tx) = tx.take()
                {
                    let _ = tx.send(ep);
                }
            }
        })
        .map_err(|e| e.to_string())?;
    match rx.recv_timeout(LISTEN_WAIT) {
        Ok(ep) => Ok(ep),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "did not say where it listens within {} s",
            LISTEN_WAIT.as_secs()
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err("ended before it listened".into())
        }
    }
}

#[cfg(test)]
mod tests;
