//! Session texts (`docs/design/sessions.md`): the working state's name and *unsaved
//! changes* in the top bar and `ac2 session status`, the rows of the Sessions dialog
//! (recent sessions, the file browser), its questions (Save / Discard / Cancel, replace a
//! session, import an old session's traces) and the toasts after them. The app and the CLI
//! both read them from here, so a session reads the same everywhere.

use ac2_proto::model::{Recoverable, Salvage, SessionFile, SessionOrigin, WorkingSession};
use ac2_proto::units::WallNs;

use crate::format;
use crate::time::{self, ClockOffset};

/// Name of a working state that belongs to no session.
pub const UNTITLED: &str = "untitled";
/// Marks a working state that differs from its session's last save.
pub const UNSAVED: &str = "unsaved changes";

/// The last component of a path (either separator), ignoring a trailing one.
pub fn file_part(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(path)
}

/// What the working state is called: a session's name, a folder's or file's name, or
/// `untitled`.
pub fn origin_name(o: Option<&SessionOrigin>) -> String {
    match o {
        None => UNTITLED.to_owned(),
        Some(SessionOrigin::Name { name }) => name.clone(),
        Some(SessionOrigin::Path { path } | SessionOrigin::Client { path }) => {
            file_part(path).to_owned()
        }
    }
}

/// Where a session is kept: `on the daemon`, `daemon folder <path>`, `<path>` (on the
/// app's machine), or `not saved yet`.
pub fn origin_place(o: Option<&SessionOrigin>) -> String {
    match o {
        None => "not saved yet".to_owned(),
        Some(SessionOrigin::Name { .. }) => "on the daemon".to_owned(),
        Some(SessionOrigin::Path { path }) => format!("daemon folder {path}"),
        Some(SessionOrigin::Client { path }) => path.clone(),
    }
}

/// The top-bar label and the status line: `friday`, `friday · unsaved changes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingLabel {
    /// Shown text.
    pub text: String,
    /// Differs from its last save: drawn in the warning colour.
    pub unsaved: bool,
    /// Longer text on hover.
    pub detail: String,
}

/// The label for `w`. Ages are on the daemon clock (`offset`); `sessions_key` is the key
/// that opens the sessions dialog, named in the detail when there is one.
pub fn working_label(
    w: &WorkingSession,
    client_now: WallNs,
    offset: ClockOffset,
    sessions_key: Option<&str>,
) -> WorkingLabel {
    let name = origin_name(w.origin.as_ref());
    let text = if w.dirty {
        format!("{name} · {UNSAVED}")
    } else {
        name
    };
    let saved = match (w.origin.as_ref(), w.saved_at) {
        (None, _) => "Not saved yet.".to_owned(),
        (Some(_), Some(t)) => format!(
            "Saved {} ({}).",
            format::ago(time::age_s(t, client_now, offset)),
            origin_place(w.origin.as_ref())
        ),
        (Some(_), None) => format!("Kept {}.", origin_place(w.origin.as_ref())),
    };
    let changed = if w.dirty {
        " Measurements or traces changed since."
    } else {
        ""
    };
    let keys = sessions_key.map_or_else(String::new, |k| format!(" {k}: sessions."));
    let detail = format!("{saved}{changed}{keys}");
    WorkingLabel {
        text,
        unsaved: w.dirty,
        detail,
    }
}

/// `2 measurements · 5 traces`.
pub fn counts(measurements: u32, traces: u32) -> String {
    let n = |v: u32, one: &str| {
        if v == 1 {
            format!("1 {one}")
        } else {
            format!("{v} {one}s")
        }
    };
    format!(
        "{} · {}",
        n(measurements, "measurement"),
        n(traces, "trace")
    )
}

/// A row of the Sessions dialog: a session on the daemon, or a file the app opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRow {
    /// `friday`.
    pub title: String,
    /// `on the daemon · 2 measurements · 5 traces · saved 3 h ago`.
    pub detail: String,
}

/// A daemon session's row.
pub fn daemon_row(f: &SessionFile, client_now: WallNs, offset: ClockOffset) -> SessionRow {
    SessionRow {
        title: f.name.clone(),
        detail: format!(
            "on the daemon · {} · saved {}",
            counts(f.measurements, f.traces),
            format::ago(time::age_s(f.saved_at, client_now, offset))
        ),
    }
}

/// A row of a session file or folder on the app's machine, opened at `at` (client clock).
pub fn local_row(path: &str, at: WallNs, client_now: WallNs) -> SessionRow {
    SessionRow {
        title: file_part(path).to_owned(),
        detail: format!(
            "{path} · opened {}",
            format::ago(time::age_s(at, client_now, ClockOffset(0)))
        ),
    }
}

/// `Recover previous: friday (0 measurements · 2 traces), replaced 3 min ago`.
pub fn recover_label(r: &Recoverable, client_now: WallNs, offset: ClockOffset) -> String {
    format!(
        "Recover previous: {} ({}), replaced {}",
        origin_name(r.origin.as_ref()),
        counts(r.measurements, r.traces),
        format::ago(time::age_s(r.replaced_at, client_now, offset))
    )
}

/// What replaces the working state after the Save / Discard / Cancel question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replacing<'a> {
    /// New session.
    New,
    /// Opening this session.
    Open(&'a str),
    /// Quitting the app with its embedded daemon; `kept`: it autosaves, so the working
    /// state comes back at the next start.
    Quit { kept: bool },
}

/// Answer indices of [`unsaved_question`].
pub const SAVE: usize = 0;
/// Discard (quit: quit without saving).
pub const DISCARD: usize = 1;
/// Cancel.
pub const CANCEL: usize = 2;

/// A question with its answers, in the order of [`SAVE`], [`DISCARD`], [`CANCEL`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    /// `Save changes to friday?`
    pub title: String,
    /// Under the title.
    pub lines: Vec<String>,
    /// Answers.
    pub answers: Vec<String>,
}

/// Asked before New, Open or quitting (embedded daemon) with unsaved changes. An untitled
/// state's Save names it first (`Save as…`).
pub fn unsaved_question(w: &WorkingSession, replacing: Replacing<'_>) -> Question {
    let name = origin_name(w.origin.as_ref());
    let title = if w.origin.is_some() {
        format!("Save changes to {name}?")
    } else {
        "Save the untitled measurements and traces?".to_owned()
    };
    let what = match replacing {
        Replacing::New => "New session starts empty.".to_owned(),
        Replacing::Open(s) => format!("Opening {s} replaces the measurements and traces."),
        Replacing::Quit { .. } => "The app closes its daemon.".to_owned(),
    };
    let kept = match replacing {
        Replacing::Quit { kept: true } => {
            "Without saving they come back when the app starts again."
        }
        Replacing::Quit { kept: false } => "Without saving they are lost.",
        _ => "Discarded changes stay recoverable once (Recover previous).",
    };
    let save = if w.origin.is_some() {
        "Save"
    } else {
        "Save as…"
    };
    let discard = match replacing {
        Replacing::Quit { .. } => "Quit without saving",
        _ => "Discard",
    };
    Question {
        title,
        lines: vec![what, kept.to_owned()],
        answers: vec![save.to_owned(), discard.to_owned(), "Cancel".to_owned()],
    }
}

/// Answer index of [`replace_question`] that replaces.
pub const REPLACE: usize = 0;

/// Asked before Save as onto another existing session.
pub fn replace_question(
    name: &str,
    saved_at: WallNs,
    client_now: WallNs,
    offset: ClockOffset,
) -> Question {
    Question {
        title: format!("Replace session {name}?"),
        lines: vec![format!(
            "{name} was saved {}; saving replaces what it holds.",
            format::ago(time::age_s(saved_at, client_now, offset))
        )],
        answers: vec!["Replace".to_owned(), "Cancel".to_owned()],
    }
}

/// Answer index of [`salvage_question`] that imports.
pub const SALVAGE: usize = 0;

/// Asked when a session of another format version is refused (`version`: its version).
pub fn salvage_question(name: &str, version: Option<u32>) -> Question {
    let which = version.map_or_else(
        || "another session format".to_owned(),
        |v| format!("session format {v}"),
    );
    Question {
        title: format!("{name} cannot be opened"),
        lines: vec![
            format!("It is {which}; this version opens only its own."),
            "Its traces can be imported into the current measurements and traces.".to_owned(),
        ],
        answers: vec!["Import its traces".to_owned(), "Cancel".to_owned()],
    }
}

/// `imported 4 traces from friday; 1 refused (traces/x.csv: …)`.
pub fn salvage_toast(name: &str, s: &Salvage) -> String {
    let n = s.imported.len();
    let mut t = format!(
        "imported {n} trace{} from {name}",
        if n == 1 { "" } else { "s" }
    );
    if let Some(first) = s.refused.first() {
        t.push_str(&format!(
            "; {} refused ({}: {})",
            s.refused.len(),
            first.file,
            first.reason
        ));
    }
    t
}

/// `opened friday (2 measurements · 5 traces), disarmed`.
pub fn opened_toast(f: &SessionFile) -> String {
    format!(
        "opened {} ({}), disarmed",
        f.name,
        counts(f.measurements, f.traces)
    )
}

/// `saved friday (2 measurements · 5 traces)`.
pub fn saved_toast(f: &SessionFile) -> String {
    format!("saved {} ({})", f.name, counts(f.measurements, f.traces))
}

/// `exported to /run/media/foh/STICK/friday.ac2session (1.2 MB)`.
pub fn exported_toast(path: &str, bytes: u64) -> String {
    format!("exported to {path} ({})", size(bytes))
}

/// `uploading friday.ac2session: 1.2 of 3.4 MB`.
pub fn transfer_progress(verb: &str, file: &str, done: u64, total: u64) -> String {
    format!(
        "{verb} {}: {} of {}",
        file_part(file),
        size_number(done, total),
        size(total)
    )
}

/// `812 kB`, `3.4 MB` (decimal units).
pub fn size(bytes: u64) -> String {
    let b = bytes as f64;
    if b < 1e3 {
        format!("{bytes} B")
    } else if b < 1e6 {
        format!("{:.0} kB", b / 1e3)
    } else {
        format!("{:.1} MB", b / 1e6)
    }
}

/// `done` in the unit [`size`] picks for `total`, without the unit.
fn size_number(done: u64, total: u64) -> String {
    let d = done as f64;
    if total < 1_000 {
        format!("{done}")
    } else if total < 1_000_000 {
        format!("{:.0}", d / 1e3)
    } else {
        format!("{:.1}", d / 1e6)
    }
}

/// What a file-browser row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// A place (home, a USB stick): goes there.
    Place,
    /// The parent folder.
    Up,
    /// A folder: goes into it.
    Folder,
    /// A session folder (holds `session.json`): opens.
    SessionFolder,
    /// An `.ac2session` file: opens.
    Archive,
}

/// A browser row's text: `▸ STICK`, `↑ up`, `shows/`, `friday  (session folder)`.
pub fn entry_label(kind: EntryKind, name: &str) -> String {
    match kind {
        EntryKind::Place => format!("▸ {name}"),
        EntryKind::Up => "↑ up".to_owned(),
        EntryKind::Folder => format!("{name}/"),
        EntryKind::SessionFolder => format!("{name}  (session folder)"),
        EntryKind::Archive => format!("{name}  (session file)"),
    }
}

/// Which page of the Sessions dialog is showing, for its buttons and key hint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogPage {
    /// The actions and the recent sessions.
    Main,
    /// The file browser.
    Browse,
    /// A name typed for Save as (`export: false`) or an export file.
    Name { export: bool },
}

/// The page's buttons, in one row of equal buttons: the confirming verb first and the way
/// back last, as every other dialog's Apply · Cancel.
pub fn page_buttons(p: DialogPage) -> &'static [&'static str] {
    match p {
        DialogPage::Main => &[],
        DialogPage::Browse => &["Places", "Back"],
        DialogPage::Name { export: false } => &["Save", "Cancel"],
        DialogPage::Name { export: true } => &["Export", "Cancel"],
    }
}

/// The key hint under the page.
pub fn page_hint(p: DialogPage) -> &'static str {
    match p {
        DialogPage::Main => "↑↓ choose · Enter opens · Esc closes",
        DialogPage::Browse => "↑↓ choose · Enter opens · Backspace: up a folder · Esc: back",
        DialogPage::Name { export: false } => {
            "type the name · Enter saves · Backspace erases · Esc: back"
        }
        DialogPage::Name { export: true } => {
            "type the name · Enter exports · Backspace erases · Esc: back"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::SalvageRefusal;

    const S: u64 = 1_000_000_000;

    fn w(origin: Option<SessionOrigin>, dirty: bool) -> WorkingSession {
        WorkingSession {
            origin,
            dirty,
            saved_at: Some(WallNs(400 * S)),
            recover: None,
        }
    }

    fn name(n: &str) -> Option<SessionOrigin> {
        Some(SessionOrigin::Name { name: n.into() })
    }

    #[test]
    fn the_working_label_names_the_session_and_its_unsaved_changes() {
        let now = WallNs(4000 * S);
        let off = ClockOffset(0);
        let l = working_label(&w(name("friday"), false), now, off, Some("Alt+S"));
        assert_eq!(l.text, "friday");
        assert!(!l.unsaved);
        assert_eq!(l.detail, "Saved 1 h ago (on the daemon). Alt+S: sessions.");
        let l = working_label(&w(name("friday"), false), now, off, None);
        assert_eq!(l.detail, "Saved 1 h ago (on the daemon).");
        let l = working_label(&w(name("friday"), true), now, off, Some("⌥S"));
        assert_eq!(
            l.detail,
            "Saved 1 h ago (on the daemon). Measurements or traces changed since. ⌥S: sessions."
        );
        assert_eq!(l.text, "friday · unsaved changes");
        assert!(l.unsaved);
        let l = working_label(
            &w(
                Some(SessionOrigin::Client {
                    path: "/run/media/foh/STICK/sat.ac2session".into(),
                }),
                false,
            ),
            now,
            off,
            None,
        );
        assert_eq!(l.text, "sat.ac2session");
        assert!(l.detail.contains("(/run/media/foh/STICK/sat.ac2session)"));
        let l = working_label(&w(None, true), now, off, None);
        assert_eq!(l.text, "untitled · unsaved changes");
        assert!(l.detail.starts_with("Not saved yet."));
    }

    #[test]
    fn rows_and_counts() {
        assert_eq!(counts(1, 0), "1 measurement · 0 traces");
        assert_eq!(counts(2, 1), "2 measurements · 1 trace");
        let f = SessionFile {
            name: "friday".into(),
            path: "/srv/ac2/sessions/friday".into(),
            saved_at: WallNs(0),
            measurements: 2,
            traces: 5,
        };
        let r = daemon_row(&f, WallNs(7200 * S), ClockOffset(0));
        assert_eq!(r.title, "friday");
        assert_eq!(
            r.detail,
            "on the daemon · 2 measurements · 5 traces · saved 2 h ago"
        );
        let r = local_row("/media/STICK/sat.ac2session", WallNs(0), WallNs(30 * S));
        assert_eq!(r.title, "sat.ac2session");
        assert_eq!(r.detail, "/media/STICK/sat.ac2session · opened just now");
        let rec = Recoverable {
            replaced_at: WallNs(0),
            origin: None,
            measurements: 0,
            traces: 2,
        };
        assert_eq!(
            recover_label(&rec, WallNs(180 * S), ClockOffset(0)),
            "Recover previous: untitled (0 measurements · 2 traces), replaced 3 min ago"
        );
    }

    #[test]
    fn questions() {
        let q = unsaved_question(&w(name("friday"), true), Replacing::Open("sat"));
        assert_eq!(q.title, "Save changes to friday?");
        assert_eq!(q.answers, ["Save", "Discard", "Cancel"]);
        assert_eq!(
            q.lines[0],
            "Opening sat replaces the measurements and traces."
        );
        let q = unsaved_question(&w(None, true), Replacing::New);
        assert_eq!(q.answers[SAVE], "Save as…");
        let q = unsaved_question(&w(name("friday"), true), Replacing::Quit { kept: true });
        assert_eq!(q.answers[DISCARD], "Quit without saving");
        assert_eq!(
            q.lines[1],
            "Without saving they come back when the app starts again."
        );
        assert_eq!(q.answers[CANCEL], "Cancel");
        let q = replace_question("a", WallNs(0), WallNs(120 * S), ClockOffset(0));
        assert_eq!(q.title, "Replace session a?");
        assert_eq!(q.answers[REPLACE], "Replace");
        let q = salvage_question("old", Some(19));
        assert_eq!(
            q.lines[0],
            "It is session format 19; this version opens only its own."
        );
        assert_eq!(q.answers[SALVAGE], "Import its traces");
    }

    #[test]
    fn toasts_and_sizes() {
        let s = Salvage {
            version: Some(19),
            imported: vec![],
            refused: vec![SalvageRefusal {
                file: "traces/x.csv".into(),
                reason: "not an ac2 trace".into(),
            }],
        };
        assert_eq!(
            salvage_toast("old", &s),
            "imported 0 traces from old; 1 refused (traces/x.csv: not an ac2 trace)"
        );
        assert_eq!(size(812), "812 B");
        assert_eq!(size(812_000), "812 kB");
        assert_eq!(size(3_400_000), "3.4 MB");
        assert_eq!(
            transfer_progress("uploading", "/m/S/fri.ac2session", 1_200_000, 3_400_000),
            "uploading fri.ac2session: 1.2 of 3.4 MB"
        );
        assert_eq!(file_part("C:\\shows\\fri\\"), "fri");
        assert_eq!(
            entry_label(EntryKind::Archive, "fri.ac2session"),
            "fri.ac2session  (session file)"
        );
    }

    #[test]
    fn every_page_confirms_first_and_backs_out_last() {
        assert!(page_buttons(DialogPage::Main).is_empty());
        assert_eq!(page_buttons(DialogPage::Browse), ["Places", "Back"]);
        assert_eq!(
            page_buttons(DialogPage::Name { export: false }),
            ["Save", "Cancel"]
        );
        assert_eq!(
            page_buttons(DialogPage::Name { export: true }),
            ["Export", "Cancel"]
        );
        assert_eq!(
            page_hint(DialogPage::Name { export: false }),
            "type the name · Enter saves · Backspace erases · Esc: back"
        );
        assert_eq!(
            page_hint(DialogPage::Browse),
            "↑↓ choose · Enter opens · Backspace: up a folder · Esc: back"
        );
    }
}
