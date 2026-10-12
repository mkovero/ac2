//! `ac2 session new|save|save-as|load|recover|salvage|export|import|list`: the saved
//! sessions of the measurements and traces (`docs/design/sessions.md`). A session is a
//! name in the daemon's session directory, a folder on the daemon's host, or an
//! `.ac2session` file (or folder) on this machine, moved to and from the daemon in checked
//! chunks.

use std::path::{Path, PathBuf};

use ac2_client::sessions::{self, LocalSession};
use ac2_client::{Client, ClientError, expect_body};
use ac2_proto::model::*;
use ac2_proto::units::TransferId;
use ac2_proto::{Command, ErrorDetail, ReplyBody};
use ac2_scene::sessions as text;
use ac2_scene::time::ClockOffset;
use serde_json::json;

use super::{is_local, state};
use crate::CliError;
use crate::args::*;
use crate::output::{self, Out};

/// The daemon reads this machine's paths: it runs here (a local or explicit endpoint).
fn same_host(cli: &Cli) -> bool {
    is_local(cli) || cli.ctrl_endpoint.is_some()
}

/// Where a session argument points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// Read by the daemon: a name, or a path on its host.
    Daemon(SessionRef),
    /// On this machine, moved as an archive.
    Here(PathBuf),
}

/// What `arg` names: an `.ac2session` file is always on this machine; for a daemon on
/// another host, so is an existing session folder here; anything else is the daemon's.
pub(crate) fn target(arg: &str, same_host: bool) -> Result<Target, CliError> {
    let p = Path::new(arg);
    if ac2_archive::is_archive_name(p) {
        return Ok(Target::Here(std::path::absolute(p)?));
    }
    if !same_host && ac2_archive::is_session_dir(p) {
        return Ok(Target::Here(std::path::absolute(p)?));
    }
    session_ref(arg, same_host).map(Target::Daemon)
}

/// Whether a session argument is a path rather than a name: anything with a separator, a
/// `.`/`~` start, or absolute.
pub(crate) fn is_path(arg: &str) -> bool {
    arg.contains('/')
        || arg.contains('\\')
        || arg.starts_with('.')
        || arg.starts_with('~')
        || Path::new(arg).is_absolute()
}

/// A daemon-side session argument: a plain name, or a path (anything with a separator,
/// `.`/`~` start, or absolute). A local path is made absolute here: the daemon does not
/// share the client's working directory.
pub(crate) fn session_ref(arg: &str, local: bool) -> Result<SessionRef, CliError> {
    if !is_path(arg) {
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
    } else if absolute_on_any_host(arg) {
        p
    } else {
        return Err(CliError::Usage(
            "a remote daemon takes session names, absolute paths on its host, or \
             .ac2session files on this machine"
                .into(),
        ));
    };
    Ok(SessionRef::Path {
        path: p.to_string_lossy().into_owned(),
    })
}

/// Whether a path is absolute on the daemon's host, whatever OS that is. The client's own
/// `Path::is_absolute` cannot decide it: `/srv/shows` is absolute on a Linux daemon but only
/// drive-relative on a Windows client, and `D:\shows` the other way round. The daemon
/// checks the path against its own filesystem.
fn absolute_on_any_host(arg: &str) -> bool {
    let b = arg.as_bytes();
    let drive =
        b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\');
    arg.starts_with('/') || arg.starts_with(r"\\") || drive
}

/// A refusal to drop unsaved changes says what to do.
fn unsaved(e: ClientError) -> CliError {
    if let ClientError::Daemon(p) = &e
        && let Some(ErrorDetail::UnsavedChanges { origin }) = &p.detail
    {
        return CliError::Refused(format!(
            "{} has {}: `ac2 session save` first, or pass --discard (what it replaces stays \
             recoverable with `ac2 session recover`)",
            text::origin_name(origin.as_ref()),
            text::UNSAVED
        ));
    }
    if let ClientError::Daemon(p) = &e
        && let Some(ErrorDetail::SessionExists { name, .. }) = &p.detail
    {
        return CliError::Refused(format!("session {name} exists: pass --force to replace it"));
    }
    e.into()
}

fn local_session(p: &Path) -> Result<LocalSession, CliError> {
    LocalSession::at(p).ok_or_else(|| {
        CliError::Usage(format!(
            "{}: not an .ac2session file or a session folder",
            p.display()
        ))
    })
}

async fn upload(c: &Client, p: &Path) -> Result<TransferId, CliError> {
    Ok(sessions::upload(c, &local_session(p)?, |_, _| {}).await?)
}

fn emit_file(out: &mut Out<'_>, f: &SessionFile, toast: String) -> Result<(), CliError> {
    out.emit(f, || format!("{toast}\n  {}", f.path))?;
    Ok(())
}

async fn export_working(c: &Client, path: &Path, out: &mut Out<'_>) -> Result<(), CliError> {
    let (p, bytes) = sessions::export(c, None, path, |_, _| {}).await?;
    let p = p.to_string_lossy().into_owned();
    out.emit(&json!({ "path": p, "bytes": bytes }), || {
        text::exported_toast(&p, bytes)
    })?;
    Ok(())
}

/// The `working` line of `session status`, and the recovery copy when there is one.
pub(crate) fn status_lines(w: &WorkingSession, now: ac2_proto::units::WallNs) -> String {
    let off = ClockOffset(0);
    let l = text::working_label(w, now, off);
    let mut t = format!(
        "working      {}  ({})",
        l.text,
        text::origin_place(w.origin.as_ref())
    );
    if let Some(r) = &w.recover {
        t.push_str(&format!(
            "\n             {}",
            text::recover_label(r, now, off)
        ));
    }
    t
}

pub(crate) async fn session(
    cli: &Cli,
    c: &Client,
    cmd: &SessionCmd,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let here = same_host(cli);
    match cmd {
        SessionCmd::New { discard } => {
            let r = c
                .call(Command::FileNew { discard: *discard })
                .await
                .map_err(unsaved)?;
            let rev = expect_body!("file.new", r, ReplyBody::Ack { rev } => rev)?;
            out.emit(&json!({ "rev": rev }), || {
                "new session: no measurements or traces".to_owned()
            })?;
        }
        SessionCmd::Save => {
            let w = state(c).await?.working;
            match w.origin {
                None => {
                    return Err(CliError::Usage(
                        "untitled: name it with `ac2 session save-as NAME`".into(),
                    ));
                }
                Some(SessionOrigin::Client { path }) => {
                    export_working(c, Path::new(&path), out).await?;
                }
                Some(SessionOrigin::Name { name }) => {
                    save(c, SessionRef::Name { name }, true, out).await?;
                }
                Some(SessionOrigin::Path { path }) => {
                    save(c, SessionRef::Path { path }, true, out).await?;
                }
            }
        }
        SessionCmd::SaveAs { session, force } => match target(session, here)? {
            Target::Here(p) => {
                if p.exists() && !force {
                    return Err(CliError::Refused(format!(
                        "{} exists: pass --force to replace it",
                        p.display()
                    )));
                }
                export_working(c, &p, out).await?;
            }
            Target::Daemon(r) => save(c, r, *force, out).await?,
        },
        SessionCmd::Load { session, discard } => {
            let r = match target(session, here)? {
                Target::Here(p) => SessionRef::Upload {
                    transfer: upload(c, &p).await?,
                },
                Target::Daemon(r) => r,
            };
            let r = c
                .call(Command::FileLoad {
                    session: r,
                    discard: *discard,
                })
                .await
                .map_err(unsaved)?;
            let f = expect_body!("file.load", r, ReplyBody::SessionFile(f) => f)?;
            emit_file(out, &f, text::opened_toast(&f))?;
        }
        SessionCmd::Recover => {
            let r = c.call(Command::FileRecover).await?;
            let rev = expect_body!("file.recover", r, ReplyBody::Ack { rev } => rev)?;
            let w = state(c).await?.working;
            out.emit(&json!({ "rev": rev, "working": w }), || {
                format!(
                    "recovered {}",
                    text::working_label(&w, crate::watch::now_wall(), ClockOffset(0)).text
                )
            })?;
        }
        SessionCmd::Salvage { session } => {
            if let Some(local) = super::salvage::local_session(session)? {
                return super::salvage::run(c, session, &local, out).await;
            }
            let r = match target(session, here)? {
                Target::Here(p) => SessionRef::Upload {
                    transfer: upload(c, &p).await?,
                },
                Target::Daemon(r) => r,
            };
            let r = c.call(Command::FileSalvage { session: r }).await?;
            let s = expect_body!("file.salvage", r, ReplyBody::Salvage(s) => s)?;
            let name = text::file_part(session).to_owned();
            out.emit(&s, || {
                format!(
                    "{}\nmeasurements are made again only from a session folder or \
                     .ac2session file on this machine",
                    text::salvage_toast(&name, &s)
                )
            })?;
        }
        SessionCmd::Export { file, session } => match session {
            None => export_working(c, file, out).await?,
            Some(s) => {
                let r = session_ref(s, here)?;
                let (p, bytes) = sessions::export(c, Some(r), file, |_, _| {}).await?;
                let p = p.to_string_lossy().into_owned();
                out.emit(&json!({ "path": p, "bytes": bytes }), || {
                    text::exported_toast(&p, bytes)
                })?;
            }
        },
        SessionCmd::Import { file, name, force } => {
            let name = match name {
                Some(n) => n.clone(),
                None => import_name(file),
            };
            let transfer = upload(c, file).await?;
            let r = c
                .call(Command::FileImport {
                    transfer,
                    name,
                    overwrite: *force,
                })
                .await
                .map_err(unsaved)?;
            let f = expect_body!("file.import", r, ReplyBody::SessionFile(f) => f)?;
            let toast = format!(
                "stored as session {} ({})",
                f.name,
                text::counts(f.measurements, f.traces)
            );
            emit_file(out, &f, toast)?;
        }
        SessionCmd::List => {
            let r = c.call(Command::FileList).await?;
            let l = expect_body!("file.list", r, ReplyBody::Sessions(l) => l)?;
            out.emit(&l, || output::sessions(&l))?;
        }
        SessionCmd::Open(_)
        | SessionCmd::Close
        | SessionCmd::Status
        | SessionCmd::Replay { .. }
        | SessionCmd::Panel { .. }
        | SessionCmd::Inputs(_) => {
            return Err(CliError::Usage("not a saved-session command".into()));
        }
    }
    Ok(())
}

/// A session name from a file or folder name: without `.ac2session`.
pub(crate) fn import_name(file: &Path) -> String {
    let n = text::file_part(&file.to_string_lossy()).to_owned();
    let lower = n.to_ascii_lowercase();
    let ext = format!(".{}", ac2_archive::EXTENSION);
    match lower.strip_suffix(&ext) {
        Some(stem) => n[..stem.len()].to_owned(),
        None => n,
    }
}

async fn save(
    c: &Client,
    session: SessionRef,
    overwrite: bool,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let r = c
        .call(Command::FileSave { session, overwrite })
        .await
        .map_err(unsaved)?;
    let f = expect_body!("file.save", r, ReplyBody::SessionFile(f) => f)?;
    emit_file(out, &f, text::saved_toast(&f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used)]
    fn targets() {
        assert_eq!(
            target("friday", false).unwrap(),
            Target::Daemon(SessionRef::Name {
                name: "friday".into()
            })
        );
        // A rooted path without a drive is made absolute on the current drive on Windows.
        assert!(matches!(
            target("/media/STICK/fri.AC2SESSION", false).unwrap(),
            Target::Here(p) if p.is_absolute() && p.ends_with("media/STICK/fri.AC2SESSION")
        ));
        assert_eq!(
            target("/srv/shows/fri", false).unwrap(),
            Target::Daemon(SessionRef::Path {
                path: "/srv/shows/fri".into()
            })
        );
        assert_eq!(
            target(r"D:\shows\fri", false).unwrap(),
            Target::Daemon(SessionRef::Path {
                path: r"D:\shows\fri".into()
            })
        );
        assert_eq!(
            target(r"\\nas\shows\fri", false).unwrap(),
            Target::Daemon(SessionRef::Path {
                path: r"\\nas\shows\fri".into()
            })
        );
        assert!(target("shows/fri", false).is_err());
        assert_eq!(
            import_name(Path::new("/m/S/Fri Show.ac2session")),
            "Fri Show"
        );
        assert_eq!(import_name(Path::new("/m/S/fri/")), "fri");
    }
}
