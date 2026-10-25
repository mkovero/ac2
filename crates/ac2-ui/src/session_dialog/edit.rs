//! Focus, roles and the text edits on the rows (mic names, output labels, curves).

use super::*;

impl SessionDialog {
    // ----- focus -------------------------------------------------------------------------

    /// Focus order, as the page reads top to bottom: the device rows, one row per input,
    /// one per output, then the rate and buffer (the device's defaults suit most rigs).
    pub fn rows(&self) -> Vec<Row> {
        let mut v = vec![Row::Backend, Row::Device, Row::OutputDevice];
        v.extend((0..self.inputs.len()).map(Row::Input));
        v.extend((0..self.outputs.len()).map(Row::Output));
        v.extend([Row::Rate, Row::Buffer]);
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

    // ----- roles, kinds and pairs ----------------------------------------------------------

    /// Space: puts the focused input or output in the session, or takes it out. An input
    /// taken out leaves its pair, the pair pick and the reference role: all are the
    /// session's.
    pub fn toggle(&mut self) {
        self.notice = None;
        self.error = None;
        match self.focus {
            Row::Input(i) => {
                if let Some(r) = self.inputs.get_mut(i) {
                    r.in_session = !r.in_session;
                    if !r.in_session {
                        r.reference = false;
                        let c = r.channel;
                        self.unpair(c);
                        if self.pair_pick == Some(c) {
                            self.pair_pick = None;
                        }
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

    /// R / M / L / A / Shift+P / S on the focused row.
    pub fn assign(&mut self, key: RoleKey) {
        self.error = None;
        self.notice = None;
        match (self.focus, key) {
            (Row::Input(i), RoleKey::Reference) => self.set_reference(i),
            (Row::Input(i), RoleKey::Pair) => self.toggle_pair(i),
            (Row::Input(i), k) if k.kind().is_some() => {
                if let Some(k) = k.kind() {
                    self.set_kind(i, k);
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
                self.notice = Some(
                    "R, M, L, A and Shift+P mark inputs: the loopback return (R), the kind \
                     (mic, line, digital) and a pair"
                        .into(),
                );
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

    /// R: the row becomes the one reference (again: no reference). The loopback carries
    /// the stimulus back, not a mic: a mic row made the reference becomes a line input.
    fn set_reference(&mut self, i: usize) {
        if self.inputs.get(i).is_some_and(|r| r.reference) {
            self.inputs[i].reference = false;
            return;
        }
        for r in &mut self.inputs {
            r.reference = false;
        }
        if let Some(r) = self.inputs.get_mut(i) {
            r.reference = true;
            r.in_session = true;
            if r.is_mic() {
                r.kind = InputKind::Line;
                self.notice = Some(
                    "the reference is the loopback cable, a line input: its mic name is \
                     dropped"
                        .into(),
                );
            }
        }
    }

    /// M / L / A: declares the row's kind (and puts it in the session). A line or digital
    /// kind applies to both rows of the row's pair (a pair is two of one kind); a mic
    /// leaves the pair (mics are never paired). A mic row gets the mic name last used on
    /// this input.
    fn set_kind(&mut self, i: usize, k: KindPref) {
        let Some(r) = self.inputs.get(i) else {
            return;
        };
        if r.reference && k == KindPref::Mic {
            self.notice = Some("the reference is the loopback cable: it has no mic".into());
            return;
        }
        let c = r.channel;
        let pair = self.pair_of(c).map(|(p, side)| (p.left, p.right, side));
        let partner = pair.map(|(l, r, side)| match side {
            PairSide::Left => r,
            PairSide::Right => l,
        });
        if k == KindPref::Mic && self.pair_pick == Some(c) {
            self.pair_pick = None;
        }
        if k == KindPref::Mic
            && let Some((l, r, _)) = pair
        {
            self.unpair(c);
            // A pair reads left then right wherever it is named.
            self.notice = Some(format!(
                "in {} and {} are no longer a pair: a pair is two line or two digital inputs",
                l + 1,
                r + 1
            ));
        }
        for row in &mut self.inputs {
            let own = row.channel == c;
            if !own && (k == KindPref::Mic || Some(row.channel) != partner) {
                continue;
            }
            if row.kind_pref() != k {
                let mic = self.mic_names.get(&row.channel).map(String::as_str);
                row.kind = kind_of(k, mic);
            }
            if own {
                row.in_session = true;
            }
        }
        if k == KindPref::Mic
            && let Some(name) = self.inputs[i].mic().map(str::to_owned)
        {
            let curve = self.setup_curve(c, &name);
            self.inputs[i].kind = InputKind::Mic {
                mic: Some(name),
                curve,
            };
        }
    }

    /// *pair* / Shift+P: on a paired row, unpairs it; else the first pick waits for a
    /// second, which makes the pair (the lower input left, both in the session) or is
    /// refused with the first pick kept. The picked row again cancels the pick. Both
    /// inputs must be line or both digital: a kind is the operator's to declare, so a
    /// refusal never changes one.
    fn toggle_pair(&mut self, i: usize) {
        let Some(r) = self.inputs.get(i) else {
            return;
        };
        let (c, kind, word) = (r.channel, r.kind_pref(), r.kind_word());
        if let Some((p, _)) = self.pair_of(c) {
            let (l, r) = (p.left, p.right);
            self.unpair(c);
            self.notice = Some(format!("in {} and {} are no longer a pair", l + 1, r + 1));
            return;
        }
        let rule = "a pair is two line or two digital inputs (L or A on the rows)";
        let Some(first) = self.pair_pick else {
            if kind == KindPref::Mic {
                self.notice = Some(format!("in {} is a mic: {rule}", c + 1));
                return;
            }
            self.pair_pick = Some(c);
            self.notice = Some(format!(
                "in {} picked: pair on the other input (again on in {}, or Esc, cancels)",
                c + 1,
                c + 1
            ));
            return;
        };
        if first == c {
            self.pair_pick = None;
            self.notice = Some(format!("in {} no longer picked for a pair", c + 1));
            return;
        }
        let Some(fi) = self.inputs.iter().position(|o| o.channel == first) else {
            self.pair_pick = None;
            return;
        };
        let (kind_f, word_f) = (self.inputs[fi].kind_pref(), self.inputs[fi].kind_word());
        let kept = format!("in {} stays picked", first + 1);
        if kind == KindPref::Mic {
            self.notice = Some(format!("in {} is a mic: {rule}; {kept}", c + 1));
            return;
        }
        if kind != kind_f {
            self.notice = Some(format!(
                "in {} is {word_f} and in {} is {word}: a pair is two of one kind (L or A on \
                 the rows); {kept}",
                first + 1,
                c + 1
            ));
            return;
        }
        self.pair_pick = None;
        let (left, right) = (first.min(c), first.max(c));
        for row in &mut self.inputs {
            if row.channel == left || row.channel == right {
                row.in_session = true;
            }
        }
        self.pairs.push(Pair {
            left,
            right,
            name: None,
        });
        self.pairs.sort_by_key(|p| p.left);
        self.notice = Some(format!(
            "in {} and {} are a pair (N names it)",
            left + 1,
            right + 1
        ));
    }

    /// Esc with an input picked for a pair: drops the pick. `false` when none is picked.
    pub fn cancel_pair_pick(&mut self) -> bool {
        let Some(c) = self.pair_pick.take() else {
            return false;
        };
        self.notice = Some(format!("in {} no longer picked for a pair", c + 1));
        true
    }

    /// Drops the pair `channel` is in, if any.
    pub(super) fn unpair(&mut self, channel: u16) {
        self.pairs
            .retain(|p| p.left != channel && p.right != channel);
    }

    /// N on an input row: types the mic name of a mic row (a line or digital row becomes
    /// a mic), or the name of the pair a paired row is in.
    pub fn start_mic_edit(&mut self) -> bool {
        let Row::Input(i) = self.focus else {
            self.notice = Some("N names the mic or the pair on an input row".into());
            return false;
        };
        let Some(r) = self.inputs.get(i) else {
            return false;
        };
        if let Some((p, _)) = self.pair_of(r.channel) {
            self.edit_text = p.name.clone().unwrap_or_default();
            self.edit = Some(Edit::PairName(i));
            return true;
        }
        if r.reference {
            self.notice = Some("the reference is the loopback cable: it has no mic".into());
            return false;
        }
        self.edit_text = r.mic().unwrap_or_default().to_owned();
        if !r.is_mic() {
            self.set_kind(i, KindPref::Mic);
            self.edit_text = self.inputs[i].mic().unwrap_or_default().to_owned();
        }
        self.inputs[i].in_session = true;
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
        self.edit_text = r.rig_label.clone().unwrap_or_default();
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
        let text = self.edit_text.trim().to_owned();
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

    /// Ends a name edit (Enter, ↑/↓, Tab). Another mic name starts with no curve chosen:
    /// the choice belonged to the other capsule. A pair name applies when it follows the
    /// rules (else the error says why and the old name stays). An output label edit is
    /// dropped: a rig-wide name is only sent on Enter.
    pub fn finish_edit(&mut self) {
        // The detection level has its own Enter and Esc.
        if self.edit == Some(Edit::DetectLevel) {
            return;
        }
        let text = self.edit_text.trim().to_owned();
        match self.edit {
            Some(Edit::OutputLabel(_) | Edit::DetectLevel) => {}
            Some(Edit::Mic(i)) => {
                if let Some(r) = self.inputs.get(i) {
                    let channel = r.channel;
                    let mic = (!text.is_empty()).then_some(text.clone());
                    let curve = self.setup_curve(channel, &text);
                    let r = &mut self.inputs[i];
                    if r.mic() != mic.as_deref() {
                        r.kind = InputKind::Mic { mic, curve };
                    }
                    if let Some(name) = r.mic() {
                        self.mic_names.insert(channel, name.to_owned());
                    }
                }
            }
            Some(Edit::PairName(i)) => {
                if let Some(c) = self.inputs.get(i).map(|r| r.channel) {
                    let name = (!text.is_empty()).then_some(text);
                    match name.as_deref().map(ac2_proto::cal::check_pair_name) {
                        Some(Err(e)) => self.error = Some(format!("{e}.")),
                        _ => {
                            if let Some(p) =
                                self.pairs.iter_mut().find(|p| p.left == c || p.right == c)
                            {
                                p.name = name;
                            }
                        }
                    }
                }
            }
            None => {}
        }
        self.edit = None;
    }

    /// The daemon's curve choice of `channel` while its mic is still `mic`, else none.
    pub(super) fn setup_curve(&self, channel: u16, mic: &str) -> CurveChoice {
        self.setup
            .iter()
            .find(|s| s.channel == channel && s.mic() == Some(mic) && !mic.is_empty())
            .map_or(CurveChoice::NotChosen, InputSetup::curve)
    }

    /// The input setup row `i` would send: its kind and its side of its pair.
    pub fn row_setup(&self, i: usize) -> Option<InputSetup> {
        let r = self.inputs.get(i)?;
        Some(InputSetup {
            channel: r.channel,
            kind: r.kind.clone(),
            pair: self.pair_of(r.channel).map(|(_, side)| side),
        })
    }

    /// What input row `i`'s mic uses, in words: `curve 90° · verified · 94.0 dB SPL at
    /// 1.00 kHz · 3 h ago`; `true` when it needs a look (a chosen curve not stored, none
    /// chosen among several). A line row with a line calibration: `dBu · full scale +13.0
    /// dBu · 3 h ago`. `None` for an unnamed mic and for a line or digital row reading dBFS.
    pub fn row_cal_text(
        &self,
        i: usize,
        st: &ac2_proto::model::State,
        now: ac2_proto::units::WallNs,
        offset: ac2_scene::time::ClockOffset,
    ) -> Option<(String, bool)> {
        use ac2_proto::cal::{CurveUse, Sensitivity, input_use, settle};
        let mut row = self.row_setup(i)?;
        let line = matches!(row.kind, InputKind::Line);
        if !line {
            row.mic()?;
        }
        settle(&mut row, &st.mics);
        let channel = row.channel;
        let rows = [row];
        let device = self.device_info().map(|d| &d.id);
        let u = input_use(
            &st.calibrations,
            &st.line_calibrations,
            &st.mics,
            &rows,
            device,
            channel,
        );
        if line {
            return matches!(u.sensitivity, Sensitivity::Line(_)).then(|| {
                (
                    ac2_scene::cal::sensitivity_state(&u.sensitivity, now, offset),
                    false,
                )
            });
        }
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
        let Some(mic) = row.mic().map(str::to_owned) else {
            self.notice = Some("←/→ choose the mic curve of a named mic (N names it)".into());
            return None;
        };
        ac2_proto::cal::settle(&mut row, mics);
        row.set_curve(ac2_proto::cal::step(
            &row.curve(),
            ac2_proto::cal::mic(mics, &mic),
            forward,
        ));
        self.inputs[i].kind = row.kind.clone();
        let captured = self
            .open
            .as_ref()
            .is_some_and(|o| o.config.input_channels.contains(&row.channel));
        let same_mic = live
            .iter()
            .any(|s| s.channel == row.channel && s.mic() == Some(mic.as_str()));
        if self.is_open_device() && captured && same_mic {
            if let Some(s) = self.setup.iter_mut().find(|s| s.channel == row.channel) {
                s.set_curve(row.curve());
            }
            Some(row)
        } else {
            self.notice = Some("the mic curve applies when the audio I/O opens (Enter)".into());
            None
        }
    }

    /// Typed text into the edit or the focused text row.
    pub fn type_text(&mut self, s: &str) {
        if self.text_focus() {
            self.error = None;
        }
        match self.edit {
            Some(Edit::Mic(_) | Edit::PairName(_)) => {
                for c in s.chars().filter(|c| !c.is_control()) {
                    if self.edit_text.chars().count() < MAX_MIC_NAME {
                        self.edit_text.push(c);
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
                    if self.edit_text.chars().count() < MAX_OUTPUT_LABEL {
                        self.edit_text.push(c);
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
            Some(Edit::Mic(_) | Edit::PairName(_) | Edit::OutputLabel(_)) => {
                self.edit_text.pop();
            }
            Some(Edit::DetectLevel) => {
                if let Some(d) = &mut self.detect {
                    d.level.pop();
                }
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
