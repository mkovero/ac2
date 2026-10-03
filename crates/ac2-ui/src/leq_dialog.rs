//! The Leq windows dialog of an SPL meter (`docs/design/leq.md`): one row per window —
//! its length and weighting picked from named choices, its limit and warn margin typed in
//! dB — a preset row that sets a published rule's limits on its windows, and the headroom
//! horizon.
//! Pure data; the reducer routes keys here and the view draws it. Enter sends the meter's
//! configuration with the new windows (`meas.update`, applied in place by the daemon).

use ac2_proto::model::{
    LeqConfig, LeqPreset, LeqWindow, MeasConfig, MeasKind, Measurement, SplConfig, Weighting,
};
use ac2_proto::units::{Db, DbSpl, MeasId, Seconds};

/// Window lengths offered, shortest first (→ longer). A length outside the list (from the
/// CLI) is kept until changed.
pub const LENGTHS: [u32; 12] = [
    5, 10, 30, 60, 300, 600, 900, 1800, 3600, 7200, 28_800, 86_400,
];
/// Headroom horizons offered.
pub const HORIZONS: [u32; 6] = [10, 30, 60, 120, 300, 900];
const WEIGHTINGS: [Weighting; 3] = [Weighting::A, Weighting::C, Weighting::Z];

/// A column of a window row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Length,
    Weighting,
    Limit,
    Margin,
}

impl Col {
    const ALL: [Col; 4] = [Col::Length, Col::Weighting, Col::Limit, Col::Margin];

    /// Column heading.
    pub fn title(self) -> &'static str {
        match self {
            Col::Length => "Window",
            Col::Weighting => "Weighting",
            Col::Limit => "Limit (dB)",
            Col::Margin => "Warn within (dB)",
        }
    }

    fn is_text(self) -> bool {
        matches!(self, Col::Limit | Col::Margin)
    }
}

/// Where the focus is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// The preset choice.
    Preset,
    /// The headroom horizon choice.
    Horizon,
    /// Window `row`, column `col`.
    Window { row: usize, col: Col },
}

/// One window being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub seconds: u32,
    pub weighting: Weighting,
    /// Typed limit; empty: none.
    pub limit: String,
    /// Typed warn margin.
    pub margin: String,
}

fn number_text(v: f64) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").map_or(s.clone(), str::to_owned)
}

impl Row {
    fn of(w: &LeqWindow) -> Self {
        Self {
            seconds: w.seconds().unwrap_or(60),
            weighting: w.weighting,
            limit: w.limit.map_or_else(String::new, |l| number_text(l.0)),
            margin: number_text(w.warn_margin.0),
        }
    }

    /// The row as the dialog shows it, column by column.
    pub fn cell(&self, c: Col) -> String {
        match c {
            Col::Length => format!(
                "L{}eq {}",
                weighting_letter(self.weighting),
                ac2_scene::leq::length(f64::from(self.seconds))
            ),
            Col::Weighting => weighting_letter(self.weighting).to_owned(),
            Col::Limit => self.limit.clone(),
            Col::Margin => self.margin.clone(),
        }
    }

    fn window(&self, n: usize) -> Result<LeqWindow, String> {
        let name = self.cell(Col::Length);
        let limit = if self.limit.trim().is_empty() {
            None
        } else {
            let v = crate::state::parse_number(&self.limit, &["db spl", "dbspl", "db"])
                .map_err(|e| format!("window {n} ({name}) limit: {e}"))?;
            if !(30.0..=150.0).contains(&v) {
                return Err(format!(
                    "window {n} ({name}): a limit is 30 … 150 dB SPL (empty for none)"
                ));
            }
            Some(DbSpl(v))
        };
        let margin = crate::state::parse_number(&self.margin, &["db"])
            .map_err(|e| format!("window {n} ({name}) warn margin: {e}"))?;
        if !(0.0..=20.0).contains(&margin) {
            return Err(format!("window {n} ({name}): the warn margin is 0 … 20 dB"));
        }
        Ok(LeqWindow {
            duration: Seconds(f64::from(self.seconds)),
            weighting: self.weighting,
            limit,
            warn_margin: Db(margin),
        })
    }
}

fn weighting_letter(w: Weighting) -> &'static str {
    match w {
        Weighting::A => "A",
        Weighting::C => "C",
        Weighting::Z => "Z",
    }
}

/// The open dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct LeqDialog {
    pub meas: MeasId,
    /// The meter's name.
    pub name: String,
    /// The meter reads dB SPL: limits are judged.
    pub calibrated: bool,
    pub rows: Vec<Row>,
    /// Index into [`HORIZONS`] (a horizon outside the list: the nearest).
    pub horizon: usize,
    /// Index into [`LeqPreset::ALL`], or `None`: "keep these limits".
    pub preset: Option<usize>,
    /// The windows before the preset row was first changed: moving through the presets
    /// shows each one over these, not over the one before. Dropped on any edit of a
    /// window, which keeps what the preset set.
    before_preset: Option<Vec<Row>>,
    pub focus: Focus,
    /// The focused text cell's text is selected: typing replaces it.
    pub selected: bool,
    /// Why the last Enter was refused.
    pub error: Option<String>,
    base: SplConfig,
}

impl LeqDialog {
    /// The dialog over SPL meter `m`; `None` for another kind of measurement.
    pub fn new(m: &Measurement, calibrated: bool) -> Option<Self> {
        let MeasKind::Spl { config } = &m.config.kind else {
            return None;
        };
        let h = config.leq.horizon_seconds().unwrap_or(60);
        let horizon = HORIZONS
            .iter()
            .enumerate()
            .min_by_key(|(_, x)| x.abs_diff(h))
            .map_or(2, |(i, _)| i);
        Some(Self {
            meas: m.id,
            name: m.config.name.clone(),
            calibrated,
            rows: config.leq.windows.iter().map(Row::of).collect(),
            horizon,
            preset: None,
            before_preset: None,
            focus: Focus::Preset,
            selected: false,
            error: None,
            base: config.clone(),
        })
    }

    /// The headroom horizon chosen, s.
    pub fn horizon_s(&self) -> u32 {
        HORIZONS[self.horizon.min(HORIZONS.len() - 1)]
    }

    /// The preset chosen, as shown.
    pub fn preset_text(&self) -> String {
        match self.preset.and_then(|i| LeqPreset::ALL.get(i)) {
            None => "none (keep these limits)".into(),
            Some(p) => ac2_scene::leq_preset::summary(*p),
        }
    }

    /// Where the chosen preset's figures come from.
    pub fn preset_source(&self) -> Option<String> {
        self.preset
            .and_then(|i| LeqPreset::ALL.get(i))
            .map(|p| ac2_scene::leq_preset::source(*p))
    }

    fn rows_count(&self) -> usize {
        // Preset, horizon, then the windows.
        2 + self.rows.len()
    }

    fn row_index(&self) -> usize {
        match self.focus {
            Focus::Preset => 0,
            Focus::Horizon => 1,
            Focus::Window { row, .. } => 2 + row,
        }
    }

    fn col(&self) -> Col {
        match self.focus {
            Focus::Window { col, .. } => col,
            _ => Col::Length,
        }
    }

    fn set_row(&mut self, i: usize, col: Col) {
        self.focus = match i {
            0 => Focus::Preset,
            1 => Focus::Horizon,
            r => Focus::Window { row: r - 2, col },
        };
        self.selected = col.is_text() && matches!(self.focus, Focus::Window { .. });
    }

    /// ↑ / ↓: the row above or below, keeping the column; wraps.
    pub fn move_row(&mut self, d: i32) {
        let n = self.rows_count() as i32;
        let i = (self.row_index() as i32 + d).rem_euclid(n) as usize;
        self.set_row(i, self.col());
    }

    /// Tab / Shift+Tab: the next or previous cell, row by row; wraps.
    pub fn move_cell(&mut self, d: i32) {
        // Cells in order: preset, horizon, then four per window.
        let cells = 2 + 4 * self.rows.len() as i32;
        let at = match self.focus {
            Focus::Preset => 0,
            Focus::Horizon => 1,
            Focus::Window { row, col } => {
                2 + 4 * row as i32 + Col::ALL.iter().position(|c| *c == col).unwrap_or(0) as i32
            }
        };
        let next = (at + d).rem_euclid(cells);
        self.focus = match next {
            0 => Focus::Preset,
            1 => Focus::Horizon,
            k => Focus::Window {
                row: ((k - 2) / 4) as usize,
                col: Col::ALL[((k - 2) % 4) as usize],
            },
        };
        self.selected = self.col().is_text() && matches!(self.focus, Focus::Window { .. });
    }

    /// Focuses window `row`, column `col` (the mouse).
    pub fn focus_cell(&mut self, row: usize, col: Col) {
        if row < self.rows.len() {
            self.set_row(row + 2, col);
        }
    }

    /// ←/→ on a choice: the next or previous option, stopping at the ends.
    pub fn cycle(&mut self, d: i32) {
        let step = |i: usize, n: usize| (i as i64 + i64::from(d)).clamp(0, n as i64 - 1) as usize;
        match self.focus {
            Focus::Preset => {
                // Option 0 is "none"; presets follow.
                let n = LeqPreset::ALL.len() + 1;
                let cur = self.preset.map_or(0, |i| i + 1);
                let next = step(cur, n);
                self.preset = next.checked_sub(1);
                let base = self
                    .before_preset
                    .get_or_insert_with(|| self.rows.clone())
                    .clone();
                self.rows = base;
                match self.preset.and_then(|i| LeqPreset::ALL.get(i)) {
                    Some(p) => {
                        let refused = self.apply_preset(*p).err();
                        self.error = refused;
                        return;
                    }
                    None => self.before_preset = None,
                }
            }
            Focus::Horizon => self.horizon = step(self.horizon, HORIZONS.len()),
            Focus::Window { row, col } => {
                self.before_preset = None;
                let Some(r) = self.rows.get_mut(row) else {
                    return;
                };
                match col {
                    Col::Length => {
                        r.seconds = if d > 0 {
                            LENGTHS
                                .iter()
                                .copied()
                                .find(|&s| s > r.seconds)
                                .unwrap_or(r.seconds)
                        } else {
                            LENGTHS
                                .iter()
                                .rev()
                                .copied()
                                .find(|&s| s < r.seconds)
                                .unwrap_or(r.seconds)
                        };
                    }
                    Col::Weighting => {
                        let i = WEIGHTINGS
                            .iter()
                            .position(|w| *w == r.weighting)
                            .unwrap_or(0);
                        r.weighting = WEIGHTINGS[step(i, WEIGHTINGS.len())];
                    }
                    Col::Limit | Col::Margin => return,
                }
            }
        }
        self.error = None;
    }

    /// Sets the preset's limits on the rows, adding the windows it lacks as
    /// [`LeqPreset::apply`] does; refused, rows unchanged, when that passes the most windows.
    fn apply_preset(&mut self, p: LeqPreset) -> Result<(), String> {
        let mut have: Vec<LeqWindow> = self
            .rows
            .iter()
            .map(|r| LeqWindow {
                duration: Seconds(f64::from(r.seconds)),
                weighting: r.weighting,
                limit: None,
                warn_margin: Db(LeqWindow::DEFAULT_WARN_MARGIN_DB),
            })
            .collect();
        // Checked on the windows as they stand: the rows' typed limits play no part.
        p.apply(&mut have)?;
        for w in p.windows() {
            let secs = w.seconds().unwrap_or(60);
            if let Some(r) = self
                .rows
                .iter_mut()
                .find(|r| r.seconds == secs && r.weighting == w.weighting)
            {
                if let Some(l) = w.limit {
                    r.limit = number_text(l.0);
                }
                continue;
            }
            let at = self
                .rows
                .iter()
                .position(|r| r.seconds > secs)
                .unwrap_or(self.rows.len());
            self.rows.insert(at, Row::of(&w));
        }
        Ok(())
    }

    fn text_mut(&mut self) -> Option<&mut String> {
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

    /// Typed text into the focused limit or margin; replaces a selected text.
    pub fn type_text(&mut self, s: &str) {
        let selected = std::mem::take(&mut self.selected);
        if let Some(t) = self.text_mut() {
            if selected {
                t.clear();
            }
            t.push_str(s);
            self.error = None;
        }
    }

    /// Deletes the last character, or the selected text.
    pub fn backspace(&mut self) {
        let selected = std::mem::take(&mut self.selected);
        if let Some(t) = self.text_mut() {
            if selected {
                t.clear();
            } else {
                t.pop();
            }
            self.error = None;
        }
    }

    /// Ctrl+A on a text cell.
    pub fn select_all(&mut self) {
        self.selected = self.col().is_text() && matches!(self.focus, Focus::Window { .. });
    }

    /// Insert: a new window after the focused one (or at the end), one length longer.
    pub fn add_window(&mut self) {
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
        let from = self.rows.get(after).cloned().unwrap_or(Row {
            seconds: 60,
            weighting: Weighting::A,
            limit: String::new(),
            margin: "3".into(),
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

    /// Delete: removes the focused window.
    pub fn remove_window(&mut self) {
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
        let cfg = LeqConfig {
            windows,
            horizon: Seconds(f64::from(self.horizon_s())),
        };
        cfg.check()?;
        Ok(cfg)
    }

    /// The meter's configuration with the new windows.
    pub fn meas_config(&self) -> Result<MeasConfig, String> {
        Ok(MeasConfig {
            name: self.name.clone(),
            kind: MeasKind::Spl {
                config: SplConfig {
                    leq: self.leq_config()?,
                    ..self.base.clone()
                },
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::{PeakWeighting, TimeWeighting};
    use ac2_proto::units::Rev;

    fn meter() -> Measurement {
        Measurement {
            id: MeasId(4),
            config: MeasConfig {
                name: "FOH SPL".into(),
                kind: MeasKind::Spl {
                    config: SplConfig::on_input(1, Weighting::A, TimeWeighting::Fast),
                },
            },
            config_rev: Rev(3),
            running: true,
            frozen: false,
            delay: None,
            grid_id: None,
        }
    }

    #[test]
    fn opens_on_the_meter_windows() {
        let d = LeqDialog::new(&meter(), true).expect("spl");
        let names: Vec<String> = d.rows.iter().map(|r| r.cell(Col::Length)).collect();
        assert_eq!(
            names,
            [
                "LAeq 1 min",
                "LAeq 5 min",
                "LAeq 10 min",
                "LAeq 30 min",
                "LAeq 60 min"
            ]
        );
        assert_eq!(d.horizon_s(), 60);
        assert_eq!(d.preset_text(), "none (keep these limits)");
        assert_eq!(d.leq_config().expect("valid"), LeqConfig::default_windows());
        let mut tf = meter();
        tf.config.kind = MeasKind::Transfer {
            config: ac2_proto::model::TransferConfig::with_inputs(0, 1),
        };
        assert!(LeqDialog::new(&tf, true).is_none());
    }

    /// Keys only: a preset, a typed limit on another window, a new window, a removal.
    #[test]
    fn keyboard_flow() {
        let mut d = LeqDialog::new(&meter(), true).expect("spl");
        // → on the preset row: DIN 15905-5 limits the 30 min window.
        d.cycle(1);
        assert_eq!(d.preset_text(), "DIN 15905-5: LAeq 30 min ≤ 99 dB");
        assert!(
            d.preset_source()
                .expect("source")
                .contains("not legal advice")
        );
        assert_eq!(d.rows[3].limit, "99");
        // → → → → : WHO adds a 15 min window between 10 and 30 min, over the windows as
        // they were before the first preset (DIN's limit goes).
        for _ in 0..4 {
            d.cycle(1);
        }
        assert_eq!(d.preset_text(), "WHO safe listening: LAeq 15 min ≤ 100 dB");
        assert_eq!(d.rows.len(), 6);
        assert_eq!(d.rows[3].cell(Col::Length), "LAeq 15 min");
        assert_eq!(d.rows[3].limit, "100");
        assert_eq!(d.rows[4].limit, "");
        // ↓ ↓ to the first window; Tab Tab to its limit; type 102.
        d.move_row(1);
        d.move_row(1);
        assert_eq!(
            d.focus,
            Focus::Window {
                row: 0,
                col: Col::Length
            }
        );
        d.move_cell(1);
        d.move_cell(1);
        assert_eq!(
            d.focus,
            Focus::Window {
                row: 0,
                col: Col::Limit
            }
        );
        assert!(d.selected, "a text cell arrives selected");
        d.type_text("102 dB");
        // ← on the length: shorter, down to 30 s.
        d.move_cell(-1);
        d.move_cell(-1);
        d.cycle(-1);
        assert_eq!(d.rows[0].cell(Col::Length), "LAeq 30 s");
        // Weighting → C.
        d.move_cell(1);
        d.cycle(1);
        assert_eq!(d.rows[0].cell(Col::Length), "LCeq 30 s");
        // Insert adds a longer window after it; Delete removes the focused one.
        d.add_window();
        assert_eq!(d.rows.len(), 7);
        assert_eq!(d.rows[1].cell(Col::Length), "LCeq 1 min");
        d.remove_window();
        assert_eq!(d.rows.len(), 6);
        let c = d.leq_config().expect("valid");
        assert_eq!(c.windows[0].limit, Some(DbSpl(102.0)));
        assert_eq!(c.windows[0].weighting, Weighting::C);
        assert_eq!(c.windows[0].duration, Seconds(30.0));
        assert_eq!(c.windows[3].limit, Some(DbSpl(100.0)));
        assert_eq!(c.windows[4].limit, None);
        // The horizon row: ← to 30 s.
        d.focus = Focus::Horizon;
        d.cycle(-1);
        assert_eq!(d.horizon_s(), 30);
        let MeasKind::Spl { config } = d.meas_config().expect("valid").kind else {
            panic!()
        };
        assert_eq!(config.input, 1);
        assert_eq!(config.peak_weighting, PeakWeighting::C);
        assert_eq!(config.leq.horizon, Seconds(30.0));
    }

    /// → through every preset: each shows over the windows as they were, two-window rules
    /// set both, and back to "none" restores the meter's windows.
    #[test]
    fn cycles_through_every_preset() {
        let mut d = LeqDialog::new(&meter(), true).expect("spl");
        d.rows[0].limit = "110".into();
        let start = d.rows.clone();
        let names = |d: &LeqDialog| -> Vec<String> {
            d.rows
                .iter()
                .map(|r| format!("{} {}", r.cell(Col::Length), r.limit))
                .collect()
        };
        for (i, p) in LeqPreset::ALL.iter().enumerate() {
            d.cycle(1);
            assert_eq!(d.preset, Some(i));
            assert_eq!(d.error, None, "{p:?}");
            assert_eq!(d.preset_text(), ac2_scene::leq_preset::summary(*p));
            let c = d.leq_config().expect("valid");
            for w in p.windows() {
                let got = c
                    .windows
                    .iter()
                    .find(|x| x.duration == w.duration && x.weighting == w.weighting)
                    .unwrap_or_else(|| panic!("{p:?} {w:?}"));
                assert_eq!(got.limit, w.limit, "{p:?}");
            }
            // Nothing of an earlier preset stays: the windows are the meter's plus this one's.
            assert_eq!(
                c.windows.len(),
                5 + p.missing(&LeqConfig::default_windows().windows)
            );
            assert_eq!(d.rows[0].limit, "110");
            match p {
                LeqPreset::France => assert_eq!(
                    names(&d),
                    [
                        "LAeq 1 min 110",
                        "LAeq 5 min ",
                        "LAeq 10 min ",
                        "LAeq 15 min 102",
                        "LCeq 15 min 118",
                        "LAeq 30 min ",
                        "LAeq 60 min "
                    ]
                ),
                LeqPreset::Flanders100 => assert_eq!(
                    names(&d),
                    [
                        "LAeq 1 min 110",
                        "LAeq 5 min ",
                        "LAeq 10 min ",
                        "LAeq 15 min ",
                        "LAeq 30 min ",
                        "LAeq 60 min 100"
                    ]
                ),
                LeqPreset::Brussels100 => {
                    assert_eq!(names(&d)[4..], ["LAeq 60 min 100", "LCeq 60 min 115"])
                }
                _ => {}
            }
        }
        // → at the end stays; ← all the way back to none restores the windows.
        d.cycle(1);
        assert_eq!(d.preset, Some(LeqPreset::ALL.len() - 1));
        for _ in 0..LeqPreset::ALL.len() {
            d.cycle(-1);
        }
        assert_eq!(d.preset, None);
        assert_eq!(d.rows, start);
    }

    /// A preset that needs windows the dialog has no room for is refused, rows unchanged;
    /// an edit after a preset keeps it.
    #[test]
    fn preset_on_full_windows_is_refused() {
        let mut d = LeqDialog::new(&meter(), true).expect("spl");
        d.focus_cell(4, Col::Length);
        for _ in 0..3 {
            d.add_window();
        }
        assert_eq!(d.rows.len(), LeqConfig::MAX_WINDOWS);
        let full = d.rows.clone();
        d.focus = Focus::Preset;
        let france = LeqPreset::ALL
            .iter()
            .position(|p| *p == LeqPreset::France)
            .expect("listed");
        for _ in 0..=france {
            d.cycle(1);
        }
        assert_eq!(d.preset, Some(france));
        assert!(
            d.error
                .as_deref()
                .is_some_and(|e| e.contains("France R1336-1 needs 2 more windows")),
            "{:?}",
            d.error
        );
        assert_eq!(d.rows, full);
        // DIN 15905-5 has its 30 min window: applies; a typed edit then keeps it when the
        // preset row moves on.
        d.preset = None;
        d.before_preset = None;
        d.cycle(1);
        assert_eq!(d.error, None);
        assert_eq!(d.rows[3].limit, "99");
        d.focus_cell(0, Col::Limit);
        d.type_text("101");
        d.focus = Focus::Preset;
        d.cycle(-1);
        assert_eq!(d.rows[3].limit, "99");
        assert_eq!(d.rows[0].limit, "101");
    }

    #[test]
    fn refusals_name_the_window() {
        let mut d = LeqDialog::new(&meter(), true).expect("spl");
        d.focus_cell(3, Col::Limit);
        d.type_text("loud");
        let e = d.leq_config().expect_err("a word is no limit");
        assert!(e.starts_with("window 4 (LAeq 30 min) limit"), "{e}");
        d.select_all();
        d.type_text("200");
        assert!(
            d.leq_config()
                .expect_err("out of range")
                .contains("30 … 150 dB SPL")
        );
        d.select_all();
        d.backspace();
        assert!(d.leq_config().is_ok(), "empty: no limit");
        for _ in 0..3 {
            d.add_window();
        }
        d.add_window();
        assert!(d.error.as_deref().is_some_and(|e| e.contains("at most 8")));
    }
}
