//! Calibration (`cal spl`, `cal mic-curve`, `cal list`, `cal rm`) and the input setup
//! (`session inputs`, `session open --mic`): `docs/design/q7-calibration.md`.

use ac2_client::{Client, expect_body};
use ac2_proto::model::{CalEntry, CalKey, CalPart, DeviceId, InputSetup, MicCurveAction, State};
use ac2_proto::units::Blob;
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::{connect, state};
use crate::CliError;
use crate::args::{CalCmd, CalMicCurve, CalRm, CalSpl, Cli, CurveSwitch, MicAssign, SessionInputs};
use crate::output::{self, Out};

/// The input setup row of `channel`, or the daemon's default (no name, curve on).
fn row(s: &State, channel: u16) -> InputSetup {
    s.inputs
        .iter()
        .find(|i| i.channel == channel)
        .cloned()
        .unwrap_or(InputSetup {
            channel,
            mic: None,
            mic_curve: true,
        })
}

/// `--mic`, else the input's mic name.
fn mic_for(s: &State, input: u16, given: Option<&String>) -> Result<String, CliError> {
    if let Some(m) = given {
        return Ok(m.clone());
    }
    row(s, input).mic.ok_or_else(|| {
        CliError::Usage(format!(
            "input {} has no mic name: give --mic NAME (or set it with `ac2 session inputs --mic {}=NAME`)",
            u32::from(input) + 1,
            u32::from(input) + 1
        ))
    })
}

/// Sends the changed rows of the input setup; returns the whole setup.
pub(crate) async fn set_inputs(
    c: &Client,
    mics: &[MicAssign],
    curves: &[CurveSwitch],
) -> Result<Vec<InputSetup>, CliError> {
    let s = state(c).await?;
    let mut rows: Vec<InputSetup> = Vec::new();
    let mut edit = |ch: u16, f: &mut dyn FnMut(&mut InputSetup)| {
        if let Some(r) = rows.iter_mut().find(|r| r.channel == ch) {
            f(r);
        } else {
            let mut r = row(&s, ch);
            f(&mut r);
            rows.push(r);
        }
    };
    for m in mics {
        edit(m.input.0, &mut |r| r.mic.clone_from(&m.mic));
    }
    for c in curves {
        edit(c.input.0, &mut |r| r.mic_curve = c.on);
    }
    let r = c.call(Command::SessionInputs { inputs: rows }).await?;
    Ok(expect_body!("session.inputs", r, ReplyBody::Inputs(i) => i)?)
}

/// `session inputs`.
pub(crate) async fn session_inputs(
    cli: &Cli,
    a: &SessionInputs,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let inputs = if a.mics.is_empty() && a.curves.is_empty() {
        state(&c).await?.inputs
    } else {
        set_inputs(&c, &a.mics, &a.curves).await?
    };
    out.emit(&inputs, || output::inputs(&inputs))?;
    Ok(())
}

/// `cal spl` / `spl cal`.
pub(crate) async fn cal_spl(cli: &Cli, a: &CalSpl, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let s = state(&c).await?;
    let mic = mic_for(&s, a.input.0, a.mic.as_ref())?;
    let r = c
        .call(Command::CalSpl {
            input: a.input.0,
            mic,
            calibrator_level: a.reference.0,
            calibrator_freq: a.freq.0,
        })
        .await?;
    let e = expect_body!("cal.spl", r, ReplyBody::Calibration(e) => e)?;
    out.emit(&e, || output::calibrations(std::slice::from_ref(&e)))?;
    Ok(())
}

async fn mic_curve(cli: &Cli, a: &CalMicCurve, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let s = state(&c).await?;
    let mic = mic_for(&s, a.input.0, a.mic.as_ref())?;
    let action = match &a.file {
        Some(path) => MicCurveAction::Import {
            file_name: path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            content: Blob(
                std::fs::read(path)
                    .map_err(|e| CliError::Usage(format!("cannot read {}: {e}", path.display())))?,
            ),
        },
        None => MicCurveAction::Clear,
    };
    let r = c
        .call(Command::CalMicCurve {
            input: a.input.0,
            mic: mic.clone(),
            action,
        })
        .await?;
    match r {
        ReplyBody::Calibration(e) => {
            out.emit(&e, || output::calibrations(std::slice::from_ref(&e)))?;
        }
        other => {
            let rev = expect_body!("cal.mic_curve", other, ReplyBody::Ack { rev } => rev)?;
            out.emit(&json!({ "rev": rev, "cleared": true }), || {
                format!("mic curve of {mic} on input {} cleared", a.input)
            })?;
        }
    }
    Ok(())
}

/// The device of the calibration `cal rm` names: `--device`, else the open session's
/// capture device, else the one device holding a calibration for this input and mic.
fn rm_device(s: &State, a: &CalRm, mic: &str) -> Result<DeviceId, CliError> {
    if let Some(d) = &a.device {
        return Ok(DeviceId(d.clone()));
    }
    if let Some(o) = &s.session.open {
        return Ok(o.input_device.clone());
    }
    let mut devices: Vec<&DeviceId> = s
        .calibrations
        .iter()
        .filter(|e| e.key.channel == a.input.0 && e.key.mic == mic)
        .map(|e| &e.key.device)
        .collect();
    devices.dedup();
    match devices.as_slice() {
        [d] => Ok((*d).clone()),
        [] => Err(CliError::Usage(format!(
            "no calibration of {mic} on input {}",
            a.input
        ))),
        many => Err(CliError::Usage(format!(
            "calibrations of {mic} on input {} exist for several devices ({}): give --device",
            a.input,
            many.iter()
                .map(|d| d.0.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// `cal rm`.
async fn rm(cli: &Cli, a: &CalRm, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let s = state(&c).await?;
    let mic = mic_for(&s, a.input.0, a.mic.as_ref())?;
    let key = CalKey {
        device: rm_device(&s, a, &mic)?,
        channel: a.input.0,
        mic,
    };
    let part = match (a.sensitivity, a.curve) {
        (true, _) => CalPart::Sensitivity,
        (_, true) => CalPart::MicCurve,
        _ => CalPart::All,
    };
    let r = c
        .call(Command::CalDelete {
            key: key.clone(),
            part,
        })
        .await?;
    let rev = expect_body!("cal.delete", r, ReplyBody::Ack { rev } => rev)?;
    let what = match part {
        CalPart::Sensitivity => "sensitivity calibration",
        CalPart::MicCurve => "mic curve",
        CalPart::All => "calibration",
    };
    out.emit(&json!({ "rev": rev, "key": key, "deleted": part }), || {
        format!(
            "{what} of {} on input {} of {} deleted",
            key.mic, a.input, key.device.0
        )
    })?;
    Ok(())
}

/// `cal …`.
pub(crate) async fn run(cli: &Cli, cmd: &CalCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        CalCmd::Spl(a) => cal_spl(cli, a, out).await,
        CalCmd::MicCurve(a) => mic_curve(cli, a, out).await,
        CalCmd::Rm(a) => rm(cli, a, out).await,
        CalCmd::List => {
            let c = connect(cli, false).await?;
            let r = c.call(Command::CalList).await?;
            let l: Vec<CalEntry> = expect_body!("cal.list", r, ReplyBody::Calibrations(l) => l)?;
            let inputs = state(&c).await?.inputs;
            out.emit(&json!({ "calibrations": l, "inputs": inputs }), || {
                format!("{}\n{}", output::calibrations(&l), output::inputs(&inputs))
            })?;
            Ok(())
        }
    }
}
