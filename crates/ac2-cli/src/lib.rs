//! The `ac2` command-line client: a thin typed client of `ac2d`.
//!
//! Every command prints a human table or, with `--json`, one JSON document (live views: JSON
//! lines). Values on the command line carry units ([`units`]). The CLI holds the stimulus
//! lease only while a foreground `ac2 gen` runs.

pub mod args;
mod cmd;
mod output;
pub mod units;
mod watch;

use std::io::Write;
use std::process::ExitCode;

use ac2_client::ClientError;
use clap::Parser;

pub use args::Cli;
pub use output::Out;

/// This build's id: `<version>+<git commit>`; the daemon reports its own in `welcome.server`
/// as `… (build <id>)`.
pub const BUILD_ID: &str = env!("AC2_BUILD_ID");

/// Why a command failed.
#[derive(Debug)]
pub enum CliError {
    /// From the client library (including typed daemon refusals).
    Client(ClientError),
    /// The daemon is not running / not reachable.
    NotRunning(String),
    /// Arguments are valid syntax but cannot be used (unknown name, wrong kind, …).
    Usage(String),
    /// Refused locally for safety before anything was sent.
    Refused(String),
    /// Writing output or files failed.
    Io(std::io::Error),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client(e) => write!(f, "{e}"),
            Self::NotRunning(m) | Self::Usage(m) => f.write_str(m),
            Self::Refused(m) => write!(f, "refused: {m}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CliError {}

impl From<ClientError> for CliError {
    fn from(e: ClientError) -> Self {
        Self::Client(e)
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl CliError {
    /// Process exit code: 1 command failed, 3 daemon not reachable, 4 refused for safety.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::NotRunning(_) => 3,
            Self::Client(ClientError::Timeout { .. }) => 3,
            Self::Refused(_) => 4,
            _ => 1,
        }
    }

    fn json(&self) -> serde_json::Value {
        let code = match self {
            Self::Client(e) => e.code().map_or("client", ac2_client::code_name),
            Self::NotRunning(_) => "not_running",
            Self::Usage(_) => "usage",
            Self::Refused(_) => "refused",
            Self::Io(_) => "io",
        };
        serde_json::json!({ "error": { "code": code, "msg": self.to_string() } })
    }
}

/// Runs one parsed command, writing its output to `out`.
pub async fn run(cli: &Cli, out: &mut Out<'_>) -> Result<(), CliError> {
    cmd::dispatch(cli, out).await
}

/// Runs `cli`, printing an error (as JSON with `--json`) to `err`; returns the exit code.
pub async fn run_reporting(cli: &Cli, out: &mut Out<'_>, err: &mut dyn Write) -> u8 {
    match run(cli, out).await {
        Ok(()) => 0,
        Err(e) => {
            if cli.json {
                let _ = writeln!(out.w, "{}", e.json());
            } else {
                let _ = writeln!(err, "ac2: {e}");
            }
            e.exit_code()
        }
    }
}

/// Process entry point.
pub fn main_entry() -> ExitCode {
    let cli = Cli::parse();
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("ac2: cannot start runtime: {e}");
            return ExitCode::from(1);
        }
    };
    let code = rt.block_on(async {
        let mut stdout = std::io::stdout();
        let mut out = Out::new(cli.json, &mut stdout);
        run_reporting(&cli, &mut out, &mut std::io::stderr()).await
    });
    // Dropping the runtime drops the client, whose I/O thread flushes queued requests (a
    // lease release) before the process exits.
    drop(rt);
    ExitCode::from(code)
}
