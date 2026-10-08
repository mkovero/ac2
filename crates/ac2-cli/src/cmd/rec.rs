//! `rec …` and `session replay`: raw capture files on the daemon host.

use std::path::{Path, PathBuf};

use ac2_client::{Client, expect_body};
use ac2_proto::model::{RecordRequest, RecordingRef, ReplayPace};
use ac2_proto::{Command, ReplyBody};

use super::{connect, is_local, state};
use crate::CliError;
use crate::args::{Cli, RecCmd, RecImportArgs};
use crate::output::{self, Out};

pub(crate) async fn run(cli: &Cli, cmd: &RecCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        RecCmd::Import(a) => import(a, out)?,
        RecCmd::Start(a) => {
            let c = connect(cli, false).await?;
            let inputs = match &a.inputs {
                Some(ch) => ch.0.clone(),
                None => state(&c)
                    .await?
                    .session
                    .open
                    .map(|o| o.config.input_channels)
                    .ok_or_else(|| {
                        CliError::Usage("no open session to record (ac2 session open)".into())
                    })?,
            };
            let r = c
                .call(Command::RecStart {
                    request: RecordRequest {
                        inputs,
                        name: a.name.clone(),
                        max_duration: a.max.0,
                        max_bytes: a.max_size.map(|b| b.0),
                    },
                })
                .await?;
            let run = expect_body!("rec.start", r, ReplyBody::Recording(r) => r)?;
            out.emit(&run, || output::recording(&run))?;
        }
        RecCmd::Stop => {
            let c = connect(cli, false).await?;
            let r = c.call(Command::RecStop).await?;
            let run = expect_body!("rec.stop", r, ReplyBody::Recording(r) => r)?;
            out.emit(&run, || output::recording(&run))?;
        }
        RecCmd::Status => {
            let c = connect(cli, false).await?;
            let run = state(&c).await?.recording;
            out.emit(&run, || {
                run.as_ref()
                    .map_or_else(|| "nothing recorded yet".to_owned(), output::recording)
            })?;
        }
        RecCmd::List => {
            let c = connect(cli, false).await?;
            let r = c.call(Command::RecList).await?;
            let l = expect_body!("rec.list", r, ReplyBody::Recordings(l) => l)?;
            out.emit(&l, || output::recordings(&l))?;
        }
    }
    Ok(())
}

/// A recording argument: a name, or (locally) a path to either of its files.
fn recording_ref(arg: &str, local: bool) -> Result<RecordingRef, CliError> {
    let is_path = arg.contains('/')
        || arg.contains('\\')
        || arg.starts_with('.')
        || Path::new(arg).is_absolute();
    if !is_path {
        return Ok(RecordingRef::Name {
            name: arg.to_owned(),
        });
    }
    let p = PathBuf::from(arg);
    let p = if local {
        std::path::absolute(&p)?
    } else if p.is_absolute() {
        p
    } else {
        return Err(CliError::Usage(
            "a remote daemon takes recording names, or absolute paths on its host".into(),
        ));
    };
    Ok(RecordingRef::Path {
        path: p.to_string_lossy().into_owned(),
    })
}

/// `session replay`.
pub(crate) async fn replay(
    cli: &Cli,
    c: &Client,
    recording: &str,
    fast: bool,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let local = is_local(cli) || cli.ctrl_endpoint.is_some();
    let r = c
        .call(Command::SessionReplay {
            recording: recording_ref(recording, local)?,
            pace: if fast {
                ReplayPace::Fast
            } else {
                ReplayPace::Realtime
            },
        })
        .await?;
    let s = expect_body!("session.replay", r, ReplyBody::Session(s) => s)?;
    out.emit(&s, || output::session(&s))?;
    Ok(())
}

/// `rec import`: a recorder's WAV as a recording beside it (or in `--dir`).
fn import(a: &RecImportArgs, out: &mut Out<'_>) -> Result<(), CliError> {
    let name = match &a.name {
        Some(n) => n.clone(),
        None => a
            .file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .ok_or_else(|| CliError::Usage(format!("{}: not a file", a.file.display())))?,
    };
    let dir = match &a.dir {
        Some(d) => d.clone(),
        None => a
            .file
            .parent()
            .map(|p| {
                if p.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    p
                }
            })
            .unwrap_or(Path::new("."))
            .to_path_buf(),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let s = ac2_traces::raw::import_wav(&a.file, &dir, &name, now)
        .map_err(|e| CliError::Usage(e.to_string()))?;
    let path = ac2_traces::raw::sidecar_path(&dir, &name);
    let path = std::path::absolute(&path).unwrap_or(path);
    out.emit(&s, || import_text(&a.file, &path, &s))?;
    Ok(())
}

pub(crate) fn import_text(src: &Path, path: &Path, s: &ac2_traces::raw::Sidecar) -> String {
    let frames = s.end.as_ref().map_or(0, |e| e.frames);
    let secs = frames as f64 / f64::from(s.audio.sample_rate.max(1));
    format!(
        "{} imported as {}: {} channel(s) at {} Hz, {:.1} s\nreplay it in real time (file \
         second t is the replay's start + t on the meter's clock): ac2 session replay \"{}\"",
        src.display(),
        path.display(),
        s.audio.channels.len(),
        s.audio.sample_rate,
        secs,
        path.display()
    )
}
