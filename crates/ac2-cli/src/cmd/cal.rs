//! Calibration (`cal spl`, `cal electrical`, `cal line`, `cal curve …`, `cal use`, `cal list`,
//! `cal rm`) and the input setup (`session inputs`, `session open --mic`):
//! `docs/design/q7-calibration.md`, `docs/design/programme-meter.md` §1–2.

use ac2_client::{Client, expect_body};
use ac2_proto::cal::input_setup;
use ac2_proto::model::{
    CalEntry, CalKey, CalRef, DeviceId, ElectricalConnection, InputKind, InputSetup, LineCalEntry,
    LineCalKey, Mic, MicCurveId, MicLoading, State,
};
use ac2_proto::units::Blob;
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::{connect, state};
use crate::CliError;
use crate::args::{
    CalCmd, CalCurveCmd, CalCurveImport, CalElectrical, CalLine, CalRm, CalSpl, Cli, CurveArg,
    CurveAssign, ElectricalMethodArg, PairArg, SessionInputs,
};
use crate::output::{self, Out};
use crate::watch::now_wall;

/// `--mic`, else the input's mic name.
fn mic_for(s: &State, input: u16, given: Option<&String>) -> Result<String, CliError> {
    if let Some(m) = given {
        return Ok(m.clone());
    }
    let n = u32::from(input) + 1;
    let kind = input_setup(&s.inputs, input).kind;
    match kind.mic() {
        Some(m) => Ok(m.to_owned()),
        None if matches!(kind, InputKind::Mic { .. }) => Err(CliError::Usage(format!(
            "input {n} has no mic name: give --mic NAME (or set it with `ac2 session inputs --mic {n}=NAME`)"
        ))),
        None => Err(CliError::Usage(format!(
            "input {n} is a {}; a sensitivity calibration is for a microphone (`ac2 session inputs --kind {n}=mic`)",
            kind.word()
        ))),
    }
}

/// The row of `ch` among the rows to send, taken from the daemon's setup on first use.
fn row<'a>(rows: &'a mut Vec<InputSetup>, s: &State, ch: u16) -> &'a mut InputSetup {
    if let Some(i) = rows.iter().position(|r| r.channel == ch) {
        &mut rows[i]
    } else {
        rows.push(input_setup(&s.inputs, ch));
        rows.last_mut().expect("just pushed")
    }
}

/// Sends the changed rows of the input setup and the pair list; returns the state after it.
/// Kinds apply first, so a kind and its mic name go in one command; a mic name or curve on a
/// line or digital input is refused here, since the kind is where a mic's name lives (the
/// daemon would otherwise be asked to turn the input into a mic by the side door). A new mic
/// name starts with no curve chosen (the daemon chooses a one-curve mic's curve).
pub(crate) async fn set_inputs(c: &Client, a: &SessionInputs) -> Result<State, CliError> {
    let s = state(c).await?;
    let mut rows: Vec<InputSetup> = Vec::new();
    for k in &a.kinds {
        let r = row(&mut rows, &s, k.input.0);
        let kind = k.kind.to_kind();
        // Mic → mic keeps the name and curve; any other change replaces the kind whole.
        if std::mem::discriminant(&r.kind) != std::mem::discriminant(&kind) {
            r.kind = kind;
        }
    }
    let not_a_mic = |r: &InputSetup, what: &str| {
        let n = u32::from(r.channel) + 1;
        CliError::Usage(format!(
            "input {n} is a {}: a {what} belongs to a mic (`--kind {n}=mic` makes it one)",
            r.kind.word()
        ))
    };
    for m in &a.mics {
        let r = row(&mut rows, &s, m.input.0);
        if !matches!(r.kind, InputKind::Mic { .. }) {
            return Err(not_a_mic(r, "mic name"));
        }
        r.set_mic(m.mic.clone());
    }
    for cv in &a.curves {
        let r = row(&mut rows, &s, cv.input.0);
        if !matches!(r.kind, InputKind::Mic { .. }) {
            return Err(not_a_mic(r, "mic curve"));
        }
        r.set_curve(cv.curve.0.clone());
    }
    let pairs = if a.no_pairs {
        Vec::new()
    } else if a.pairs.is_empty() {
        s.pairs.clone()
    } else {
        a.pairs.iter().map(PairArg::to_pair).collect()
    };
    let r = c
        .call(Command::SessionInputs {
            inputs: rows,
            pairs,
        })
        .await?;
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
    let s = if a.is_empty() {
        state(&c).await?
    } else {
        set_inputs(&c, a).await?
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

/// `cal electrical`.
async fn cal_electrical(cli: &Cli, a: &CalElectrical, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let s = state(&c).await?;
    let mic = mic_for(&s, a.input.0, a.mic.as_ref())?;
    let connection = match a.method {
        ElectricalMethodArg::Inline => ElectricalConnection::InLine,
        ElectricalMethodArg::Injected => ElectricalConnection::Injected,
    };
    let r = c
        .call(Command::CalSplElectrical {
            input: a.input.0,
            mic,
            connection,
            volts: a.volts.0,
            freq: a.freq.0,
            mic_sensitivity: a.sensitivity.map(|x| x.0),
            loading: a
                .mic_impedance
                .zip(a.input_impedance)
                .map(|(m, i)| MicLoading {
                    mic_impedance: m.0,
                    input_impedance: i.0,
                }),
            uncertainty: a.uncertainty.map(|x| x.0),
            replace_acoustic: a.replace_acoustic,
        })
        .await?;
    let e = expect_body!("cal.spl_electrical", r, ReplyBody::Calibration(e) => e)?;
    out.emit(&e, || output::electrical_calibration(&e, connection))?;
    Ok(())
}

/// `cal line`: the operator's voltmeter reading goes to the daemon, which measures the
/// tone's level and stores the volts at 0 dBFS; the dBu of full scale is printed from them.
async fn cal_line(cli: &Cli, a: &CalLine, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let r = c
        .call(Command::CalLine {
            channel: a.input.0,
            volts: a.volts.0,
        })
        .await?;
    let e = expect_body!("cal.line", r, ReplyBody::LineCalibration(e) => e)?;
    out.emit(
        &json!({
            "key": e.key,
            "line": e.line,
            "dbu_fs": ac2_proto::cal::dbu_fs(e.line.v_fs.0),
        }),
        || output::line_calibration(&e),
    )?;
    Ok(())
}

/// What a curve change leaves: the mic's curves and the inputs that use the mic.
fn mic_text(m: &Mic, s: &State) -> String {
    let mut text = output::mics(std::slice::from_ref(m));
    let on: Vec<String> = s
        .inputs
        .iter()
        .filter(|i| i.kind.mic() == Some(m.name.as_str()))
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
    input: ac2_scene::unit_text::Channel,
    curve: &CurveArg,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let s = state(&c).await?;
    if input_setup(&s.inputs, input.0).kind.mic().is_none() {
        return Err(CliError::Usage(format!(
            "input {input} has no mic name: set it first (`ac2 session inputs --mic {input}=NAME`)"
        )));
    }
    let s = set_inputs(
        &c,
        &SessionInputs {
            curves: vec![CurveAssign {
                input,
                curve: curve.clone(),
            }],
            ..SessionInputs::default()
        },
    )
    .await?;
    emit_inputs(&s, out)
}

/// The device of the calibration `cal rm` names: `--device`, else the open session's
/// capture device, else the one device among `holders` (the devices holding such a
/// calibration for this input).
fn rm_device(
    s: &State,
    a: &CalRm,
    mut holders: Vec<&DeviceId>,
    what: &str,
) -> Result<DeviceId, CliError> {
    if let Some(d) = &a.device {
        return Ok(DeviceId(d.clone()));
    }
    if let Some(o) = &s.session.open {
        return Ok(o.input_device.clone());
    }
    holders.dedup();
    match holders.as_slice() {
        [d] => Ok((*d).clone()),
        [] => Err(CliError::Usage(format!("no {what} on input {}", a.input))),
        many => Err(CliError::Usage(format!(
            "{what}s on input {} exist for several devices ({}): give --device",
            a.input,
            many.iter()
                .map(|d| d.0.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// `cal rm`: a sensitivity calibration by input and mic, or with `--line` the input's line
/// calibration.
async fn rm(cli: &Cli, a: &CalRm, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let s = state(&c).await?;
    // JSON carries the bare key of the entry removed: the flags given say which kind it is,
    // so a tag would only repeat them.
    let (key, key_json, text) = if a.line {
        let holders = s
            .line_calibrations
            .iter()
            .filter(|e| e.key.channel == a.input.0)
            .map(|e| &e.key.device)
            .collect();
        let key = LineCalKey {
            device: rm_device(&s, a, holders, "line calibration")?,
            channel: a.input.0,
        };
        let text = format!(
            "line calibration of input {} of {} deleted",
            a.input, key.device.0
        );
        let key_json = json!(key);
        (CalRef::Line(key), key_json, text)
    } else {
        let mic = mic_for(&s, a.input.0, a.mic.as_ref())?;
        let holders = s
            .calibrations
            .iter()
            .filter(|e| e.key.channel == a.input.0 && e.key.mic == mic)
            .map(|e| &e.key.device)
            .collect();
        let key = CalKey {
            device: rm_device(&s, a, holders, &format!("calibration of {mic}"))?,
            channel: a.input.0,
            mic,
        };
        let text = format!(
            "sensitivity calibration of {} on input {} of {} deleted",
            key.mic, a.input, key.device.0
        );
        let key_json = json!(key);
        (CalRef::Spl(key), key_json, text)
    };
    let r = c.call(Command::CalDelete { key }).await?;
    let rev = expect_body!("cal.delete", r, ReplyBody::Ack { rev } => rev)?;
    out.emit(&json!({ "rev": rev, "key": key_json }), || text)?;
    Ok(())
}

/// `cal …`.
pub(crate) async fn run(cli: &Cli, cmd: &CalCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        CalCmd::Spl(a) => cal_spl(cli, a, out).await,
        CalCmd::Electrical(a) => cal_electrical(cli, a, out).await,
        CalCmd::Line(a) => cal_line(cli, a, out).await,
        CalCmd::Curve(c) => curve(cli, c, out).await,
        CalCmd::Use { input, curve } => use_curve(cli, *input, curve, out).await,
        CalCmd::Rm(a) => rm(cli, a, out).await,
        CalCmd::List => {
            let c = connect(cli, false).await?;
            let r = c.call(Command::CalList).await?;
            let (l, lines, mics): (Vec<CalEntry>, Vec<LineCalEntry>, Vec<Mic>) = expect_body!(
                "cal.list",
                r,
                ReplyBody::Calibrations { calibrations, line_calibrations, mics } => {
                    (calibrations, line_calibrations, mics)
                }
            )?;
            let s = state(&c).await?;
            out.emit(
                &json!({
                    "calibrations": l,
                    "line_calibrations": lines,
                    "mics": mics,
                    "inputs": output::inputs_json(&s),
                }),
                || {
                    format!(
                        "sensitivity calibrations\n{}\nline calibrations\n{}\nmic library\n{}\ninputs\n{}",
                        output::calibrations(&l),
                        output::line_calibrations(&lines, now_wall()),
                        output::mics(&mics),
                        output::inputs(&s, now_wall())
                    )
                },
            )?;
            Ok(())
        }
    }
}
