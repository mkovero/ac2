//! The Leq windows dialog of an SPL meter (`docs/design/leq.md`): one row per window —
//! its length and weighting picked from named choices, its limit and warn margin typed in
//! dB — a preset row that replaces the windows (and peak limits) with a published rule's,
//! the headroom horizon, and under the windows the LCpeak and LAFmax limits and the
//! measuring-position correction, typed; last the band meter ([`bands`]): on with a
//! rule's limits, its window, the §13 corrections, the measured transfer as it stands.
//! Pure data; the reducer routes keys here and the view draws it. Enter sends the meter's
//! configuration with the new windows (`meas.update`, applied in place by the daemon).

use ac2_proto::model::{
    LeqConfig, LeqPreset, LeqWindow, MeasConfig, MeasKind, Measurement, PeakLimit, PeakLimits,
    PeakQuantity, PositionCorrection, SplConfig, Weighting,
};
use ac2_proto::units::{Db, DbSpl, MeasId, Seconds};

mod bands;
pub use bands::{BandField, BandLimits, BandSection};

/// Window lengths offered, shortest first (→ longer). A length outside the list (from the
/// CLI) is kept until changed.
pub const LENGTHS: [u32; 13] = [
    5, 10, 30, 60, 300, 600, 900, 1800, 3600, 7200, 14_400, 28_800, 86_400,
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

/// A typed setting under the windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extra {
    /// The LCpeak limit.
    LcPeak,
    /// The LAFmax limit.
    LafMax,
    /// The measuring-position correction of the energy levels.
    Position,
    /// The measuring-position correction of the peak levels.
    PositionPeak,
}

impl Extra {
    /// Top to bottom.
    pub const ALL: [Extra; 4] = [
        Extra::LcPeak,
        Extra::LafMax,
        Extra::Position,
        Extra::PositionPeak,
    ];

    fn index(self) -> usize {
        match self {
            Extra::LcPeak => 0,
            Extra::LafMax => 1,
            Extra::Position => 2,
            Extra::PositionPeak => 3,
        }
    }

    /// Its label in the dialog.
    pub fn title(self) -> &'static str {
        match self {
            Extra::LcPeak => "LCpeak limit (dB)",
            Extra::LafMax => "LAFmax limit (dB)",
            Extra::Position => "Position correction (dB)",
            Extra::PositionPeak => "… for peaks (dB)",
        }
    }

    /// What an empty field means.
    pub fn empty(self) -> &'static str {
        match self {
            Extra::LcPeak | Extra::LafMax => "no limit",
            Extra::Position => "none",
            Extra::PositionPeak => "as above",
        }
    }

    /// What the field does, beside it.
    pub fn note(self) -> &'static str {
        match self {
            Extra::LcPeak => "over when a second's C-weighted peak exceeds it (held 10 s)",
            Extra::LafMax => "over when a second's A-weighted Fast level exceeds it (held 10 s)",
            Extra::Position => {
                "added to every level: from the mic to where the limit applies (FOH → \
                 loudest audience spot); the log keeps what was measured"
            }
            Extra::PositionPeak => "a peak may differ (DIN 15905-5: K2 beside K1)",
        }
    }

    fn quantity(self) -> Option<PeakQuantity> {
        match self {
            Extra::LcPeak => Some(PeakQuantity::LcPeak),
            Extra::LafMax => Some(PeakQuantity::LafMax),
            Extra::Position | Extra::PositionPeak => None,
        }
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
    /// A setting under the windows.
    Extra(Extra),
    /// A setting of the band meter, last.
    Band(BandField),
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
    /// Index into [`LeqPreset::ALL`], or `None`: "keep these windows".
    pub preset: Option<usize>,
    /// Typed settings under the windows, in [`Extra::ALL`] order.
    pub extra: [String; 4],
    /// The band meter.
    pub bands: BandSection,
    /// The windows and peak limits before the preset row was first changed: back at "none"
    /// they return. Dropped on any edit of a window, which keeps what the preset set.
    before_preset: Option<(Vec<Row>, [String; 2])>,
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
        let peak = |q: PeakQuantity| {
            config
                .leq
                .peaks
                .get(q)
                .map_or_else(String::new, |l| number_text(l.limit.0))
        };
        let (position, position_peak) = match config.position {
            Some(p) if p.peak == p.level => (number_text(p.level.0), String::new()),
            Some(p) => (number_text(p.level.0), number_text(p.peak.0)),
            None => (String::new(), String::new()),
        };
        Some(Self {
            meas: m.id,
            name: m.config.name.clone(),
            calibrated,
            rows: config.leq.windows.iter().map(Row::of).collect(),
            horizon,
            preset: None,
            extra: [
                peak(PeakQuantity::LcPeak),
                peak(PeakQuantity::LafMax),
                position,
                position_peak,
            ],
            bands: BandSection::new(config.bands.as_deref()),
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
            None => "none (keep these windows)".into(),
            Some(p) => ac2_scene::leq_preset::summary(*p),
        }
    }

    /// What choosing a preset does, shown under the preset row.
    pub fn preset_note(&self) -> &'static str {
        "A preset replaces the windows with the rule's own (its limits only); Insert adds \
         more after."
    }

    /// Where the chosen preset's figures come from.
    pub fn preset_source(&self) -> Option<String> {
        self.preset
            .and_then(|i| LeqPreset::ALL.get(i))
            .map(|p| ac2_scene::leq_preset::source(*p))
    }

    fn rows_count(&self) -> usize {
        // Preset, horizon, the windows, the settings under them, the band meter.
        2 + self.rows.len() + Extra::ALL.len() + BandField::ALL.len()
    }

    fn row_index(&self) -> usize {
        match self.focus {
            Focus::Preset => 0,
            Focus::Horizon => 1,
            Focus::Window { row, .. } => 2 + row,
            Focus::Extra(e) => 2 + self.rows.len() + e.index(),
            Focus::Band(b) => 2 + self.rows.len() + Extra::ALL.len() + b.index(),
        }
    }

    /// The text typed in setting `e`.
    pub fn extra_text(&self, e: Extra) -> String {
        self.extra[e.index()].clone()
    }

    /// The focus is on typed text.
    fn on_text(&self) -> bool {
        match self.focus {
            Focus::Window { col, .. } => col.is_text(),
            Focus::Extra(_) => true,
            Focus::Preset | Focus::Horizon | Focus::Band(_) => false,
        }
    }

    fn col(&self) -> Col {
        match self.focus {
            Focus::Window { col, .. } => col,
            _ => Col::Length,
        }
    }

    fn set_row(&mut self, i: usize, col: Col) {
        let n = self.rows.len();
        self.focus = match i {
            0 => Focus::Preset,
            1 => Focus::Horizon,
            r if r < 2 + n => Focus::Window { row: r - 2, col },
            r if r < 2 + n + Extra::ALL.len() => Focus::Extra(Extra::ALL[r - 2 - n]),
            r => Focus::Band(
                BandField::ALL[(r - 2 - n - Extra::ALL.len()).min(BandField::ALL.len() - 1)],
            ),
        };
        self.selected = self.on_text();
    }

    /// ↑ / ↓: the row above or below, keeping the column; wraps.
    pub fn move_row(&mut self, d: i32) {
        let n = self.rows_count() as i32;
        let i = (self.row_index() as i32 + d).rem_euclid(n) as usize;
        self.set_row(i, self.col());
    }

    /// Tab / Shift+Tab: the next or previous cell, row by row; wraps.
    pub fn move_cell(&mut self, d: i32) {
        // Cells in order: preset, horizon, four per window, then the settings under them.
        let w = 4 * self.rows.len() as i32;
        let x = Extra::ALL.len() as i32;
        let cells = 2 + w + x + BandField::ALL.len() as i32;
        let at = match self.focus {
            Focus::Preset => 0,
            Focus::Horizon => 1,
            Focus::Window { row, col } => {
                2 + 4 * row as i32 + Col::ALL.iter().position(|c| *c == col).unwrap_or(0) as i32
            }
            Focus::Extra(e) => 2 + w + e.index() as i32,
            Focus::Band(b) => 2 + w + x + b.index() as i32,
        };
        let next = (at + d).rem_euclid(cells);
        self.focus = match next {
            0 => Focus::Preset,
            1 => Focus::Horizon,
            k if k < 2 + w => Focus::Window {
                row: ((k - 2) / 4) as usize,
                col: Col::ALL[((k - 2) % 4) as usize],
            },
            k if k < 2 + w + x => Focus::Extra(Extra::ALL[(k - 2 - w) as usize]),
            k => Focus::Band(BandField::ALL[(k - 2 - w - x) as usize]),
        };
        self.selected = self.on_text();
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
                let peaks_now = [self.extra[0].clone(), self.extra[1].clone()];
                let before = self
                    .before_preset
                    .get_or_insert_with(|| (self.rows.clone(), peaks_now))
                    .clone();
                match self.preset.and_then(|i| LeqPreset::ALL.get(i)) {
                    Some(p) => {
                        self.rows = LeqPreset::windows_of(&[*p]).iter().map(Row::of).collect();
                        let peaks = p.peaks();
                        for (k, q) in PeakQuantity::ALL.into_iter().enumerate() {
                            self.extra[k] = peaks
                                .get(q)
                                .map_or_else(String::new, |l| number_text(l.limit.0));
                        }
                    }
                    None => {
                        let (rows, [lc, laf]) = before;
                        self.rows = rows;
                        self.extra[0] = lc;
                        self.extra[1] = laf;
                        self.before_preset = None;
                    }
                }
            }
            Focus::Horizon => self.horizon = step(self.horizon, HORIZONS.len()),
            Focus::Band(b) => self.bands.cycle(b, d),
            Focus::Extra(_) => return,
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

    fn text_mut(&mut self) -> Option<&mut String> {
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
        self.selected = self.on_text();
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
        let after = after.min(self.rows.len().saturating_sub(1));
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
        let mut peaks = PeakLimits::default();
        for e in [Extra::LcPeak, Extra::LafMax] {
            let Some(q) = e.quantity() else { continue };
            let t = self.extra[e.index()].trim();
            if t.is_empty() {
                continue;
            }
            let name = ac2_scene::leq::peak_name(q);
            let v = crate::state::parse_number(t, &["db spl", "dbspl", "db"])
                .map_err(|err| format!("{name} limit: {err}"))?;
            if !(60.0..=170.0).contains(&v) {
                return Err(format!(
                    "{name}: a limit is 60 … 170 dB SPL (empty for none)"
                ));
            }
            // A margin set before (from the CLI) stays; a new limit warns 3 dB under.
            let warn_margin = self
                .base
                .leq
                .peaks
                .get(q)
                .map_or(Db(LeqWindow::DEFAULT_WARN_MARGIN_DB), |l| l.warn_margin);
            *peaks.get_mut(q) = Some(PeakLimit {
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
            crate::state::parse_number(t, &["db"]).map_err(|e| format!("{what}: {e}"))
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

    /// The meter's configuration with the new windows, peak limits and correction.
    pub fn meas_config(&self) -> Result<MeasConfig, String> {
        Ok(MeasConfig {
            name: self.name.clone(),
            kind: MeasKind::Spl {
                config: SplConfig {
                    leq: self.leq_config()?,
                    position: self.position()?,
                    bands: self.bands.config()?.map(Box::new),
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
        assert_eq!(d.preset_text(), "none (keep these windows)");
        assert_eq!(d.leq_config().expect("valid"), LeqConfig::default_windows());
        let mut tf = meter();
        tf.config.kind = MeasKind::Transfer {
            config: ac2_proto::model::TransferConfig::with_inputs(0, 1),
        };
        assert!(LeqDialog::new(&tf, true).is_none());
    }

    /// Keys only: a preset, a typed limit, a new window, a removal.
    #[test]
    fn keyboard_flow() {
        let mut d = LeqDialog::new(&meter(), true).expect("spl");
        assert!(d.preset_note().contains("replaces the windows"));
        // → on the preset row: DIN 15905-5 is its 30 min window, nothing else.
        d.cycle(1);
        assert_eq!(
            d.preset_text(),
            "DIN 15905-5: LAeq 30 min ≤ 99 dB, LCpeak ≤ 135 dB"
        );
        assert!(
            d.preset_source()
                .expect("source")
                .contains("not legal advice")
        );
        assert_eq!(names(&d), ["LAeq 30 min 99"]);
        // → → → → : WHO, its 15 min window only.
        for _ in 0..4 {
            d.cycle(1);
        }
        assert_eq!(d.preset_text(), "WHO safe listening: LAeq 15 min ≤ 100 dB");
        assert_eq!(names(&d), ["LAeq 15 min 100"]);
        // ↓ ↓ to the window; Insert adds a longer one after it, without a limit.
        d.move_row(1);
        d.move_row(1);
        assert_eq!(
            d.focus,
            Focus::Window {
                row: 0,
                col: Col::Length
            }
        );
        d.add_window();
        assert_eq!(names(&d), ["LAeq 15 min 100", "LAeq 30 min "]);
        // Tab Tab to its limit; type 102.
        d.move_cell(1);
        d.move_cell(1);
        assert_eq!(
            d.focus,
            Focus::Window {
                row: 1,
                col: Col::Limit
            }
        );
        assert!(d.selected, "a text cell arrives selected");
        d.type_text("102 dB");
        // → on the length: longer, 60 min.
        d.move_cell(-1);
        d.move_cell(-1);
        d.cycle(1);
        assert_eq!(d.rows[1].cell(Col::Length), "LAeq 60 min");
        // Weighting → C.
        d.move_cell(1);
        d.cycle(1);
        assert_eq!(d.rows[1].cell(Col::Length), "LCeq 60 min");
        // Another, and Delete removes it again.
        d.add_window();
        assert_eq!(d.rows.len(), 3);
        d.remove_window();
        assert_eq!(d.rows.len(), 2);
        let c = d.leq_config().expect("valid");
        assert_eq!(c.windows[0].limit, Some(DbSpl(100.0)));
        assert_eq!(c.windows[1].limit, Some(DbSpl(102.0)));
        assert_eq!(c.windows[1].weighting, Weighting::C);
        assert_eq!(c.windows[1].duration, Seconds(3600.0));
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

    fn names(d: &LeqDialog) -> Vec<String> {
        d.rows
            .iter()
            .map(|r| format!("{} {}", r.cell(Col::Length), r.limit))
            .collect()
    }

    /// → through every preset: each shows exactly its own windows and limits, shortest
    /// first; back to "none" the meter's windows return as they were.
    #[test]
    fn cycles_through_every_preset() {
        let mut d = LeqDialog::new(&meter(), true).expect("spl");
        d.rows[0].limit = "110".into();
        let start = d.rows.clone();
        for (i, p) in LeqPreset::ALL.iter().enumerate() {
            d.cycle(1);
            assert_eq!(d.preset, Some(i));
            assert_eq!(d.error, None, "{p:?}");
            assert_eq!(d.preset_text(), ac2_scene::leq_preset::summary(*p));
            let c = d.leq_config().expect("valid");
            assert_eq!(c.windows, LeqPreset::windows_of(&[*p]), "{p:?}");
            match p {
                LeqPreset::France => {
                    assert_eq!(names(&d), ["LAeq 15 min 102", "LCeq 15 min 118"]);
                }
                LeqPreset::Flanders100 => {
                    assert_eq!(names(&d), ["LAeq 15 min ", "LAeq 60 min 100"]);
                }
                LeqPreset::Brussels100 => {
                    assert_eq!(names(&d), ["LAeq 60 min 100", "LCeq 60 min 115"]);
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
        assert_eq!(d.preset_text(), "none (keep these windows)");
        assert_eq!(d.rows, start);
    }

    /// An edit after a preset keeps it: the windows it set stay when the preset row moves
    /// back to "none", and the next preset replaces them in turn.
    #[test]
    fn an_edit_keeps_the_preset() {
        let mut d = LeqDialog::new(&meter(), true).expect("spl");
        d.cycle(1);
        assert_eq!(names(&d), ["LAeq 30 min 99"]);
        d.focus_cell(0, Col::Limit);
        d.type_text("101");
        d.focus = Focus::Preset;
        d.cycle(-1);
        assert_eq!(d.preset, None);
        assert_eq!(names(&d), ["LAeq 30 min 101"]);
        d.cycle(1);
        d.cycle(1);
        assert_eq!(names(&d), ["LAeq 60 min 93"]);
        d.cycle(-1);
        d.cycle(-1);
        assert_eq!(names(&d), ["LAeq 30 min 101"], "the edited windows return");
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

    /// The band meter's rows come last: Tab reaches them, ←/→ turn it on with a preset and
    /// set the corrections, Enter sends them with the windows.
    #[test]
    fn the_band_meter_rows() {
        let mut d = LeqDialog::new(&meter(), true).expect("spl");
        d.move_cell(-4);
        assert_eq!(d.focus, Focus::Band(BandField::Limits));
        d.cycle(1);
        d.move_cell(1);
        d.move_cell(1);
        assert_eq!(d.focus, Focus::Band(BandField::Impulse));
        d.cycle(1);
        let MeasKind::Spl { config } = d.meas_config().expect("valid").kind else {
            panic!()
        };
        let b = config.bands.expect("on");
        assert_eq!(b.night[5], Some(DbSpl(42.0)));
        assert_eq!(b.correction.db(), 5.0);
        assert_eq!(config.leq, d.leq_config().expect("windows"));
    }

    /// Under the windows: the peak limits and the position correction, reached with ↓ and
    /// Tab, typed; a preset sets its peak limits and back at "none" they return; the
    /// correction is the operator's and no preset touches it.
    #[test]
    fn peak_limits_and_the_position_correction() {
        let mut d = LeqDialog::new(&meter(), true).expect("spl");
        // ↑ from the preset row wraps to the band meter's settings, last; above them the
        // peaks' correction.
        d.move_row(-1);
        assert_eq!(d.focus, Focus::Band(BandField::Tonal));
        for _ in 0..BandField::ALL.len() {
            d.move_row(-1);
        }
        assert_eq!(d.focus, Focus::Extra(Extra::PositionPeak));
        d.move_row(-1);
        assert_eq!(d.focus, Focus::Extra(Extra::Position));
        assert!(d.selected);
        d.type_text("4 dB");
        // Shift+Tab twice: LCpeak's limit.
        d.move_cell(-1);
        d.move_cell(-1);
        assert_eq!(d.focus, Focus::Extra(Extra::LcPeak));
        d.type_text("130");
        let MeasKind::Spl { config } = d.meas_config().expect("valid").kind else {
            panic!()
        };
        assert_eq!(config.leq.peaks.lcpeak.map(|l| l.limit), Some(DbSpl(130.0)));
        assert_eq!(
            config.leq.peaks.lcpeak.map(|l| l.warn_margin),
            Some(Db(3.0))
        );
        assert_eq!(config.leq.peaks.lafmax, None);
        assert_eq!(config.position, Some(PositionCorrection::both(4.0)));
        // A different peak correction.
        d.focus = Focus::Extra(Extra::PositionPeak);
        d.type_text("2");
        assert_eq!(
            d.position().expect("valid").map(|p| (p.level, p.peak)),
            Some((Db(4.0), Db(2.0)))
        );
        // Swiss 100: its LAFmax 125 replaces the peak limits; the correction stays.
        d.focus = Focus::Preset;
        for _ in 0..4 {
            d.cycle(1);
        }
        assert_eq!(
            d.preset_text(),
            "Swiss V-NISSG 100 dB: LAeq 60 min ≤ 100 dB, LAFmax ≤ 125 dB"
        );
        assert_eq!(d.extra_text(Extra::LcPeak), "");
        assert_eq!(d.extra_text(Extra::LafMax), "125");
        assert_eq!(d.extra_text(Extra::Position), "4 dB");
        for _ in 0..4 {
            d.cycle(-1);
        }
        assert_eq!(d.extra_text(Extra::LcPeak), "130", "back at none");
        // Refusals name the field.
        d.focus = Focus::Extra(Extra::LafMax);
        d.type_text("loud");
        assert!(
            d.leq_config()
                .expect_err("a word")
                .starts_with("LAFmax limit")
        );
        d.select_all();
        d.type_text("200");
        assert!(d.leq_config().expect_err("range").contains("60 … 170"));
        d.select_all();
        d.backspace();
        d.focus = Focus::Extra(Extra::Position);
        d.select_all();
        d.type_text("40");
        assert!(d.meas_config().expect_err("range").contains("±30"));
        d.select_all();
        d.backspace();
        assert!(
            d.meas_config()
                .expect_err("a peak correction alone")
                .contains("needs one for the levels")
        );
        // A meter with a correction opens with it.
        let mut m = meter();
        if let MeasKind::Spl { config } = &mut m.config.kind {
            config.position = Some(PositionCorrection::both(-2.5));
        }
        let d = LeqDialog::new(&m, true).expect("spl");
        assert_eq!(
            (
                d.extra_text(Extra::Position),
                d.extra_text(Extra::PositionPeak)
            ),
            ("-2.5".to_owned(), String::new())
        );
    }
}
