//! `daemon start|stop|status`, `status`.
//!
//! Staleness is decided by build id: the daemon reports `… (build <id>)` in
//! `welcome.server`; a local daemon whose id differs from this binary's is stale (an older
//! `ac2d` still running after an upgrade). File modification times are never consulted.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use ac2_client::{Client, Retry, expect_body};
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::{connect, is_local, target};
use crate::args::{Cli, DaemonCmd};
use crate::output::{self, Out};
use crate::{BUILD_ID, CliError};

/// The build id in a `welcome.server` string such as `ac2d 0.1.0 (build 0.1.0+1a2b3c4d5e6f)`.
pub fn parse_build_id(server: &str) -> Option<&str> {
    let rest = &server[server.find("(build ")? + "(build ".len()..];
    let id = &rest[..rest.find(')')?];
    let id = id.trim();
    (!id.is_empty() && !id.contains(char::is_whitespace)).then_some(id)
}

pub(crate) async fn run(cli: &Cli, cmd: &DaemonCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        DaemonCmd::Status => status(cli, out).await,
        DaemonCmd::Start => start(cli, out).await,
        DaemonCmd::Stop => stop(cli, out).await,
    }
}

fn status_json(
    c: &Client,
    session: &ac2_proto::model::Session,
    st: &ac2_proto::model::State,
    local: bool,
) -> serde_json::Value {
    let w = c.welcome();
    let build = parse_build_id(&w.server);
    let matches = build.map(|b| b == BUILD_ID);
    json!({
        "server": w.server,
        "build_id": build,
        "client_build_id": BUILD_ID,
        "build_matches": matches,
        // Only a local daemon can be "stale": a remote one is simply another build.
        "stale": if local { matches.map(|m| !m) } else { None },
        "client_id": w.client_id,
        "daemon_incarnation": format!("{:016x}", w.daemon_incarnation.0),
        "session_epoch": w.session_epoch,
        "rev": w.rev,
        "session": session,
        "timing": st.timing,
        "autosave": st.autosave,
        "inputs": output::inputs_json(st),
    })
}

fn status_text(
    c: &Client,
    session: &ac2_proto::model::Session,
    st: &ac2_proto::model::State,
    local: bool,
) -> String {
    let w = c.welcome();
    let mut s = format!("daemon       {}", w.server);
    match parse_build_id(&w.server) {
        Some(b) if b != BUILD_ID && local => s.push_str(&format!(
            "\nSTALE        daemon build {b}, this ac2 is {BUILD_ID}; restart it: `ac2 daemon stop && ac2 daemon start`"
        )),
        Some(b) if b != BUILD_ID => {
            s.push_str(&format!("\nbuild        {b} (this ac2: {BUILD_ID})"))
        }
        Some(_) => {}
        None => s.push_str("\nbuild        unknown (the daemon did not report a build id)"),
    }
    s.push_str(&format!(
        "\nclient id    {}\nincarnation  {:016x}   epoch {}   rev {}\n{}",
        w.client_id.0,
        w.daemon_incarnation.0,
        w.session_epoch,
        w.rev,
        output::session(session)
    ));
    if let Some(stopped) = &session.stopped {
        s.push('\n');
        s.push_str(
            &ac2_scene::audio::audio_stopped_text(
                stopped,
                crate::watch::now_wall(),
                output::local_offset_s,
            )
            .status,
        );
    }
    if let Some(c) = output::clock(session, &st.timing) {
        s.push('\n');
        s.push_str(&c);
    }
    s.push('\n');
    s.push_str(&output::autosave(&st.autosave, crate::watch::now_wall()));
    let inputs = output::inputs(st, crate::watch::now_wall());
    if !st.inputs.is_empty() || session.open.is_some() {
        s.push_str("\ninputs\n");
        s.push_str(&inputs);
    }
    s
}

pub(crate) async fn status(cli: &Cli, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let r = c.call(Command::SessionStatus).await?;
    let session = expect_body!("session.status", r, ReplyBody::Session(s) => s)?;
    let st = super::state(&c).await?;
    let local = is_local(cli);
    out.emit(&status_json(&c, &session, &st, local), || {
        status_text(&c, &session, &st, local)
    })?;
    Ok(())
}

/// A quick probe: hello with a short deadline.
async fn probe(cli: &Cli) -> Option<Client> {
    #[cfg(unix)]
    if !ac2_client::endpoint::runtime_dir()
        .join("ctrl.sock")
        .exists()
    {
        return None;
    }
    let (mut cfg, _) = target(cli).ok()?;
    cfg.mirror = false;
    cfg.retry = Retry {
        timeout: Duration::from_millis(300),
        retries: 1,
    };
    Client::connect(cfg).await.ok()
}

fn local_only(cli: &Cli, what: &str) -> Result<(), CliError> {
    if is_local(cli) {
        Ok(())
    } else {
        Err(CliError::Usage(format!(
            "`daemon {what}` manages the local daemon only"
        )))
    }
}

fn exe_name() -> String {
    format!("ac2d{}", std::env::consts::EXE_SUFFIX)
}

/// `ac2d` next to this binary, else the first on PATH.
pub fn find_ac2d() -> Option<PathBuf> {
    let name = exe_name();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
    {
        let p = dir.join(&name);
        if p.is_file() {
            return Some(p);
        }
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(&name))
        .find(|p| p.is_file())
}

async fn start(cli: &Cli, out: &mut Out<'_>) -> Result<(), CliError> {
    local_only(cli, "start")?;
    if let Some(c) = probe(cli).await {
        let r = c.call(Command::SessionStatus).await?;
        let session = expect_body!("session.status", r, ReplyBody::Session(s) => s)?;
        let st = super::state(&c).await?;
        let mut j = status_json(&c, &session, &st, true);
        j["started"] = json!(false);
        j["already_running"] = json!(true);
        out.emit(&j, || {
            format!("already running\n{}", status_text(&c, &session, &st, true))
        })?;
        return Ok(());
    }
    let exe = find_ac2d().ok_or_else(|| {
        CliError::Usage(format!(
            "{} not found next to this binary or on PATH",
            exe_name()
        ))
    })?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own process group: Ctrl-C in this terminal must not reach the daemon.
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| CliError::Usage(format!("cannot start {}: {e}", exe.display())))?;
    let pid = child.id();
    let deadline = Instant::now() + Duration::from_secs(10);
    let c = loop {
        if let Ok(Some(st)) = child.try_wait() {
            return Err(CliError::NotRunning(format!(
                "{} exited at start ({st}); run it in a terminal to see why",
                exe.display()
            )));
        }
        if let Some(c) = probe(cli).await {
            break c;
        }
        if Instant::now() > deadline {
            return Err(CliError::NotRunning(format!(
                "{} (pid {pid}) did not answer within 10 s",
                exe.display()
            )));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let r = c.call(Command::SessionStatus).await?;
    let session = expect_body!("session.status", r, ReplyBody::Session(s) => s)?;
    let st = super::state(&c).await?;
    let mut j = status_json(&c, &session, &st, true);
    j["started"] = json!(true);
    j["pid"] = json!(pid);
    j["exe"] = json!(exe);
    out.emit(&j, || {
        format!(
            "started {} (pid {pid})\n{}",
            exe.display(),
            status_text(&c, &session, &st, true)
        )
    })?;
    Ok(())
}

fn signal_terminate(pid: u32) -> Result<(), CliError> {
    #[cfg(unix)]
    let st = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status();
    // A detached console process has no window to receive taskkill's close request, so
    // Windows can only end it forcefully; `stop` has already faded the output out and
    // closed the session through the daemon itself.
    #[cfg(windows)]
    let st = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .status();
    match st {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(CliError::Usage(format!("cannot signal pid {pid} ({s})"))),
        Err(e) => Err(CliError::Usage(format!("cannot signal pid {pid}: {e}"))),
    }
}

async fn stop(cli: &Cli, out: &mut Out<'_>) -> Result<(), CliError> {
    local_only(cli, "stop")?;
    let pid_file = ac2_client::endpoint::pid_file();
    let pid = match std::fs::read_to_string(&pid_file) {
        Ok(t) => t
            .trim()
            .parse::<u32>()
            .map_err(|_| CliError::Usage(format!("{}: not a pid", pid_file.display())))?,
        Err(_) => {
            if probe(cli).await.is_some() {
                return Err(CliError::Usage(format!(
                    "the daemon answers but wrote no {}",
                    pid_file.display()
                )));
            }
            out.emit(&json!({ "stopped": false, "running": false }), || {
                "not running".to_owned()
            })?;
            return Ok(());
        }
    };
    #[cfg(windows)]
    if let Some(c) = probe(cli).await {
        // The forced end below skips the daemon's own shutdown, so do its safety part
        // first: stop (fade out) any stimulus, then close the stream.
        let _ = c.call(Command::GenStop).await;
        let _ = c.call(Command::SessionClose).await;
    }
    signal_terminate(pid)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while probe(cli).await.is_some() {
        if Instant::now() > deadline {
            return Err(CliError::Usage(format!(
                "pid {pid} still answers 10 s after SIGTERM"
            )));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    out.emit(&json!({ "stopped": true, "pid": pid }), || {
        format!("stopped (pid {pid})")
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_ids() {
        assert_eq!(
            parse_build_id("ac2d 0.1.0 (build 0.1.0+1a2b3c4d5e6f)"),
            Some("0.1.0+1a2b3c4d5e6f")
        );
        assert_eq!(parse_build_id("ac2d 0.0.0 (build fake)"), Some("fake"));
        for s in ["ac2d 0.1.0", "(build )", "(build a b)", "(build x", ""] {
            assert_eq!(parse_build_id(s), None, "{s}");
        }
    }
}
