//! `ac2d`: the ac2 daemon.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;

use ac2d::{
    Advertise, AutosaveConfig, BackendChoice, Daemon, DaemonConfig, Listen, NetworkSecurity,
    RigIdSource,
};

/// The ac2 daemon.
///
/// With no options: local mode (ipc in the runtime dir; loopback TCP on Windows), on this
/// platform's audio: JACK on Linux (JACK2, or PipeWire through pipewire-jack), Core Audio
/// on macOS, ASIO on Windows.
#[derive(Debug, Parser)]
#[command(
    name = "ac2d",
    args_override_self = true,
    version = concat!(env!("CARGO_PKG_VERSION"), " (build ", env!("AC2_BUILD_ID"), ")"),
    after_help = "Logging: RUST_LOG (default warn,ac2=info) to stderr; when not started by \
                  systemd (which keeps stderr in the journal) also to ac2d.log in the ac2 log \
                  directory (~/.local/state/ac2 on Linux), the previous run's kept as ac2d.log.1."
)]
struct Args {
    /// Audio backend: jack (Linux), coreaudio (macOS) or asio (Windows), the platform's
    /// default; or fake (a simulated rig: out 1 → in 1 loopback, out 1 → in 2 acoustic
    /// path; only when named here, never as a fallback).
    #[arg(long, value_name = "NAME")]
    backend: Option<BackendChoice>,
    /// Network mode: ctrl on port (default 47820), data on port+1, CURVE on both.
    #[arg(long, value_name = "tcp://IFACE[:PORT]", conflicts_with_all = ["ctrl", "data"])]
    listen: Option<String>,
    /// Local ctrl endpoint (ipc:// or tcp://127.0.0.1:port).
    #[arg(long, value_name = "ENDPOINT", requires = "data")]
    ctrl: Option<String>,
    /// Local data endpoint.
    #[arg(long, value_name = "ENDPOINT", requires = "ctrl")]
    data: Option<String>,
    /// Server key pair (network mode; generated if missing).
    #[arg(long, value_name = "PATH")]
    key_file: Option<PathBuf>,
    /// Authorized clients (network mode; created empty if missing).
    #[arg(long, value_name = "PATH")]
    authorized: Option<PathBuf>,
    /// Rig name, shown by every client and advertised over mDNS in network mode (default:
    /// ac2 on <hostname>).
    #[arg(long, value_name = "TEXT")]
    name: Option<String>,
    /// Network mode without the mDNS advert (clients then need the address; pairing is
    /// required either way).
    #[arg(long, requires = "listen")]
    no_mdns: bool,
    /// Hard upper bound of the system max level, dBFS RMS: `-10` or `-10dbfs`; clients lower
    /// the level in force at run time (and raise it up to this bound after a confirmation),
    /// kept in the rig settings.
    #[arg(long, value_name = "DBFS", default_value = "-10", allow_hyphen_values = true,
          value_parser = max_level)]
    max_level: f64,
    /// Rig settings: system max level, output labels, rig SPL meters, monitor at start
    /// (default rig.json in the ac2 config directory); an unreadable file is never
    /// overwritten.
    #[arg(long, value_name = "PATH")]
    settings: Option<PathBuf>,
    /// Where the rig SPL meters keep their logs (default: rig-logs in the ac2 data
    /// directory, ~/.local/share/ac2 on Linux).
    #[arg(long, value_name = "DIR")]
    rig_logs: Option<PathBuf>,
    /// Calibration store (default calibrations.json in the ac2 config directory,
    /// ~/.config/ac2 on Linux); an unreadable file is never overwritten.
    #[arg(long, value_name = "PATH")]
    cal_store: Option<PathBuf>,
    /// Where measurements and traces are autosaved (default: autosave in the ac2 data
    /// directory, ~/.local/share/ac2 on Linux); the previous manifest is kept as
    /// <DIR>/session.prev.json.
    #[arg(long, value_name = "DIR")]
    autosave: Option<PathBuf>,
    /// Start empty: the autosave is not loaded but moved aside to <DIR>.unrestored.
    #[arg(long)]
    no_restore: bool,
    /// Keep measurements and traces in memory only.
    #[arg(long, conflicts_with_all = ["autosave", "no_restore"])]
    no_autosave: bool,
    /// Where raw capture files are recorded (default: recordings in the ac2 data directory,
    /// ~/.local/share/ac2 on Linux).
    #[arg(long, value_name = "DIR")]
    recordings: Option<PathBuf>,
}

/// `-50` or `-50dbfs`.
fn max_level(v: &str) -> Result<f64, String> {
    let n = v.strip_suffix("dbfs").unwrap_or(v);
    n.parse().map_err(|_| format!("not a number of dBFS: {n}"))
}

fn main() -> ExitCode {
    let args = Args::parse();
    // systemd sets JOURNAL_STREAM when stderr goes to the journal, which already keeps and
    // rotates the log; a second copy on disk would only grow.
    let file_dir = std::env::var_os("JOURNAL_STREAM")
        .is_none()
        .then(ac2_paths::log_dir);
    if let Some(path) = ac2_paths::log::init_in(file_dir.as_deref(), "ac2d") {
        tracing::info!(
            "ac2d {}: logging to {}",
            env!("CARGO_PKG_VERSION"),
            path.display()
        );
    }
    ac2_paths::log::install_panic_hook("ac2d");
    let backend = args.backend.unwrap_or_else(BackendChoice::platform);
    let backends = match ac2d::backends(backend) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("ac2d: {e}");
            return ExitCode::from(2);
        }
    };
    let listen = match (&args.listen, &args.ctrl, &args.data) {
        (Some(l), None, None) => {
            let sec = NetworkSecurity {
                server_key_file: args.key_file.clone().unwrap_or_else(ac2_paths::server_key),
                authorized_clients_file: args
                    .authorized
                    .clone()
                    .unwrap_or_else(ac2_paths::authorized_clients),
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
            eprintln!("ac2d: use either --listen, or both --ctrl and --data");
            return ExitCode::from(2);
        }
    };
    let advertise = (listen.is_network() && !args.no_mdns).then(|| Advertise {
        mdns: ac2_discovery::Options::default(),
    });
    let mut config = DaemonConfig::new(Arc::clone(&backends[0]), listen, args.max_level);
    config.backends = backends;
    if let Some(name) = &args.name {
        config.name.clone_from(name);
    }
    config.rig_id = RigIdSource::File(ac2_paths::rig_id());
    config.systemd_notify = true;
    config.advertise = advertise;
    config.cal_store = Some(args.cal_store.clone().unwrap_or_else(ac2_paths::cal_store));
    config.rig_settings = Some(
        args.settings
            .clone()
            .unwrap_or_else(ac2_paths::rig_settings),
    );
    config.rig_logs = Some(args.rig_logs.clone().unwrap_or_else(ac2_paths::rig_log_dir));
    config.autosave = (!args.no_autosave).then(|| AutosaveConfig {
        dir: args
            .autosave
            .clone()
            .unwrap_or_else(ac2_paths::autosave_dir),
        restore: !args.no_restore,
    });
    config.recording_dir = Some(
        args.recordings
            .clone()
            .unwrap_or_else(ac2_paths::recording_dir),
    );
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
