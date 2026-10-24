//! `ac2 osc target|test`, `ac2 cue`, `ac2 rig status|monitor-at-start` and `ac2 meas
//! scope`: the rig monitor and its cues.

use ac2_client::{Client, ClientError, expect_body};
use ac2_proto::model::*;
use ac2_proto::units::{Hz, Rev};
use ac2_proto::{Command, ErrorCode, ReplyBody, Warning};
use serde_json::json;

use super::leq::spl_config;
use super::{connect, find_meas, state};
use crate::CliError;
use crate::args::*;
use crate::output::Out;

pub(crate) async fn osc(cli: &Cli, cmd: &OscCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    match cmd {
        OscCmd::Target { addr } => {
            let addr = addr.0;
            let rig = rig_edit(&c, |base| Command::OscTarget { base, addr }).await?;
            out.emit(&rig, || match rig.osc_target {
                Some(a) => format!("OSC cues go to {a}"),
                None => "OSC target off: no cues are sent".to_owned(),
            })?;
        }
        OscCmd::Test { path, args } => {
            let msg = OscMessage {
                path: path.0.clone(),
                args: args.iter().map(|a| a.0.clone()).collect(),
                append_level: false,
            };
            let r = c.call(Command::OscTest { msg: msg.clone() }).await?;
            let rev = expect_body!("osc.test", r, ReplyBody::Ack { rev } => rev)?;
            let rig = state(&c).await?.rig;
            let target = rig
                .osc_target
                .map_or_else(|| "?".to_owned(), |a| a.to_string());
            out.emit(&json!({ "rev": rev, "sent": msg, "osc": rig.osc }), || {
                let mut s = format!("sent {} to {target}", message_text(&msg));
                if let Some(e) = &rig.osc.last_error {
                    s.push_str(&format!("\nlast error: {e}"));
                }
                s
            })?;
        }
    }
    Ok(())
}

pub(crate) async fn rig(cli: &Cli, cmd: &RigCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    match cmd {
        RigCmd::MonitorAtStart { on } => {
            let on = on.on();
            let rig = rig_edit(&c, |base| Command::RigMonitorStart { base, on }).await?;
            out.emit(&rig, || {
                if rig.monitor_at_start {
                    "monitor at start on: a restarted daemon reopens the last session's \
                     device capture-only for the rig SPL meters"
                        .to_owned()
                } else {
                    "monitor at start off: a restarted daemon leaves the audio closed".to_owned()
                }
            })?;
        }
        RigCmd::Status => {
            let st = state(&c).await?;
            let meters: Vec<_> = st
                .measurements
                .iter()
                .filter(|m| m.config.scope == MeasScope::Rig)
                .map(|m| {
                    let cues: Vec<_> = st
                        .rig
                        .cues
                        .iter()
                        .filter(|q| q.meas == m.id)
                        .map(|q| (q.subject.to_string(), q.state))
                        .collect();
                    (m, cues)
                })
                .collect();
            let json = json!({
                "osc_target": st.rig.osc_target.map(|a| a.to_string()),
                "artnet_target": st.rig.artnet_target,
                "artnet_fade": st.rig.artnet_fade,
                "monitor_at_start": st.rig.monitor_at_start,
                "osc": st.rig.osc,
                "artnet": st.rig.artnet,
                "dmx": st.rig.dmx,
                "meters": meters.iter().map(|(m, cues)| json!({
                    "id": m.id,
                    "name": m.config.name,
                    "running": m.running,
                    "cues": cues.iter().map(|(s, q)| json!({ "subject": s, "state": q }))
                        .collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            });
            out.emit(&json, || {
                let mut s = format!(
                    "OSC target: {}\nsent: {}\nlast error: {}\n",
                    st.rig
                        .osc_target
                        .map_or_else(|| "off".to_owned(), |a| a.to_string()),
                    st.rig.osc.sent,
                    st.rig.osc.last_error.as_deref().unwrap_or("none"),
                );
                s.push_str(&super::artnet::status_text(&st.rig));
                s.push_str(&super::levels::status_text(&st.rig));
                s.push_str(&format!(
                    "monitor at start: {}\n",
                    if st.rig.monitor_at_start { "on" } else { "off" }
                ));
                if meters.is_empty() {
                    s.push_str("rig SPL meters: none (`ac2 meas scope <meter> rig`)");
                }
                for (m, cues) in &meters {
                    let run = if m.running { "" } else { ", stopped" };
                    s.push_str(&format!("{} ({}){run}: ", m.config.name, m.id.0));
                    if cues.is_empty() {
                        s.push_str("no cues\n");
                    } else {
                        let c: Vec<String> = cues.iter().map(|(l, q)| format!("{l} {q}")).collect();
                        s.push_str(&format!("{}\n", c.join(", ")));
                    }
                }
                s
            })?;
        }
    }
    Ok(())
}

/// `meas scope`.
pub(crate) async fn meas_scope(
    c: &Client,
    meas: &MeasRef,
    scope: ScopeArg,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let scope = MeasScope::from(scope);
    let m = meas_edit(c, meas, |m| {
        if scope == MeasScope::Rig {
            spl_config(m)?;
        }
        Ok(MeasConfig {
            scope,
            ..m.config.clone()
        })
    })
    .await?;
    out.emit(&m, || match m.config.scope {
        MeasScope::Rig => format!(
            "{} ({}) is the rig's: kept across session loads and restarts",
            m.config.name, m.id.0
        ),
        MeasScope::Session => format!(
            "{} ({}) is the session's: saved in session files, replaced by a load",
            m.config.name, m.id.0
        ),
    })?;
    Ok(())
}

/// `cue`.
pub(crate) async fn cue(cli: &Cli, a: &CueArgs, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = &connect(cli, false).await?;
    let show_only = !a.clear
        && CueState::ALL.iter().all(|s| {
            let (m, d) = a.given(*s);
            m.is_none() && d.is_none()
        });
    if show_only {
        // Nothing to change: show the cue as it is.
        let st = state(c).await?;
        let m = find_meas(&st, &a.meas)?;
        let mut cfg = spl_config(m)?.clone();
        let (subject, slot) = cue_slot(&mut cfg, a.subject)?;
        let cue = slot.as_deref().cloned();
        return show_cue(out, m, subject, cue.as_ref());
    }
    let mut subject = CueSubject::Meter;
    let m = meas_edit(c, &a.meas, |m| {
        let mut cfg = spl_config(m)?.clone();
        let (s, slot) = cue_slot(&mut cfg, a.subject)?;
        subject = s;
        *slot = edited(slot.as_deref(), a).map(Box::new);
        Ok(MeasConfig {
            kind: MeasKind::Spl { config: cfg },
            ..m.config.clone()
        })
    })
    .await?;
    let mut cfg = spl_config(&m)?.clone();
    let (_, slot) = cue_slot(&mut cfg, a.subject)?;
    let cue = slot.as_deref().cloned();
    show_cue(out, &m, subject, cue.as_ref())
}

fn show_cue(
    out: &mut Out<'_>,
    m: &Measurement,
    subject: CueSubject,
    cue: Option<&Cue>,
) -> Result<(), CliError> {
    let label = subject.to_string();
    out.emit(
        &json!({ "meas": m.id, "subject": subject, "label": label, "cue": cue }),
        || {
            let head = format!("{} ({}) {label}", m.config.name, m.id.0);
            match cue {
                None => format!("{head}: no cue"),
                Some(q) => {
                    let mut lines = vec![format!("{head}:")];
                    for st in CueState::ALL {
                        lines.push(format!("  {st:<5} {}", action_text(q.get(st))));
                    }
                    lines.join("\n")
                }
            }
        },
    )?;
    Ok(())
}

/// One state's action as `cue` shows it: `/light/red i:1 + dB SPL · dmx 12=255 13=0`, or
/// `none`.
fn action_text(a: Option<&CueAction>) -> String {
    let Some(a) = a.filter(|a| !a.is_empty()) else {
        return "none".to_owned();
    };
    let mut parts = Vec::new();
    if let Some(m) = &a.osc {
        parts.push(message_text(m));
    }
    if !a.dmx.is_empty() {
        parts.push(format!("dmx {}", dmx_list_text(&a.dmx)));
    }
    parts.join(" · ")
}

/// `old` with the messages and levels `a` gives; a state left with neither is `None`, and
/// the cue is `None` when no state does anything.
fn edited(old: Option<&Cue>, a: &CueArgs) -> Option<Cue> {
    if a.clear {
        return None;
    }
    let mut q = old.cloned().unwrap_or_default();
    for st in CueState::ALL {
        let (msg, dmx) = a.given(st);
        let slot = q.get_mut(st);
        let mut act = slot.take().unwrap_or_default();
        if let Some(m) = msg {
            act.osc = m.message(a.append_level);
        }
        if let Some(d) = dmx {
            act.dmx = d.0.clone();
        }
        *slot = (!act.is_empty()).then_some(act);
    }
    (q != Cue::default()).then_some(q)
}

/// The subject `s` names in `cfg`, and its cue to edit.
fn cue_slot(
    cfg: &mut SplConfig,
    s: CueSubjectArg,
) -> Result<(CueSubject, &mut Option<Box<Cue>>), CliError> {
    let usage = |m: String| CliError::Usage(m);
    let same = |a: f64, b: u32| (a - f64::from(b)).abs() < 1e-6;
    match s {
        CueSubjectArg::Meter => Ok((CueSubject::Meter, &mut cfg.cue)),
        CueSubjectArg::WindowIndex(n) => {
            let names = window_names(cfg);
            let w = cfg.leq.windows.get_mut(n - 1).ok_or_else(|| {
                usage(format!(
                    "window:{n}: the meter has {} Leq window(s) ({})",
                    names.len(),
                    windows_text(&names)
                ))
            })?;
            Ok((
                CueSubject::Window {
                    duration: w.duration,
                    weighting: w.weighting,
                },
                &mut w.cue,
            ))
        }
        CueSubjectArg::Window(want) => {
            let names = window_names(cfg);
            let w = cfg
                .leq
                .windows
                .iter_mut()
                .find(|w| w.weighting == want.weighting() && same(w.duration.0, want.seconds))
                .ok_or_else(|| {
                    usage(format!(
                        "the meter has no {} window ({}); `ac2 spl leq set --windows` sets them",
                        ac2_scene::band_leq::window_name(f64::from(want.seconds), want.weighting()),
                        windows_text(&names)
                    ))
                })?;
            Ok((
                CueSubject::Window {
                    duration: w.duration,
                    weighting: w.weighting,
                },
                &mut w.cue,
            ))
        }
        CueSubjectArg::Peak(q) => {
            let name = q.name();
            let p = cfg.leq.peaks.get_mut(q).as_mut().ok_or_else(|| {
                usage(format!(
                    "the meter has no {name} limit; `ac2 spl leq set --peak {}=<db>` sets one",
                    name.to_ascii_lowercase()
                ))
            })?;
            Ok((CueSubject::Peak { quantity: q }, &mut p.cue))
        }
        CueSubjectArg::Band { band, window } => {
            let hz = BAND_NOMINAL_HZ[band];
            let weighting = window.weighting();
            let label =
                ac2_scene::band_leq::band_window_name(hz, f64::from(window.seconds), weighting);
            let b = cfg.bands.as_mut().ok_or_else(|| {
                usage("the meter's band meter is off; `ac2 spl bands set` turns it on".into())
            })?;
            let w = b
                .windows
                .iter_mut()
                .find(|w| {
                    w.band == Hz(hz)
                        && w.weighting == weighting
                        && same(w.duration.0, window.seconds)
                })
                .ok_or_else(|| {
                    usage(format!(
                        "the band meter has no {label} window; `ac2 spl bands` lists them"
                    ))
                })?;
            Ok((
                CueSubject::Band {
                    duration: w.duration,
                    weighting: w.weighting,
                    nominal: w.band,
                },
                &mut w.cue,
            ))
        }
        CueSubjectArg::Predicted => {
            let p = cfg
                .bands
                .as_mut()
                .and_then(|b| b.predicted.as_mut())
                .ok_or_else(|| {
                    usage(
                        "the meter has no predicted window; `ac2 spl bands set --preset` sets one"
                            .into(),
                    )
                })?;
            Ok((CueSubject::Predicted, &mut p.cue))
        }
    }
}

/// The meter's Leq windows by name, in order (`window:1` first).
fn window_names(cfg: &SplConfig) -> Vec<String> {
    cfg.leq
        .windows
        .iter()
        .map(|w| ac2_scene::band_leq::window_name(w.duration.0, w.weighting))
        .collect()
}

fn windows_text(names: &[String]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}

/// A message as `cue` takes it: `/light/red i:1 f:0.5 s:stage`, `+ dB SPL` when the level
/// is appended.
pub(crate) fn message_text(m: &OscMessage) -> String {
    let mut s = m.path.to_string();
    for a in &m.args {
        s.push(' ');
        s.push_str(&match a {
            OscArg::Int(i) => format!("i:{i}"),
            OscArg::Float(f) => format!("f:{f}"),
            OscArg::Str(t) => format!("s:{t}"),
        });
    }
    if m.append_level {
        s.push_str(" + dB SPL");
    }
    s
}

fn conflict(e: &CliError) -> bool {
    matches!(e, CliError::Client(ClientError::Daemon(p)) if p.code == ErrorCode::Conflict)
}

/// A rev-guarded rig edit made on the current `rig.rev`; on `conflict` (another client
/// edited the rig meanwhile) made once more on the new one. Returns the rig after it.
pub(super) async fn rig_edit(c: &Client, cmd: impl Fn(Rev) -> Command) -> Result<Rig, CliError> {
    let mut tries = 0;
    loop {
        let base = state(c).await?.rig.rev;
        let r = match c.call(cmd(base)).await {
            Ok(r) => r,
            Err(e) => {
                let e = CliError::from(e);
                tries += 1;
                if tries < 2 && conflict(&e) {
                    continue;
                }
                return Err(e);
            }
        };
        expect_body!("rig", r, ReplyBody::Ack { rev: _ } => ())?;
        return Ok(state(c).await?.rig);
    }
}

/// `meas.update` of the measurement `meas` names, with the config `edit` makes of it, on
/// its current `config_rev`; on `conflict` (edited meanwhile) re-read and made once more.
async fn meas_edit(
    c: &Client,
    meas: &MeasRef,
    mut edit: impl FnMut(&Measurement) -> Result<MeasConfig, CliError>,
) -> Result<Measurement, CliError> {
    let mut tries = 0;
    loop {
        let st = state(c).await?;
        let m = find_meas(&st, meas)?;
        let config = edit(m)?;
        let r = c
            .call(Command::MeasUpdate {
                meas: m.id,
                base: m.config_rev,
                config,
            })
            .await;
        match r {
            Ok(r) => return Ok(expect_body!("meas.update", r, ReplyBody::Measurement(m) => m)?),
            Err(e) => {
                let e = CliError::from(e);
                tries += 1;
                if tries < 2 && conflict(&e) {
                    continue;
                }
                return Err(e);
            }
        }
    }
}

/// Notes the reply's warnings in `out`, naming the measurements they list.
pub(crate) async fn note_warnings(c: &Client, warnings: &[Warning], out: &mut Out<'_>) {
    if warnings.is_empty() {
        return;
    }
    let st = state(c).await.ok();
    out.warnings
        .extend(warnings.iter().map(|w| warning_text(w, st.as_ref())));
}

/// A reply warning in words, the measurements by name where `st` knows them.
pub(crate) fn warning_text(w: &Warning, st: Option<&State>) -> String {
    let names = |ids: &[ac2_proto::units::MeasId]| {
        ids.iter()
            .map(|id| {
                st.and_then(|s| s.measurements.iter().find(|m| m.id == *id))
                    .map_or_else(
                        || format!("{}", id.0),
                        |m| format!("{} ({})", m.config.name, id.0),
                    )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    match w {
        Warning::RigMonitorBlind { meas } => format!(
            "rig SPL meter(s) {} stop metering until a session opens again; their cues report \
             fault",
            names(meas)
        ),
        Warning::RigMonitorMoved { meas } => format!(
            "rig SPL meter(s) {} now meter this device's inputs",
            names(meas)
        ),
    }
}
