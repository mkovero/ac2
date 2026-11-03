//! The `ac2d` binary as a process a parent starts: one per user on the default local
//! endpoints (an instance lock; a second exits 3 and touches nothing), explicit endpoints
//! that are another daemon by intent, `--print-endpoints`, `--stop-on-stdin-eof` (the
//! parent's pipe closing stops it in order), `--ephemeral` (nothing kept), and a hard kill
//! that neither blocks the next start nor costs the last good autosave.
//!
//! Every run is on the fake backend with its runtime, config, data and log directories in
//! a temp dir of its own, so no test reaches the user's daemon, stores or audio.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command as Process, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use ac2_proto::model::{ImportFormat, ImportRole, TraceMeta, TraceOwner};
use ac2_proto::units::Blob;
use ac2_proto::{Command, ReplyBody};
use ac2_zmq::Context;
use ac2d::instance::{Instance, exit};
use common::Client;

const T: Duration = Duration::from_secs(10);
/// The daemon's wait after the last edit of a burst before it writes the autosave.
const DEBOUNCE: Duration = Duration::from_millis(1500);
/// Loopback TCP on a free port: explicit endpoints, so no lock and no pid file.
const ANY_PORT: &str = "tcp://127.0.0.1:0";

/// A private home for one daemon run: every directory the daemon may touch.
struct Home {
    root: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self, sub: &str) -> PathBuf {
        self.root.path().join(sub)
    }

    fn runtime(&self) -> PathBuf {
        self.path("runtime")
    }

    fn logs(&self) -> PathBuf {
        self.path("logs")
    }

    fn autosave(&self) -> PathBuf {
        self.path("data/autosave")
    }

    /// `ac2d --backend fake` with every per-user directory inside this home: the
    /// variables `ac2_paths` and the runtime dir follow, and on Linux the XDG ones the
    /// platform dirs come from.
    fn ac2d(&self) -> Process {
        let mut p = Process::new(env!("CARGO_BIN_EXE_ac2d"));
        p.arg("--backend")
            .arg("fake")
            .env("AC2_RUNTIME_DIR", self.runtime())
            .env("AC2_CONFIG_DIR", self.path("config"))
            .env("AC2_LOG_DIR", self.logs())
            .env("AC2_SESSION_DIR", self.path("data/sessions"))
            .env("HOME", self.path("home"))
            .env("XDG_CONFIG_HOME", self.path("xdg/config"))
            .env("XDG_DATA_HOME", self.path("xdg/data"))
            .env("XDG_STATE_HOME", self.path("xdg/state"))
            .env("XDG_RUNTIME_DIR", self.path("xdg/runtime"))
            .env("RUST_LOG", "warn,ac2=info")
            .env_remove("JOURNAL_STREAM")
            .env_remove("NOTIFY_SOCKET")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        p
    }

    /// [`Self::ac2d`] keeping its stores in this home's data dir, as `ac2d` with no
    /// store flags keeps them in the user's.
    fn ac2d_with_stores(&self) -> Process {
        let mut p = self.ac2d();
        p.arg("--settings")
            .arg(self.path("config/rig.json"))
            .arg("--cal-store")
            .arg(self.path("config/calibrations.json"))
            .arg("--plans")
            .arg(self.path("data/plans"))
            .arg("--rig-logs")
            .arg(self.path("data/rig-logs"))
            .arg("--recordings")
            .arg(self.path("data/recordings"))
            .arg("--autosave")
            .arg(self.autosave());
        p
    }

    /// Every file under this home, relative to it.
    fn files(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, root, out);
                } else {
                    out.push(p.strip_prefix(root).unwrap().to_owned());
                }
            }
        }
        let mut out = Vec::new();
        walk(self.root.path(), self.root.path(), &mut out);
        out.sort();
        out
    }
}

/// A running `ac2d`, killed if a test fails before it ends.
struct Daemon {
    child: Child,
}

impl Daemon {
    fn spawn(mut p: Process) -> Self {
        Self {
            child: p.spawn().unwrap(),
        }
    }

    /// Spawns with `--print-endpoints` and reads the line: (ctrl, data).
    fn spawn_printing(mut p: Process) -> (Self, String, String) {
        p.arg("--print-endpoints").stdout(Stdio::piped());
        let mut d = Self::spawn(p);
        let mut line = String::new();
        BufReader::new(d.child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let rest = line
            .trim_end()
            .strip_prefix("ac2d: listening ctrl=")
            .unwrap_or_else(|| panic!("not the endpoints line: {line:?}"));
        let (ctrl, data) = rest.split_once(" data=").unwrap();
        (d, ctrl.to_owned(), data.to_owned())
    }

    /// Waits up to `limit` for the process to end.
    fn wait(&mut self, limit: Duration) -> ExitStatus {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(st) = self.child.try_wait().unwrap() {
                return st;
            }
            assert!(
                Instant::now() < deadline,
                "ac2d still running after {limit:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn stdin(&mut self) -> ChildStdin {
        self.child.stdin.take().unwrap()
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.kill();
    }
}

fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + T;
    while !ok() {
        assert!(Instant::now() < deadline, "no {what} within {T:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn answers(ctx: &Context, ctrl: &str) -> bool {
    let mut c = Client::connect(ctx, ctrl);
    matches!(c.ok(Command::StateSnapshot), ReplyBody::Snapshot(_))
}

fn import(c: &mut Client, name: &str) -> TraceMeta {
    let content = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../ac2-traces/tests/fixtures/rew_export.txt"),
    )
    .unwrap();
    match c.ok(Command::TraceImport {
        file_name: format!("{name}.txt"),
        format: ImportFormat::Auto,
        role: ImportRole::Trace,
        ir: None,
        content: Blob(content),
        owner: TraceOwner::Imported,
    }) {
        ReplyBody::Trace(t) => t,
        other => panic!("{other:?}"),
    }
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[cfg(unix)]
fn ipc_ctrl(home: &Home) -> String {
    format!("ipc://{}", home.runtime().join("ctrl.sock").display())
}

/// A held instance lock: the daemon on the default endpoints exits 3 at once, before it
/// rotates the log, writes the pid file or binds a socket.
#[test]
fn a_held_lock_refuses_a_second_daemon() {
    let home = Home::new();
    let _held = Instance::lock(&home.runtime()).unwrap();
    std::fs::create_dir_all(home.logs()).unwrap();
    std::fs::write(home.logs().join("ac2d.log"), "the running daemon's log\n").unwrap();
    std::fs::write(home.runtime().join("ac2d.pid"), "4242\n").unwrap();
    let mut p = home.ac2d_with_stores();
    p.stderr(Stdio::piped());
    let mut d = Daemon::spawn(p);
    assert_eq!(d.wait(T).code(), Some(i32::from(exit::ANOTHER_RUNNING)));
    let mut err = String::new();
    std::io::Read::read_to_string(&mut d.child.stderr.take().unwrap(), &mut err).unwrap();
    assert!(
        err.contains("another ac2d is running for this user"),
        "{err}"
    );
    assert_eq!(
        read(&home.logs().join("ac2d.log")),
        "the running daemon's log\n"
    );
    assert!(!home.logs().join("ac2d.log.1").exists());
    assert_eq!(read(&home.runtime().join("ac2d.pid")), "4242\n");
    assert!(!home.runtime().join("ctrl.sock").exists());
    assert!(!home.path("data").exists(), "{:?}", home.files());
}

/// Without the lock, libzmq would let a second daemon unlink and take over `ctrl.sock`
/// while the first keeps the audio interface: the second exits 3 and the first keeps
/// serving on its socket, its pid file untouched.
#[cfg(unix)]
#[test]
fn a_second_daemon_leaves_the_first_serving() {
    let home = Home::new();
    let first = Daemon::spawn(home.ac2d_with_stores());
    let pid_file = home.runtime().join("ac2d.pid");
    wait_for("pid file", || pid_file.exists());
    let pid = format!("{}\n", first.child.id());
    assert_eq!(read(&pid_file), pid);
    let ctx = Context::new().unwrap();
    assert!(answers(&ctx, &ipc_ctrl(&home)));
    let mut second = Daemon::spawn(home.ac2d_with_stores());
    assert_eq!(
        second.wait(T).code(),
        Some(i32::from(exit::ANOTHER_RUNNING))
    );
    assert!(answers(&ctx, &ipc_ctrl(&home)));
    assert_eq!(read(&pid_file), pid);
}

/// `kill -9` leaves `ctrl.sock` and the pid file behind, but the OS drops the lock with
/// the process: the next daemon starts, serves and writes its own pid; stopped in order it
/// removes the pid file.
#[cfg(unix)]
#[test]
fn a_hard_killed_daemon_does_not_block_the_next() {
    let home = Home::new();
    let pid_file = home.runtime().join("ac2d.pid");
    let mut first = Daemon::spawn(home.ac2d_with_stores());
    wait_for("pid file", || pid_file.exists());
    first.kill();
    assert!(home.runtime().join("ctrl.sock").exists());
    assert!(pid_file.exists());
    let mut p = home.ac2d_with_stores();
    p.arg("--stop-on-stdin-eof").stdin(Stdio::piped());
    let mut next = Daemon::spawn(p);
    let pid = format!("{}\n", next.child.id());
    wait_for("the next daemon's pid", || {
        std::fs::read_to_string(&pid_file).is_ok_and(|s| s == pid)
    });
    let ctx = Context::new().unwrap();
    assert!(answers(&ctx, &ipc_ctrl(&home)));
    drop(next.stdin());
    assert!(next.wait(T).success());
    assert!(!pid_file.exists());
}

/// Explicit endpoints are another daemon by intent: it serves with the lock held by
/// someone else and creates neither a lock nor a pid file (the runtime dir stays absent).
#[test]
fn explicit_endpoints_take_no_lock_and_write_no_pid_file() {
    let home = Home::new();
    let explicit = || {
        let mut p = home.ac2d_with_stores();
        p.args(["--ctrl", ANY_PORT, "--data", ANY_PORT]);
        p
    };
    let ctx = Context::new().unwrap();
    let (_alone, ctrl, _) = Daemon::spawn_printing(explicit());
    assert!(answers(&ctx, &ctrl));
    assert!(!home.runtime().exists(), "{:?}", home.files());

    let _held = Instance::lock(&home.runtime()).unwrap();
    let (_beside, ctrl, _) = Daemon::spawn_printing(explicit());
    assert!(answers(&ctx, &ctrl));
    assert!(!home.runtime().join("ac2d.pid").exists());
}

/// `tcp://127.0.0.1:0` passes local-mode validation and `--print-endpoints` reports the
/// ports the OS chose, where a client reaches the daemon.
#[test]
fn print_endpoints_reports_the_bound_ports() {
    let home = Home::new();
    let mut p = home.ac2d();
    p.args(["--ephemeral", "--ctrl", ANY_PORT, "--data", ANY_PORT]);
    let (_d, ctrl, data) = Daemon::spawn_printing(p);
    for e in [&ctrl, &data] {
        let port: u16 = e
            .strip_prefix("tcp://127.0.0.1:")
            .unwrap_or_else(|| panic!("{e}"))
            .parse()
            .unwrap();
        assert_ne!(port, 0, "{e}");
    }
    assert_ne!(ctrl, data);
    assert!(answers(&Context::new().unwrap(), &ctrl));
}

/// The parent's pipe closing stops the daemon through its orderly shutdown: exit 0, and
/// an edit made just before (inside the autosave's debounce) is in the autosave.
#[test]
fn stdin_eof_stops_in_order_with_the_autosave_written() {
    let home = Home::new();
    let mut p = home.ac2d_with_stores();
    p.args([
        "--ctrl",
        ANY_PORT,
        "--data",
        ANY_PORT,
        "--stop-on-stdin-eof",
    ])
    .stdin(Stdio::piped());
    let (mut d, ctrl, _) = Daemon::spawn_printing(p);
    let stdin = d.stdin();
    let ctx = Context::new().unwrap();
    let mut c = Client::connect(&ctx, &ctrl);
    let edited = Instant::now();
    import(&mut c, "kept");
    drop(stdin);
    assert!(d.wait(T).success());
    assert!(
        edited.elapsed() < DEBOUNCE,
        "the debounced write could have run before the stop"
    );
    let saved = ac2_traces::session::load(&home.autosave()).unwrap();
    assert_eq!(saved.traces.len(), 1);
}

/// A stdin that ends at once was never a parent's pipe: the daemon still stops in order,
/// and the log says at error level that the flag was misused.
#[test]
fn an_early_stdin_eof_is_logged_as_an_error() {
    let home = Home::new();
    let mut p = home.ac2d();
    p.args([
        "--ephemeral",
        "--ctrl",
        ANY_PORT,
        "--data",
        ANY_PORT,
        "--stop-on-stdin-eof",
    ]);
    let mut d = Daemon::spawn(p);
    assert!(d.wait(T).success());
    let log = read(&home.logs().join("ac2d-sim.log"));
    let line = log
        .lines()
        .find(|l| l.contains("--stop-on-stdin-eof"))
        .unwrap_or_else(|| panic!("{log}"));
    assert!(line.contains("ERROR"), "{line}");
}

/// `--ephemeral` keeps nothing: after an edit, a wait past the autosave's debounce and an
/// orderly stop, the only file in the whole home is its log, `ac2d-sim.log`.
#[test]
fn ephemeral_writes_nothing_but_its_log() {
    let home = Home::new();
    let mut p = home.ac2d();
    p.args([
        "--ephemeral",
        "--ctrl",
        ANY_PORT,
        "--data",
        ANY_PORT,
        "--stop-on-stdin-eof",
    ])
    .stdin(Stdio::piped());
    let (mut d, ctrl, _) = Daemon::spawn_printing(p);
    let stdin = d.stdin();
    let mut c = Client::connect(&Context::new().unwrap(), &ctrl);
    import(&mut c, "forgotten");
    std::thread::sleep(DEBOUNCE + Duration::from_millis(500));
    drop(stdin);
    assert!(d.wait(T).success());
    assert_eq!(home.files(), vec![PathBuf::from("logs/ac2d-sim.log")]);
}

/// A hard kill at instants around the debounced write (traces first, then the manifest
/// renamed into place) leaves an autosave that loads, with at least the edits of the write
/// before. The write takes milliseconds, so the kills sample the window rather than
/// promise to land inside it.
#[test]
fn slow_a_kill_around_the_autosave_write_leaves_a_loadable_autosave() {
    for offset_ms in [0u64, 5, 10, 20, 40] {
        let home = Home::new();
        let mut p = home.ac2d_with_stores();
        p.args(["--ctrl", ANY_PORT, "--data", ANY_PORT]);
        let (mut d, ctrl, _) = Daemon::spawn_printing(p);
        let mut c = Client::connect(&Context::new().unwrap(), &ctrl);
        import(&mut c, "first");
        let manifest = home.autosave().join("session.json");
        wait_for("first autosave", || manifest.exists());
        for i in 0..4 {
            import(&mut c, &format!("more {i}"));
        }
        std::thread::sleep(DEBOUNCE + Duration::from_millis(offset_ms));
        d.kill();
        let saved = ac2_traces::session::load(&home.autosave())
            .unwrap_or_else(|e| panic!("killed {offset_ms} ms into the write: {e}"));
        assert!(
            matches!(saved.traces.len(), 1 | 5),
            "killed {offset_ms} ms into the write: {} traces",
            saved.traces.len()
        );
    }
}
