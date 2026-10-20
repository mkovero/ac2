//! The Sessions dialog (`docs/design/sessions.md`, Alt+S): New, Open…, Save, Save as…,
//! Export…, Recover previous, and the recent sessions — the daemon's named sessions merged
//! with the files and folders this app opened or exported, most recent first. Open… and
//! Export… use an in-app file browser with big rows (the Pi kiosk has no native dialog)
//! that starts at the places: home and removable media.
//!
//! Pure data except the browser's directory listing; the reducer
//! (`state/sessions_impl.rs`) routes keys and clicks here and turns choices into
//! [`SessionJob`]s, which the link runs step by step ([`crate::conn`]).

use std::path::{Path, PathBuf};

use ac2_proto::model::{Salvage, SessionFile, SessionOrigin, SessionRef, WorkingSession};
use ac2_proto::units::WallNs;
use ac2_proto::{ErrorCode, ErrorDetail};
use ac2_scene::sessions::{self as text, EntryKind, SessionRow};
use ac2_scene::time::ClockOffset;

use crate::prefs::RecentSession;

/// Where a session to open is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Read by the daemon (a named session).
    Daemon(SessionRef),
    /// A file or folder on this machine, uploaded.
    Here(PathBuf),
}

impl Source {
    /// How the texts name it.
    pub fn name(&self) -> String {
        match self {
            Source::Daemon(SessionRef::Name { name }) => name.clone(),
            Source::Daemon(SessionRef::Path { path }) => text::file_part(path).to_owned(),
            Source::Daemon(SessionRef::Upload { .. }) => "the upload".to_owned(),
            Source::Here(p) => text::file_part(&p.to_string_lossy()).to_owned(),
        }
    }
}

/// One step of a [`SessionJob`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionStep {
    /// `file.save`.
    Save {
        session: SessionRef,
        overwrite: bool,
    },
    /// The working state (`None`) or a named session written to this machine at `path`
    /// (a file, or a session folder replaced whole).
    Export {
        session: Option<SessionRef>,
        path: PathBuf,
    },
    /// `file.new`.
    New { discard: bool },
    /// `file.load` (a file here uploaded first).
    Open { from: Source, discard: bool },
    /// `file.recover`.
    Recover,
    /// `file.salvage` (a file here uploaded first).
    Salvage { from: Source },
    /// The app quits (after the steps before it succeeded).
    Quit,
}

impl SessionStep {
    /// The same step told to drop unsaved changes.
    pub fn discarding(self) -> Self {
        match self {
            SessionStep::New { .. } => SessionStep::New { discard: true },
            SessionStep::Open { from, .. } => SessionStep::Open {
                from,
                discard: true,
            },
            s => s,
        }
    }
}

/// Steps the link runs in order; the first failure ends it ([`SessionOutcome`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionJob {
    pub steps: Vec<SessionStep>,
}

/// What a finished step did.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionDone {
    Saved(SessionFile),
    Exported {
        path: PathBuf,
        bytes: u64,
        /// The working state was exported (it now belongs to that file).
        working: bool,
    },
    New,
    Opened {
        file: SessionFile,
        /// Opened from this machine.
        here: Option<PathBuf>,
    },
    Recovered,
    Salvaged {
        name: String,
        salvage: Salvage,
    },
    Quit,
}

/// Why a step failed, with the steps after it (not run).
#[derive(Clone, Debug, PartialEq)]
pub struct SessionFailure {
    pub step: SessionStep,
    pub rest: Vec<SessionStep>,
    pub error: String,
    pub code: Option<ErrorCode>,
    pub detail: Option<ErrorDetail>,
}

/// A [`SessionJob`] finished.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionOutcome {
    pub done: Vec<SessionDone>,
    pub failed: Option<SessionFailure>,
}

/// An action of the dialog's first page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    New,
    /// Browse this computer.
    Open,
    Save,
    SaveAs,
    /// Browse this computer for a folder.
    Export,
    Recover,
    Recent(Source),
}

/// A row of the first page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MainRow {
    pub action: Action,
    pub title: String,
    pub detail: String,
}

/// What the browser is choosing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowsePurpose {
    /// A session file or folder to open.
    Open,
    /// The folder an export goes to.
    Export,
}

/// A row of the browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowseRow {
    pub kind: EntryKind,
    pub name: String,
    pub path: PathBuf,
}

impl BrowseRow {
    pub fn label(&self) -> String {
        text::entry_label(self.kind, &self.name)
    }
}

/// The in-app file browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Browser {
    pub purpose: BrowsePurpose,
    /// `None`: the places.
    pub dir: Option<PathBuf>,
    pub rows: Vec<BrowseRow>,
    pub selected: usize,
    pub error: Option<String>,
}

/// What a typed name is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamePurpose {
    /// A session on the daemon; then `then`.
    SaveAs { then: Vec<SessionStep> },
    /// A file in `dir` on this machine.
    ExportAs { dir: PathBuf },
}

/// A name being typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameEntry {
    pub purpose: NamePurpose,
    pub text: String,
    pub error: Option<String>,
}

impl NameEntry {
    pub fn title(&self) -> String {
        match &self.purpose {
            NamePurpose::SaveAs { .. } => "Save as: session name on the daemon".to_owned(),
            NamePurpose::ExportAs { dir } => format!("Export to {}: file name", dir.display()),
        }
    }
}

/// The dialog's page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page {
    Main,
    Browse(Browser),
    Name(NameEntry),
}

impl Page {
    /// The page as `ac2_scene::sessions` names it, for its buttons and key hint.
    pub fn scene(&self) -> text::DialogPage {
        match self {
            Page::Main => text::DialogPage::Main,
            Page::Browse(_) => text::DialogPage::Browse,
            Page::Name(n) => text::DialogPage::Name {
                export: matches!(n.purpose, NamePurpose::ExportAs { .. }),
            },
        }
    }
}

/// The Sessions dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionsDialog {
    pub page: Page,
    /// The daemon's named sessions (`None`: asked, not answered yet).
    pub daemon: Option<Vec<SessionFile>>,
    /// `file.list` failed.
    pub list_error: Option<String>,
    /// Highlighted row of the first page.
    pub selected: usize,
    /// A job is running: what it is doing.
    pub busy: Option<String>,
}

impl Default for SessionsDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionsDialog {
    pub fn new() -> Self {
        Self {
            page: Page::Main,
            daemon: None,
            list_error: None,
            selected: 0,
            busy: None,
        }
    }

    /// The first page's rows: the actions (Recover only with a recovery copy), then the
    /// recents.
    pub fn rows(
        &self,
        w: &WorkingSession,
        recents: &[RecentSession],
        client_now: WallNs,
        offset: ClockOffset,
    ) -> Vec<MainRow> {
        let name = text::origin_name(w.origin.as_ref());
        let row = |action, title: &str, detail: String| MainRow {
            action,
            title: title.to_owned(),
            detail,
        };
        let mut v = vec![
            row(
                Action::New,
                "New session",
                "no measurements or traces; then the audio settings".to_owned(),
            ),
            row(
                Action::Open,
                "Open…",
                "a session file or folder on this computer (USB stick…)".to_owned(),
            ),
            row(
                Action::Save,
                "Save",
                match &w.origin {
                    None => "untitled: name it first".to_owned(),
                    Some(o) => format!("{name} ({})", text::origin_place(Some(o))),
                },
            ),
            row(
                Action::SaveAs,
                "Save as…",
                "a new session on the daemon".to_owned(),
            ),
            row(
                Action::Export,
                "Export…",
                "an .ac2session file on this computer".to_owned(),
            ),
        ];
        if let Some(r) = &w.recover {
            v.push(MainRow {
                action: Action::Recover,
                title: "Recover previous".to_owned(),
                detail: text::recover_label(r, client_now, offset)
                    .trim_start_matches("Recover previous: ")
                    .to_owned(),
            });
        }
        v.extend(
            self.recents(recents, client_now, offset)
                .into_iter()
                .map(|(src, r)| MainRow {
                    action: Action::Recent(src),
                    title: r.title,
                    detail: r.detail,
                }),
        );
        v
    }

    /// The daemon's sessions and the files here, most recent first. Times compare on the
    /// client clock (a daemon save time moved by `offset`).
    pub fn recents(
        &self,
        recents: &[RecentSession],
        client_now: WallNs,
        offset: ClockOffset,
    ) -> Vec<(Source, SessionRow)> {
        let mut v: Vec<(i128, Source, SessionRow)> = Vec::new();
        for f in self.daemon.iter().flatten() {
            v.push((
                i128::from(f.saved_at.0) - i128::from(offset.0),
                Source::Daemon(SessionRef::Name {
                    name: f.name.clone(),
                }),
                text::daemon_row(f, client_now, offset),
            ));
        }
        for r in recents {
            v.push((
                i128::from(r.opened_at),
                Source::Here(PathBuf::from(&r.path)),
                text::local_row(&r.path, WallNs(r.opened_at), client_now),
            ));
        }
        v.sort_by_key(|x| std::cmp::Reverse(x.0));
        v.into_iter().map(|(_, s, r)| (s, r)).collect()
    }
}

/// Rows of the places: home, then removable media.
pub fn places() -> Vec<BrowseRow> {
    let mut v = Vec::new();
    if let Some(h) = home() {
        v.push(BrowseRow {
            kind: EntryKind::Place,
            name: "Home".to_owned(),
            path: h,
        });
    }
    let user = std::env::var("USER").unwrap_or_default();
    let mut roots: Vec<PathBuf> = Vec::new();
    if !user.is_empty() {
        roots.push(Path::new("/run/media").join(&user));
        roots.push(Path::new("/media").join(&user));
    }
    roots.push(PathBuf::from("/media"));
    roots.push(PathBuf::from("/mnt"));
    for root in roots {
        let Ok(rd) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut found: Vec<BrowseRow> = rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter(|e| {
                // /media/$USER is a root of its own, not a stick.
                !(root == Path::new("/media") && e.file_name().to_string_lossy() == user)
            })
            .map(|e| BrowseRow {
                kind: EntryKind::Place,
                name: e.file_name().to_string_lossy().into_owned(),
                path: e.path(),
            })
            .collect();
        found.sort_by(|a, b| a.name.cmp(&b.name));
        for r in found {
            if !v.iter().any(|x| x.path == r.path) {
                v.push(r);
            }
        }
    }
    v
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Rows of `dir`: up, then folders and sessions by name (hidden names left out). For an
/// export, only folders (a session folder is a folder there too).
pub fn list_dir(dir: &Path, purpose: BrowsePurpose) -> Result<Vec<BrowseRow>, String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut v: Vec<BrowseRow> = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        let kind = if path.is_dir() {
            if purpose == BrowsePurpose::Open && ac2_archive::is_session_dir(&path) {
                EntryKind::SessionFolder
            } else {
                EntryKind::Folder
            }
        } else if purpose == BrowsePurpose::Open && ac2_archive::is_archive_name(&path) {
            EntryKind::Archive
        } else {
            continue;
        };
        v.push(BrowseRow { kind, name, path });
    }
    // Sessions first: they are what the operator came for.
    v.sort_by(|a, b| {
        let rank = |k: EntryKind| u8::from(k == EntryKind::Folder);
        (rank(a.kind), a.name.to_lowercase()).cmp(&(rank(b.kind), b.name.to_lowercase()))
    });
    let mut rows = vec![BrowseRow {
        kind: EntryKind::Up,
        name: String::new(),
        path: dir.parent().map_or_else(PathBuf::new, Path::to_path_buf),
    }];
    rows.extend(v);
    Ok(rows)
}

impl Browser {
    /// At the places.
    pub fn new(purpose: BrowsePurpose) -> Self {
        Self {
            purpose,
            dir: None,
            rows: places(),
            selected: 0,
            error: None,
        }
    }

    /// At `dir` (the places when it cannot be read).
    pub fn at(purpose: BrowsePurpose, dir: &Path) -> Self {
        let mut b = Self::new(purpose);
        b.go(Some(dir.to_owned()));
        b
    }

    /// Goes to `dir` (`None`: the places); stays put, with the reason, when it cannot be
    /// read.
    pub fn go(&mut self, dir: Option<PathBuf>) {
        let rows = match &dir {
            None => Ok(places()),
            Some(d) => list_dir(d, self.purpose),
        };
        match rows {
            Ok(rows) => {
                self.rows = rows;
                self.dir = dir;
                self.selected = 0;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Up one folder; from a place's top, the places.
    pub fn up(&mut self) {
        let Some(d) = &self.dir else {
            return;
        };
        let at_place = places().iter().any(|p| &p.path == d);
        let parent = d.parent().map(Path::to_path_buf);
        self.go(if at_place { None } else { parent });
    }

    /// Rows to show; an export's browser starts with "Export here" (the current folder).
    pub fn here_row(&self) -> bool {
        self.purpose == BrowsePurpose::Export && self.dir.is_some()
    }
}

/// What activating a browser row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowseOutcome {
    /// Moved (or nothing to do).
    Moved,
    /// This session file or folder is chosen.
    Open(PathBuf),
    /// Export into this folder.
    ExportHere(PathBuf),
}

impl Browser {
    /// Activates row `i` (with an export's "Export here" row as 0).
    pub fn activate(&mut self, i: usize) -> BrowseOutcome {
        let i = if self.here_row() {
            if i == 0 {
                return self
                    .dir
                    .clone()
                    .map_or(BrowseOutcome::Moved, BrowseOutcome::ExportHere);
            }
            i - 1
        } else {
            i
        };
        let Some(r) = self.rows.get(i).cloned() else {
            return BrowseOutcome::Moved;
        };
        match r.kind {
            EntryKind::Up => self.up(),
            EntryKind::Place | EntryKind::Folder => self.go(Some(r.path)),
            EntryKind::SessionFolder | EntryKind::Archive => {
                return BrowseOutcome::Open(r.path);
            }
        }
        BrowseOutcome::Moved
    }

    /// Rows counted for the highlight.
    pub fn len(&self) -> usize {
        self.rows.len() + usize::from(self.here_row())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A session name typed for Save as: the daemon's rules, checked here for a quick answer.
pub fn check_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("type a name".to_owned());
    }
    if n.starts_with('.') || n.contains(['/', '\\']) {
        return Err("a name, not a path (no / or \\, not starting with .)".to_owned());
    }
    Ok(n.to_owned())
}

/// The file an export named `name` in `dir` writes.
pub fn export_path(dir: &Path, name: &str) -> PathBuf {
    ac2_client::sessions::export_target(&dir.join(name.trim()), name.trim())
}

/// The name Save as and Export start from.
pub fn suggested_name(w: &WorkingSession) -> String {
    match &w.origin {
        None => String::new(),
        Some(SessionOrigin::Name { name }) => name.clone(),
        Some(o) => {
            let n = text::origin_name(Some(o));
            let ext = format!(".{}", ac2_archive::EXTENSION);
            match n.to_ascii_lowercase().strip_suffix(&ext) {
                Some(stem) => n[..stem.len()].to_owned(),
                None => n,
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const S: u64 = 1_000_000_000;

    fn file(name: &str, at_s: u64) -> SessionFile {
        SessionFile {
            name: name.into(),
            path: format!("/srv/sessions/{name}"),
            saved_at: WallNs(at_s * S),
            measurements: 1,
            traces: 2,
        }
    }

    #[test]
    fn recents_merge_the_daemon_and_this_computer_newest_first() {
        let mut d = SessionsDialog::new();
        d.daemon = Some(vec![file("old", 100), file("new", 300)]);
        let here = [RecentSession {
            path: "/media/STICK/mid.ac2session".into(),
            opened_at: 200 * S,
        }];
        let r = d.recents(&here, WallNs(400 * S), ClockOffset(0));
        let titles: Vec<_> = r.iter().map(|x| x.1.title.as_str()).collect();
        assert_eq!(titles, ["new", "mid.ac2session", "old"]);
        assert_eq!(
            r[1].0,
            Source::Here(PathBuf::from("/media/STICK/mid.ac2session"))
        );
        let w = WorkingSession {
            origin: None,
            dirty: true,
            saved_at: None,
            recover: None,
        };
        let rows = d.rows(&w, &here, WallNs(400 * S), ClockOffset(0));
        let titles: Vec<_> = rows.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "New session",
                "Open…",
                "Save",
                "Save as…",
                "Export…",
                "new",
                "mid.ac2session",
                "old"
            ]
        );
        assert_eq!(rows[2].detail, "untitled: name it first");
    }

    #[test]
    fn the_browser_shows_folders_and_sessions_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("shows/fri/traces")).unwrap();
        std::fs::write(root.join("shows/fri/session.json"), "{}").unwrap();
        std::fs::create_dir_all(root.join("shows/plain")).unwrap();
        std::fs::write(root.join("shows/sat.AC2SESSION"), "x").unwrap();
        std::fs::write(root.join("shows/notes.txt"), "x").unwrap();
        std::fs::create_dir_all(root.join("shows/.hidden")).unwrap();
        let mut b = Browser::at(BrowsePurpose::Open, &root.join("shows"));
        let labels: Vec<_> = b.rows.iter().map(BrowseRow::label).collect();
        assert_eq!(
            labels,
            [
                "↑ up",
                "fri  (session folder)",
                "sat.AC2SESSION  (session file)",
                "plain/"
            ]
        );
        assert_eq!(b.activate(1), BrowseOutcome::Open(root.join("shows/fri")));
        assert_eq!(b.activate(3), BrowseOutcome::Moved);
        assert_eq!(b.dir.as_deref(), Some(root.join("shows/plain").as_path()));
        b.activate(0);
        assert_eq!(b.dir.as_deref(), Some(root.join("shows").as_path()));

        let mut e = Browser::at(BrowsePurpose::Export, &root.join("shows"));
        let labels: Vec<_> = e.rows.iter().map(BrowseRow::label).collect();
        assert_eq!(labels, ["↑ up", "fri/", "plain/"]);
        assert_eq!(e.len(), 4, "export here first");
        assert_eq!(e.activate(0), BrowseOutcome::ExportHere(root.join("shows")));

        let mut bad = Browser::at(BrowsePurpose::Open, &root.join("shows"));
        bad.go(Some(root.join("missing")));
        assert!(bad.error.is_some());
        assert_eq!(bad.dir.as_deref(), Some(root.join("shows").as_path()));
    }

    #[test]
    fn names() {
        assert_eq!(check_name("  fri show "), Ok("fri show".into()));
        assert!(check_name("a/b").is_err());
        assert!(check_name(".x").is_err());
        assert!(check_name(" ").is_err());
        assert_eq!(
            export_path(Path::new("/m/S"), "fri"),
            PathBuf::from("/m/S/fri.ac2session")
        );
        let w = |o| WorkingSession {
            origin: Some(o),
            dirty: false,
            saved_at: None,
            recover: None,
        };
        assert_eq!(
            suggested_name(&w(SessionOrigin::Client {
                path: "/m/S/Fri.ac2session".into()
            })),
            "Fri"
        );
        assert_eq!(
            suggested_name(&w(SessionOrigin::Name { name: "sat".into() })),
            "sat"
        );
    }
}
