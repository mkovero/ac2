//! `ac2d`: the ac2 daemon.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;

use ac2d::instance::{Instance, LockError, exit};
use ac2d::{
    Advertise, AutosaveConfig, BackendChoice, Daemon, DaemonConfig, Ended, Listen, NetworkSecurity,
    RigIdSource, Stopper,
};

/// Stores whose defaults live in the config and data dirs: `--ephemeral` keeps none.
const STORES: [&str; 10] = [
    "settings",
    "rig_logs",
    "cal_store",
    "plans",
    "autosave",
    "no_restore",
    "no_autosave",
    "recordings",
    "key_file",
    "authorized",
];

/// A stdin that ends sooner than this after start was never a pipe a parent holds (a
/// terminal's `</dev/null`, a service manager's null stdin): the flag was misused.
const EARLY_EOF: Duration = Duration::from_secs(1);

/// The ac2 daemon.
///
/// With no options: local mode (ipc in the runtime dir; loopback TCP on Windows), on this
/// platform's audio: JACK on Linux (JACK2, or PipeWire through pipewire-jack), Core Audio
/// on macOS, ASIO on Windows. On those default endpoints one ac2d runs per user: a second
/// exits with code 3 and touches nothing.
#[derive(Debug, Parser)]
#[command(
    name = "ac2d",
    args_override_self = true,
    version = concat!(env!("CARGO_PKG_VERSION"), " (build ", env!("AC2_BUILD_ID"), ")"),
    after_help = "Logging: RUST_LOG (default warn,ac2=info) to stderr; when not started by \
                  systemd (which keeps stderr in the journal) also to ac2d.log in the ac2 log \
                  directory (~/.local/state/ac2 on Linux), the previous run's kept as ac2d.log.1 \
                  (ac2d-sim.log with --ephemeral).\n\n\
                  Exit codes: 0 stopped in order, 1 failed to start or ended on a fatal error, \
                  2 unusable arguments or instance lock, 3 another ac2d is running for this user \
                  (on the default local endpoints, which take a lock in the runtime directory)."
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
    /// The plan library: one room plan per <name>.json (default: plans in the ac2 data
    /// directory, ~/.local/share/ac2 on Linux).
    #[arg(long, value_name = "DIR")]
    plans: Option<PathBuf>,
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
    /// Keep nothing: a random rig id and no autosave, calibration store, rig settings,
    /// plans, rig logs or recordings; nothing read or written in the ac2 config and data
    /// directories (local mode only). Logs to ac2d-sim.log.
    #[arg(long, conflicts_with_all = STORES, conflicts_with = "listen")]
    ephemeral: bool,
    /// Print the endpoints as bound on one stdout line, `ac2d: listening ctrl=… data=…`
    /// (a `tcp://127.0.0.1:0` request shows its real port).
    #[arg(long)]
    print_endpoints: bool,
    /// Stop in order when stdin reaches its end: for a parent holding a pipe on this
    /// process's stdin, whose exit (or crash) closes it.
    #[arg(long)]
    stop_on_stdin_eof: bool,
}

/// `-50` or `-50dbfs`.
fn max_level(v: &str) -> Result<f64, String> {
    let n = v.strip_suffix("dbfs").unwrap_or(v);
    n.parse().map_err(|_| format!("not a number of dBFS: {n}"))
}

fn main() -> ExitCode {
    let started = Instant::now();
    let args = Args::parse();
    let listen = match listen(&args) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("ac2d: {e}");
            return ExitCode::from(exit::SETUP);
        }
    };
    // First of all, so a second daemon leaves the log, the audio and the sockets alone.
    let default_local = args.listen.is_none() && args.ctrl.is_none();
    let mut instance = if default_local {
        match Instance::lock(&ac2d::runtime_dir()) {
            Ok(i) => Some(i),
            Err(e @ LockError::Held(_)) => {
                eprintln!("ac2d: {e}");
                return ExitCode::from(exit::ANOTHER_RUNNING);
            }
            Err(e @ LockError::Io(..)) => {
                eprintln!("ac2d: {e}");
                return ExitCode::from(exit::SETUP);
            }
        }
    } else {
        None
    };
    // systemd sets JOURNAL_STREAM when stderr goes to the journal, which already keeps and
    // rotates the log; a second copy on disk would only grow.
    let file_dir = std::env::var_os("JOURNAL_STREAM")
        .is_none()
        .then(ac2_paths::log_dir);
    let log_name = if args.ephemeral { "ac2d-sim" } else { "ac2d" };
    if let Some(path) = ac2_paths::log::init_in(file_dir.as_deref(), log_name) {
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
            return ExitCode::from(exit::SETUP);
        }
    };
    let config = config(&args, backends, listen);
    let handle = match Daemon::start(config) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("ac2d: {e}");
            return ExitCode::from(exit::FATAL);
        }
    };
    // `ac2 daemon stop` reads this and sends SIGTERM (taskkill on Windows).
    if let Some(i) = &mut instance
        && let Err(e) = i.write_pid()
    {
        tracing::warn!("cannot write {}: {e}", i.pid_file().display());
    }
    if args.print_endpoints {
        println!(
            "ac2d: listening ctrl={} data={}",
            handle.ctrl_endpoint(),
            handle.data_endpoint()
        );
    }
    let stopper = handle.stopper();
    // SIGINT, SIGTERM and SIGHUP (console close on Windows): orderly shutdown with fade-out.
    let on_signal = stopper.clone();
    if let Err(e) = ctrlc::set_handler(move || on_signal.stop()) {
        tracing::warn!("no signal handler: {e}");
    }
    if args.stop_on_stdin_eof
        && let Err(e) = stop_on_stdin_eof(stopper, started)
    {
        // Serving on would outlive the parent the flag ties this daemon to.
        tracing::error!("cannot watch stdin: {e}");
        handle.shutdown();
        return ExitCode::from(exit::FATAL);
    }
    let ended = handle.wait();
    drop(instance);
    match ended {
        Ended::Stopped => ExitCode::SUCCESS,
        Ended::Fatal(_) => ExitCode::from(exit::FATAL),
    }
}

fn listen(args: &Args) -> Result<Listen, String> {
    match (&args.listen, &args.ctrl, &args.data) {
        (Some(l), None, None) => {
            let sec = NetworkSecurity {
                server_key_file: args.key_file.clone().unwrap_or_else(ac2_paths::server_key),
                authorized_clients_file: args
                    .authorized
                    .clone()
                    .unwrap_or_else(ac2_paths::authorized_clients),
            };
            Listen::network(l, sec).map_err(|e| e.to_string())
        }
        (None, None, None) => Ok(Listen::default_local()),
        (None, Some(c), Some(d)) => Ok(Listen::Local {
            ctrl: c.clone(),
            data: d.clone(),
        }),
        _ => Err("use either --listen, or both --ctrl and --data".into()),
    }
}

fn config(args: &Args, backends: Vec<Arc<dyn ac2_audio::Backend>>, listen: Listen) -> DaemonConfig {
    let advertise = (listen.is_network() && !args.no_mdns).then(|| Advertise {
        mdns: ac2_discovery::Options::default(),
    });
    let mut config = DaemonConfig::new(Arc::clone(&backends[0]), listen, args.max_level);
    config.backends = backends;
    if let Some(name) = &args.name {
        config.name.clone_from(name);
    }
    config.systemd_notify = true;
    config.advertise = advertise;
    if args.ephemeral {
        // `DaemonConfig::new` keeps nothing: an ephemeral id and every store off.
        return config;
    }
    config.rig_id = RigIdSource::File(ac2_paths::rig_id());
    config.cal_store = Some(args.cal_store.clone().unwrap_or_else(ac2_paths::cal_store));
    config.plan_dir = Some(args.plans.clone().unwrap_or_else(ac2_paths::plan_dir));
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
    config
}

/// Reads stdin to its end on a thread of its own, then stops the daemon in order: the
/// parent holding the pipe has let go or died.
fn stop_on_stdin_eof(stopper: Stopper, started: Instant) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("ac2d-stdin".into())
        .spawn(move || {
            let mut buf = [0u8; 256];
            let mut stdin = std::io::stdin().lock();
            let why = loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break "closed".to_owned(),
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => break format!("unreadable ({e})"),
                }
            };
            let after = started.elapsed();
            if after < EARLY_EOF {
                tracing::error!(
                    "stdin {why} {} ms after start: --stop-on-stdin-eof is for a parent \
                     holding a pipe on stdin; stopping",
                    after.as_millis()
                );
            } else {
                tracing::info!("stdin {why}: stopping");
            }
            stopper.stop();
        })
        .map(drop)
}
