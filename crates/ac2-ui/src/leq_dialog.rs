//! The Leq windows dialog of an SPL meter (`docs/design/leq.md`): one row per window —
//! its length and weighting picked from named choices, its limit and warn margin typed in
//! dB — a preset row that replaces the windows (and peak limits) with a published rule's,
//! the headroom horizon, and under the windows the LCpeak and LAFmax limits and the
//! measuring-position correction, typed; last the band meter ([`bands`]): off, on or a
//! rule's preset, its windows (one band each, a row like a Leq window's; a range row adds
//! one per band), the §13 corrections, the transfer as it stands; then the rig: whether
//! the rig keeps the meter across session loads, and the cues ([`cues`]), folded until
//! shown.
//! Pure data; the reducer routes keys here and the view draws it. Enter sends the meter's
//! configuration with the new windows (`meas.update`, applied in place by the daemon).

use ac2_proto::model::{
    LeqConfig, LeqPreset, LeqWindow, MeasConfig, MeasKind, Measurement, PeakLimit, PeakLimits,
    PeakQuantity, PositionCorrection, SplConfig, Weighting,
};
use ac2_proto::units::{Db, DbSpl, MeasId, Rev, Seconds};

use crate::caret::Caret;

mod bands;
mod cues;
mod edit;
mod transfer;
pub use bands::{
    BandCol, BandField, BandFocus, BandMeter, BandRow, BandSection, RangeCol, RangeRow,
};
pub use cues::{CueCol, CueEdit, CueRow};
pub use transfer::{Ask, Sources, Span, TransferStep};

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
    /// A cell of the band meter's section.
    Band(BandFocus),
    /// Whether the rig keeps the meter ([`ac2_scene::osc::KEEP_ON_RIG`]).
    Scope,
    /// The cues' fold: shown or hidden.
    CueFold,
    /// A cell of a cue row, last.
    Cue { row: CueRow, col: CueCol },
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
    /// Its cue.
    pub cue: CueEdit,
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
            cue: CueEdit::of(w.cue.as_deref()),
        }
    }

    /// The row as the dialog shows it, column by column.
    pub fn cell(&self, c: Col) -> String {
        match c {
            Col::Length => format!(
                "L{}eq {}",
                self.weighting.letter(),
                ac2_scene::leq::length(f64::from(self.seconds))
            ),
            Col::Weighting => self.weighting.letter().to_owned(),
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
            cue: self.cue.checked(&name)?,
            duration: Seconds(f64::from(self.seconds)),
            weighting: self.weighting,
            limit,
            warn_margin: Db(margin),
        })
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
    /// Where typing goes in a focused cue line (a message or DMX levels): those lines run
    /// long, so they are edited in place rather than replaced.
    pub caret: Caret,
    /// Why the last Enter was refused.
    pub error: Option<String>,
    /// The last Apply was refused as made on a configuration changed since: the edits stay.
    pub notice: Option<String>,
    /// The band transfer step, open over the dialog.
    pub transfer: Option<TransferStep>,
    /// The rig keeps the meter: it survives session loads.
    pub on_rig: bool,
    /// The OSC cue rows are shown.
    pub cues_shown: bool,
    /// The fold as the operator last set it on this page (`None`: untouched): the app
    /// keeps it for the next time the page opens.
    pub cues_chosen: Option<bool>,
    /// The meter-wide cue: the worst state of all its limits.
    pub meter_cue: CueEdit,
    /// The peak limits' cues, in [`PeakQuantity::ALL`] order.
    pub peak_cues: [CueEdit; 2],
    /// The predicted window's cue.
    pub predicted_cue: CueEdit,
    /// The meter's configuration rev the dialog was opened on: Enter names it, so an edit
    /// made meanwhile by another client is refused rather than overwritten.
    pub base_rev: Rev,
    base: SplConfig,
}

impl LeqDialog {
    /// The dialog over SPL meter `m`; `None` for another kind of measurement.
    /// The cues open shown when any is set, else as `shown` (the fold last chosen).
    pub fn new(m: &Measurement, calibrated: bool, shown: bool) -> Option<Self> {
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
        let mut d = Self {
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
            caret: Caret::default(),
            error: None,
            notice: None,
            transfer: None,
            on_rig: m.config.scope == ac2_proto::model::MeasScope::Rig,
            cues_shown: shown,
            cues_chosen: None,
            meter_cue: CueEdit::of(config.cue.as_deref()),
            peak_cues: PeakQuantity::ALL
                .map(|q| CueEdit::of(config.leq.peaks.get(q).and_then(|l| l.cue.as_deref()))),
            predicted_cue: CueEdit::of(
                config
                    .bands
                    .as_deref()
                    .and_then(|b| b.predicted.as_ref())
                    .and_then(|p| p.cue.as_deref()),
            ),
            base_rev: m.config_rev,
            base: config.clone(),
        };
        d.cues_shown |= d.any_cue_set();
        Some(d)
    }

    /// Any cue row holds a message or DMX levels.
    pub fn any_cue_set(&self) -> bool {
        self.cue_rows()
            .into_iter()
            .filter_map(|r| self.cue_edit(r))
            .any(|c| c.set() + c.dmx_set() > 0)
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
        "A preset replaces the windows with the rule's own (its limits only); + adds more."
    }

    /// Where the chosen preset's figures come from.
    pub fn preset_source(&self) -> Option<String> {
        self.preset
            .and_then(|i| LeqPreset::ALL.get(i))
            .map(|p| ac2_scene::leq_preset::source(*p))
    }

    fn rows_count(&self) -> usize {
        // Preset, horizon, the windows, the settings under them, the band meter, the rig.
        self.head_rows() + self.tail().len()
    }

    /// The rows above the rig's.
    fn head_rows(&self) -> usize {
        2 + self.rows.len() + Extra::ALL.len() + self.bands.grid().len()
    }

    /// The rig's rows, each its cells left to right: the scope, the cues' fold, the cue
    /// rows while shown.
    fn tail(&self) -> Vec<Vec<Focus>> {
        let mut t = vec![vec![Focus::Scope], vec![Focus::CueFold]];
        if self.cues_shown {
            for row in self.cue_rows() {
                t.push(CueCol::ALL.map(|col| Focus::Cue { row, col }).to_vec());
                t.push(CueCol::DMX.map(|col| Focus::Cue { row, col }).to_vec());
            }
        }
        t
    }

    /// The cued subjects, top to bottom: the meter, the Leq windows, the peak limits that
    /// have a limit, the band windows, the predicted window.
    pub fn cue_rows(&self) -> Vec<CueRow> {
        let mut v = vec![CueRow::Meter];
        v.extend((0..self.rows.len()).map(CueRow::Window));
        for (k, q) in PeakQuantity::ALL.into_iter().enumerate() {
            if !self.extra[k].trim().is_empty() {
                v.push(CueRow::Peak(q));
            }
        }
        if self.bands.is_on() {
            v.extend((0..self.bands.rows.len()).map(CueRow::Band));
        }
        if self.bands.has_predicted() {
            v.push(CueRow::Predicted);
        }
        v
    }

    /// A cue row's subject as named everywhere.
    pub fn cue_name(&self, r: CueRow) -> String {
        match r {
            CueRow::Meter => "Meter (worst of all)".into(),
            CueRow::Window(i) => self
                .rows
                .get(i)
                .map_or_else(String::new, |w| w.cell(Col::Length)),
            CueRow::Peak(q) => q.name().to_owned(),
            CueRow::Band(i) => self
                .bands
                .rows
                .get(i)
                .map_or_else(String::new, |b| b.name()),
            CueRow::Predicted => "Predicted LAeq".into(),
        }
    }

    /// A cue row's cue.
    pub fn cue_edit(&self, r: CueRow) -> Option<&CueEdit> {
        match r {
            CueRow::Meter => Some(&self.meter_cue),
            CueRow::Window(i) => self.rows.get(i).map(|w| &w.cue),
            CueRow::Peak(q) => Some(&self.peak_cues[peak_index(q)]),
            CueRow::Band(i) => self.bands.rows.get(i).map(|b| &b.cue),
            CueRow::Predicted => Some(&self.predicted_cue),
        }
    }

    fn cue_edit_mut(&mut self, r: CueRow) -> Option<&mut CueEdit> {
        match r {
            CueRow::Meter => Some(&mut self.meter_cue),
            CueRow::Window(i) => self.rows.get_mut(i).map(|w| &mut w.cue),
            CueRow::Peak(q) => Some(&mut self.peak_cues[peak_index(q)]),
            CueRow::Band(i) => self.bands.rows.get_mut(i).map(|b| &mut b.cue),
            CueRow::Predicted => Some(&mut self.predicted_cue),
        }
    }

    /// The cues' fold row as shown: `hidden · 3 cues set`.
    pub fn cue_fold_text(&self) -> String {
        let n: usize = self
            .cue_rows()
            .into_iter()
            .filter_map(|r| self.cue_edit(r))
            .map(CueEdit::set)
            .sum();
        let dmx: usize = self
            .cue_rows()
            .into_iter()
            .filter_map(|r| self.cue_edit(r))
            .map(CueEdit::dmx_set)
            .sum();
        let shown = if self.cues_shown { "shown" } else { "hidden" };
        let mut t = match (n, dmx) {
            (0, 0) => return format!("{shown} · none set"),
            (0, _) => format!("{shown} · no message set"),
            (1, _) => format!("{shown} · 1 message set"),
            (n, _) => format!("{shown} · {n} messages set"),
        };
        match dmx {
            0 => {}
            1 => t.push_str(" · DMX in 1 state"),
            k => t.push_str(&format!(" · DMX in {k} states")),
        }
        t
    }

    /// The band section's row and cell of `b`.
    fn band_at(&self, b: BandFocus) -> (usize, usize) {
        self.bands
            .grid()
            .iter()
            .enumerate()
            .find_map(|(r, cells)| cells.iter().position(|c| *c == b).map(|i| (r, i)))
            .unwrap_or((0, 0))
    }

    fn row_index(&self) -> usize {
        match self.focus {
            Focus::Preset => 0,
            Focus::Horizon => 1,
            Focus::Window { row, .. } => 2 + row,
            Focus::Extra(e) => 2 + self.rows.len() + e.index(),
            Focus::Band(b) => 2 + self.rows.len() + Extra::ALL.len() + self.band_at(b).0,
            Focus::Scope | Focus::CueFold | Focus::Cue { .. } => {
                self.head_rows()
                    + self
                        .tail()
                        .iter()
                        .position(|r| r.contains(&self.focus))
                        .unwrap_or(0)
            }
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
            Focus::Band(b) => b.is_text(),
            Focus::Cue { col, .. } => col.is_text(),
            Focus::Preset | Focus::Horizon | Focus::Scope | Focus::CueFold => false,
        }
    }

    fn col(&self) -> Col {
        match self.focus {
            Focus::Window { col, .. } => col,
            _ => Col::Length,
        }
    }

    /// Row `i`: a window row in column `col`, a band row in its cell `cell` (or its last).
    fn set_row(&mut self, i: usize, col: Col, cell: usize) {
        let n = self.rows.len();
        let head = self.head_rows();
        self.focus = match i {
            0 => Focus::Preset,
            1 => Focus::Horizon,
            r if r < 2 + n => Focus::Window { row: r - 2, col },
            r if r < 2 + n + Extra::ALL.len() => Focus::Extra(Extra::ALL[r - 2 - n]),
            r if r >= head => {
                let t = self.tail();
                let cells = &t[(r - head).min(t.len() - 1)];
                cells[cell.min(cells.len() - 1)]
            }
            r => {
                let g = self.bands.grid();
                let cells = &g[(r - 2 - n - Extra::ALL.len()).min(g.len() - 1)];
                Focus::Band(cells[cell.min(cells.len() - 1)])
            }
        };
        self.entered();
    }

    /// ↑ / ↓: the row above or below, keeping the column; wraps.
    pub fn move_row(&mut self, d: i32) {
        let n = self.rows_count() as i32;
        let i = (self.row_index() as i32 + d).rem_euclid(n) as usize;
        let cell = match self.focus {
            Focus::Band(b) => self.band_at(b).1,
            Focus::Window { col, .. } => Col::ALL.iter().position(|c| *c == col).unwrap_or(0),
            Focus::Cue { col, .. } => col.cell(),
            _ => 0,
        };
        self.set_row(i, self.col(), cell);
    }

    /// Tab / Shift+Tab: the next or previous cell, row by row; wraps.
    pub fn move_cell(&mut self, d: i32) {
        // Cells in order: preset, horizon, four per window, then the settings under them.
        let w = 4 * self.rows.len() as i32;
        let x = Extra::ALL.len() as i32;
        let band: Vec<BandFocus> = self.bands.grid().into_iter().flatten().collect();
        let tail: Vec<Focus> = self.tail().into_iter().flatten().collect();
        let b = band.len() as i32;
        let cells = 2 + w + x + b + tail.len() as i32;
        let at = match self.focus {
            Focus::Preset => 0,
            Focus::Horizon => 1,
            Focus::Window { row, col } => {
                2 + 4 * row as i32 + Col::ALL.iter().position(|c| *c == col).unwrap_or(0) as i32
            }
            Focus::Extra(e) => 2 + w + e.index() as i32,
            Focus::Band(b) => 2 + w + x + band.iter().position(|c| *c == b).unwrap_or(0) as i32,
            f @ (Focus::Scope | Focus::CueFold | Focus::Cue { .. }) => {
                2 + w + x + b + tail.iter().position(|c| *c == f).unwrap_or(0) as i32
            }
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
            k if k < 2 + w + x + b => Focus::Band(band[(k - 2 - w - x) as usize]),
            k => tail[(k - 2 - w - x - b) as usize],
        };
        self.entered();
    }

    /// Focuses window `row`, column `col` (the mouse).
    pub fn focus_cell(&mut self, row: usize, col: Col) {
        if row < self.rows.len() {
            self.set_row(row + 2, col, 0);
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
            // Two-way toggles: → on, ← off.
            Focus::Scope => self.on_rig = d > 0,
            Focus::CueFold => {
                self.cues_shown = d > 0;
                self.cues_chosen = Some(self.cues_shown);
            }
            Focus::Cue { row, col } => {
                let Some(c) = self.cue_edit_mut(row) else {
                    return;
                };
                match col {
                    CueCol::Append => c.append = d > 0,
                    CueCol::State(_) | CueCol::Dmx(_) => return,
                }
            }
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
}

fn peak_index(q: PeakQuantity) -> usize {
    PeakQuantity::ALL.iter().position(|x| *x == q).unwrap_or(0)
}

impl LeqDialog {
    /// The meter changed elsewhere while these edits were made: they stay, now on its
    /// current configuration (what the dialog does not edit comes from there), and a line
    /// says Apply again overwrites.
    pub fn rebase(&mut self, m: &Measurement) {
        if let MeasKind::Spl { config } = &m.config.kind {
            self.base = config.clone();
        }
        self.base_rev = m.config_rev;
        self.name = m.config.name.clone();
        self.notice = Some(ac2_scene::osc::CHANGED_ELSEWHERE.to_owned());
    }

    /// The `meas.update` Enter sends: on the rev the dialog was opened on, with the scope
    /// the dialog chose.
    pub fn command(&self) -> Result<ac2_proto::Command, String> {
        Ok(ac2_proto::Command::MeasUpdate {
            meas: self.meas,
            base: self.base_rev,
            config: self.meas_config()?,
        })
    }
}

#[cfg(test)]
mod tests;
