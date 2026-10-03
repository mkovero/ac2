//! `trace …` and `session save|load|list`.

use std::path::{Path, PathBuf};

use ac2_client::{Client, expect_body};
use ac2_proto::model::*;
use ac2_proto::units::Blob;
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::{connect, find_meas, find_trace, is_local, state};
use crate::CliError;
use crate::args::*;
use crate::output::{self, Out};

fn method(m: AverageArg) -> AverageMethod {
    match m {
        AverageArg::Power => AverageMethod::Power,
        AverageArg::Complex => AverageMethod::Complex,
        AverageArg::Coherence => AverageMethod::CoherenceWeighted,
    }
}

pub(crate) async fn trace(cli: &Cli, cmd: &TraceCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    match cmd {
        TraceCmd::Capture { meas, name, slot } => {
            let s = state(&c).await?;
            let id = find_meas(&s, meas)?.id;
            let r = c
                .call(Command::TraceCapture {
                    meas: id,
                    name: name.clone(),
                    slot: *slot,
                })
                .await?;
            let t = expect_body!("trace.capture", r, ReplyBody::Trace(t) => t)?;
            out.emit(&t, || output::traces(std::slice::from_ref(&t)))?;
        }
        TraceCmd::List => {
            let r = c.call(Command::TraceList).await?;
            let l = expect_body!("trace.list", r, ReplyBody::Traces(l) => l)?;
            out.emit(&l, || output::traces(&l))?;
        }
        TraceCmd::Show { trace, data } => {
            let s = state(&c).await?;
            let t = find_trace(&s, trace)?.clone();
            if *data {
                let r = c.call(Command::TraceGet { trace: t.id }).await?;
                let d = expect_body!("trace.get", r, ReplyBody::TraceData(d) => d)?;
                let grid = c.grid(t.grid_id).await?;
                out.emit(&d, || {
                    format!(
                        "{}\n{}",
                        output::trace_meta(&t),
                        output::trace_columns(&d, &grid)
                    )
                })?;
            } else {
                out.emit(&t, || output::trace_meta(&t))?;
            }
        }
        TraceCmd::Rm { traces } => {
            let s = state(&c).await?;
            let ids = traces
                .iter()
                .map(|r| find_trace(&s, r).map(|t| t.id))
                .collect::<Result<Vec<_>, _>>()?;
            for id in &ids {
                c.call(Command::TraceDelete { trace: *id }).await?;
            }
            out.emit(&json!({ "deleted": ids }), || {
                format!(
                    "deleted {}",
                    ids.iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        }
        TraceCmd::Average {
            traces,
            name,
            method: m,
            reference,
            ref_delay,
        } => {
            let s = state(&c).await?;
            let ids = traces
                .iter()
                .map(|r| find_trace(&s, r).map(|t| t.id))
                .collect::<Result<Vec<_>, _>>()?;
            // Decision 8b: the selected trace's measured delay, else an explicit one.
            let reference = match (reference, ref_delay) {
                (_, Some(d)) => DelayReference::Fixed { delay: d.0 },
                (Some(r), None) => DelayReference::Trace {
                    trace: find_trace(&s, r)?.id,
                },
                (None, None) => DelayReference::Trace { trace: ids[0] },
            };
            let r = c
                .call(Command::TraceAverage {
                    traces: ids,
                    method: method(*m),
                    reference,
                    name: name.clone(),
                })
                .await?;
            let t = expect_body!("trace.average", r, ReplyBody::Trace(t) => t)?;
            out.emit(&t, || output::traces(std::slice::from_ref(&t)))?;
        }
        TraceCmd::Math {
            a,
            b,
            name,
            complex,
        } => {
            let s = state(&c).await?;
            let (a, b) = (find_trace(&s, a)?.id, find_trace(&s, b)?.id);
            let op = if *complex {
                MathOp::ComplexDivision
            } else {
                MathOp::MagnitudeDifference
            };
            let r = c
                .call(Command::TraceMath {
                    a,
                    b,
                    op,
                    name: name.clone(),
                })
                .await?;
            let t = expect_body!("trace.math", r, ReplyBody::Trace(t) => t)?;
            out.emit(&t, || output::traces(std::slice::from_ref(&t)))?;
        }
        TraceCmd::Import {
            file,
            target,
            format,
            name,
        } => {
            let content = std::fs::read(file)
                .map_err(|e| CliError::Usage(format!("cannot read {}: {e}", file.display())))?;
            let file_name = file.file_name().map_or_else(
                || file.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            let r = c
                .call(Command::TraceImport {
                    file_name,
                    format: match format {
                        ImportFormatArg::Auto => ImportFormat::Auto,
                        ImportFormatArg::Ac2 => ImportFormat::Ac2Csv,
                        ImportFormatArg::Text => ImportFormat::AnalyzerText,
                    },
                    role: if *target {
                        ImportRole::Target
                    } else {
                        ImportRole::Trace
                    },
                    content: Blob(content),
                })
                .await?;
            let mut t = expect_body!("trace.import", r, ReplyBody::Trace(t) => t)?;
            if let Some(n) = name {
                let mut edit = t.edit.clone();
                edit.name = n.clone();
                let r = c.call(Command::TraceUpdate { trace: t.id, edit }).await?;
                t = expect_body!("trace.update", r, ReplyBody::Trace(t) => t)?;
            }
            out.emit(&t, || {
                let mut s = output::traces(std::slice::from_ref(&t));
                if let TraceSource::Imported { notes, .. } = &t.source {
                    for n in notes {
                        s.push_str("\nnote: ");
                        s.push_str(ac2_scene::trace::import_note(*n));
                    }
                }
                s
            })?;
        }
        TraceCmd::Export { trace, csv } => {
            let s = state(&c).await?;
            let t = find_trace(&s, trace)?;
            let r = c
                .call(Command::TraceExport {
                    trace: t.id,
                    format: ExportFormat::Ac2Csv,
                })
                .await?;
            let (file_name, content) = expect_body!(
                "trace.export", r, ReplyBody::Export { file_name, content } => (file_name, content)
            )?;
            if csv.as_os_str() == "-" {
                // The CSV itself is the output; nothing else may be mixed in.
                out.w.write_all(&content.0)?;
                out.w.flush()?;
                return Ok(());
            }
            std::fs::write(csv, &content.0)?;
            let n = content.0.len();
            out.emit(
                &json!({ "trace": t.id, "file": csv, "bytes": n, "suggested_name": file_name }),
                || format!("wrote {} ({n} bytes)", csv.display()),
            )?;
        }
        TraceCmd::Smooth {
            trace,
            fraction,
            phase,
            magnitude_only,
        } => {
            let s = state(&c).await?;
            let t = find_trace(&s, trace)?.clone();
            let mut edit = t.edit.clone();
            edit.smoothing = fraction.0.map(|fraction| Smoothing {
                fraction,
                mode: if *phase {
                    SmoothingMode::MagnitudePhase
                } else if *magnitude_only {
                    SmoothingMode::Magnitude
                } else {
                    t.edit
                        .smoothing
                        .map_or(SmoothingMode::MagnitudePhase, |s| s.mode)
                },
            });
            let r = c.call(Command::TraceUpdate { trace: t.id, edit }).await?;
            let t = expect_body!("trace.update", r, ReplyBody::Trace(t) => t)?;
            out.emit(&t, || output::trace_meta(&t))?;
        }
        TraceCmd::Display {
            trace,
            state: shown,
        } => {
            let s = state(&c).await?;
            let t = find_trace(&s, trace)?.clone();
            let mut edit = t.edit.clone();
            edit.visible = *shown == Shown::On;
            let r = c.call(Command::TraceUpdate { trace: t.id, edit }).await?;
            let t = expect_body!("trace.update", r, ReplyBody::Trace(t) => t)?;
            out.emit(&t, || output::traces(std::slice::from_ref(&t)))?;
        }
        TraceCmd::Rename { trace, name } => {
            let name = name.trim();
            if name.is_empty() {
                return Err(CliError::Usage("a trace name cannot be empty".into()));
            }
            let s = state(&c).await?;
            let t = find_trace(&s, trace)?.clone();
            let mut edit = t.edit.clone();
            edit.name = name.to_string();
            let r = c.call(Command::TraceUpdate { trace: t.id, edit }).await?;
            let t = expect_body!("trace.update", r, ReplyBody::Trace(t) => t)?;
            out.emit(&t, || output::traces(std::slice::from_ref(&t)))?;
        }
        TraceCmd::Slot { trace, slot } => {
            let s = state(&c).await?;
            let t = find_trace(&s, trace)?.clone();
            let mut edit = t.edit.clone();
            edit.slot = slot.0;
            let r = c.call(Command::TraceUpdate { trace: t.id, edit }).await?;
            let t = expect_body!("trace.update", r, ReplyBody::Trace(t) => t)?;
            out.emit(&t, || output::traces(std::slice::from_ref(&t)))?;
        }
        TraceCmd::Mic { trace, mic, label } => {
            let s = state(&c).await?;
            let id = find_trace(&s, trace)?.id;
            let curve = if mic.trim().eq_ignore_ascii_case("none") {
                None
            } else {
                Some(curve_of(&s, mic, label.as_deref())?)
            };
            let r = c.call(Command::TraceMicCurve { trace: id, curve }).await?;
            let t = expect_body!("trace.mic_curve", r, ReplyBody::Trace(t) => t)?;
            out.emit(&t, || output::trace_meta(&t))?;
        }
    }
    Ok(())
}

/// The curve `label` of `mic`; without a label, the mic's only curve.
fn curve_of(
    s: &ac2_proto::model::State,
    mic: &str,
    label: Option<&str>,
) -> Result<ac2_proto::model::MicCurveId, CliError> {
    let id = |label: &str| ac2_proto::model::MicCurveId {
        mic: mic.to_owned(),
        label: label.to_owned(),
    };
    if let Some(l) = label {
        return Ok(id(l));
    }
    match ac2_proto::cal::mic(&s.mics, mic).map(|m| m.curves.as_slice()) {
        Some([only]) => Ok(id(&only.label)),
        Some(many) if !many.is_empty() => Err(CliError::Usage(format!(
            "{mic} has {} curves ({}): choose one with --label",
            many.len(),
            ac2_scene::cal::labels(many)
        ))),
        _ => Err(CliError::Usage(format!(
            "no curve of {mic:?} in the mic library (import one with `ac2 cal curve import`)"
        ))),
    }
}

/// A session argument: a plain name, or a path (anything with a separator, `.`/`~` start,
/// or absolute). A local path is made absolute here: the daemon does not share the
/// client's working directory.
pub(crate) fn session_ref(arg: &str, local: bool) -> Result<SessionRef, CliError> {
    let is_path = arg.contains('/')
        || arg.contains('\\')
        || arg.starts_with('.')
        || arg.starts_with('~')
        || Path::new(arg).is_absolute();
    if !is_path {
        return Ok(SessionRef::Name {
            name: arg.to_owned(),
        });
    }
    let p = match arg.strip_prefix('~') {
        Some(rest) if local => {
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .ok_or_else(|| CliError::Usage("no home directory to expand ~".into()))?;
            PathBuf::from(home).join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(arg),
    };
    let p = if local {
        std::path::absolute(&p)?
    } else if p.is_absolute() {
        p
    } else {
        return Err(CliError::Usage(
            "a remote daemon takes session names, or absolute paths on its host".into(),
        ));
    };
    Ok(SessionRef::Path {
        path: p.to_string_lossy().into_owned(),
    })
}

/// `session save` / `session load`.
pub(crate) async fn save_or_load(
    cli: &Cli,
    c: &Client,
    session: &str,
    load: bool,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let local = is_local(cli) || cli.ctrl_endpoint.is_some();
    let session = session_ref(session, local)?;
    let (r, what) = if load {
        (
            c.call(Command::FileLoad { session }).await?,
            "loaded (disarmed)",
        )
    } else {
        (c.call(Command::FileSave { session }).await?, "saved")
    };
    let f = expect_body!("file", r, ReplyBody::SessionFile(f) => f)?;
    out.emit(&f, || {
        format!(
            "session {:?} {what}: {} measurement(s), {} trace(s)\n  {}",
            f.name, f.measurements, f.traces, f.path
        )
    })?;
    Ok(())
}

/// `session list`.
pub(crate) async fn list(c: &Client, out: &mut Out<'_>) -> Result<(), CliError> {
    let r = c.call(Command::FileList).await?;
    let l = expect_body!("file.list", r, ReplyBody::Sessions(l) => l)?;
    out.emit(&l, || output::sessions(&l))?;
    Ok(())
}
