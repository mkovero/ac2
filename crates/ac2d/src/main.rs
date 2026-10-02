//! `ac2d`: the ac2 daemon.

use std::path::PathBuf;
use std::process::ExitCode;

use ac2d::{BackendChoice, Daemon, DaemonConfig, Listen, NetworkSecurity};

const USAGE: &str = "\
usage: ac2d [options]

With no options: local mode (ipc in the runtime dir; loopback TCP on Windows), cpal backend.

  --backend <name>       audio backend: cpal (default), jack, or fake (a simulated rig:
                         out 1 → in 1 loopback, out 1 → in 2 acoustic path; only when
                         named here, never as a fallback)
  --listen <tcp://iface[:port]>
                         network mode: ctrl on port (default 47820), data on port+1,
                         CURVE on both
  --ctrl <endpoint>      local ctrl endpoint (ipc:// or tcp://127.0.0.1:port)
  --data <endpoint>      local data endpoint
  --key-file <path>      server key pair (network mode; generated if missing)
  --authorized <path>    authorized clients (network mode; created empty if missing)
  --max-level <dBFS>     global generator maximum, dBFS RMS (default -10)
  --cal-store <path>     calibration store (default calibrations.json in the ac2 config
                         directory, ~/.config/ac2 on Linux); an unreadable
                         file is never overwritten
  -h, --help             this text

Logging: RUST_LOG (default info).";

#[derive(Debug)]
struct Args {
    backend: BackendChoice,
    listen: Option<String>,
    ctrl: Option<String>,
    data: Option<String>,
    key_file: Option<PathBuf>,
    authorized: Option<PathBuf>,
    max_level: f64,
    cal_store: Option<PathBuf>,
}

fn parse() -> Result<Option<Args>, String> {
    let mut it = std::env::args().skip(1);
    let mut backend = None;
    let mut a = Args {
        backend: BackendChoice::Cpal,
        listen: None,
        ctrl: None,
        data: None,
        key_file: None,
        authorized: None,
        max_level: -10.0,
        cal_store: None,
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
            "--cal-store" => a.cal_store = Some(PathBuf::from(val()?)),
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
    if let Some(b) = backend {
        a.backend = b.parse()?;
    }
    Ok(Some(a))
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
    let backend = match ac2d::backend(args.backend) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("ac2d: {e}");
            return ExitCode::from(2);
        }
    };
    let listen = match (&args.listen, &args.ctrl, &args.data) {
        (Some(l), None, None) => {
            let dir = ac2_paths::config_dir();
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
    let mut config = DaemonConfig::new(backend, listen, args.max_level);
    config.cal_store = Some(args.cal_store.clone().unwrap_or_else(ac2_paths::cal_store));
    let handle = match Daemon::start(config) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("ac2d: {e}");
            return ExitCode::FAILURE;
        }
    };
    // `ac2 daemon stop` reads this and sends SIGTERM (taskkill on Windows).
    let pid_file = ac2d::pid_file();
    if let Err(e) =
        ac2_paths::write_private_atomic(&pid_file, format!("{}\n", std::process::id()).as_bytes())
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
