//! Calibration (`cal spl`, `cal curve …`, `cal use`, `cal list`, `cal rm`) and the input
//! setup (`session inputs`, `session open --mic`): `docs/design/q7-calibration.md`.

use ac2_client::{Client, expect_body};
use ac2_proto::cal::input_setup;
use ac2_proto::model::{CalEntry, CalKey, DeviceId, InputSetup, Mic, MicCurveId, State};
use ac2_proto::units::Blob;
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::{connect, state};
use crate::CliError;
use crate::args::{
    CalCmd, CalCurveCmd, CalCurveImport, CalRm, CalSpl, Cli, CurveArg, CurveAssign, MicAssign,
    SessionInputs,
};
use crate::output::{self, Out};
use crate::watch::now_wall;

/// `--mic`, else the input's mic name.
fn mic_for(s: &State, input: u16, given: Option<&String>) -> Result<String, CliError> {
    if let Some(m) = given {
        return Ok(m.clone());
    }
    input_setup(&s.inputs, input).mic.ok_or_else(|| {
        CliError::Usage(format!(
            "input {} has no mic name: give --mic NAME (or set it with `ac2 session inputs --mic {}=NAME`)",
            u32::from(input) + 1,
            u32::from(input) + 1
        ))
    })
}

/// Sends the changed rows of the input setup; returns the state after it. A new mic name
/// starts with no curve chosen (the daemon chooses a one-curve mic's curve).
pub(crate) async fn set_inputs(
    c: &Client,
    mics: &[MicAssign],
    curves: &[CurveAssign],
) -> Result<State, CliError> {
    let s = state(c).await?;
    let mut rows: Vec<InputSetup> = Vec::new();
    let mut edit = |ch: u16, f: &mut dyn FnMut(&mut InputSetup)| {
        if let Some(r) = rows.iter_mut().find(|r| r.channel == ch) {
            f(r);
        } else {
            let mut r = input_setup(&s.inputs, ch);
            f(&mut r);
            rows.push(r);
        }
    };
    for m in mics {
        edit(m.input.0, &mut |r| {
            if r.mic != m.mic {
                r.mic.clone_from(&m.mic);
                r.curve = ac2_proto::model::CurveChoice::NotChosen;
            }
        });
    }
    for c in curves {
        edit(c.input.0, &mut |r| r.curve = c.curve.0.clone());
    }
    let r = c.call(Command::SessionInputs { inputs: rows }).await?;
    expect_body!("session.inputs", r, ReplyBody::Inputs(i) => i)?;
    state(c).await
}

/// The input table of `s`, as text and JSON.
fn emit_inputs(s: &State, out: &mut Out<'_>) -> Result<(), CliError> {
    out.emit(&output::inputs_json(s), || output::inputs(s, now_wall()))?;
    Ok(())
}

/// `session inputs`.
pub(crate) async fn session_inputs(
    cli: &Cli,
    a: &SessionInputs,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let s = if a.mics.is_empty() && a.curves.is_empty() {
        state(&c).await?
    } else {
        set_inputs(&c, &a.mics, &a.curves).await?
    };
    emit_inputs(&s, out)
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

/// What a curve change leaves: the mic's curves and the inputs that use the mic.
fn mic_text(m: &Mic, s: &State) -> String {
    let mut text = output::mics(std::slice::from_ref(m));
    let on: Vec<String> = s
        .inputs
        .iter()
        .filter(|i| i.mic.as_deref() == Some(m.name.as_str()))
        .map(|i| {
            let u = ac2_proto::cal::state_input_use(s, i.channel);
            format!(
                "input {}: {}",
                u32::from(i.channel) + 1,
                ac2_scene::cal::curve_state(&u.curve)
            )
        })
        .collect();
    if !on.is_empty() {
        text.push('\n');
        text.push_str(&on.join("\n"));
    }
    text
}

async fn curve_import(cli: &Cli, a: &CalCurveImport, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let s = state(&c).await?;
    let mic = match (&a.mic, a.input) {
        (Some(m), _) => m.clone(),
        (None, Some(i)) => mic_for(&s, i.0, None)?,
        (None, None) => return Err(CliError::Usage("give --mic NAME or --input N".into())),
    };
    let r = c
        .call(Command::CalCurveImport {
            mic,
            label: a.label.clone(),
            file_name: a.file.file_name().map_or_else(
                || a.file.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            content: Blob(
                std::fs::read(&a.file).map_err(|e| {
                    CliError::Usage(format!("cannot read {}: {e}", a.file.display()))
                })?,
            ),
            input: a.input.map(|i| i.0),
        })
        .await?;
    let m = expect_body!("cal.curve_import", r, ReplyBody::Mic(m) => m)?;
    let s = state(&c).await?;
    out.emit(&m, || mic_text(&m, &s))?;
    Ok(())
}

async fn curve(cli: &Cli, cmd: &CalCurveCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        CalCurveCmd::Import(a) => curve_import(cli, a, out).await,
        CalCurveCmd::Rename {
            mic,
            label,
            new_label,
        } => {
            let c = connect(cli, false).await?;
            let r = c
                .call(Command::CalCurveRename {
                    curve: MicCurveId {
                        mic: mic.clone(),
                        label: label.clone(),
                    },
                    label: new_label.clone(),
                })
                .await?;
            let m = expect_body!("cal.curve_rename", r, ReplyBody::Mic(m) => m)?;
            let s = state(&c).await?;
            out.emit(&m, || mic_text(&m, &s))?;
            Ok(())
        }
        CalCurveCmd::Rm { mic, label } => {
            let c = connect(cli, false).await?;
            let curve = MicCurveId {
                mic: mic.clone(),
                label: label.clone(),
            };
            let r = c
                .call(Command::CalCurveDelete {
                    curve: curve.clone(),
                })
                .await?;
            let rev = expect_body!("cal.curve_delete", r, ReplyBody::Ack { rev } => rev)?;
            out.emit(&json!({ "rev": rev, "deleted": curve }), || {
                format!("mic curve {label} of {mic} deleted")
            })?;
            Ok(())
        }
    }
}

/// `cal use IN LABEL|off`.
async fn use_curve(
    cli: &Cli,
    input: crate::units::Channel,
    curve: &CurveArg,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let s = state(&c).await?;
    if input_setup(&s.inputs, input.0).mic.is_none() {
        return Err(CliError::Usage(format!(
            "input {input} has no mic name: set it first (`ac2 session inputs --mic {input}=NAME`)"
        )));
    }
    let s = set_inputs(
        &c,
        &[],
        &[CurveAssign {
            input,
            curve: curve.clone(),
        }],
    )
    .await?;
    emit_inputs(&s, out)
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
    let r = c.call(Command::CalDelete { key: key.clone() }).await?;
    let rev = expect_body!("cal.delete", r, ReplyBody::Ack { rev } => rev)?;
    out.emit(&json!({ "rev": rev, "key": key }), || {
        format!(
            "sensitivity calibration of {} on input {} of {} deleted",
            key.mic, a.input, key.device.0
        )
    })?;
    Ok(())
}

/// `cal …`.
pub(crate) async fn run(cli: &Cli, cmd: &CalCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        CalCmd::Spl(a) => cal_spl(cli, a, out).await,
        CalCmd::Curve(c) => curve(cli, c, out).await,
        CalCmd::Use { input, curve } => use_curve(cli, *input, curve, out).await,
        CalCmd::Rm(a) => rm(cli, a, out).await,
        CalCmd::List => {
            let c = connect(cli, false).await?;
            let r = c.call(Command::CalList).await?;
            let (l, mics): (Vec<CalEntry>, Vec<Mic>) = expect_body!(
                "cal.list",
                r,
                ReplyBody::Calibrations { calibrations, mics } => (calibrations, mics)
            )?;
            let s = state(&c).await?;
            out.emit(
                &json!({
                    "calibrations": l,
                    "mics": mics,
                    "inputs": output::inputs_json(&s),
                }),
                || {
                    format!(
                        "sensitivity calibrations\n{}\nmic library\n{}\ninputs\n{}",
                        output::calibrations(&l),
                        output::mics(&mics),
                        output::inputs(&s, now_wall())
                    )
                },
            )?;
            Ok(())
        }
    }
}
