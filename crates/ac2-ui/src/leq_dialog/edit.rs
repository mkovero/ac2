//! Typing in the text cells, inserting and removing windows, and the commands built from the
//! edits.

use super::*;

impl LeqDialog {
    fn text_mut(&mut self) -> Option<&mut String> {
        if let Focus::Cue { row, col } = self.focus {
            return self.cue_edit_mut(row).and_then(|c| c.cell_mut(col));
        }
        if let Focus::Band(b) = self.focus {
            return self.bands.text_mut(b);
        }
        if let Focus::Extra(e) = self.focus {
            if e.quantity().is_some() {
                self.before_preset = None;
            }
            return Some(&mut self.extra[e.index()]);
        }
        let Focus::Window { row, col } = self.focus else {
            return None;
        };
        self.before_preset = None;
        let r = self.rows.get_mut(row)?;
        match col {
            Col::Limit => Some(&mut r.limit),
            Col::Margin => Some(&mut r.margin),
            Col::Length | Col::Weighting => None,
        }
    }

    /// The focus is on a cue line, edited at the caret.
    pub fn on_caret_text(&self) -> bool {
        matches!(self.focus, Focus::Cue { col, .. } if col.is_text())
    }

    /// The focused cue line's text.
    pub fn caret_text(&self) -> Option<&str> {
        let Focus::Cue { row, col } = self.focus else {
            return None;
        };
        let c = self.cue_edit(row)?;
        match col {
            CueCol::State(s) => Some(c.text_of(s)),
            CueCol::Dmx(s) => Some(c.dmx_of(s)),
            CueCol::Append => None,
        }
    }

    /// The focus arrived on a cell: a cue line puts the caret at its end, other text is
    /// selected (typing replaces it).
    pub fn entered(&mut self) {
        if let Some(t) = self.caret_text() {
            self.caret = Caret::end_of(t);
            self.selected = false;
        } else {
            self.selected = self.on_text();
        }
    }

    /// A click on cue line `at`, before its character `n`.
    pub fn caret_at(&mut self, at: Focus, n: usize) {
        self.focus = at;
        self.entered();
        if let Some(t) = self.caret_text() {
            self.caret = Caret::at(t, n);
        }
    }

    /// Applies `f` to the focused text and the caret, kept apart from `self` meanwhile.
    fn edit_text(&mut self, f: impl FnOnce(&mut String, &mut Caret, bool)) {
        let selected = std::mem::take(&mut self.selected);
        let mut c = self.caret;
        if let Some(t) = self.text_mut() {
            f(t, &mut c, selected);
            self.error = None;
        }
        self.caret = c;
    }

    /// Typed text into the focused cell: at the caret on a cue line, else replacing a
    /// selected text or after the text.
    pub fn type_text(&mut self, s: &str) {
        let caret = self.on_caret_text();
        self.edit_text(|t, c, selected| {
            if selected {
                t.clear();
                *c = Caret::default();
            }
            if caret {
                c.insert(t, s);
            } else {
                t.push_str(s);
            }
        });
    }

    /// Backspace: the character before the caret on a cue line, else the last one; or the
    /// selected text.
    pub fn backspace(&mut self) {
        let caret = self.on_caret_text();
        self.edit_text(|t, c, selected| {
            if selected {
                t.clear();
                *c = Caret::default();
            } else if caret {
                c.backspace(t);
            } else {
                t.pop();
            }
        });
    }

    /// Delete on a cue line: the character after the caret, or the selected text.
    pub fn delete_char(&mut self) {
        self.edit_text(|t, c, selected| {
            if selected {
                t.clear();
                *c = Caret::default();
            } else {
                c.delete(t);
            }
        });
    }

    /// ←/→ on a cue line: the caret a character left or right.
    pub fn caret_step(&mut self, d: i32) {
        self.selected = false;
        if let Some(t) = self.caret_text() {
            let mut c = self.caret;
            c.step(t, d);
            self.caret = c;
        }
    }

    /// Home / End on a cue line.
    pub fn caret_edge(&mut self, end: bool) {
        self.selected = false;
        if let Some(t) = self.caret_text() {
            let mut c = self.caret;
            if end {
                c.end(t);
            } else {
                c.home();
            }
            self.caret = c;
        }
    }

    /// Ctrl+A on a text cell.
    pub fn select_all(&mut self) {
        self.selected = self.on_text();
    }

    /// Insert: a new window after the focused one (or at the end), one length longer; in
    /// the band section a band window.
    pub fn add_window(&mut self) {
        if let Focus::Band(b) = self.focus {
            match self.bands.add_window(b) {
                Ok(f) => {
                    self.focus = Focus::Band(f);
                    self.selected = false;
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
            return;
        }
        self.before_preset = None;
        if self.rows.len() >= LeqConfig::MAX_WINDOWS {
            self.error = Some(format!(
                "at most {} windows per meter",
                LeqConfig::MAX_WINDOWS
            ));
            return;
        }
        let after = match self.focus {
            Focus::Window { row, .. } => row,
            _ => self.rows.len().saturating_sub(1),
        };
        let after = after.min(self.rows.len().saturating_sub(1));
        let from = self.rows.get(after).cloned().unwrap_or(Row {
            seconds: 60,
            weighting: Weighting::A,
            limit: String::new(),
            margin: "3".into(),
            cue: CueEdit::default(),
        });
        let seconds = LENGTHS
            .iter()
            .copied()
            .find(|&s| s > from.seconds)
            .unwrap_or(from.seconds);
        let at = (after + 1).min(self.rows.len());
        self.rows.insert(
            at,
            Row {
                seconds,
                limit: String::new(),
                cue: CueEdit::default(),
                ..from
            },
        );
        self.focus = Focus::Window {
            row: at,
            col: Col::Length,
        };
        self.selected = false;
        self.error = None;
    }

    /// Shift+Insert, or "+ range…" at the band windows' heading: the range row opened.
    pub fn open_range(&mut self) {
        let at = match self.focus {
            Focus::Band(b) => b,
            _ => BandFocus::Field(BandField::Meter),
        };
        self.focus = Focus::Band(self.bands.open_range(at));
        self.selected = false;
        self.error = None;
    }

    /// Whether the focus is on the open range row (Enter adds it, Delete closes it).
    pub fn on_range(&self) -> bool {
        matches!(self.focus, Focus::Band(BandFocus::Range(_))) && self.bands.range.is_some()
    }

    /// Add on the range row: one band window per band of it.
    pub fn add_range(&mut self) {
        match self.bands.add_range() {
            Ok(f) => {
                self.focus = Focus::Band(f);
                self.selected = false;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// The range row closed without adding.
    pub fn close_range(&mut self) {
        self.focus = Focus::Band(self.bands.close_range());
        self.selected = false;
        self.error = None;
    }

    /// Delete: removes the focused window (closes the range row).
    pub fn remove_window(&mut self) {
        if self.on_range() {
            self.close_range();
            return;
        }
        if let Focus::Band(BandFocus::Window { row, col }) = self.focus {
            self.focus = Focus::Band(self.bands.remove_window(row, col));
            self.entered();
            self.error = None;
            return;
        }
        let Focus::Window { row, col } = self.focus else {
            return;
        };
        self.before_preset = None;
        if row < self.rows.len() {
            self.rows.remove(row);
        }
        self.focus = if self.rows.is_empty() {
            Focus::Horizon
        } else {
            Focus::Window {
                row: row.min(self.rows.len() - 1),
                col,
            }
        };
        self.error = None;
    }

    /// The windows as configured, checked.
    pub fn leq_config(&self) -> Result<LeqConfig, String> {
        let windows = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| r.window(i + 1))
            .collect::<Result<Vec<_>, _>>()?;
        let mut peaks = PeakLimits::default();
        for e in [Extra::LcPeak, Extra::LafMax] {
            let Some(q) = e.quantity() else { continue };
            let t = self.extra[e.index()].trim();
            if t.is_empty() {
                continue;
            }
            let name = q.name();
            let v = ac2_scene::unit_text::in_field::<ac2_scene::unit_text::SplLevel>(t, "db")
                .map_err(|err| format!("{name} limit: {err}"))?
                .0
                .0;
            if !(60.0..=170.0).contains(&v) {
                return Err(format!(
                    "{name}: a limit is 60 … 170 dB SPL (empty for none)"
                ));
            }
            // A margin set before (from the CLI) stays; a new limit warns 3 dB under.
            let before = self.base.leq.peaks.get(q);
            let warn_margin =
                before.map_or(Db(LeqWindow::DEFAULT_WARN_MARGIN_DB), |l| l.warn_margin);
            *peaks.get_mut(q) = Some(PeakLimit {
                cue: self.peak_cues[peak_index(q)].checked(name)?,
                limit: DbSpl(v),
                warn_margin,
            });
        }
        let cfg = LeqConfig {
            windows,
            horizon: Seconds(f64::from(self.horizon_s())),
            peaks,
        };
        cfg.check()?;
        Ok(cfg)
    }

    /// The measuring-position correction as typed: none when empty; the peaks' the same
    /// as the energy levels' unless typed.
    pub fn position(&self) -> Result<Option<PositionCorrection>, String> {
        let parse = |t: &str, what: &str| {
            ac2_scene::unit_text::in_field::<ac2_scene::unit_text::Gain>(t, "db")
                .map(|g| g.0.0)
                .map_err(|e| format!("{what}: {e}"))
        };
        let level = self.extra[Extra::Position.index()].trim();
        let peak = self.extra[Extra::PositionPeak.index()].trim();
        if level.is_empty() {
            return if peak.is_empty() {
                Ok(None)
            } else {
                Err("a correction for peaks needs one for the levels too".into())
            };
        }
        let level = parse(level, "position correction")?;
        let peak = if peak.is_empty() {
            level
        } else {
            parse(peak, "position correction for peaks")?
        };
        let p = PositionCorrection {
            level: Db(level),
            peak: Db(peak),
        };
        if !p.is_valid() {
            return Err(format!(
                "a position correction is at most ±{} dB",
                PositionCorrection::MAX_DB
            ));
        }
        Ok(Some(p))
    }

    /// The meter's configuration with the new windows, peak limits, correction, scope and
    /// cues.
    pub fn meas_config(&self) -> Result<MeasConfig, String> {
        let mut bands = self.bands.config()?;
        if let Some(p) = bands.as_mut().and_then(|b| b.predicted.as_mut()) {
            p.cue = self.predicted_cue.checked("the predicted LAeq")?;
        }
        Ok(MeasConfig {
            scope: if self.on_rig {
                ac2_proto::model::MeasScope::Rig
            } else {
                ac2_proto::model::MeasScope::Session
            },
            name: self.name.clone(),
            kind: MeasKind::Spl {
                config: SplConfig {
                    leq: self.leq_config()?,
                    position: self.position()?,
                    bands: bands.map(Box::new),
                    cue: self.meter_cue.checked("the meter")?,
                    ..self.base.clone()
                },
            },
        })
    }
}
