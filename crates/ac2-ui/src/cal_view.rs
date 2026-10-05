//! The calibrations view (palette "Input setup…" / "Calibrations…"): what each input uses
//! (mic, mic curve, sensitivity calibration), every mic of the mic library with its curves,
//! and every sensitivity calibration. Keyboard-first like the other dialogs:
//!
//! - ↑/↓ move between lines, PageUp / PageDown a page, Home / End to the first / last;
//! - ←/→ on an input steps its mic curve: off → 0° → 90° … (applied at once: the
//!   correction is a display correction, so averages need no reset);
//! - C on an input opens the acoustic calibration dialog ([`crate::acoustic_dialog`]: a
//!   calibrator on the mic), E the electrical one ([`crate::electrical_dialog`]);
//! - N names the mic on an input; I imports a curve file for the focused mic (path);
//!   R renames the focused curve; Delete deletes the focused curve or sensitivity
//!   calibration (pressed twice);
//! - Enter ends a typed edit; Esc closes.
//!
//! Pure data: the lines come from the mirrored state on every call, so the view always
//! shows the daemon's truth; the reducer turns [`CalAction`]s into requests.

use ac2_proto::cal;
use ac2_proto::model::{CalKey, CurveChoice, InputSetup, MicCurveId, State};

use crate::acoustic_dialog::AcousticDialog;
use crate::electrical_dialog::ElectricalDialog;

/// One line of the view.
#[derive(Clone, Debug, PartialEq)]
pub enum CalLine {
    /// An input: the session's, or one with a mic name.
    Input(u16),
    /// A curve of the mic library.
    Curve(MicCurveId),
    /// A sensitivity calibration.
    Sensitivity(CalKey),
}

/// The lines of `s`: inputs (the open session's captured inputs and every input with a mic
/// name), then the mic library curve by curve, then the sensitivity calibrations.
pub fn lines(s: &State) -> Vec<CalLine> {
    let mut inputs: Vec<u16> = s
        .inputs
        .iter()
        .filter(|i| i.mic.is_some())
        .map(|i| i.channel)
        .collect();
    if let Some(o) = &s.session.open {
        inputs.extend(o.config.input_channels.iter().copied());
    }
    inputs.sort_unstable();
    inputs.dedup();
    let mut v: Vec<CalLine> = inputs.into_iter().map(CalLine::Input).collect();
    for m in &s.mics {
        v.extend(m.curves.iter().map(|c| {
            CalLine::Curve(MicCurveId {
                mic: m.name.clone(),
                label: c.label.clone(),
            })
        }));
    }
    v.extend(
        s.calibrations
            .iter()
            .map(|e| CalLine::Sensitivity(e.key.clone())),
    );
    v
}

/// What is being typed.
#[derive(Clone, Debug, PartialEq)]
pub enum EditKind {
    /// The mic name of an input.
    MicName(u16),
    /// The path of a curve file to import for `mic` (setting `input`'s mic when given).
    ImportPath { mic: String, input: Option<u16> },
    /// A new label for a curve.
    Rename(MicCurveId),
}

#[derive(Clone, Debug, PartialEq)]
pub struct CalEdit {
    pub kind: EditKind,
    pub text: String,
}

/// What the reducer does for the view.
#[derive(Clone, Debug, PartialEq)]
pub enum CalAction {
    /// `session.inputs` with this row.
    Inputs(InputSetup, String),
    /// Read the file and `cal.curve_import` it.
    Import {
        path: std::path::PathBuf,
        mic: String,
        input: Option<u16>,
    },
    /// `cal.curve_rename`.
    Rename(MicCurveId, String),
    /// `cal.curve_delete`.
    DeleteCurve(MicCurveId),
    /// `cal.delete`.
    DeleteSensitivity(CalKey),
    /// `cal.spl` or `cal.spl_electrical` from a calibration dialog; the reply comes back by
    /// `what`.
    Calibrate(ac2_proto::Command, String),
}

/// The view's own state: focus, a typed edit, a pending deletion.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CalView {
    pub focus: usize,
    pub edit: Option<CalEdit>,
    /// The line Delete was pressed on once; a second press deletes.
    pub confirm: Option<usize>,
    pub notice: Option<String>,
    pub error: Option<String>,
    /// The electrical calibration dialog, over the view.
    pub electrical: Option<ElectricalDialog>,
    /// The acoustic calibration dialog, over the view.
    pub acoustic: Option<AcousticDialog>,
}

fn input_no(c: u16) -> u32 {
    u32::from(c) + 1
}

impl CalView {
    /// Opened with `focus` on the input `on` (when listed), else on the first line.
    pub fn new(s: &State, on: Option<u16>) -> Self {
        let focus = on
            .and_then(|c| lines(s).iter().position(|l| *l == CalLine::Input(c)))
            .unwrap_or(0);
        Self {
            focus,
            ..Self::default()
        }
    }

    /// The focused line (clamped: lines come and go with the state).
    pub fn focused(&self, s: &State) -> Option<CalLine> {
        let l = lines(s);
        l.get(self.focus.min(l.len().saturating_sub(1))).cloned()
    }

    fn clear(&mut self) {
        self.confirm = None;
        self.notice = None;
        self.error = None;
    }

    /// ↑/↓, wrapping. Ends (drops) a typed edit.
    pub fn move_focus(&mut self, s: &State, d: i32) {
        self.clear();
        self.edit = None;
        let n = lines(s).len() as i32;
        if n > 0 {
            self.focus = (self.focus.min(n as usize - 1) as i32 + d).rem_euclid(n) as usize;
        }
    }

    /// Moves the focus `d` lines without wrapping (PageUp / PageDown, Home / End).
    pub fn jump_focus(&mut self, s: &State, d: i32) {
        self.clear();
        self.edit = None;
        let n = lines(s).len();
        if n > 0 {
            let to = (self.focus.min(n - 1) as i64 + i64::from(d)).clamp(0, n as i64 - 1);
            self.focus = to as usize;
        }
    }

    /// ←/→ on an input: its next curve (off, then the mic's curves in import order).
    pub fn step_curve(&mut self, s: &State, forward: bool) -> Option<CalAction> {
        self.clear();
        let Some(CalLine::Input(c)) = self.focused(s) else {
            self.notice = Some("←/→ choose the mic curve on an input line".into());
            return None;
        };
        let mut row = cal::input_setup(&s.inputs, c);
        let Some(mic) = row.mic.clone() else {
            self.notice = Some(format!(
                "input {} has no mic name: N names it, then ←/→ choose its curve",
                input_no(c)
            ));
            return None;
        };
        cal::settle(&mut row, &s.mics);
        row.curve = cal::step(&row.curve, cal::mic(&s.mics, &mic), forward);
        let what = match &row.curve {
            CurveChoice::Curve { label } => format!(
                "input {}: mic curve {}",
                input_no(c),
                ac2_scene::cal::curve_name(&mic, label)
            ),
            _ => format!("input {}: mic curve off ({mic})", input_no(c)),
        };
        Some(CalAction::Inputs(row, what))
    }

    /// N on an input: types its mic name.
    pub fn start_mic_name(&mut self, s: &State) -> bool {
        self.clear();
        let Some(CalLine::Input(c)) = self.focused(s) else {
            self.notice = Some("N names the mic on an input line".into());
            return false;
        };
        self.edit = Some(CalEdit {
            kind: EditKind::MicName(c),
            text: cal::input_setup(&s.inputs, c).mic.unwrap_or_default(),
        });
        true
    }

    /// I: types the path of a curve file for the focused mic.
    pub fn start_import(&mut self, s: &State) -> bool {
        self.clear();
        let (mic, input) = match self.focused(s) {
            Some(CalLine::Input(c)) => (cal::input_setup(&s.inputs, c).mic, Some(c)),
            Some(CalLine::Curve(id)) => (Some(id.mic), None),
            Some(CalLine::Sensitivity(k)) => (Some(k.mic), None),
            None => (None, None),
        };
        let Some(mic) = mic else {
            self.notice = Some("name the mic first (N), then I imports its curve".into());
            return false;
        };
        self.edit = Some(CalEdit {
            kind: EditKind::ImportPath { mic, input },
            text: String::new(),
        });
        true
    }

    /// Typing goes to a typed edit or a calibration dialog.
    pub fn typing(&self) -> bool {
        self.edit.is_some() || self.electrical.is_some() || self.acoustic.is_some()
    }

    /// Closes an open calibration dialog (Esc); `false` when none was open.
    pub fn close_dialog(&mut self) -> bool {
        self.electrical.take().is_some() || self.acoustic.take().is_some()
    }

    /// C on an input of the open session: the acoustic calibration dialog (the mic named
    /// there, or typed in it).
    pub fn start_acoustic(&mut self, s: &State) -> bool {
        self.clear();
        let Some(CalLine::Input(c)) = self.focused(s) else {
            self.notice = Some("C calibrates an input with a calibrator: on an input line".into());
            return false;
        };
        let captured = s
            .session
            .open
            .as_ref()
            .is_some_and(|o| o.config.input_channels.contains(&c));
        if !captured {
            self.notice = Some(format!(
                "input {} is not in the open session: a calibration reads the input live",
                input_no(c)
            ));
            return false;
        }
        self.acoustic = Some(AcousticDialog::new(s, c));
        true
    }

    /// E on an input of the open session with a mic name: the electrical calibration
    /// dialog.
    pub fn start_electrical(&mut self, s: &State) -> bool {
        self.clear();
        let Some(CalLine::Input(c)) = self.focused(s) else {
            self.notice = Some("E calibrates an input electrically: on an input line".into());
            return false;
        };
        let captured = s
            .session
            .open
            .as_ref()
            .is_some_and(|o| o.config.input_channels.contains(&c));
        if !captured {
            self.notice = Some(format!(
                "input {} is not in the open session: a calibration reads the input live",
                input_no(c)
            ));
            return false;
        }
        let Some(mic) = cal::input_setup(&s.inputs, c).mic else {
            self.notice = Some(format!(
                "input {} has no mic name: N names it, then E calibrates it",
                input_no(c)
            ));
            return false;
        };
        self.electrical = Some(ElectricalDialog::new(s, c, &mic));
        true
    }

    /// A command's reply: the electrical dialog's own closes it on success and says what
    /// to do next.
    pub fn reply(&mut self, what: &str, result: &Result<(), String>) {
        if let Some(d) = &mut self.acoustic
            && d.reply(what, result)
        {
            let after = ac2_scene::cal::acoustic_after();
            self.notice = Some(format!("{what}: stored. {after}"));
            self.acoustic = None;
        }
        let Some(d) = &mut self.electrical else {
            return;
        };
        if d.reply(what, result) {
            let after = ac2_scene::cal::electrical_after(d.connection);
            self.notice = Some(format!("{what}: stored. {after}"));
            self.electrical = None;
        }
    }

    /// R on a curve: types its new label.
    pub fn start_rename(&mut self, s: &State) -> bool {
        self.clear();
        let Some(CalLine::Curve(id)) = self.focused(s) else {
            self.notice = Some("R renames the curve on a curve line".into());
            return false;
        };
        self.edit = Some(CalEdit {
            text: id.label.clone(),
            kind: EditKind::Rename(id),
        });
        true
    }

    /// Delete: asks on the first press, deletes on the second.
    pub fn delete(&mut self, s: &State) -> Option<CalAction> {
        let line = self.focused(s);
        let what = match &line {
            Some(CalLine::Curve(id)) => {
                format!(
                    "curve {} of the mic library",
                    ac2_scene::cal::curve_name(&id.mic, &id.label)
                )
            }
            Some(CalLine::Sensitivity(k)) => format!(
                "the sensitivity calibration of {} on input {} of {}",
                k.mic,
                input_no(k.channel),
                k.device.0
            ),
            _ => {
                self.clear();
                self.notice = Some("Delete removes a curve or a sensitivity calibration".into());
                return None;
            }
        };
        if self.confirm != Some(self.focus) {
            self.clear();
            self.confirm = Some(self.focus);
            self.notice = Some(format!("Delete again to delete {what}"));
            return None;
        }
        self.clear();
        match line {
            Some(CalLine::Curve(id)) => Some(CalAction::DeleteCurve(id)),
            Some(CalLine::Sensitivity(k)) => Some(CalAction::DeleteSensitivity(k)),
            _ => None,
        }
    }

    pub fn type_text(&mut self, t: &str) {
        if let Some(d) = &mut self.acoustic {
            d.type_text(t);
        } else if let Some(d) = &mut self.electrical {
            d.type_text(t);
        } else if let Some(e) = &mut self.edit {
            e.text.extend(t.chars().filter(|c| !c.is_control()));
            self.error = None;
        }
    }

    pub fn backspace(&mut self) {
        if let Some(d) = &mut self.acoustic {
            d.backspace();
        } else if let Some(d) = &mut self.electrical {
            d.backspace();
        } else if let Some(e) = &mut self.edit {
            e.text.pop();
        }
    }

    /// Enter: the typed edit's action, or why not (the edit stays open).
    pub fn finish(&mut self, s: &State) -> Option<CalAction> {
        let e = self.edit.as_ref()?;
        let text = e.text.trim().to_owned();
        let r = match &e.kind {
            EditKind::MicName(c) => {
                let mut row = cal::input_setup(&s.inputs, *c);
                let mic = (!text.is_empty()).then(|| text.clone());
                if let Some(m) = &mic
                    && let Err(err) = cal::check_mic_name(m)
                {
                    Err(err)
                } else {
                    if row.mic != mic {
                        row.mic = mic;
                        row.curve = CurveChoice::NotChosen;
                    }
                    let what = match &row.mic {
                        Some(m) => format!("input {}: mic {m}", input_no(*c)),
                        None => format!("input {}: mic name cleared", input_no(*c)),
                    };
                    Ok(CalAction::Inputs(row, what))
                }
            }
            EditKind::ImportPath { mic, input } => {
                if text.is_empty() {
                    Err("type the path of the curve file".to_owned())
                } else {
                    Ok(CalAction::Import {
                        path: std::path::PathBuf::from(&text),
                        mic: mic.clone(),
                        input: *input,
                    })
                }
            }
            EditKind::Rename(id) => {
                cal::check_label(&text).map(|()| CalAction::Rename(id.clone(), text))
            }
        };
        match r {
            Ok(a) => {
                self.edit = None;
                self.error = None;
                Some(a)
            }
            Err(err) => {
                self.error = Some(err);
                None
            }
        }
    }

    /// The prompt of the typed edit: `Mic on input 2`, `Curve file for MM1 34804 (path)`.
    pub fn edit_label(&self) -> Option<String> {
        Some(match &self.edit.as_ref()?.kind {
            EditKind::MicName(c) => format!("Mic on input {} (empty clears)", input_no(*c)),
            EditKind::ImportPath { mic, .. } => format!("Curve file for {mic} (path)"),
            EditKind::Rename(id) => format!(
                "New label for {}",
                ac2_scene::cal::curve_name(&id.mic, &id.label)
            ),
        })
    }
}

/// The rows the view draws, every string from `ac2_scene::cal`.
#[derive(Clone, Debug, PartialEq)]
pub struct LineText {
    pub line: CalLine,
    /// `in 2 · MM1 34804`, `MM1 34804 · 90°`, `MM1 34804 on in 2 of hw:UMC`.
    pub title: String,
    /// The curve state / file details / calibrator.
    pub detail: String,
    /// The sensitivity state (inputs), the inputs using a curve, the calibration's age.
    pub extra: String,
    /// Something to look at: a chosen curve not stored, no curve chosen among several.
    pub warn: bool,
}

/// The text of every line of `s`; `now` and `offset` age the calibrations.
pub fn line_texts(
    s: &State,
    now: ac2_proto::units::WallNs,
    offset: ac2_scene::time::ClockOffset,
) -> Vec<LineText> {
    use ac2_proto::cal::CurveUse;
    lines(s)
        .into_iter()
        .map(|line| match &line {
            CalLine::Input(c) => {
                let u = cal::state_input_use(s, *c);
                let captured = s
                    .session
                    .open
                    .as_ref()
                    .is_some_and(|o| o.config.input_channels.contains(c));
                LineText {
                    title: format!(
                        "in {} · {}{}",
                        input_no(*c),
                        u.mic.unwrap_or("no mic name"),
                        if captured {
                            ""
                        } else {
                            " (not in the session)"
                        }
                    ),
                    detail: ac2_scene::cal::curve_row(&u.curve),
                    extra: ac2_scene::cal::sensitivity_state(&u.sensitivity, now, offset),
                    warn: matches!(
                        u.curve,
                        CurveUse::Missing { .. } | CurveUse::NotChosen { .. }
                    ),
                    line,
                }
            }
            CalLine::Curve(id) => {
                let c = cal::curve(&s.mics, &id.mic, &id.label);
                let users: Vec<String> = s
                    .inputs
                    .iter()
                    .filter(|i| {
                        i.mic.as_deref() == Some(id.mic.as_str())
                            && i.curve
                                == (CurveChoice::Curve {
                                    label: id.label.clone(),
                                })
                    })
                    .map(|i| format!("in {}", input_no(i.channel)))
                    .collect();
                LineText {
                    title: ac2_scene::cal::curve_name(&id.mic, &id.label),
                    detail: c.map(ac2_scene::cal::curve_line).unwrap_or_default(),
                    extra: if users.is_empty() {
                        "not in use".into()
                    } else {
                        format!("in use on {}", users.join(", "))
                    },
                    warn: false,
                    line,
                }
            }
            CalLine::Sensitivity(k) => {
                let e = s.calibrations.iter().find(|e| e.key == *k);
                LineText {
                    title: format!("{} on in {} of {}", k.mic, input_no(k.channel), k.device.0),
                    detail: e.map(ac2_scene::cal::method_detail).unwrap_or_default(),
                    extra: e
                        .map(|e| {
                            format!(
                                "calibrated {}",
                                ac2_scene::format::ago(ac2_scene::time::age_s(
                                    e.spl.calibrated_at,
                                    now,
                                    offset
                                ))
                            )
                        })
                        .unwrap_or_default(),
                    warn: false,
                    line,
                }
            }
        })
        .collect()
}
