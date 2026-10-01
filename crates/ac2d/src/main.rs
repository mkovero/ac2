//! `ac2d`: the ac2 daemon.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use ac2_audio::{Backend, CpalBackend, FakeBackend, FakeConfig};
use ac2d::{Daemon, DaemonConfig, Listen, NetworkSecurity};

const USAGE: &str = "\
usage: ac2d [options]

With no options: local mode (ipc in the runtime dir; loopback TCP on Windows), cpal backend.

  --backend <name>       audio backend: cpal (default), jack, or fake (a simulated device;
                         only when named here, never as a fallback)
  --listen <tcp://iface[:port]>
                         network mode: ctrl on port (default 47820), data on port+1,
                         CURVE on both
  --ctrl <endpoint>      local ctrl endpoint (ipc:// or tcp://127.0.0.1:port)
  --data <endpoint>      local data endpoint
  --key-file <path>      server key pair (network mode; generated if missing)
  --authorized <path>    authorized clients (network mode; created empty if missing)
  --max-level <dBFS>     global generator maximum, dBFS RMS (default -10)
  -h, --help             this text

Logging: RUST_LOG (default info).";

#[derive(Debug)]
struct Args {
    backend: String,
    listen: Option<String>,
    ctrl: Option<String>,
    data: Option<String>,
    key_file: Option<PathBuf>,
    authorized: Option<PathBuf>,
    max_level: f64,
}

fn parse() -> Result<Option<Args>, String> {
    let mut it = std::env::args().skip(1);
    let mut backend = None;
    let mut a = Args {
        backend: String::new(),
        listen: None,
        ctrl: None,
        data: None,
        key_file: None,
        authorized: None,
        max_level: -10.0,
    };
    while let Some(flag) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "-h" | "--help" => return Ok(None),
            "--backend" => backend = Some(val()?),
            "--listen" => a.listen = Some(val()?),
            "--ctrl" => a.ctrl = Some(val()?),
            "--data" => a.data = Some(val()?),
            "--key-file" => a.key_file = Some(PathBuf::from(val()?)),
            "--authorized" => a.authorized = Some(PathBuf::from(val()?)),
            "--max-level" => {
                let v = val()?;
                let v = v.strip_suffix("dbfs").unwrap_or(&v);
                a.max_level = v
                    .parse()
                    .map_err(|_| format!("--max-level: not a number of dBFS: {v}"))?;
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    a.backend = backend.unwrap_or_else(|| "cpal".into());
    Ok(Some(a))
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ac2")
}

fn backend(name: &str) -> Result<Arc<dyn Backend>, String> {
    match name {
        "cpal" => Ok(Arc::new(CpalBackend::new())),
        "fake" => {
            let cfg = FakeConfig {
                drive: ac2_audio::fake::FakeDrive::Thread(ac2_audio::fake::Pace::Realtime),
                ..FakeConfig::default()
            };
            Ok(Arc::new(FakeBackend::new(cfg).map_err(|e| e.to_string())?))
        }
        #[cfg(all(feature = "jack", target_os = "linux"))]
        "jack" => Ok(Arc::new(ac2_audio::JackBackend::new(
            ac2_audio::JackConfig::default(),
        ))),
        #[cfg(not(all(feature = "jack", target_os = "linux")))]
        "jack" => Err("this build has no JACK backend (feature `jack`, Linux)".into()),
        other => Err(format!("unknown backend {other} (jack, cpal or fake)")),
    }
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = match parse() {
        Ok(Some(a)) => a,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("ac2d: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let backend = match backend(&args.backend) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("ac2d: {e}");
            return ExitCode::from(2);
        }
    };
    let listen = match (&args.listen, &args.ctrl, &args.data) {
        (Some(l), None, None) => {
            let dir = config_dir();
            let sec = NetworkSecurity {
                server_key_file: args
                    .key_file
                    .clone()
                    .unwrap_or_else(|| dir.join("server.key")),
                authorized_clients_file: args
                    .authorized
                    .clone()
                    .unwrap_or_else(|| dir.join("authorized_clients")),
            };
            match Listen::network(l, sec) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("ac2d: {e}");
                    return ExitCode::from(2);
                }
            }
        }
        (None, None, None) => Listen::default_local(),
        (None, Some(c), Some(d)) => Listen::Local {
            ctrl: c.clone(),
            data: d.clone(),
        },
        _ => {
            eprintln!("ac2d: use either --listen, or both --ctrl and --data\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let handle = match Daemon::start(DaemonConfig::new(backend, listen, args.max_level)) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("ac2d: {e}");
            return ExitCode::FAILURE;
        }
    };
    // `ac2 daemon stop` reads this and sends SIGTERM (taskkill on Windows).
    let pid_file = ac2d::pid_file();
    if let Err(e) =
        ac2d::keys::write_private_atomic(&pid_file, format!("{}\n", std::process::id()).as_bytes())
    {
        tracing::warn!("cannot write {}: {e}", pid_file.display());
    }
    let stopper = handle.stopper();
    // SIGINT, SIGTERM and SIGHUP (console close on Windows): orderly shutdown with fade-out.
    if let Err(e) = ctrlc::set_handler(move || stopper.stop()) {
        tracing::warn!("no signal handler: {e}");
    }
    handle.wait();
    if std::fs::read_to_string(&pid_file).is_ok_and(|s| s.trim() == std::process::id().to_string())
    {
        let _ = std::fs::remove_file(&pid_file);
    }
    ExitCode::SUCCESS
}
