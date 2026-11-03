//! The ownership rule against a stub child: this test binary run again as
//! [`stub_child`], which acts as the mode in [`STUB`] says (never an `ac2d`, so nothing
//! opens audio).

use super::*;

const STUB: &str = "AC2_UI_STUB_CHILD";

/// The stub's behaviours; without [`STUB`] set it is an ordinary test that does nothing.
#[test]
fn stub_child() {
    let Ok(mode) = std::env::var(STUB) else {
        return;
    };
    match mode.as_str() {
        // An `ac2d` that lost the instance lock.
        "lost-lock" => std::process::exit(ANOTHER_RUNNING),
        // A fatal end.
        "fatal" => std::process::exit(1),
        // `--stop-on-stdin-eof`: stops once the pipe closes.
        "until-eof" => {
            let mut sink = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut sink);
            std::process::exit(0)
        }
        // Hung: never reads its pipe.
        "deaf" => loop {
            std::thread::sleep(Duration::from_secs(1));
        },
        // Runs a while, then ends by itself.
        "brief" => {
            std::thread::sleep(Duration::from_secs(2));
            std::process::exit(0)
        }
        m => panic!("unknown stub mode {m}"),
    }
}

fn stub(mode: &str, owned: bool) -> Spawned {
    let exe = std::env::current_exe().unwrap_or_else(|e| panic!("{e}"));
    let mut cmd = Command::new(&exe);
    cmd.args(["--exact", "spawn::tests::stub_child", "--nocapture"])
        .env(STUB, mode)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if owned {
        cmd.stdin(Stdio::piped());
    } else {
        detach(&mut cmd);
    }
    start(cmd, &exe, Kind::Local, PathBuf::from("/logs/ac2d.log")).unwrap_or_else(|e| panic!("{e}"))
}

fn wait_exit(s: &mut Spawned) -> ExitStatus {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(st) = s.exited() {
            return st;
        }
        assert!(Instant::now() < end, "the stub did not end");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn ac2d_is_looked_for_beside_the_app_only() {
    let app = Path::new("/opt/ac2/bin").join(format!("ac2-ui{}", std::env::consts::EXE_SUFFIX));
    assert_eq!(beside(&app), Path::new("/opt/ac2/bin").join(exe_name()));
}

/// An owned child stops once its pipe is dropped, well before the kill.
#[test]
fn an_owned_child_stops_at_the_end_of_its_pipe() {
    let s = stub("until-eof", true);
    assert!(s.owned());
    let t0 = Instant::now();
    assert_eq!(s.release().finish_in_time(), Stop::Stopped);
    assert!(t0.elapsed() < STOP_WAIT / 2, "{:?}", t0.elapsed());
}

/// One that does not stop is killed at the deadline.
#[test]
fn a_hung_owned_child_is_killed_at_the_deadline() {
    let s = stub("deaf", true);
    let stopping = s.release();
    let t0 = Instant::now();
    assert_eq!(
        stopping.finish(Instant::now() + Duration::from_millis(300)),
        Stop::Killed
    );
    assert!(t0.elapsed() < Duration::from_secs(5));
}

/// A detached child is never stopped by the app.
#[test]
fn a_detached_child_is_left_running() {
    let s = stub("brief", false);
    assert!(!s.owned());
    let t0 = Instant::now();
    assert_eq!(s.release().finish_in_time(), Stop::LeftRunning);
    assert!(t0.elapsed() < Duration::from_secs(1));
}

/// Exit 3 is another daemon serving, whenever it comes; any other end is a failure to start
/// before the rig's link connected, a stop after.
#[test]
fn what_an_end_means() {
    let log = Path::new("/logs/ac2d.log");
    let mut s = stub("lost-lock", false);
    let st = wait_exit(&mut s);
    assert_eq!(ended(st, false, log), Ended::LostLock);
    let mut s = stub("fatal", false);
    let st = wait_exit(&mut s);
    assert_eq!(
        ended(st, false, log),
        Ended::AtStart(format!(
            "ac2d exited at start (exit 1); its log: {}",
            log.display()
        ))
    );
    assert_eq!(
        ended(st, true, log),
        Ended::Stopped(format!("ac2d stopped (exit 1); its log: {}", log.display()))
    );
}

/// The simulated rig's endpoints come from its first `listening` line; the rest of its
/// output is read on, and an output that ends first is an error.
#[test]
fn the_endpoints_line_is_read_from_stdout() {
    let out = b"something else\nac2d: listening ctrl=tcp://127.0.0.1:41000 data=tcp://127.0.0.1:41001\nmore\n";
    assert_eq!(
        read_endpoints(std::io::Cursor::new(out.to_vec())),
        Ok(Endpoints {
            ctrl: "tcp://127.0.0.1:41000".into(),
            data: "tcp://127.0.0.1:41001".into(),
        })
    );
    assert_eq!(
        read_endpoints(std::io::Cursor::new(b"no line\n".to_vec())),
        Err("ended before it listened".into())
    );
}

#[test]
fn backend_names_are_ac2ds() {
    assert_eq!(backend_arg(BackendKind::CoreAudio), "coreaudio");
    assert_eq!(backend_arg(BackendKind::Asio), "asio");
    assert_eq!(backend_arg(BackendKind::Jack), "jack");
}
