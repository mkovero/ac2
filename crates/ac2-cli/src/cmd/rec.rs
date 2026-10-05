//! `rec …` and `session replay`: raw capture files on the daemon host.

use std::path::{Path, PathBuf};

use ac2_client::{Client, expect_body};
use ac2_proto::model::{RecordRequest, RecordingRef, ReplayPace};
use ac2_proto::{Command, ReplyBody};

use super::{connect, is_local, state};
use crate::CliError;
use crate::args::{Cli, RecCmd};
use crate::output::{self, Out};

pub(crate) async fn run(cli: &Cli, cmd: &RecCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    match cmd {
        RecCmd::Start(a) => {
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
            let r = c.call(Command::RecStop).await?;
            let run = expect_body!("rec.stop", r, ReplyBody::Recording(r) => r)?;
            out.emit(&run, || output::recording(&run))?;
        }
        RecCmd::Status => {
            let run = state(&c).await?.recording;
            out.emit(&run, || {
                run.as_ref()
                    .map_or_else(|| "nothing recorded yet".to_owned(), output::recording)
            })?;
        }
        RecCmd::List => {
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
