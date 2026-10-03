//! Command implementations.

mod auth;
mod basic;
mod cal;
mod daemon;
mod discover;
mod gen_;
mod traces;

use std::time::Duration;

use ac2_client::{Client, ClientConfig, ClientError, Endpoints, KeyDir, Retry};
use ac2_proto::model::{Measurement, State, TraceMeta};

use crate::args::{Cli, Cmd, MeasRef, SessionCmd};
use crate::output::Out;
use crate::{BUILD_ID, CliError};

pub(crate) async fn dispatch(cli: &Cli, out: &mut Out<'_>) -> Result<(), CliError> {
    match &cli.cmd {
        Cmd::Devices => basic::devices(cli, out).await,
        Cmd::Status => daemon::status(cli, out).await,
        Cmd::Daemon { cmd } => daemon::run(cli, cmd, out).await,
        Cmd::Session {
            cmd: SessionCmd::Inputs(a),
        } => cal::session_inputs(cli, a, out).await,
        Cmd::Session { cmd } => basic::session(cli, cmd, out).await,
        Cmd::Gen { cmd } => gen_::run(cli, cmd, out).await,
        Cmd::Meas { cmd } => basic::meas(cli, cmd, out).await,
        Cmd::Delay { cmd } => basic::delay(cli, cmd, out).await,
        Cmd::Spl { cmd } => basic::spl(cli, cmd, out).await,
        Cmd::Cal { cmd } => cal::run(cli, cmd, out).await,
        Cmd::Timing { watch } => basic::timing(cli, *watch, out).await,
        Cmd::Trace { cmd } => traces::trace(cli, cmd, out).await,
        Cmd::State { cmd } => basic::state_dump(cli, cmd, out).await,
        Cmd::Discover(a) => discover::run(cli, a, out),
        Cmd::Auth { cmd } => auth::run(cli, cmd, out),
    }
}

pub(crate) fn key_dir(cli: &Cli) -> KeyDir {
    KeyDir::new(
        cli.key_dir
            .clone()
            .unwrap_or_else(ac2_client::keys::default_key_dir),
    )
}

/// Endpoints, CURVE settings and a description for messages.
pub(crate) fn target(cli: &Cli) -> Result<(ClientConfig, String), CliError> {
    let (endpoints, curve, what) = match (&cli.ctrl_endpoint, &cli.data_endpoint, &cli.remote) {
        (Some(c), Some(d), _) => (
            Endpoints {
                ctrl: c.clone(),
                data: d.clone(),
            },
            None,
            format!("daemon at {c}"),
        ),
        (_, _, Some(r)) => (
            Endpoints::remote(r),
            Some(key_dir(cli).curve_client(&r.host)?),
            format!("daemon at {r}"),
        ),
        _ => (Endpoints::local(), None, "local daemon".to_owned()),
    };
    let mut cfg = ClientConfig::new(endpoints, format!("ac2-cli {BUILD_ID}"));
    cfg.curve = curve;
    cfg.retry = Retry {
        timeout: Duration::from_secs_f64(cli.timeout.0.0.max(0.01)),
        retries: 2,
    };
    Ok((cfg, what))
}

pub(crate) fn is_local(cli: &Cli) -> bool {
    cli.remote.is_none() && cli.ctrl_endpoint.is_none()
}

/// Connects; `mirror` turns on state mirroring (live views only).
pub(crate) async fn connect(cli: &Cli, mirror: bool) -> Result<Client, CliError> {
    let (mut cfg, what) = target(cli)?;
    cfg.mirror = mirror;
    #[cfg(unix)]
    if is_local(cli) {
        // An ipc connect to a missing socket file would just wait; say so at once.
        let sock = ac2_client::endpoint::runtime_dir().join("ctrl.sock");
        if !sock.exists() {
            return Err(CliError::NotRunning(format!(
                "local daemon is not running (no {}); start it with `ac2 daemon start`",
                sock.display()
            )));
        }
    }
    let hint = cfg.endpoints.firewall_hint();
    match Client::connect(cfg).await {
        Ok(c) => Ok(c),
        Err(ClientError::Timeout { .. }) => Err(CliError::NotRunning(match hint {
            Some(h) => format!("{what} is not responding; {h}"),
            None => format!("{what} is not responding"),
        })),
        Err(e) => Err(e.into()),
    }
}

pub(crate) async fn state(c: &Client) -> Result<State, CliError> {
    Ok(c.snapshot().await?.state)
}

/// Resolves `r` among `items` (`what` names them in messages): the item whose id it is,
/// else the one item of that name. A reference that is one item's id and another item's
/// name is refused, naming both.
fn resolve<'s, T>(
    items: &'s [T],
    r: &MeasRef,
    what: &str,
    id: impl Fn(&T) -> u32,
    name: impl Fn(&T) -> &str,
) -> Result<&'s T, CliError> {
    let text = r.0.as_str();
    let by_id = r.id().and_then(|n| items.iter().find(|t| id(t) == n));
    let by_name: Vec<&T> = items.iter().filter(|t| name(t) == text).collect();
    let describe = |t: &T| format!("{what} {} ({:?})", id(t), name(t));
    match (by_id, by_name.as_slice()) {
        (Some(t), named) if named.iter().all(|n| id(n) == id(t)) => Ok(t),
        (Some(t), named) => {
            let mut all = vec![format!("{} by id", describe(t))];
            all.extend(named.iter().map(|n| format!("{} by name", describe(n))));
            Err(CliError::Usage(format!(
                "{text:?} is ambiguous: it matches {}; rename one of them",
                all.join(" and ")
            )))
        }
        (None, [t]) => Ok(t),
        (None, []) if r.id().is_some() => {
            Err(CliError::Usage(format!("no {what} with id or name {text}")))
        }
        (None, []) => Err(CliError::Usage(format!("no {what} named {text:?}"))),
        (None, named) => Err(CliError::Usage(format!(
            "{} {what}s are named {text:?} (ids {}); use the id",
            named.len(),
            named
                .iter()
                .map(|t| id(t).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

pub(crate) fn find_meas<'s>(s: &'s State, r: &MeasRef) -> Result<&'s Measurement, CliError> {
    resolve(
        &s.measurements,
        r,
        "measurement",
        |m| m.id.0,
        |m| &m.config.name,
    )
}

pub(crate) fn find_trace<'s>(s: &'s State, r: &MeasRef) -> Result<&'s TraceMeta, CliError> {
    resolve(&s.traces, r, "trace", |t| t.id.0, |t| &t.edit.name)
}

/// The open session's sample rate.
pub(crate) fn rate(s: &State) -> Option<u32> {
    s.session.open.as_ref().map(|o| o.sample_rate_hz)
}
