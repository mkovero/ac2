//! The Sessions dialog's reducer (`docs/design/sessions.md`): its keys and clicks, the
//! questions before unsaved changes are replaced (Save / Discard / Cancel), before another
//! session is overwritten and after an old-format session is refused, and what a finished
//! job does (toasts, the recents, the audio settings after New, quitting).

use ac2_proto::model::{SessionOrigin, SessionRef, WorkingSession};
use ac2_proto::units::WallNs;
use ac2_proto::{ErrorCode, ErrorDetail};
use ac2_scene::meas_list::Choice;
use ac2_scene::sessions::{self as text, Question, Replacing};
use ac2_scene::time::ClockOffset;
use eframe::egui::Key;

use super::display::{ChoicePrompt, ChoicePurpose};
use super::{AppState, Overlay};
use crate::conn::Request;
use crate::keys::Chord;
use crate::sessions::{
    Action, BrowseOutcome, BrowsePurpose, Browser, NameEntry, NamePurpose, Page, SessionDone,
    SessionFailure, SessionJob, SessionOutcome, SessionStep, SessionsDialog, Source,
};

/// Mouse and touch on the Sessions dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionsMsg {
    /// A row of the first page.
    Row(usize),
    /// A row of the browser.
    Browse(usize),
    /// The browser's places.
    Places,
    /// Back to the first page.
    Back,
    /// The typed name, taken.
    Submit,
}

/// This machine's wall clock.
pub(crate) fn wall_now() -> WallNs {
    WallNs(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos().min(u128::from(u64::MAX)) as u64),
    )
}

const QUESTION_HINT: &str = "←/→ pick · Enter takes it · Esc cancels";

impl AppState {
    /// The working state as the daemon last said (untitled and clean before it said).
    pub fn working(&self) -> WorkingSession {
        self.daemon().map_or_else(
            || WorkingSession {
                origin: None,
                dirty: false,
                saved_at: None,
                recover: None,
            },
            |s| s.working.clone(),
        )
    }

    /// The daemon clock's offset from this machine's.
    pub fn clock_offset(&self) -> ClockOffset {
        ClockOffset(
            self.mirror
                .as_ref()
                .and_then(|v| v.clock_offset_ns)
                .map_or(0, |o| {
                    o.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
                }),
        )
    }

    /// A palette session command: the dialog's action, without the dialog's first page.
    pub(super) fn session_command(&mut self, c: crate::keys::CommandId, out: &mut Vec<Request>) {
        use crate::keys::CommandId as C;
        if !self.connected() || self.daemon().is_none() {
            self.warn("not connected");
            return;
        }
        let w = self.working();
        match c {
            C::SessionNew => self.replacing(vec![SessionStep::New { discard: false }], out),
            C::SessionSave => self.save(Vec::new(), out),
            C::SessionSaveAs => self.save_as(Vec::new(), &w),
            C::SessionLoad => {
                self.open_sessions(Page::Browse(Browser::new(BrowsePurpose::Open)), out)
            }
            C::SessionExport => {
                self.open_sessions(Page::Browse(Browser::new(BrowsePurpose::Export)), out)
            }
            _ => {}
        }
    }

    /// The Sessions dialog, when open.
    pub fn sessions_dialog(&self) -> Option<&SessionsDialog> {
        match &self.overlay {
            Overlay::Sessions(d) => Some(d),
            _ => None,
        }
    }

    fn sessions_dialog_mut(&mut self) -> Option<&mut SessionsDialog> {
        match &mut self.overlay {
            Overlay::Sessions(d) => Some(d),
            _ => None,
        }
    }

    /// Opens the dialog on `page` and asks the daemon for its sessions.
    pub(super) fn open_sessions(&mut self, page: Page, out: &mut Vec<Request>) {
        if !self.connected() || self.daemon().is_none() {
            self.warn("not connected");
            return;
        }
        let mut d = SessionsDialog::new();
        d.page = page;
        self.overlay = Overlay::Sessions(Box::new(d));
        out.push(Request::SessionList);
    }

    /// With no audio I/O open, a command that needs one opens the Sessions dialog
    /// instead (its New leads to the audio setup, then the New measurement chooser), or the
    /// audio setup when measurements are kept; `true` when it did.
    pub(super) fn sessions_first(&mut self, out: &mut Vec<Request>) -> bool {
        if !self.connected() || self.daemon().is_none() || self.open_session().is_some() {
            return false;
        }
        // With measurements kept, New session would discard them: their audio is what
        // is missing.
        if self.nothing_kept() {
            self.open_sessions(Page::Main, out);
        } else {
            self.open_audio_setup(out);
        }
        true
    }

    /// The rows of the first page as shown now.
    pub fn sessions_rows(&self, client_now: WallNs) -> Vec<crate::sessions::MainRow> {
        self.sessions_dialog().map_or_else(Vec::new, |d| {
            d.rows(
                &self.working(),
                &self.prefs.recent_sessions,
                client_now,
                self.clock_offset(),
            )
        })
    }

    pub(super) fn sessions_msg(&mut self, m: SessionsMsg, out: &mut Vec<Request>) {
        match m {
            SessionsMsg::Row(i) => self.sessions_row(i, out),
            SessionsMsg::Browse(i) => self.browse(i, out),
            SessionsMsg::Places => {
                if let Some(Page::Browse(b)) = self.sessions_dialog_mut().map(|d| &mut d.page) {
                    b.go(None);
                }
            }
            SessionsMsg::Back => {
                if let Some(d) = self.sessions_dialog_mut() {
                    d.page = Page::Main;
                }
            }
            SessionsMsg::Submit => self.submit_name(out),
        }
    }

    /// A key while the dialog is open.
    pub(super) fn sessions_key(
        &mut self,
        chord: Chord,
        swallow: Option<char>,
        out: &mut Vec<Request>,
    ) {
        let now = wall_now();
        let n_main = self.sessions_rows(now).len();
        let Some(d) = self.sessions_dialog_mut() else {
            return;
        };
        if d.busy.is_some() {
            return;
        }
        let step = |sel: &mut usize, n: usize| {
            if n == 0 {
                return;
            }
            match chord.key {
                Key::ArrowDown => *sel = (*sel + 1).min(n - 1),
                Key::ArrowUp => *sel = sel.saturating_sub(1),
                Key::PageDown => *sel = (*sel + 8).min(n - 1),
                Key::PageUp => *sel = sel.saturating_sub(8),
                Key::Home => *sel = 0,
                Key::End => *sel = n - 1,
                _ => {}
            }
        };
        match &mut d.page {
            Page::Main => {
                if chord.key == Key::Enter {
                    let i = d.selected;
                    self.sessions_row(i, out);
                } else {
                    step(&mut d.selected, n_main);
                }
            }
            Page::Browse(b) => {
                if chord.key == Key::Enter {
                    let i = b.selected;
                    self.browse(i, out);
                } else {
                    let n = b.len();
                    step(&mut b.selected, n);
                }
            }
            Page::Name(_) => {
                if chord.key == Key::Enter {
                    self.submit_name(out);
                } else {
                    self.swallow_text = swallow;
                }
            }
        }
    }

    pub(super) fn sessions_text(&mut self, t: &str) {
        if let Some(Page::Name(n)) = self.sessions_dialog_mut().map(|d| &mut d.page) {
            n.text.push_str(t);
            n.error = None;
        }
    }

    /// Backspace: a typed name loses a character; the browser goes up a folder.
    pub(super) fn sessions_backspace(&mut self) {
        match self.sessions_dialog_mut().map(|d| &mut d.page) {
            Some(Page::Name(n)) => {
                n.text.pop();
                n.error = None;
            }
            Some(Page::Browse(b)) => b.up(),
            _ => {}
        }
    }

    /// Esc: an inner page goes back to the first; the first page closes.
    pub(super) fn sessions_close(&mut self) {
        match self.sessions_dialog_mut() {
            Some(d) if d.page != Page::Main && d.busy.is_none() => d.page = Page::Main,
            _ => self.overlay = Overlay::None,
        }
    }

    fn sessions_row(&mut self, i: usize, out: &mut Vec<Request>) {
        let rows = self.sessions_rows(wall_now());
        let Some(r) = rows.get(i) else {
            return;
        };
        if let Some(d) = self.sessions_dialog_mut() {
            d.selected = i;
        }
        let w = self.working();
        match r.action.clone() {
            Action::New => self.replacing(vec![SessionStep::New { discard: false }], out),
            Action::Open => self.set_page(Page::Browse(Browser::new(BrowsePurpose::Open))),
            Action::Recent(from) => self.replacing(
                vec![SessionStep::Open {
                    from,
                    discard: false,
                }],
                out,
            ),
            Action::Save => self.save(Vec::new(), out),
            Action::SaveAs => self.save_as(Vec::new(), &w),
            Action::Export => {
                let b = match &w.origin {
                    Some(SessionOrigin::Client { path }) => std::path::Path::new(path)
                        .parent()
                        .filter(|p| p.is_dir())
                        .map_or_else(
                            || Browser::new(BrowsePurpose::Export),
                            |p| Browser::at(BrowsePurpose::Export, p),
                        ),
                    _ => Browser::new(BrowsePurpose::Export),
                };
                self.set_page(Page::Browse(b));
            }
            Action::Recover => self.run_job(vec![SessionStep::Recover], out),
        }
    }

    fn set_page(&mut self, page: Page) {
        match self.sessions_dialog_mut() {
            Some(d) => d.page = page,
            None => {
                let mut d = SessionsDialog::new();
                d.page = page;
                self.overlay = Overlay::Sessions(Box::new(d));
            }
        }
    }

    fn browse(&mut self, i: usize, out: &mut Vec<Request>) {
        let w = self.working();
        let Some(Page::Browse(b)) = self.sessions_dialog_mut().map(|d| &mut d.page) else {
            return;
        };
        b.selected = i;
        match b.activate(i) {
            BrowseOutcome::Moved => {}
            BrowseOutcome::Open(p) => self.replacing(
                vec![SessionStep::Open {
                    from: Source::Here(p),
                    discard: false,
                }],
                out,
            ),
            BrowseOutcome::ExportHere(dir) => {
                let text = crate::sessions::suggested_name(&w);
                self.set_page(Page::Name(NameEntry {
                    purpose: NamePurpose::ExportAs { dir },
                    text,
                    error: None,
                }));
            }
        }
    }

    /// Steps that save the working state where it belongs; `None` when it must be named
    /// first (untitled, or a file this machine cannot reach).
    fn save_steps(&self, w: &WorkingSession) -> Option<Vec<SessionStep>> {
        Some(vec![match w.origin.clone()? {
            SessionOrigin::Name { name } => SessionStep::Save {
                session: SessionRef::Name { name },
                overwrite: true,
            },
            SessionOrigin::Path { path } => SessionStep::Save {
                session: SessionRef::Path { path },
                overwrite: true,
            },
            SessionOrigin::Client { path } => {
                let p = std::path::PathBuf::from(&path);
                // Written by another machine, or its stick is gone: name it again.
                if !p.parent().is_some_and(std::path::Path::is_dir) {
                    return None;
                }
                SessionStep::Export {
                    session: None,
                    path: p,
                }
            }
        }])
    }

    /// Save, then `then`.
    fn save(&mut self, then: Vec<SessionStep>, out: &mut Vec<Request>) {
        let w = self.working();
        match self.save_steps(&w) {
            Some(mut steps) => {
                steps.extend(then);
                self.run_job(steps, out);
            }
            None => self.save_as(then, &w),
        }
    }

    /// Save as (a name on the daemon), then `then`.
    fn save_as(&mut self, then: Vec<SessionStep>, w: &WorkingSession) {
        let text = match &w.origin {
            Some(SessionOrigin::Name { name }) => name.clone(),
            _ => crate::sessions::suggested_name(w),
        };
        self.set_page(Page::Name(NameEntry {
            purpose: NamePurpose::SaveAs { then },
            text,
            error: None,
        }));
    }

    fn submit_name(&mut self, out: &mut Vec<Request>) {
        let Some(Page::Name(n)) = self.sessions_dialog_mut().map(|d| &mut d.page) else {
            return;
        };
        let name = match crate::sessions::check_name(&n.text) {
            Ok(v) => v,
            Err(e) => {
                n.error = Some(e);
                return;
            }
        };
        let steps = match &n.purpose {
            NamePurpose::SaveAs { then } => {
                let mut s = vec![SessionStep::Save {
                    session: SessionRef::Name { name },
                    overwrite: false,
                }];
                s.extend(then.iter().cloned());
                s
            }
            NamePurpose::ExportAs { dir } => {
                let path = crate::sessions::export_path(dir, &name);
                if path.exists() && n.error.as_deref() != Some(EXISTS) {
                    n.error = Some(EXISTS.to_owned());
                    return;
                }
                vec![SessionStep::Export {
                    session: None,
                    path,
                }]
            }
        };
        self.run_job(steps, out);
    }

    /// `then` replaces the working state: asked first when it has unsaved changes.
    fn replacing(&mut self, then: Vec<SessionStep>, out: &mut Vec<Request>) {
        let w = self.working();
        if w.dirty {
            self.ask_unsaved(then);
        } else {
            self.run_job(then, out);
        }
    }

    fn ask_unsaved(&mut self, then: Vec<SessionStep>) {
        let w = self.working();
        let src_name;
        let replacing = match then.first() {
            Some(SessionStep::Open { from, .. }) => {
                src_name = from.name();
                Replacing::Open(&src_name)
            }
            Some(SessionStep::Quit) => Replacing::Quit {
                kept: self.embedded_autosave == Some(true),
            },
            _ => Replacing::New,
        };
        let q = text::unsaved_question(&w, replacing);
        self.ask(ChoicePurpose::Unsaved { then }, q, text::SAVE);
    }

    fn ask(&mut self, purpose: ChoicePurpose, q: Question, index: usize) {
        self.overlay = Overlay::Choose(Box::new(ChoicePrompt {
            purpose,
            label: String::new(),
            title: q.title,
            lines: q.lines,
            choices: q
                .answers
                .into_iter()
                .map(|label| Choice {
                    label,
                    blocked: None,
                })
                .collect(),
            owners: Vec::new(),
            index,
            hint: QUESTION_HINT.to_owned(),
        }));
    }

    /// The answer to a session question.
    pub(super) fn session_answer(
        &mut self,
        purpose: ChoicePurpose,
        i: usize,
        out: &mut Vec<Request>,
    ) {
        match purpose {
            ChoicePurpose::Unsaved { then } => match i {
                // `then` does not discard: when the save does not happen, the daemon keeps
                // the changes and the question comes back.
                text::SAVE => self.save(then, out),
                text::DISCARD => {
                    if then.first() == Some(&SessionStep::Quit) {
                        self.quit = true;
                    } else {
                        self.run_job(then.into_iter().map(SessionStep::discarding).collect(), out);
                    }
                }
                _ => {}
            },
            ChoicePurpose::Replace { rest } if i == text::REPLACE => self.run_job(rest, out),
            ChoicePurpose::Salvage { from } if i == text::SALVAGE => {
                self.run_job(vec![SessionStep::Salvage { from }], out);
            }
            _ => {}
        }
    }

    fn run_job(&mut self, steps: Vec<SessionStep>, out: &mut Vec<Request>) {
        if steps.is_empty() {
            return;
        }
        if let Some(d) = self.sessions_dialog_mut() {
            d.busy = Some("working…".to_owned());
        }
        out.push(Request::Sessions(SessionJob { steps }));
    }

    pub(super) fn session_progress(&mut self, t: String) {
        if let Some(d) = self.sessions_dialog_mut() {
            d.busy = Some(t);
        }
    }

    pub(super) fn session_list(&mut self, r: Result<Vec<ac2_proto::model::SessionFile>, String>) {
        if let Some(d) = self.sessions_dialog_mut() {
            match r {
                Ok(l) => {
                    d.daemon = Some(l);
                    d.list_error = None;
                }
                Err(e) => {
                    d.daemon = Some(Vec::new());
                    d.list_error = Some(e);
                }
            }
        }
    }

    pub(super) fn session_outcome(&mut self, o: SessionOutcome, out: &mut Vec<Request>) {
        let now = wall_now();
        let mut new = false;
        for d in o.done {
            match d {
                SessionDone::Saved(f) => self.toast(text::saved_toast(&f)),
                SessionDone::Exported {
                    path,
                    bytes,
                    working,
                } => {
                    let p = path.to_string_lossy().into_owned();
                    if working {
                        self.prefs.note_recent_session(&p, now);
                        self.prefs_dirty = true;
                    }
                    self.toast(text::exported_toast(&p, bytes));
                }
                SessionDone::New => {
                    self.toast("new session: no measurements or traces");
                    new = true;
                }
                SessionDone::Opened { file, here } => {
                    if let Some(p) = here {
                        self.prefs.note_recent_session(&p.to_string_lossy(), now);
                        self.prefs_dirty = true;
                    }
                    self.toast(text::opened_toast(&file));
                }
                SessionDone::Recovered => {
                    let w = self.working();
                    self.toast(format!(
                        "recovered {}",
                        text::origin_name(w.origin.as_ref())
                    ));
                }
                SessionDone::Salvaged { name, salvage } => {
                    self.toast(text::salvage_toast(&name, &salvage));
                }
                SessionDone::Quit => self.quit = true,
            }
        }
        match o.failed {
            None => {
                if self.sessions_dialog().is_some() {
                    self.overlay = Overlay::None;
                }
                if new {
                    // The new session picks its interface and channels; an opened one keeps
                    // the audio it finds.
                    self.open_audio_setup(out);
                    if let Overlay::Settings(s) = &mut self.overlay {
                        s.new_session = true;
                    }
                }
            }
            Some(f) => self.session_failed(f),
        }
    }

    fn session_failed(&mut self, f: SessionFailure) {
        if let Some(d) = self.sessions_dialog_mut() {
            d.busy = None;
        }
        let SessionFailure {
            step,
            rest,
            error,
            code,
            detail,
        } = f;
        match (&step, detail, code) {
            // Changed in the meantime (another client): asked now.
            (_, Some(ErrorDetail::UnsavedChanges { .. }), _) => {
                let mut then = vec![step];
                then.extend(rest);
                self.ask_unsaved(then);
            }
            (
                SessionStep::Save {
                    session,
                    overwrite: false,
                },
                Some(ErrorDetail::SessionExists { name, saved_at }),
                _,
            ) => {
                let q = text::replace_question(&name, saved_at, wall_now(), self.clock_offset());
                let mut steps = vec![SessionStep::Save {
                    session: session.clone(),
                    overwrite: true,
                }];
                steps.extend(rest);
                self.ask(ChoicePurpose::Replace { rest: steps }, q, 1);
            }
            (SessionStep::Open { from, .. }, _, Some(ErrorCode::Unsupported)) => {
                let q = text::salvage_question(&from.name(), None);
                self.ask(ChoicePurpose::Salvage { from: from.clone() }, q, 1);
            }
            _ => self.fault(format!("session: {error}")),
        }
    }

    /// Quit (the Quit key, the window's close): with an embedded daemon and unsaved
    /// changes, asked first.
    pub(super) fn request_quit(&mut self) {
        if self.embedded_autosave.is_some() && self.working().dirty {
            self.ask_unsaved(vec![SessionStep::Quit]);
        } else {
            self.quit = true;
        }
    }

    /// The window is closing: `false` keeps it open to ask about unsaved changes.
    pub fn close_requested(&mut self) -> bool {
        if self.quit {
            return true;
        }
        let asked = matches!(
            &self.overlay,
            Overlay::Choose(c) if matches!(&c.purpose, ChoicePurpose::Unsaved { then } if then.first() == Some(&SessionStep::Quit))
        );
        if asked {
            return false;
        }
        self.request_quit();
        self.quit
    }
}

const EXISTS: &str = "a file of that name exists: Enter again replaces it";
