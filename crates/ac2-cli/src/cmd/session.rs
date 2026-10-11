//! `ac2 devices` and `ac2 session open|close|status|replay|inputs`: the live audio
//! session; the saved-session verbs go on to `sessions`.

use ac2_client::expect_body;
use ac2_proto::model::*;
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::{connect, state};
use crate::CliError;
use crate::args::*;
use crate::output::{self, Out};

pub(crate) async fn devices(cli: &Cli, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let r = c.call(Command::SessionDevices).await?;
    let d = expect_body!("session.devices", r, ReplyBody::Backends(d) => d)?;
    out.emit(&d, || output::devices(&d))?;
    Ok(())
}

fn backend(b: BackendArg) -> BackendKind {
    match b {
        BackendArg::Jack => BackendKind::Jack,
        BackendArg::Cpal => BackendKind::Cpal,
        BackendArg::Fake => BackendKind::Fake,
    }
}

pub(crate) async fn session(
    cli: &Cli,
    cmd: &SessionCmd,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    match cmd {
        SessionCmd::Open(o) => {
            let r = c.call(Command::SessionDevices).await?;
            let backends = expect_body!("session.devices", r, ReplyBody::Backends(d) => d)?;
            let want = backend(o.backend);
            let Some(b) = backends.iter().find(|b| b.kind == want) else {
                return Err(CliError::Usage(format!(
                    "the daemon offers no {want:?} backend (see `ac2 devices`)"
                )));
            };
            if let Availability::Unavailable { reason } = &b.availability {
                return Err(CliError::Usage(format!(
                    "{want:?} is unavailable: {reason}"
                )));
            }
            let of_backend: Vec<&DeviceInfo> = b.devices.iter().collect();
            let named = |sel: &String| {
                of_backend
                    .iter()
                    .find(|d| &d.id.0 == sel || &d.name == sel)
                    .copied()
                    .ok_or_else(|| {
                        CliError::Usage(format!("no {want:?} device {sel:?} (see `ac2 devices`)"))
                    })
            };
            let dev = match &o.device {
                Some(sel) => named(sel)?,
                None => match of_backend.as_slice() {
                    [d] => *d,
                    [] => {
                        return Err(CliError::Usage(format!(
                            "the daemon lists no {want:?} device"
                        )));
                    }
                    _ => {
                        return Err(CliError::Usage(format!(
                            "{} {want:?} devices; choose one with --device (see `ac2 devices`)",
                            of_backend.len()
                        )));
                    }
                },
            };
            let rate = match o.rate {
                None => None,
                Some(f) if f.0.0.fract() == 0.0 && f.0.0 <= f64::from(u32::MAX) => {
                    Some(f.0.0 as u32)
                }
                Some(f) => {
                    return Err(CliError::Usage(format!(
                        "sample rate {} Hz is not a whole number",
                        f.0.0
                    )));
                }
            };
            let buffer = match o.buffer {
                None => None,
                Some(n) => Some(
                    u32::try_from(n.0)
                        .map_err(|_| CliError::Usage(format!("buffer of {} samples", n.0)))?,
                ),
            };
            let loopback = match (o.loopback_out, o.loopback_in) {
                (Some(out_ch), Some(in_ch)) => Some(LoopbackRoute {
                    output: out_ch.0,
                    input: in_ch.0,
                }),
                _ => None,
            };
            let out_dev = match &o.out_device {
                Some(sel) => named(sel)?,
                None => default_output(dev, &of_backend).unwrap_or(dev),
            };
            let has_outputs = out_dev.output.as_ref().is_some_and(|d| d.max_channels > 0);
            if o.outputs > 0 && !has_outputs {
                return Err(CliError::Usage(format!(
                    "{} has no outputs: choose the playback device with --out-device, or \
                     --outputs 0 (see `ac2 devices`)",
                    out_dev.name
                )));
            }
            let config = SessionConfig {
                backend: Some(want),
                input_device: DeviceSelector::Id { id: dev.id.clone() },
                output_device: DeviceSelector::Id {
                    id: out_dev.id.clone(),
                },
                input_channels: o.inputs.0.clone(),
                output_channels: o.outputs,
                sample_rate_hz: rate,
                buffer_frames: buffer,
                loopback,
            };
            let r = c.call(Command::SessionOpen { config }).await?;
            let s = expect_body!("session.open", r, ReplyBody::Session(s) => s)?;
            let inputs = if o.mics.is_empty() {
                None
            } else {
                Some(super::cal::set_inputs(&c, &o.mics, &[]).await?)
            };
            out.emit(&s, || match &inputs {
                None => output::session(&s),
                Some(i) => format!(
                    "{}\n{}",
                    output::session(&s),
                    output::inputs(i, crate::watch::now_wall())
                ),
            })?;
        }
        SessionCmd::Close => {
            let r = c.call(Command::SessionClose).await?;
            let rev = expect_body!("session.close", r, ReplyBody::Ack { rev } => rev)?;
            out.emit(&json!({ "rev": rev }), || "session closed".to_owned())?;
        }
        SessionCmd::Status => {
            let r = c.call(Command::SessionStatus).await?;
            let s = expect_body!("session.status", r, ReplyBody::Session(s) => s)?;
            let st = state(&c).await?;
            let (autosave, recording, working) = (st.autosave, st.recording, st.working);
            let mut j = json!(s);
            j["autosave"] = json!(autosave);
            j["working"] = json!(working);
            j["recording"] = json!(recording);
            out.emit(&j, || {
                let mut t = format!(
                    "{}\n{}",
                    output::session(&s),
                    output::autosave(&autosave, crate::watch::now_wall())
                );
                t.push('\n');
                t.push_str(&super::sessions::status_lines(
                    &working,
                    crate::watch::now_wall(),
                ));
                if let Some(r) = &recording {
                    t.push_str(&format!("\nrecording    {}", output::recording_line(r)));
                }
                t
            })?;
        }
        SessionCmd::New { .. }
        | SessionCmd::Save
        | SessionCmd::SaveAs { .. }
        | SessionCmd::Load { .. }
        | SessionCmd::Recover
        | SessionCmd::Salvage { .. }
        | SessionCmd::Export { .. }
        | SessionCmd::Import { .. }
        | SessionCmd::List => super::sessions::session(cli, &c, cmd, out).await?,
        SessionCmd::Replay { recording, fast } => {
            super::rec::replay(cli, &c, recording, *fast, out).await?;
        }
        SessionCmd::Inputs(a) => return super::cal::session_inputs(cli, a, out).await,
    }
    Ok(())
}

/// The playback device when none is named: the capture device itself when it has outputs
/// (one device, one clock where the host allows it), else the system's default output.
fn default_output<'a>(input: &'a DeviceInfo, devices: &[&'a DeviceInfo]) -> Option<&'a DeviceInfo> {
    if input.output.is_some() {
        return Some(input);
    }
    devices
        .iter()
        .find(|d| d.output.as_ref().is_some_and(|o| o.system_default))
        .copied()
}
