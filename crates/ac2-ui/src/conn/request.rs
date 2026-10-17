//! Requests from the UI turned into daemon calls, and their names and errors.

use super::*;

pub(super) fn describe_err(e: &ClientError, target: &Target) -> String {
    match e {
        ClientError::Timeout { .. } => not_responding(target),
        other => other.to_string(),
    }
}

/// "not responding", with what to check on a daemon on another host (its firewall, and
/// whether it authorized this client's key).
pub(super) fn not_responding(target: &Target) -> String {
    format!("not responding{}", target.config.not_responding_hints())
}

pub(super) fn request_name(r: &Request) -> String {
    match r {
        Request::Call { what, .. } => what.clone(),
        Request::StimArm { .. } => "arm".into(),
        Request::StimSet(_) => "stimulus".into(),
        Request::StimStop => "stop".into(),
        Request::Sweep { label, .. } => format!("sweep {label}"),
        Request::Capture { slot, .. } => format!("capture slot {slot}"),
        Request::Import { path, .. } => format!("import {}", path.display()),
        Request::ImportCurve { path, .. } => format!("import curve {}", path.display()),
        Request::ExportTrace { name, .. } => format!("export {name}"),
        Request::FindDelay { .. } => "delay find".into(),
        Request::Devices => "list devices".into(),
        Request::OpenSession { what, .. } => what.clone(),
        Request::Preview { .. } => "device meters".into(),
        Request::PreviewStop => "close device meters".into(),
        Request::Meters(_) => "meters".into(),
        Request::DetectLoopback(_) => "detect loopback".into(),
        Request::CreateMeas { config } => format!("new measurement {}", config.name),
        Request::LeqBackfill { .. } => "SPL history".into(),
        Request::LeqSpan { .. } => "SPL history span".into(),
        Request::ExportLeqLog { info, .. } => format!("export {} log", info.name),
        Request::BandLogGet(_) => "band log".into(),
        Request::BandTransfer { .. } => "band transfer".into(),
        Request::Topics(_) => "subscribe".into(),
        Request::DisplayPeriod(_) => "display rate".into(),
        Request::Reconnect => "reconnect".into(),
        Request::ServerInfo => "server info".into(),
        Request::ServerCall { what, .. } => what.clone(),
        Request::Sessions(_) => "session".into(),
        Request::SessionList => "list sessions".into(),
    }
}

/// The warnings a reply carried, to the UI ahead of the reply itself.
fn send_warnings(o: &Out, warnings: Vec<ac2_proto::Warning>) {
    if !warnings.is_empty() {
        o.send(ConnEvent::Warnings(warnings));
    }
}

pub(super) fn handle(client: &Client, r: Request, stim: &mpsc::UnboundedSender<StimOp>, out: &Out) {
    match r {
        Request::Call { cmd, what } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                match c.call_warned(cmd).await {
                    Err(e) if e.code() == Some(ac2_proto::ErrorCode::Conflict) => {
                        o.send(ConnEvent::Conflict {
                            what,
                            error: e.to_string(),
                        });
                    }
                    r => {
                        let result = r
                            .map(|(_, warnings)| send_warnings(&o, warnings))
                            .map_err(|e| e.to_string());
                        o.send(ConnEvent::Reply { what, result });
                    }
                }
            });
        }
        Request::FindDelay {
            meas,
            pick,
            band,
            observation,
        } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let r = c
                    .call(Command::DelayFind {
                        meas,
                        band,
                        observation,
                    })
                    .await
                    .and_then(|r| expect_body!("delay.find", r, ReplyBody::DelayFinding(f) => f));
                match r {
                    Ok(finding) => o.send(ConnEvent::DelayFound {
                        meas,
                        pick,
                        finding: Box::new(finding),
                    }),
                    Err(e) => o.send(ConnEvent::Reply {
                        what: "delay find".into(),
                        result: Err(e.to_string()),
                    }),
                }
            });
        }
        Request::Capture {
            meas,
            slot,
            name,
            replace,
        } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let r = capture(&c, meas, slot, name, replace).await;
                match r {
                    Ok(trace) => o.send(ConnEvent::Captured {
                        slot,
                        trace: Box::new(trace),
                    }),
                    Err(e) => o.send(ConnEvent::Reply {
                        what: format!("capture slot {slot}"),
                        result: Err(e.to_string()),
                    }),
                }
            });
        }
        Request::ImportCurve { path, mic, input } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let what = format!("import curve {}", path.display());
                let result = import_curve(&c, &path, &mic, input).await;
                o.send(ConnEvent::Reply {
                    what: match &result {
                        Ok(label) => {
                            format!("curve {} imported", ac2_scene::cal::curve_name(&mic, label))
                        }
                        Err(_) => what,
                    },
                    result: result.map(|_| ()),
                });
            });
        }
        Request::Import { path, role, ir } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let what = format!("import {}", path.display());
                let result = import(&c, &path, role, ir).await.map(|t| {
                    o.send(ConnEvent::Reply {
                        what: format!("{} imported", t.edit.name),
                        result: Ok(()),
                    });
                });
                if let Err(e) = result {
                    o.send(ConnEvent::Reply {
                        what,
                        result: Err(e),
                    });
                }
            });
        }
        Request::ExportTrace { trace, name, path } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let result = export_trace(&c, trace, path).await;
                o.send(ConnEvent::Reply {
                    what: match &result {
                        Ok((p, n)) => format!("{name} exported to {} ({n} bytes)", p.display()),
                        Err(_) => format!("export {name}"),
                    },
                    result: result.map(|_| ()),
                });
            });
        }
        Request::Sessions(job) => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let outcome = session_job(&c, &o, job.steps).await;
                o.send(ConnEvent::Sessions(Box::new(outcome)));
            });
        }
        Request::SessionList => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let result = c
                    .call(Command::FileList)
                    .await
                    .and_then(|r| expect_body!("file.list", r, ReplyBody::Sessions(l) => l))
                    .map_err(|e| e.to_string());
                o.send(ConnEvent::SessionList(result));
            });
        }
        Request::ServerInfo => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let result = c
                    .call(Command::ServerInfo)
                    .await
                    .and_then(|r| expect_body!("server.info", r, ReplyBody::Server(s) => s))
                    .map_err(|e| e.to_string());
                o.send(ConnEvent::Server { what: None, result });
            });
        }
        Request::ServerCall { cmd, what } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let op = cmd.name();
                let result = c
                    .call(cmd)
                    .await
                    .and_then(|r| expect_body!(op, r, ReplyBody::Server(s) => s))
                    .map_err(|e| e.to_string());
                o.send(ConnEvent::Server {
                    what: Some(what),
                    result,
                });
            });
        }
        Request::Devices => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let r = c
                    .call(Command::SessionDevices)
                    .await
                    .and_then(|r| expect_body!("session.devices", r, ReplyBody::Backends(d) => d))
                    .map_err(|e| e.to_string());
                o.send(ConnEvent::Devices(r));
            });
        }
        Request::OpenSession {
            config,
            inputs,
            what,
        } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                match c.call_warned(Command::SessionOpen { config }).await {
                    Ok((_, warnings)) => send_warnings(&o, warnings),
                    Err(e) => {
                        o.send(ConnEvent::Reply {
                            what,
                            result: Err(e.to_string()),
                        });
                        return;
                    }
                }
                if !inputs.is_empty()
                    && let Err(e) = c.call(Command::SessionInputs { inputs }).await
                {
                    o.send(ConnEvent::Reply {
                        what: format!("{what}, but the mic names were not set"),
                        result: Err(e.to_string()),
                    });
                    return;
                }
                o.send(ConnEvent::Reply {
                    what,
                    result: Ok(()),
                });
                o.send(ConnEvent::SessionOpened);
            });
        }
        // Ordered through `preview_task`.
        Request::Preview { .. } | Request::PreviewStop | Request::Meters(_) => {}
        Request::DetectLoopback(d) => {
            let _ = stim.send(StimOp::Detect(d));
        }
        Request::CreateMeas { config } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let name = config.name.clone();
                let created = c
                    .call(Command::MeasCreate { config })
                    .await
                    .and_then(|r| expect_body!("meas.create", r, ReplyBody::Measurement(m) => m));
                let m = match created {
                    Ok(m) => m,
                    Err(e) => {
                        o.send(ConnEvent::Reply {
                            what: format!("new measurement {name}"),
                            result: Err(e.to_string()),
                        });
                        return;
                    }
                };
                let id = m.id;
                let job = m.config.kind.is_job();
                o.send(ConnEvent::MeasCreated(Box::new(m)));
                // A sweep measurement waits: only a fired run plays it.
                if !job {
                    o.send(ConnEvent::Reply {
                        what: format!("{name} created: Space on the sweep pane arms a run"),
                        result: Ok(()),
                    });
                    return;
                }
                // The dialog makes a running measurement or none: a stopped one left behind
                // would be made again by the operator's retry.
                let (what, result) = match c.call(Command::MeasStart { meas: id }).await {
                    Ok(_) => (format!("{name} created and started"), Ok(())),
                    Err(e) => {
                        let undo = Command::MeasDelete {
                            meas: id,
                            traces: OwnedTraces::Delete,
                        };
                        match c.call(undo).await {
                            Ok(_) => (name, Err(format!("not created: {e}"))),
                            Err(_) => (name, Err(format!("created but not started: {e}"))),
                        }
                    }
                };
                o.send(ConnEvent::Reply { what, result });
            });
        }
        Request::StimArm { settings, force } => {
            let _ = stim.send(StimOp::Arm { settings, force });
        }
        Request::StimSet(d) => {
            let _ = stim.send(StimOp::Set(d));
        }
        Request::StimStop => {
            let _ = stim.send(StimOp::Stop);
        }
        Request::Sweep { meas, update, .. } => {
            let _ = stim.send(StimOp::Sweep { meas, update });
        }
        Request::LeqBackfill { meas, ask } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let result = c
                    .call(Command::SplHistoryGet {
                        meas,
                        span: ac2_proto::model::HistorySpan::Newest {
                            seconds: ac2_scene::leq::HISTORY_S as u32,
                        },
                    })
                    .await
                    .and_then(|r| expect_body!("spl.history_get", r, ReplyBody::SplHistory(h) => h))
                    .map_err(|e| e.to_string());
                o.send(ConnEvent::LeqBackfill { meas, ask, result });
            });
        }
        Request::LeqSpan {
            meas,
            from,
            to,
            step_s,
            ask,
        } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let result = c
                    .call(Command::SplHistoryGet {
                        meas,
                        span: ac2_proto::model::HistorySpan::Range { from, to, step_s },
                    })
                    .await
                    .and_then(|r| expect_body!("spl.history_get", r, ReplyBody::SplHistory(h) => h))
                    .map_err(|e| e.to_string());
                o.send(ConnEvent::LeqSpan { meas, ask, result });
            });
        }
        Request::ExportLeqLog { info, log, path } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let name = info.name.clone();
                let result = export_leq_log(&c, info, log, path).await;
                o.send(ConnEvent::Reply {
                    what: match &result {
                        Ok((p, n)) => format!("{n} seconds of {name} written to {}", p.display()),
                        Err(_) => format!("export {name} log"),
                    },
                    result: result.map(|_| ()),
                });
            });
        }
        Request::BandLogGet(ask) => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let result = c
                    .call(Command::SplBandLogGet {
                        meas: ask.meas,
                        from: ask.from,
                        until: ask.until,
                        step: None,
                    })
                    .await
                    .and_then(
                        |r| expect_body!("spl.band_log_get", r, ReplyBody::SplBandLog(l) => l),
                    )
                    .map_err(|e| e.to_string());
                o.send(ConnEvent::BandLog { ask, result });
            });
        }
        Request::BandTransfer {
            meas,
            sources,
            place,
        } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let (foh, at_place, background) = *sources;
                let result = c
                    .call(Command::SplBandTransfer {
                        meas,
                        foh,
                        at_place,
                        background,
                        place,
                    })
                    .await
                    .and_then(
                        |r| expect_body!("spl.band_transfer", r, ReplyBody::Measurement(m) => m),
                    )
                    .map(Box::new)
                    .map_err(|e| e.to_string());
                o.send(ConnEvent::BandTransfer(result));
            });
        }
        // Applied by the session loop.
        Request::Topics(_) | Request::DisplayPeriod(_) | Request::Reconnect => {}
    }
}

pub(super) async fn capture(
    c: &Client,
    meas: MeasId,
    slot: u8,
    name: String,
    replace: Option<TraceId>,
) -> Result<TraceMeta, ClientError> {
    if let Some(old) = replace {
        // The slot's previous trace may already be gone; the capture still proceeds.
        let _ = c.call(Command::TraceDelete { trace: old }).await;
    }
    let r = c
        .call(Command::TraceCapture {
            meas,
            name,
            slot: Some(slot),
        })
        .await?;
    expect_body!("trace.capture", r, ReplyBody::Trace(t) => t)
}

/// Runs `steps` in order; the first failure ends the job.
pub(super) async fn session_job(
    c: &Client,
    o: &Out,
    steps: Vec<crate::sessions::SessionStep>,
) -> crate::sessions::SessionOutcome {
    use crate::sessions::{SessionFailure, SessionOutcome};
    let mut done = Vec::new();
    let mut it = steps.into_iter();
    while let Some(step) = it.next() {
        match session_step(c, o, &step).await {
            Ok(d) => done.push(d),
            Err(e) => {
                let (code, detail) = match &e {
                    ClientError::Daemon(p) => (Some(p.code), p.detail.clone()),
                    _ => (None, None),
                };
                return SessionOutcome {
                    done,
                    failed: Some(SessionFailure {
                        step,
                        rest: it.collect(),
                        error: e.to_string(),
                        code,
                        detail,
                    }),
                };
            }
        }
    }
    SessionOutcome { done, failed: None }
}

/// A session reference for `from`: a file here is uploaded first.
pub(super) async fn session_source(
    c: &Client,
    o: &Out,
    from: &crate::sessions::Source,
) -> Result<ac2_proto::model::SessionRef, ClientError> {
    use ac2_client::sessions::LocalSession;
    match from {
        crate::sessions::Source::Daemon(r) => Ok(r.clone()),
        crate::sessions::Source::Here(p) => {
            let local = LocalSession::at(p).ok_or_else(|| ClientError::Io {
                path: p.clone(),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "not an .ac2session file or a session folder",
                ),
            })?;
            let file = p.to_string_lossy().into_owned();
            let transfer = ac2_client::sessions::upload(c, &local, |done, total| {
                o.send(ConnEvent::SessionProgress(
                    ac2_scene::sessions::transfer_progress("uploading", &file, done, total),
                ));
            })
            .await?;
            Ok(ac2_proto::model::SessionRef::Upload { transfer })
        }
    }
}

pub(super) async fn session_step(
    c: &Client,
    o: &Out,
    step: &crate::sessions::SessionStep,
) -> Result<crate::sessions::SessionDone, ClientError> {
    use crate::sessions::{SessionDone as D, SessionStep as S, Source};
    Ok(match step {
        S::Save { session, overwrite } => {
            let r = c
                .call(Command::FileSave {
                    session: session.clone(),
                    overwrite: *overwrite,
                })
                .await?;
            D::Saved(expect_body!("file.save", r, ReplyBody::SessionFile(f) => f)?)
        }
        S::Export { session, path } => {
            let file = path.to_string_lossy().into_owned();
            let (path, bytes) =
                ac2_client::sessions::export(c, session.clone(), path, |done, total| {
                    o.send(ConnEvent::SessionProgress(
                        ac2_scene::sessions::transfer_progress("exporting", &file, done, total),
                    ));
                })
                .await?;
            D::Exported {
                path,
                bytes,
                working: session.is_none(),
            }
        }
        S::New { discard } => {
            c.call(Command::FileNew { discard: *discard }).await?;
            D::New
        }
        S::Open { from, discard } => {
            let session = session_source(c, o, from).await?;
            let r = c
                .call(Command::FileLoad {
                    session,
                    discard: *discard,
                })
                .await?;
            let file = expect_body!("file.load", r, ReplyBody::SessionFile(f) => f)?;
            D::Opened {
                file,
                here: match from {
                    Source::Here(p) => Some(p.clone()),
                    Source::Daemon(_) => None,
                },
            }
        }
        S::Recover => {
            c.call(Command::FileRecover).await?;
            D::Recovered
        }
        S::Salvage { from } => {
            let session = session_source(c, o, from).await?;
            let r = c.call(Command::FileSalvage { session }).await?;
            D::Salvaged {
                name: from.name(),
                salvage: expect_body!("file.salvage", r, ReplyBody::Salvage(s) => s)?,
            }
        }
        S::Quit => D::Quit,
    })
}

pub(super) async fn import(
    c: &Client,
    path: &std::path::Path,
    role: ImportRole,
    ir: Option<ac2_proto::model::IrImport>,
) -> Result<TraceMeta, String> {
    let content = tokio::task::spawn_blocking({
        let p = path.to_owned();
        move || std::fs::read(&p)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("cannot read: {e}"))?;
    let file_name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let r = c
        .call(Command::TraceImport {
            file_name,
            format: ImportFormat::Auto,
            role,
            ir,
            content: ac2_proto::units::Blob(content),
        })
        .await
        .map_err(|e| e.to_string())?;
    expect_body!("trace.import", r, ReplyBody::Trace(t) => t).map_err(|e| e.to_string())
}

/// `trace.export`s `trace` and writes it to `path` (into it, under the daemon's suggested
/// name, when `path` is a folder); returns where it went and its size. A `.wav` path takes
/// the trace's impulse response as WAV, anything else ac2 CSV.
/// What a typed export path asks for: its impulse response for a `.wav` file, ac2 CSV
/// otherwise (a folder too).
pub fn export_format(path: &std::path::Path) -> ac2_proto::model::ExportFormat {
    let wav = !path.is_dir()
        && path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("wav"));
    if wav {
        ac2_proto::model::ExportFormat::IrWav
    } else {
        ac2_proto::model::ExportFormat::Ac2Csv
    }
}

pub(super) async fn export_trace(
    c: &Client,
    trace: TraceId,
    path: std::path::PathBuf,
) -> Result<(std::path::PathBuf, usize), String> {
    let r = c
        .call(Command::TraceExport {
            trace,
            format: export_format(&path),
        })
        .await
        .map_err(|e| e.to_string())?;
    let (file_name, content) = expect_body!(
        "trace.export", r, ReplyBody::Export { file_name, content } => (file_name, content)
    )
    .map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || {
        let path = if path.is_dir() {
            path.join(file_name)
        } else {
            path
        };
        // Said back absolute: a relative path is relative to where the app was started.
        let path = std::path::absolute(&path).unwrap_or(path);
        std::fs::write(&path, &content.0)
            .map(|()| (path.clone(), content.0.len()))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Every row of one of a meter's logs, paged off the daemon, written as the log CSV to
/// `path` (its folder made first); returns the absolute path and the rows written.
pub(super) async fn export_leq_log(
    c: &Client,
    info: ac2_proto::spl_log::SplLogInfo,
    log: ac2_proto::model::SplLogWhich,
    path: std::path::PathBuf,
) -> Result<(std::path::PathBuf, usize), String> {
    let rows = ac2_client::spl_log::all_rows(c, info.meas, log)
        .await
        .map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || {
        let csv = ac2_proto::spl_log::export_csv(&info, &rows);
        write_new_file(&path, csv.as_bytes()).map(|p| (p, rows.len()))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Writes `bytes` to `path`, making its folder first; returns the path made absolute (a
/// relative one is relative to where the app was started).
pub fn write_new_file(path: &std::path::Path, bytes: &[u8]) -> Result<std::path::PathBuf, String> {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, bytes)
        .map(|()| path.clone())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Reads `path` and imports it into the mic library; returns the curve's label.
pub(super) async fn import_curve(
    c: &Client,
    path: &std::path::Path,
    mic: &str,
    input: Option<u16>,
) -> Result<String, String> {
    let content = tokio::task::spawn_blocking({
        let p = path.to_owned();
        move || std::fs::read(&p)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("cannot read: {e}"))?;
    let file_name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let r = c
        .call(Command::CalCurveImport {
            mic: mic.to_owned(),
            label: None,
            file_name,
            content: ac2_proto::units::Blob(content),
            input,
        })
        .await
        .map_err(|e| e.to_string())?;
    let m =
        expect_body!("cal.curve_import", r, ReplyBody::Mic(m) => m).map_err(|e| e.to_string())?;
    // The curve just imported is the newest of the mic's.
    Ok(m.curves
        .iter()
        .max_by_key(|c| c.imported_at)
        .map(|c| c.label.clone())
        .unwrap_or_default())
}

pub(super) async fn fetch_trace(c: Client, id: TraceId, grid: GridId, out: Out) {
    let data = match c.call(Command::TraceGet { trace: id }).await {
        Ok(ReplyBody::TraceData(d)) => *d,
        // Not every daemon build serves trace data yet; the trace stays listed without a
        // curve.
        _ => return,
    };
    if let Ok(g) = c.grid(grid).await {
        out.send(ConnEvent::Trace(Arc::new(data), g));
    }
}

/// Opens, renews and stops the device preview strictly in the order the reducer asked:
/// closing the session dialog and opening it again sends a stop and then a preview, and a
/// stop that overtook its preview would close the preview just asked for, leaving the
/// meters blank until the next renewal.
pub(super) async fn preview_task(
    client: Client,
    mut reqs: mpsc::UnboundedReceiver<Request>,
    out: Out,
) {
    while let Some(r) = reqs.recv().await {
        match r {
            Request::Preview { backend, device } => {
                let result = client
                    .call(Command::SessionPreview {
                        backend,
                        device: device.clone(),
                    })
                    .await
                    .and_then(|r| expect_body!("session.preview", r, ReplyBody::Preview(p) => p))
                    .map_err(|e| e.to_string());
                out.send(ConnEvent::Preview {
                    backend,
                    device,
                    result,
                });
            }
            // A preview the daemon already closed (session open, expiry) is fine.
            _ => {
                let _ = client.call(Command::SessionPreviewStop).await;
            }
        }
    }
}
