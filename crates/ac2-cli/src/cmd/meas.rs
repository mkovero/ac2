//! `ac2 meas new|list|set|layout|start|stop|rm`.

use ac2_client::{Client, expect_body};
use ac2_proto::model::*;
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::meas_config::{MeasSet, meas_config, meas_set, spec_level, uncaptured, window};
use super::{connect, find_meas, state};
use crate::CliError;
use crate::args::*;
use crate::output::{self, Out};
use crate::watch;

pub(crate) async fn meas_call(c: &Client, cmd: Command) -> Result<Measurement, CliError> {
    let op = cmd.name();
    let r = c.call(cmd).await?;
    Ok(expect_body!(op, r, ReplyBody::Measurement(m) => m)?)
}

pub(crate) async fn meas(cli: &Cli, cmd: &MeasCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    if let MeasCmd::List { watch: true } = cmd {
        let c = connect(cli, true).await?;
        return watch::meas_list(&c, out).await;
    }
    let c = connect(cli, false).await?;
    match cmd {
        MeasCmd::New(n) => {
            let config = meas_config(n)?;
            if n.start
                && let Some(open) = &state(&c).await?.session.open
            {
                uncaptured(&config.kind, &open.config.input_channels)?;
            }
            let mut m = meas_call(&c, Command::MeasCreate { config }).await?;
            if n.start {
                match meas_call(&c, Command::MeasStart { meas: m.id }).await {
                    Ok(started) => m = started,
                    Err(e) => {
                        // `--start` is one request from the operator's side: a measurement
                        // left behind stopped would be duplicated by the retry.
                        let undo = Command::MeasDelete {
                            meas: m.id,
                            traces: OwnedTraces::Delete,
                        };
                        if let Err(d) = c.call(undo).await {
                            return Err(CliError::Usage(format!(
                                "{e}; measurement {} ({}) was created stopped and could not \
                                 be removed: {d}",
                                m.id.0, m.config.name
                            )));
                        }
                        return Err(e);
                    }
                }
            }
            out.emit(&m, || output::measurement(&m))?;
        }
        MeasCmd::List { .. } => {
            let s = state(&c).await?;
            out.emit(&s.measurements, || output::measurements(&s.measurements))?;
        }
        MeasCmd::Set {
            meas,
            average,
            blocks,
            fft,
            window: win,
            resolution,
            level,
            colour,
            style,
            peak,
            span,
            colours,
            offset,
        } => {
            let s = state(&c).await?;
            let mut m = find_meas(&s, meas)?.clone();
            let set = MeasSet {
                average: average.map(|a| a.0),
                blocks: *blocks,
                fft: *fft,
                window: win.map(window),
                resolution: *resolution,
                level: level.map(spec_level),
                style: style.map(Into::into),
                peak: peak.map(Into::into),
                span_s: *span,
                colours: colours.map(Into::into),
            };
            if !set.is_empty() || (colour.is_none() && offset.is_none()) {
                let config = meas_set(&m.config, set)?;
                m = meas_call(
                    &c,
                    Command::MeasUpdate {
                        meas: m.id,
                        base: m.config_rev,
                        config,
                    },
                )
                .await?;
            }
            if let Some(colour) = colour {
                m = meas_call(
                    &c,
                    Command::MeasColour {
                        meas: m.id,
                        colour: colour.0,
                    },
                )
                .await?;
            }
            if let Some(offset) = offset {
                m = meas_call(
                    &c,
                    Command::MeasOffset {
                        meas: m.id,
                        offset: ac2_proto::units::Db(*offset),
                    },
                )
                .await?;
            }
            out.emit(&m, || output::measurement(&m))?;
        }
        MeasCmd::Layout {
            cmd: MeasLayoutCmd::Clear { meas },
        } => {
            let s = state(&c).await?;
            let id = find_meas(&s, meas)?.id;
            let m = meas_call(
                &c,
                Command::MeasLayout {
                    meas: id,
                    layout: None,
                },
            )
            .await?;
            out.emit(&m, || output::measurement(&m))?;
        }
        MeasCmd::Scope { meas, scope } => super::osc::meas_scope(&c, meas, *scope, out).await?,
        MeasCmd::Start { meas } | MeasCmd::Stop { meas } => {
            let s = state(&c).await?;
            let id = find_meas(&s, meas)?.id;
            let cmd = if matches!(cmd, MeasCmd::Start { .. }) {
                Command::MeasStart { meas: id }
            } else {
                Command::MeasStop { meas: id }
            };
            let m = meas_call(&c, cmd).await?;
            out.emit(&m, || output::measurement(&m))?;
        }
        MeasCmd::Rm {
            meas,
            keep_traces,
            delete_traces,
        } => {
            let snap = c.snapshot().await?;
            let m = find_meas(&snap.state, meas)?;
            let owner = TraceOwner::Meas { meas: m.id };
            let traces = snap
                .state
                .traces
                .iter()
                .filter(|t| t.edit.owner == owner)
                .count();
            let maths = snap
                .state
                .measurements
                .iter()
                .filter(|x| matches!(&x.config.kind, MeasKind::Math { config } if config.owner == owner))
                .count();
            // What it owns goes nowhere by default: the operator says which.
            let what = match (keep_traces, delete_traces) {
                (true, _) => OwnedTraces::Keep,
                (_, true) => OwnedTraces::Delete,
                _ if traces + maths == 0 => OwnedTraces::Keep,
                _ => {
                    return Err(CliError::Usage(format!(
                        "{} owns {traces} trace(s) and {maths} math channel(s): add \
                         --keep-traces (moved to the imported group) or --delete-traces",
                        m.config.name
                    )));
                }
            };
            // Guarded by the rev the name was resolved at: if the measurements changed in
            // between, the daemon refuses instead of deleting the wrong one.
            let r = c
                .call_expect(
                    Command::MeasDelete {
                        meas: m.id,
                        traces: what,
                    },
                    snap.rev,
                )
                .await?;
            let rev = expect_body!("meas.delete", r, ReplyBody::Ack { rev } => rev)?;
            let name = m.config.name.clone();
            out.emit(&json!({ "deleted": m.id, "rev": rev }), || {
                format!("deleted measurement {} {name}", m.id)
            })?;
        }
    }
    Ok(())
}
