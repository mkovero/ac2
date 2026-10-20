//! The EQ editor's keys and replies ([`crate::eq_editor`]): every change is one `meas.eq`
//! of the working copy, sent at once; the daemon's refusal shows in the editor verbatim.

use ac2_proto::model::{EqBody, EqEdit, Measurement};

use crate::eq_editor::{self as ed, EqEditor, HeaderField, Row, Step};
use crate::state::*;

/// `what` of the editor's `put`s: answered silently (every step said aloud would bury the
/// curve the operator is watching).
const EQ_EDIT: &str = "EQ edit";

impl AppState {
    /// Alt+E / `EQ…`: the editor on the transfer measurement, making `EQ 1` when it has no
    /// set yet.
    pub(in crate::state) fn open_eq(&mut self, out: &mut Vec<Request>) {
        if matches!(self.overlay, Overlay::Eq(_)) {
            self.overlay = Overlay::None;
            return;
        }
        let Some(m) = self.need_tf() else {
            return;
        };
        let set = m.eq.first().map(|s| s.id);
        if set.is_none() {
            self.eq_new_set(&m, out);
        }
        self.overlay = Overlay::Eq(Box::new(EqEditor::new(m.id, set)));
    }

    fn eq_new_set(&mut self, m: &Measurement, out: &mut Vec<Request>) {
        self.call(
            out,
            Command::MeasEq {
                meas: m.id,
                edit: EqEdit::New {
                    name: None,
                    realisation: None,
                    peak_q: None,
                    sections: Vec::new(),
                },
            },
            format!("{}: new EQ set", m.config.name),
        );
    }

    /// The editor's measurement, set body and the editor itself.
    fn eq_parts(&mut self) -> Option<(Measurement, Option<EqBody>)> {
        let Overlay::Eq(e) = &self.overlay else {
            return None;
        };
        let m = self.meas(e.meas)?.clone();
        let body = e.body(&m);
        Some((m, body))
    }

    fn eq_editor(&mut self) -> Option<&mut EqEditor> {
        match &mut self.overlay {
            Overlay::Eq(e) => Some(e),
            _ => None,
        }
    }

    /// Sends `body` as the set's new content.
    fn eq_put(&mut self, m: &Measurement, body: EqBody, out: &mut Vec<Request>) {
        let Some(e) = self.eq_editor() else {
            return;
        };
        let Some(set) = e.set_of(m).map(|s| s.id) else {
            return;
        };
        e.set = Some(set);
        e.working = Some(body.clone());
        e.pending += 1;
        e.refusal = None;
        self.call(
            out,
            Command::MeasEq {
                meas: m.id,
                edit: EqEdit::Put { set, body },
            },
            EQ_EDIT.to_owned(),
        );
    }

    /// A reply to an editor `put`: `true` when it was one.
    pub(in crate::state) fn eq_reply(&mut self, what: &str, result: &Result<(), String>) -> bool {
        if what != EQ_EDIT {
            return false;
        }
        match self.eq_editor() {
            Some(e) => {
                e.pending = e.pending.saturating_sub(1);
                if let Err(r) = result {
                    e.refusal = Some(r.clone());
                    // What is shown again is what the daemon holds.
                    e.working = None;
                    e.pending = 0;
                }
            }
            None => {
                if let Err(r) = result {
                    self.fault(format!("EQ: {r}"));
                }
            }
        }
        true
    }

    pub(in crate::state) fn eq_key(
        &mut self,
        chord: Chord,
        swallow: Option<char>,
        keymap: &Keymap,
        out: &mut Vec<Request>,
    ) {
        use eframe::egui::Key;
        // The key that opened it closes it, as the palette's key does.
        if keymap.lookup(Scope::Transfer, chord) == Some(CommandId::EqEditor) {
            self.overlay = Overlay::None;
            return;
        }
        let Some((m, body)) = self.eq_parts() else {
            return;
        };
        let Some(body) = body else {
            // The first set is on its way: only Esc acts.
            self.swallow_text = swallow;
            return;
        };
        let Some(set) = self.eq_editor().and_then(|e| e.set_of(&m).cloned()) else {
            return;
        };
        let curves = ed::curves(&m, &self.trace_list());
        let Some(e) = self.eq_editor() else {
            return;
        };
        if e.confirm_mark {
            e.confirm_mark = false;
            if chord.key == Key::M && !(chord.command || chord.alt) {
                let clear = chord.shift;
                self.call(
                    out,
                    Command::MeasEq {
                        meas: m.id,
                        edit: EqEdit::Mark { set: set.id, clear },
                    },
                    if clear {
                        format!("{}: mark cleared", set.body.name)
                    } else {
                        format!("{} rev {} marked as in the console", set.body.name, set.rev)
                    },
                );
            }
            return;
        }
        let plain = !(chord.command || chord.alt || chord.shift);
        let step = if chord.shift {
            Step::Coarse
        } else if chord.alt {
            Step::Fine
        } else {
            Step::Plain
        };
        let row = e.row;
        let n = body.sections.len();
        let mut next = body.clone();
        match chord.key {
            Key::ArrowUp => e.move_row(-1, n, curves.len()),
            Key::ArrowDown => e.move_row(1, n, curves.len()),
            Key::Tab => e.move_field(if chord.shift { -1 } else { 1 }, &body),
            Key::ArrowLeft | Key::ArrowRight => {
                let d = if chord.key == Key::ArrowLeft { -1 } else { 1 };
                e.typed = None;
                match row {
                    Row::Header => match e.header_field() {
                        Some(HeaderField::Set) => {
                            let at = m.eq.iter().position(|s| s.id == set.id).unwrap_or(0);
                            let k = (at as i32 + d).rem_euclid(m.eq.len() as i32) as usize;
                            e.set = Some(m.eq[k].id);
                            e.working = None;
                            e.pending = 0;
                            return;
                        }
                        Some(HeaderField::Realisation) => {
                            next.realisation = ed::step_realisation(next.realisation, d);
                        }
                        Some(HeaderField::PeakQ) => next.peak_q = ed::step_peak_q(next.peak_q, d),
                        None => return,
                    },
                    Row::Section(i) => {
                        let Some(f) = e.section_field(&body) else {
                            return;
                        };
                        let peak_q = next.peak_q;
                        if let Some(s) = next.sections.get_mut(i) {
                            ed::step_field(&mut s.shape, f, d, step, peak_q);
                        }
                    }
                    Row::Curve(_) => return,
                }
                self.eq_put(&m, next, out);
            }
            Key::Insert if chord.shift => self.eq_new_set(&m, out),
            Key::Insert => {
                let at = match row {
                    Row::Section(i) => i + 1,
                    Row::Header => 0,
                    Row::Curve(_) => n,
                };
                next.sections.insert(at, ed::new_section());
                e.row = Row::Section(at);
                e.field = 1;
                self.eq_put(&m, next, out);
            }
            Key::Delete => {
                if let Row::Section(i) = row {
                    next.sections.remove(i);
                    if next.sections.is_empty() {
                        e.row = Row::Header;
                    } else {
                        e.row = Row::Section(i.min(next.sections.len() - 1));
                    }
                    self.eq_put(&m, next, out);
                }
            }
            Key::B if chord.shift && !(chord.command || chord.alt) => {
                next.enabled = !next.enabled;
                self.eq_put(&m, next, out);
            }
            Key::B if plain => {
                if let Row::Section(i) = row {
                    next.sections[i].enabled = !next.sections[i].enabled;
                    self.eq_put(&m, next, out);
                }
            }
            Key::OpenBracket | Key::CloseBracket if plain => {
                if let Row::Section(i) = row {
                    let j = if chord.key == Key::OpenBracket {
                        i.checked_sub(1)
                    } else {
                        (i + 1 < n).then_some(i + 1)
                    };
                    if let Some(j) = j {
                        next.sections.swap(i, j);
                        e.row = Row::Section(j);
                        self.eq_put(&m, next, out);
                    }
                }
            }
            Key::P if plain => {
                if let Row::Curve(i) = row
                    && let Some(c) = curves.get(i)
                {
                    let on = !ed::predicted(&self.measurements(), m.id, set.id, &c.operand);
                    self.call(
                        out,
                        Command::MeasEq {
                            meas: m.id,
                            edit: EqEdit::Predict {
                                set: set.id,
                                curve: c.operand,
                                on,
                            },
                        },
                        format!(
                            "{} {} {}",
                            c.name,
                            if on {
                                "predicted after"
                            } else {
                                "no longer predicted after"
                            },
                            set.body.name
                        ),
                    );
                }
            }
            Key::M if plain => e.confirm_mark = true,
            _ => self.swallow_text = swallow,
        }
    }

    /// A number typed into the selected section's field: applied as soon as it reads as
    /// one.
    pub(in crate::state) fn eq_text(&mut self, t: &str, out: &mut Vec<Request>) {
        let Some((m, Some(body))) = self.eq_parts() else {
            return;
        };
        let Some(e) = self.eq_editor() else {
            return;
        };
        let Row::Section(i) = e.row else {
            return;
        };
        let Some(field) = e.section_field(&body) else {
            return;
        };
        let typed: String = t
            .chars()
            .filter(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'k' | 'K'))
            .collect();
        if typed.is_empty() {
            return;
        }
        let mut buf = e.typed.take().unwrap_or_default();
        buf.push_str(&typed.to_lowercase());
        e.typed = Some(buf.clone());
        let mut next = body;
        if let Some(s) = next.sections.get_mut(i)
            && ed::type_field(&mut s.shape, field, &buf).is_some()
        {
            self.eq_put(&m, next, out);
        }
    }

    /// Backspace: edits the typed number, else removes the selected section.
    pub(in crate::state) fn eq_backspace(&mut self, out: &mut Vec<Request>) {
        let Some(e) = self.eq_editor() else {
            return;
        };
        match e.typed.as_mut() {
            Some(t) => {
                t.pop();
            }
            None => {
                let keymap = Keymap::default();
                self.eq_key(Chord::key(eframe::egui::Key::Delete), None, &keymap, out);
            }
        }
    }
}
