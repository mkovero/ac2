//! `ac2 devices` and `ac2 session open|close|status|replay|inputs`: the live audio
//! session; the saved-session verbs go on to `sessions`.

use ac2_client::expect_body;
use ac2_proto::model::*;
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::{connect, devsel, state};
use crate::CliError;
use crate::args::*;
use crate::output::{self, Out};
use ac2_audio::Direction;

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
        BackendArg::Asio => BackendKind::Asio,
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
            let (dev, out_dev) = pick(&of_backend, o.device.as_deref(), o.out_device.as_deref())
                .map_err(|e| match e {
                    CliError::Usage(m) => CliError::Usage(format!("{want:?}: {m}")),
                    e => e,
                })?;
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

/// The capture and playback devices `session open` asks for. Unnamed capture: the host's
/// default capture device (or the only one). Unnamed playback: the capture device itself
/// when it plays (one device, one clock where the host allows it), else the host's default
/// playback device, else the capture device (which then has no outputs to offer).
fn pick<'a>(
    devices: &[&'a DeviceInfo],
    device: Option<&str>,
    out_device: Option<&str>,
) -> Result<(&'a DeviceInfo, &'a DeviceInfo), CliError> {
    let dev = match device {
        Some(sel) => devsel::named(devices, sel, Direction::Input)?,
        None => match devsel::default_for(devices, Direction::Input) {
            Some(d) => d,
            None => {
                return Err(CliError::Usage(
                    match devsel::count(devices, Direction::Input) {
                        0 => "no capture device listed".to_owned(),
                        n => format!(
                            "{n} capture devices and no system default; choose one with \
                             --device (see `ac2 devices`)"
                        ),
                    },
                ));
            }
        },
    };
    let out_dev = match out_device {
        Some(sel) => devsel::named(devices, sel, Direction::Output)?,
        None if dev.output.is_some() => dev,
        None => devices
            .iter()
            .find(|d| d.output.as_ref().is_some_and(|o| o.system_default))
            .copied()
            .unwrap_or(dev),
    };
    Ok((dev, out_dev))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::devsel::tests::info;

    #[test]
    fn unnamed_devices_follow_the_system_defaults_among_many() {
        let devs = [
            info("{play}", "Speakers (USB Audio)", 0, 2, true),
            info("{rec}", "Speakers (USB Audio)", 2, 0, true),
            info("{mic}", "Microphone (Webcam)", 1, 0, false),
            info("{hdmi}", "HDMI", 0, 8, false),
        ];
        let refs: Vec<&DeviceInfo> = devs.iter().collect();
        let (i, o) = pick(&refs, None, None).expect("defaults");
        assert_eq!((i.id.0.as_str(), o.id.0.as_str()), ("{rec}", "{play}"));

        let (i, o) = pick(
            &refs,
            Some("Speakers (USB Audio)"),
            Some("Speakers (USB Audio)"),
        )
        .expect("same name, both directions");
        assert_eq!((i.id.0.as_str(), o.id.0.as_str()), ("{rec}", "{play}"));

        let (i, o) = pick(&refs, Some("microphone (webcam)"), Some("HDMI")).expect("named");
        assert_eq!((i.id.0.as_str(), o.id.0.as_str()), ("{mic}", "{hdmi}"));
        assert!(pick(&refs, Some("HDMI"), None).is_err());
    }

    #[test]
    fn a_duplex_device_plays_through_itself_and_ambiguity_is_refused() {
        let devs = [
            info("a", "Interface", 8, 8, false),
            info("b", "Speakers", 0, 2, true),
        ];
        let refs: Vec<&DeviceInfo> = devs.iter().collect();
        let (i, o) = pick(&refs, None, None).expect("the only capture device");
        assert_eq!((i.id.0.as_str(), o.id.0.as_str()), ("a", "a"));

        let devs = [info("a", "A", 2, 0, false), info("b", "B", 2, 0, false)];
        let refs: Vec<&DeviceInfo> = devs.iter().collect();
        let e = pick(&refs, None, None).expect_err("refused");
        assert!(e.to_string().contains("choose one with --device"), "{e}");
        assert!(pick(&[], None, None).is_err());
    }
}
