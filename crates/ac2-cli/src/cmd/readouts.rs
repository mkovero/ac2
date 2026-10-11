//! `ac2 spl`, `ac2 timing` and `ac2 state dump`.

use ac2_client::Client;
use ac2_proto::Command;
use ac2_proto::model::*;

use super::meas::meas_call;
use super::meas_config::{time_weighting, weighting};
use super::{connect, find_meas, rate, state};
use crate::CliError;
use crate::args::*;
use crate::output::{self, Out};
use crate::watch;

pub(crate) async fn spl(cli: &Cli, cmd: &SplCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        SplCmd::Cal(a) => super::cal::cal_spl(cli, a, out).await,
        SplCmd::Leq { cmd } => super::leq::run(cli, cmd, out).await,
        SplCmd::Bands { cmd } => super::bands::run(cli, cmd, out).await,
        SplCmd::Set(a) => {
            super::leq::set_weightings(
                cli,
                a,
                a.weight.map(weighting),
                a.time.map(time_weighting),
                out,
            )
            .await
        }
        SplCmd::Watch(w) => {
            let c = connect(cli, true).await?;
            let s = state(&c).await?;
            let (id, input, created) = match (&w.meas, w.input) {
                (Some(r), _) => {
                    let m = find_meas(&s, r)?;
                    let MeasKind::Spl { config } = &m.config.kind else {
                        return Err(CliError::Usage(format!(
                            "{} is not an SPL measurement",
                            m.config.name
                        )));
                    };
                    (m.id, config.input, false)
                }
                (None, Some(input)) => {
                    // Always a meter of its own: another one on the same input (the UI's, or
                    // one a killed watch left behind) integrates over a span this command
                    // knows nothing of, and its Leq would not describe what it watched.
                    let want =
                        SplConfig::on_input(input.0, weighting(w.weight), time_weighting(w.time));
                    let m = meas_call(
                        &c,
                        Command::MeasCreate {
                            config: MeasConfig {
                                name: format!("spl-in{input}"),
                                kind: MeasKind::Spl { config: want },
                            },
                        },
                    )
                    .await?;
                    (m.id, input.0, true)
                }
                (None, None) => return Err(CliError::Usage("give --meas or --input".into())),
            };
            let running = s
                .measurements
                .iter()
                .find(|m| m.id == id)
                .is_some_and(|m| m.running);
            if !running {
                meas_call(&c, Command::MeasStart { meas: id }).await?;
            }
            let until = w.duration.map(|t| {
                std::time::Instant::now() + std::time::Duration::from_secs_f64(t.0.0.max(0.0))
            });
            let result = watch::spl(&c, id, input, until, out).await;
            if created {
                // A meter made for this view owns nothing.
                let _ = c
                    .call(Command::MeasDelete {
                        meas: id,
                        traces: OwnedTraces::Keep,
                    })
                    .await;
            } else if !running {
                let _ = c.call(Command::MeasStop { meas: id }).await;
            }
            result
        }
    }
}

pub(crate) async fn timing(cli: &Cli, live: bool, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, live).await?;
    if live {
        return watch::timing(&c, out).await;
    }
    let s = state(&c).await?;
    out.emit(&s.timing, || output::timing(&s.timing, rate(&s)))?;
    Ok(())
}

async fn dump(c: &Client, out: &mut Out<'_>) -> Result<(), CliError> {
    let snap = c.snapshot().await?;
    // A dump is JSON in both modes.
    let s = serde_json::to_string_pretty(&snap).map_err(std::io::Error::other)?;
    writeln!(out.w, "{s}")?;
    Ok(())
}

pub(crate) async fn state_dump(
    cli: &Cli,
    cmd: &StateCmd,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    match cmd {
        StateCmd::Dump => {
            let c = connect(cli, false).await?;
            dump(&c, out).await
        }
    }
}
