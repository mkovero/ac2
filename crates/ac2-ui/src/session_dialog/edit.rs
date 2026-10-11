//! Focus, roles and the text edits on the rows (mic names, output labels, curves).

use super::*;

impl SessionDialog {
    // ----- focus -------------------------------------------------------------------------

    /// Focus order: the device rows, then one row per input, then one per output.
    pub fn rows(&self) -> Vec<Row> {
        let mut v = vec![
            Row::Backend,
            Row::Device,
            Row::OutputDevice,
            Row::Rate,
            Row::Buffer,
        ];
        v.extend((0..self.inputs.len()).map(Row::Input));
        v.extend((0..self.outputs.len()).map(Row::Output));
        v
    }

    /// ↑/↓ (Tab / Shift+Tab), wrapping. Ends a mic-name edit; drops an output label edit.
    pub fn move_focus(&mut self, d: i32) {
        self.finish_edit();
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let i = rows.iter().position(|r| *r == self.focus).unwrap_or(0) as i32;
        let n = rows.len() as i32;
        self.focus = rows[(i + d).rem_euclid(n) as usize];
    }

    /// Focuses `row` (the mouse).
    pub fn focus_row(&mut self, row: Row) {
        if self.rows().contains(&row) {
            self.finish_edit();
            self.focus = row;
        }
    }

    /// The focused row takes typed text (rate, buffer) or an edit is running.
    pub fn text_focus(&self) -> bool {
        self.edit.is_some() || matches!(self.focus, Row::Rate | Row::Buffer)
    }

    // ----- roles -------------------------------------------------------------------------

    /// Space: puts the focused input or output in the session, or takes it out.
    pub fn toggle(&mut self) {
        self.notice = None;
        self.error = None;
        match self.focus {
            Row::Input(i) => {
                if let Some(r) = self.inputs.get_mut(i) {
                    r.in_session = !r.in_session;
                    if !r.in_session {
                        r.role = InputRole::None;
                    }
                }
            }
            Row::Output(o) => {
                let o = o as u16;
                self.out_count = if o < self.out_count { o } else { o + 1 };
                let n = self.out_count;
                for row in &mut self.outputs {
                    if row.channel >= n {
                        row.stimulus = false;
                    }
                }
            }
            _ => {}
        }
    }

    /// R / M / S on the focused row.
    pub fn assign(&mut self, key: RoleKey) {
        self.error = None;
        self.notice = None;
        match (self.focus, key) {
            (Row::Input(i), RoleKey::Reference) => {
                if self
                    .inputs
                    .get(i)
                    .is_some_and(|r| r.role == InputRole::Reference)
                {
                    self.inputs[i].role = InputRole::None;
                    return;
                }
                for r in &mut self.inputs {
                    if r.role == InputRole::Reference {
                        r.role = InputRole::None;
                    }
                }
                if let Some(r) = self.inputs.get_mut(i) {
                    r.role = InputRole::Reference;
                    r.in_session = true;
                }
            }
            (Row::Input(i), RoleKey::Mic) => {
                if let Some(r) = self.inputs.get_mut(i) {
                    r.role = if r.role == InputRole::Mic {
                        InputRole::None
                    } else {
                        r.in_session = true;
                        InputRole::Mic
                    };
                }
            }
            (Row::Output(o), RoleKey::Stimulus) => {
                if let Some(r) = self.outputs.get_mut(o) {
                    r.stimulus = !r.stimulus;
                    if r.stimulus {
                        self.out_count = self.out_count.max(r.channel + 1);
                    }
                }
            }
            (Row::Output(_), _) => {
                self.notice =
                    Some("R and M mark inputs: the loopback return (R) and the mics (M)".into());
            }
            (Row::Input(_), RoleKey::Stimulus) => {
                self.notice = Some(
                    "S marks an output: the one that feeds your speakers and the loopback".into(),
                );
            }
            _ => {
                self.notice = Some("move to an input or output row first (↑↓)".into());
            }
        }
    }

    /// N: types the focused input's mic name (it becomes a mic).
    pub fn start_mic_edit(&mut self) -> bool {
        let Row::Input(i) = self.focus else {
            self.notice = Some("N names the mic on an input row".into());
            return false;
        };
        if self
            .inputs
            .get(i)
            .is_some_and(|r| r.role == InputRole::Reference)
        {
            self.notice = Some("the reference is the loopback cable: it has no mic".into());
            return false;
        }
        if let Some(r) = self.inputs.get_mut(i) {
            r.role = InputRole::Mic;
            r.in_session = true;
        }
        self.edit = Some(Edit::Mic(i));
        true
    }

    /// N on an output row: types its label (the rig's name for it, for every client).
    pub fn start_label_edit(&mut self) -> bool {
        let Row::Output(i) = self.focus else {
            return false;
        };
        let Some(r) = self.outputs.get(i) else {
            return false;
        };
        self.label_text = r.rig_label.clone().unwrap_or_default();
        self.edit = Some(Edit::OutputLabel(i));
        self.notice = None;
        self.error = None;
        true
    }

    /// Enter on an output label edit: the row to send (`None` label clears it), or why not.
    pub fn commit_label(&mut self) -> Result<Option<OutputSetup>, String> {
        let Some(Edit::OutputLabel(i)) = self.edit else {
            return Ok(None);
        };
        let Some(r) = self.outputs.get(i) else {
            self.edit = None;
            return Ok(None);
        };
        let text = self.label_text.trim().to_owned();
        let label = (!text.is_empty()).then_some(text);
        if let Some(l) = &label {
            ac2_proto::model::check_output_label(l)?;
        }
        let row = OutputSetup {
            channel: r.channel,
            label,
        };
        self.edit = None;
        Ok((self.outputs[i].rig_label != row.label).then_some(row))
    }

    /// Ends a mic-name edit (Enter, ↑/↓, Tab). Another mic name starts with no curve
    /// chosen: the choice belonged to the other capsule. An output label edit is dropped:
    /// a rig-wide name is only sent on Enter.
    pub fn finish_edit(&mut self) {
        if let Some(Edit::OutputLabel(_)) = self.edit {
            self.edit = None;
            return;
        }
        if let Some(Edit::Mic(i)) = self.edit
            && let Some(r) = self.inputs.get(i)
        {
            let mic = r.mic.trim().to_owned();
            let curve = self.setup_curve(r.channel, &mic);
            let r = &mut self.inputs[i];
            r.mic = mic;
            r.curve = curve;
            self.edit = None;
        }
    }

    /// The daemon's curve choice of `channel` while its mic is still `mic`, else none.
    pub(super) fn setup_curve(&self, channel: u16, mic: &str) -> CurveChoice {
        self.setup
            .iter()
            .find(|s| s.channel == channel && s.mic.as_deref() == Some(mic) && !mic.is_empty())
            .map_or(CurveChoice::NotChosen, |s| s.curve.clone())
    }

    /// The input setup row `i` would send: its mic (a mic row's name) and curve.
    pub fn row_setup(&self, i: usize) -> Option<InputSetup> {
        let r = self.inputs.get(i)?;
        let mic =
            (r.role == InputRole::Mic && !r.mic.trim().is_empty()).then(|| r.mic.trim().to_owned());
        Some(InputSetup {
            channel: r.channel,
            curve: if mic.is_some() {
                r.curve.clone()
            } else {
                CurveChoice::NotChosen
            },
            mic,
        })
    }

    /// What input row `i`'s mic uses, in words: `curve 90° · verified · 94.0 dB SPL at
    /// 1.00 kHz · 3 h ago`; `true` when it needs a look (a chosen curve not stored, none
    /// chosen among several). `None` for a row without a mic name.
    pub fn row_cal_text(
        &self,
        i: usize,
        st: &ac2_proto::model::State,
        now: ac2_proto::units::WallNs,
        offset: ac2_scene::time::ClockOffset,
    ) -> Option<(String, bool)> {
        use ac2_proto::cal::{CurveUse, input_use, settle};
        let mut row = self.row_setup(i)?;
        row.mic.as_ref()?;
        settle(&mut row, &st.mics);
        let channel = row.channel;
        let rows = [row];
        let device = self.device_info().map(|d| &d.id);
        let u = input_use(&st.calibrations, &st.mics, &rows, device, channel);
        Some((
            format!(
                "{} · {}",
                ac2_scene::cal::curve_row(&u.curve),
                ac2_scene::cal::sensitivity_state(&u.sensitivity, now, offset)
            ),
            matches!(
                u.curve,
                CurveUse::Missing { .. } | CurveUse::NotChosen { .. }
            ),
        ))
    }

    /// ←/→ on a mic's input row: the next of its curves (off, then the library's curves of
    /// the mic in import order). Returns the row to send now when the open session captures
    /// this input with this mic already (`live` is the daemon's input setup): the change then
    /// applies to the running measurements without reopening anything.
    pub fn step_curve(
        &mut self,
        forward: bool,
        mics: &[Mic],
        live: &[InputSetup],
    ) -> Option<InputSetup> {
        self.notice = None;
        let Row::Input(i) = self.focus else {
            return None;
        };
        let mut row = self.row_setup(i)?;
        let Some(mic) = row.mic.clone() else {
            self.notice = Some("←/→ choose the mic curve of a named mic (N names it)".into());
            return None;
        };
        ac2_proto::cal::settle(&mut row, mics);
        row.curve = ac2_proto::cal::step(&row.curve, ac2_proto::cal::mic(mics, &mic), forward);
        self.inputs[i].curve = row.curve.clone();
        let captured = self
            .open
            .as_ref()
            .is_some_and(|o| o.config.input_channels.contains(&row.channel));
        let same_mic = live
            .iter()
            .any(|s| s.channel == row.channel && s.mic.as_deref() == Some(mic.as_str()));
        if self.is_open_device() && captured && same_mic {
            if let Some(s) = self.setup.iter_mut().find(|s| s.channel == row.channel) {
                s.curve = row.curve.clone();
            }
            Some(row)
        } else {
            self.notice = Some("the mic curve applies when the session opens (Enter)".into());
            None
        }
    }

    /// Typed text into the edit or the focused text row.
    pub fn type_text(&mut self, s: &str) {
        if self.text_focus() {
            self.error = None;
        }
        match self.edit {
            Some(Edit::Mic(i)) => {
                if let Some(r) = self.inputs.get_mut(i) {
                    for c in s.chars().filter(|c| !c.is_control()) {
                        if r.mic.chars().count() < MAX_MIC_NAME {
                            r.mic.push(c);
                        }
                    }
                }
            }
            Some(Edit::DetectLevel) => {
                if let Some(d) = &mut self.detect {
                    d.level.push_str(s);
                    d.error = None;
                }
            }
            Some(Edit::OutputLabel(_)) => {
                for c in s.chars().filter(|c| !c.is_control()) {
                    if self.label_text.chars().count() < MAX_OUTPUT_LABEL {
                        self.label_text.push(c);
                    }
                }
            }
            None => match self.focus {
                Row::Rate => self.rate.push_str(s),
                Row::Buffer => self.buffer.push_str(s),
                _ => {}
            },
        }
    }

    pub fn backspace(&mut self) {
        match self.edit {
            Some(Edit::Mic(i)) => {
                if let Some(r) = self.inputs.get_mut(i) {
                    r.mic.pop();
                }
            }
            Some(Edit::DetectLevel) => {
                if let Some(d) = &mut self.detect {
                    d.level.pop();
                }
            }
            Some(Edit::OutputLabel(_)) => {
                self.label_text.pop();
            }
            None => match self.focus {
                Row::Rate => {
                    self.rate.pop();
                }
                Row::Buffer => {
                    self.buffer.pop();
                }
                _ => {}
            },
        }
    }
}
