//! `trace …`.

use ac2_client::expect_body;
use ac2_proto::model::*;
use ac2_proto::units::Blob;
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::{connect, find_meas, find_trace, state};
use crate::CliError;
use crate::args::*;
use crate::output::{self, Out};

pub(crate) fn method(m: AverageArg) -> AverageMethod {
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
        TraceCmd::Move {
            traces,
            to,
            imported,
        } => {
            let s = state(&c).await?;
            let owner = match (to, imported) {
                (Some(m), false) => TraceOwner::Meas {
                    meas: find_meas(&s, m)?.id,
                },
                (None, true) => TraceOwner::Imported,
                _ => {
                    return Err(CliError::Usage("give --to MEAS or --imported".into()));
                }
            };
            let ts = traces
                .iter()
                .map(|r| find_trace(&s, r).cloned())
                .collect::<Result<Vec<_>, _>>()?;
            let mut moved = Vec::new();
            for t in ts {
                let r = c
                    .call(Command::TraceUpdate {
                        trace: t.id,
                        edit: TraceEdit {
                            owner,
                            ..t.edit.clone()
                        },
                    })
                    .await?;
                moved.push(expect_body!("trace.update", r, ReplyBody::Trace(t) => t)?);
            }
            out.emit(&moved, || output::traces(&moved))?;
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
        TraceCmd::Import {
            file,
            target,
            format,
            name,
            channel,
            gate,
            gate_pre,
            keep_delay,
            resolution,
        } => {
            let ir_given =
                channel.is_some() || gate.is_some() || *keep_delay || resolution.is_some();
            let ir = ir_given.then(|| IrImport {
                channel: channel.map(|c| c - 1),
                window: match gate {
                    Some(post) => IrImportWindow::Gate {
                        pre: gate_pre.0,
                        post: post.0,
                    },
                    None => IrImportWindow::Whole,
                },
                align: if *keep_delay {
                    IrAlign::Start
                } else {
                    IrAlign::Peak
                },
                resolution: resolution.map_or_else(Default::default, |r| r.0),
            });
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
                        ImportFormatArg::Wav => ImportFormat::IrWav,
                    },
                    role: if *target {
                        ImportRole::Target
                    } else {
                        ImportRole::Trace
                    },
                    ir,
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
                if let TraceSource::Imported { notes, ir_file, .. } = &t.source {
                    if let Some(f) = ir_file {
                        s.push('\n');
                        s.push_str(&ac2_scene::trace::ir_file_text(f));
                    }
                    for n in notes {
                        s.push_str("\nnote: ");
                        s.push_str(ac2_scene::trace::import_note(*n));
                    }
                }
                s
            })?;
        }
        TraceCmd::Export { trace, csv, wav } => {
            let s = state(&c).await?;
            let t = find_trace(&s, trace)?;
            let (format, path) = match (csv, wav) {
                (_, Some(w)) => (ExportFormat::IrWav, w),
                (Some(p), None) => (ExportFormat::Ac2Csv, p),
                (None, None) => return Err(CliError::Usage("--csv FILE or --wav FILE".into())),
            };
            let r = c
                .call(Command::TraceExport {
                    trace: t.id,
                    format,
                })
                .await?;
            let (file_name, content) = expect_body!(
                "trace.export", r, ReplyBody::Export { file_name, content } => (file_name, content)
            )?;
            if path.as_os_str() == "-" {
                // The file itself is the output; nothing else may be mixed in.
                out.w.write_all(&content.0)?;
                out.w.flush()?;
                return Ok(());
            }
            std::fs::write(path, &content.0)?;
            let n = content.0.len();
            out.emit(
                &json!({ "trace": t.id, "file": path, "bytes": n, "suggested_name": file_name }),
                || format!("wrote {} ({n} bytes)", path.display()),
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
        TraceCmd::DelayDiff { a, b, temp } => {
            let s = state(&c).await?;
            let (ta, tb) = (find_trace(&s, a)?, find_trace(&s, b)?);
            let text = ac2_scene::readout::arrival_difference(ta.delay.0, tb.delay.0, temp.0);
            let j = json!({
                "a": ta.id,
                "b": tb.id,
                "a_delay_s": ta.delay.0,
                "b_delay_s": tb.delay.0,
                "difference_s": ta.delay.0 - tb.delay.0,
                "text": text,
            });
            out.emit(&j, || {
                format!(
                    "{} − {}: {text}  ({} − {})\n",
                    ta.edit.name,
                    tb.edit.name,
                    ac2_scene::format::delay(ta.delay.0),
                    ac2_scene::format::delay(tb.delay.0)
                )
            })?;
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
