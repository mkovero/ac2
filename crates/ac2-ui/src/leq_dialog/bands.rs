//! The band meter's section of the Leq dialog (`docs/design/band-leq.md`): off, on, or a
//! rule's preset; a table of band windows like the Leq windows — each one band, its
//! length, weighting, limit, day offset and warn margin on a row, + / Insert and − / Delete
//! adding and removing one, "+ range…" / Shift+Insert a row of bands from … to that adds
//! one window per band; the §13 corrections in force; and the transfer as it stands (measured in its
//! band transfer step or with `ac2 spl bands transfer`). A preset replaces the windows and
//! the predicted window only; everything stays editable after.

use ac2_proto::model::{
    BAND_COUNT, BAND_NOMINAL_HZ, BandCorrection, BandLeqConfig, BandLeqPreset, BandTransferSet,
    BandWindow, ImpulseCorrection, LF_BAND_COUNT, PredictedWindow, TonalCorrection, Weighting,
};
use ac2_proto::units::{Db, DbSpl, Hz, Seconds};

use super::{LENGTHS, WEIGHTINGS, number_text, weighting_letter};

/// A setting of the band section outside its windows, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandField {
    /// Off, on, or a rule's preset.
    Meter,
    /// §13 impulse correction.
    Impulse,
    /// §13 narrowband correction.
    Tonal,
}

impl BandField {
    /// Its label in the dialog.
    pub fn title(self) -> &'static str {
        match self {
            BandField::Meter => "Band meter",
            BandField::Impulse => "§13 impulse",
            BandField::Tonal => "§13 narrowband",
        }
    }
}

/// A column of a band window row, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandCol {
    /// The window's band.
    Band,
    Length,
    Weighting,
    /// The band's limit (the night's when a day offset is typed).
    Limit,
    /// How much higher the day limit is; empty: the same day and night.
    DayOffset,
    Margin,
}

impl BandCol {
    /// Left to right.
    pub const ALL: [BandCol; 6] = [
        BandCol::Band,
        BandCol::Length,
        BandCol::Weighting,
        BandCol::Limit,
        BandCol::DayOffset,
        BandCol::Margin,
    ];

    /// Column heading.
    pub fn title(self) -> &'static str {
        match self {
            BandCol::Band => "Band",
            BandCol::Length => "Band window",
            BandCol::Weighting => "Weighting",
            BandCol::Limit => "Limit (dB)",
            BandCol::DayOffset => "Day +dB",
            BandCol::Margin => "Warn within (dB)",
        }
    }

    fn is_text(self) -> bool {
        matches!(self, BandCol::Limit | BandCol::DayOffset | BandCol::Margin)
    }
}

/// A cell of the range row: bands `from` … `to` added as one window each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeCol {
    From,
    To,
    Length,
    Weighting,
}

impl RangeCol {
    /// Left to right.
    pub const ALL: [RangeCol; 4] = [
        RangeCol::From,
        RangeCol::To,
        RangeCol::Length,
        RangeCol::Weighting,
    ];
}

/// Where the focus is in the band section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandFocus {
    Field(BandField),
    /// The range row, while it is open.
    Range(RangeCol),
    /// Band window `row`, column `col`.
    Window {
        row: usize,
        col: BandCol,
    },
}

impl BandFocus {
    /// The focus is on typed text.
    pub fn is_text(self) -> bool {
        match self {
            BandFocus::Field(_) | BandFocus::Range(_) => false,
            BandFocus::Window { col, .. } => col.is_text(),
        }
    }
}

/// The band meter's state as chosen on its first row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandMeter {
    Off,
    /// On, as set below (a preset's settings once edited).
    On,
    /// A rule's preset, unedited.
    Preset(BandLeqPreset),
}

fn hz_text(b: usize) -> String {
    format!("{} Hz", ac2_scene::band_leq::band_label(BAND_NOMINAL_HZ[b]))
}

/// `d` lengths up (`d` > 0) or down from `seconds`, stopping at the ends.
fn step_length(seconds: u32, d: i32) -> u32 {
    (0..d.unsigned_abs()).fold(seconds, |s, _| {
        if d > 0 {
            LENGTHS.iter().copied().find(|&l| l > s).unwrap_or(s)
        } else {
            LENGTHS.iter().rev().copied().find(|&l| l < s).unwrap_or(s)
        }
    })
}

/// One band window being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct BandRow {
    /// Its band, an index into [`BAND_NOMINAL_HZ`].
    pub band: usize,
    pub seconds: u32,
    pub weighting: Weighting,
    /// Typed limit; empty: none.
    pub limit: String,
    /// Typed; empty: the limit holds day and night.
    pub day_offset: String,
    /// Typed warn margin.
    pub margin: String,
}

impl BandRow {
    fn of(w: &BandWindow) -> Self {
        Self {
            band: w.index().unwrap_or(0),
            seconds: w.seconds().unwrap_or(3600),
            weighting: w.weighting,
            limit: w.limit.map_or_else(String::new, |l| number_text(l.0)),
            day_offset: w.day_offset.map_or_else(String::new, |o| number_text(o.0)),
            margin: number_text(w.warn_margin.0),
        }
    }

    /// A new window: the lowest band, LZeq 60 min.
    fn new_row() -> Self {
        Self::of(&BandWindow::minutes(
            Hz(BAND_NOMINAL_HZ[0]),
            60,
            Weighting::Z,
        ))
    }

    /// Whether it is band `band` over the same length and weighting as `of`.
    fn same(&self, band: usize, of: &BandRow) -> bool {
        self.band == band && self.seconds == of.seconds && self.weighting == of.weighting
    }

    /// The window as named everywhere: `63 Hz band LZeq 60 min`.
    pub fn name(&self) -> String {
        ac2_scene::band_leq::band_window_name(
            BAND_NOMINAL_HZ[self.band],
            f64::from(self.seconds),
            self.weighting,
        )
    }

    /// The cell as the dialog shows it.
    pub fn cell(&self, c: BandCol) -> String {
        match c {
            BandCol::Band => hz_text(self.band),
            BandCol::Length => {
                ac2_scene::band_leq::window_name(f64::from(self.seconds), self.weighting)
            }
            BandCol::Weighting => weighting_letter(self.weighting).to_owned(),
            BandCol::Limit => self.limit.clone(),
            BandCol::DayOffset => self.day_offset.clone(),
            BandCol::Margin => self.margin.clone(),
        }
    }

    fn window(&self, n: usize) -> Result<BandWindow, String> {
        let name = self.name();
        let parse = |t: &str, what: &str| {
            crate::state::parse_number(t, &["db spl", "dbspl", "db"])
                .map_err(|e| format!("band window {n} ({name}) {what}: {e}"))
        };
        let limit = if self.limit.trim().is_empty() {
            None
        } else {
            let v = parse(&self.limit, "limit")?;
            if !(0.0..=150.0).contains(&v) {
                return Err(format!(
                    "band window {n} ({name}): a band limit is 0 … 150 dB SPL (empty for none)"
                ));
            }
            Some(DbSpl(v))
        };
        let day_offset = if self.day_offset.trim().is_empty() {
            None
        } else {
            let o = parse(&self.day_offset, "day offset")?;
            if !(-30.0..=30.0).contains(&o) {
                return Err(format!(
                    "band window {n} ({name}): the day offset is −30 … 30 dB (empty: the same \
                     limit day and night)"
                ));
            }
            Some(Db(o))
        };
        let margin = parse(&self.margin, "warn margin")?;
        if !(0.0..=20.0).contains(&margin) {
            return Err(format!(
                "band window {n} ({name}): the warn margin is 0 … 20 dB"
            ));
        }
        Ok(BandWindow {
            band: Hz(BAND_NOMINAL_HZ[self.band]),
            duration: Seconds(f64::from(self.seconds)),
            weighting: self.weighting,
            limit,
            day_offset,
            warn_margin: Db(margin),
        })
    }
}

/// The range row: bands `from` … `to` over one length and weighting, added as one window
/// per band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RangeRow {
    /// Indices into [`BAND_NOMINAL_HZ`], `from` ≤ `to`.
    pub from: usize,
    pub to: usize,
    pub seconds: u32,
    pub weighting: Weighting,
}

impl RangeRow {
    /// The cell as the dialog shows it.
    pub fn cell(&self, c: RangeCol) -> String {
        match c {
            RangeCol::From => hz_text(self.from),
            RangeCol::To => hz_text(self.to),
            RangeCol::Length => {
                ac2_scene::band_leq::window_name(f64::from(self.seconds), self.weighting)
            }
            RangeCol::Weighting => weighting_letter(self.weighting).to_owned(),
        }
    }

    /// What Add does: `adds 11 windows, 20–200 Hz LZeq 60 min`.
    pub fn summary(&self, rows: &[BandRow]) -> String {
        let new = self.new_bands(rows).len();
        let what = format!(
            "{} {}",
            ac2_scene::band_leq::bands_text(&(self.from..=self.to).collect::<Vec<_>>()),
            ac2_scene::band_leq::window_name(f64::from(self.seconds), self.weighting)
        );
        match new {
            0 => format!("every band {what} is a window already"),
            1 => format!("adds 1 window, {what}"),
            n => format!("adds {n} windows, {what}"),
        }
    }

    /// Its bands not yet a window over its length and weighting.
    fn new_bands(&self, rows: &[BandRow]) -> Vec<usize> {
        let like = BandRow {
            band: self.from,
            seconds: self.seconds,
            weighting: self.weighting,
            limit: String::new(),
            day_offset: String::new(),
            margin: String::new(),
        };
        (self.from..=self.to)
            .filter(|&b| !rows.iter().any(|r| r.same(b, &like)))
            .collect()
    }
}

/// The band section being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct BandSection {
    pub meter: BandMeter,
    pub rows: Vec<BandRow>,
    pub impulse: ImpulseCorrection,
    pub tonal: TonalCorrection,
    /// The predicted window: a preset's or the CLI's, kept.
    predicted: Option<PredictedWindow>,
    /// The meter's transfer: every preset keeps it.
    transfer: Option<BandTransferSet>,
    /// The range row, while open.
    pub range: Option<RangeRow>,
}

fn preset_of(c: &BandLeqConfig) -> Option<BandLeqPreset> {
    BandLeqPreset::ALL
        .into_iter()
        .find(|p| c.windows == p.windows() && c.predicted == Some(p.predicted()))
}

impl BandSection {
    pub fn new(base: Option<&BandLeqConfig>) -> Self {
        let meter = match base {
            None => BandMeter::Off,
            Some(c) => preset_of(c).map_or(BandMeter::On, BandMeter::Preset),
        };
        Self {
            meter,
            rows: base.map_or_else(Vec::new, |c| c.windows.iter().map(BandRow::of).collect()),
            impulse: base.map(|c| c.correction.impulse).unwrap_or_default(),
            tonal: base.map(|c| c.correction.tonal).unwrap_or_default(),
            predicted: base.and_then(|c| c.predicted),
            transfer: base.and_then(|c| c.transfer.clone()),
            range: None,
        }
    }

    /// Every window's band, low to high, each once.
    pub fn shown(&self) -> Vec<usize> {
        let mut b: Vec<usize> = self.rows.iter().map(|r| r.band).collect();
        b.sort_unstable();
        b.dedup();
        b
    }

    pub fn is_on(&self) -> bool {
        self.meter != BandMeter::Off
    }

    /// The section's rows top to bottom, each its cells left to right: the meter alone
    /// while it is off.
    pub fn grid(&self) -> Vec<Vec<BandFocus>> {
        let f = |x| vec![BandFocus::Field(x)];
        if !self.is_on() {
            return vec![f(BandField::Meter)];
        }
        let mut g = vec![f(BandField::Meter)];
        if self.range.is_some() {
            g.push(RangeCol::ALL.map(BandFocus::Range).to_vec());
        }
        for row in 0..self.rows.len() {
            g.push(
                BandCol::ALL
                    .map(|col| BandFocus::Window { row, col })
                    .to_vec(),
            );
        }
        g.push(f(BandField::Impulse));
        g.push(f(BandField::Tonal));
        g
    }

    fn choices(&self) -> Vec<BandMeter> {
        let mut out = vec![BandMeter::Off, BandMeter::On];
        out.extend(BandLeqPreset::ALL.map(BandMeter::Preset));
        out
    }

    /// An edit: a preset's settings become the operator's own.
    fn edited(&mut self) {
        if let BandMeter::Preset(_) = self.meter {
            self.meter = BandMeter::On;
        }
    }

    /// ←/→ on `f`: the next or previous option, stopping at the ends.
    pub fn cycle(&mut self, f: BandFocus, d: i32) {
        fn step<T: Copy + PartialEq>(all: &[T], cur: T, d: i32) -> T {
            let i = all.iter().position(|x| *x == cur).unwrap_or(0);
            all[(i as i64 + i64::from(d)).clamp(0, all.len() as i64 - 1) as usize]
        }
        let idx = |i: usize, lo: usize, hi: usize| {
            (i as i64 + i64::from(d)).clamp(lo as i64, hi as i64) as usize
        };
        match f {
            BandFocus::Field(BandField::Meter) => {
                self.meter = step(&self.choices(), self.meter, d);
                match self.meter {
                    BandMeter::Preset(p) => {
                        let c = p.apply(None);
                        self.rows = c.windows.iter().map(BandRow::of).collect();
                        self.predicted = c.predicted;
                    }
                    BandMeter::On if self.rows.is_empty() => self.rows.push(BandRow::new_row()),
                    BandMeter::Off => self.range = None,
                    BandMeter::On => {}
                }
            }
            // The rest set nothing while the meter is off.
            _ if !self.is_on() => {}
            BandFocus::Field(BandField::Impulse) => {
                use ImpulseCorrection as I;
                self.impulse = step(&[I::None, I::Plus5, I::Plus10], self.impulse, d);
            }
            BandFocus::Field(BandField::Tonal) => {
                use TonalCorrection as T;
                self.tonal = step(&[T::None, T::Plus3, T::Plus6], self.tonal, d);
            }
            BandFocus::Range(col) => {
                let Some(r) = &mut self.range else {
                    return;
                };
                match col {
                    // The other end follows so that `from` ≤ `to`.
                    RangeCol::From => {
                        r.from = idx(r.from, 0, BAND_COUNT - 1);
                        r.to = r.to.max(r.from);
                    }
                    RangeCol::To => {
                        r.to = idx(r.to, 0, BAND_COUNT - 1);
                        r.from = r.from.min(r.to);
                    }
                    RangeCol::Length => r.seconds = step_length(r.seconds, d),
                    RangeCol::Weighting => r.weighting = step(&WEIGHTINGS, r.weighting, d),
                }
            }
            BandFocus::Window { row, col } => {
                let Some(r) = self.rows.get_mut(row) else {
                    return;
                };
                match col {
                    BandCol::Band => r.band = idx(r.band, 0, BAND_COUNT - 1),
                    BandCol::Length => r.seconds = step_length(r.seconds, d),
                    BandCol::Weighting => r.weighting = step(&WEIGHTINGS, r.weighting, d),
                    BandCol::Limit | BandCol::DayOffset | BandCol::Margin => return,
                }
                self.edited();
            }
        }
    }

    /// The typed text at `f`, to edit (an edit makes a preset's settings the operator's).
    pub fn text_mut(&mut self, f: BandFocus) -> Option<&mut String> {
        if !self.is_on() || !f.is_text() {
            return None;
        }
        self.edited();
        match f {
            BandFocus::Window { row, col } => {
                let r = self.rows.get_mut(row)?;
                match col {
                    BandCol::Limit => Some(&mut r.limit),
                    BandCol::DayOffset => Some(&mut r.day_offset),
                    BandCol::Margin => Some(&mut r.margin),
                    BandCol::Band | BandCol::Length | BandCol::Weighting => None,
                }
            }
            BandFocus::Field(_) | BandFocus::Range(_) => None,
        }
    }

    /// The row new windows copy their length, weighting and warn margin from: the focused
    /// one, else the last; and where they go: after it.
    fn template(&self, at: BandFocus) -> Option<usize> {
        match at {
            BandFocus::Window { row, .. } if row < self.rows.len() => Some(row),
            _ => self.rows.len().checked_sub(1),
        }
    }

    fn room(&self, n: usize) -> Result<(), String> {
        if self.rows.len() + n > BandLeqConfig::MAX_WINDOWS {
            return Err(format!(
                "at most {} band windows per meter",
                BandLeqConfig::MAX_WINDOWS
            ));
        }
        Ok(())
    }

    /// Insert, or the + at the heading: a new band window after the focused one (or at the
    /// end) of its length, weighting and warn margin, on the next band up not yet a window
    /// of that length and weighting (the first: [`BandRow::new_row`]); the focus to move
    /// to: its band.
    pub fn add_window(&mut self, at: BandFocus) -> Result<BandFocus, String> {
        self.room(1)?;
        let after = self.template(at);
        let from = after.map_or_else(BandRow::new_row, |i| self.rows[i].clone());
        let band = if after.is_none() {
            from.band
        } else {
            // Upward from the copied band, then from the lowest.
            (1..=BAND_COUNT)
                .map(|k| (from.band + k) % BAND_COUNT)
                .find(|&b| !self.rows.iter().any(|r| r.same(b, &from)))
                .ok_or_else(|| {
                    format!(
                        "all {BAND_COUNT} bands are {} windows already",
                        ac2_scene::band_leq::window_name(f64::from(from.seconds), from.weighting)
                    )
                })?
        };
        if !self.is_on() {
            self.meter = BandMeter::On;
        }
        self.edited();
        let row = after.map_or(0, |i| i + 1);
        // Its own limit, typed: none yet.
        self.rows.insert(
            row,
            BandRow {
                band,
                limit: String::new(),
                day_offset: String::new(),
                ..from
            },
        );
        Ok(BandFocus::Window {
            row,
            col: BandCol::Band,
        })
    }

    /// Shift+Insert, or "+ range…" at the heading: the range row opened under the heading,
    /// 20 … 200 Hz over the focused (or last) window's length and weighting; the focus to
    /// move to: its first band.
    pub fn open_range(&mut self, at: BandFocus) -> BandFocus {
        let from = self
            .template(at)
            .map_or_else(BandRow::new_row, |i| self.rows[i].clone());
        if !self.is_on() {
            self.meter = BandMeter::On;
        }
        self.range = Some(RangeRow {
            from: 0,
            to: LF_BAND_COUNT - 1,
            seconds: from.seconds,
            weighting: from.weighting,
        });
        BandFocus::Range(RangeCol::From)
    }

    /// Closes the range row without adding; the focus to move to.
    pub fn close_range(&mut self) -> BandFocus {
        self.range = None;
        self.first_window()
    }

    fn first_window(&self) -> BandFocus {
        if self.rows.is_empty() {
            BandFocus::Field(BandField::Meter)
        } else {
            BandFocus::Window {
                row: 0,
                col: BandCol::Band,
            }
        }
    }

    /// Add on the range row (or Enter on it): one window per band of the range not yet a
    /// window of its length and weighting, after the last, the warn margin of the last;
    /// the row closes. The focus to move to: the first added window's band.
    pub fn add_range(&mut self) -> Result<BandFocus, String> {
        let Some(r) = self.range else {
            return Ok(self.first_window());
        };
        let bands = r.new_bands(&self.rows);
        if bands.is_empty() {
            return Err(r.summary(&self.rows));
        }
        self.room(bands.len())?;
        let margin = self
            .rows
            .last()
            .map_or_else(|| BandRow::new_row().margin, |l| l.margin.clone());
        let first = self.rows.len();
        self.rows.extend(bands.into_iter().map(|band| BandRow {
            band,
            seconds: r.seconds,
            weighting: r.weighting,
            limit: String::new(),
            day_offset: String::new(),
            margin: margin.clone(),
        }));
        self.edited();
        self.range = None;
        Ok(BandFocus::Window {
            row: first,
            col: BandCol::Band,
        })
    }

    /// − / Delete on band window `row`: removed; the focus to move to.
    pub fn remove_window(&mut self, row: usize, col: BandCol) -> BandFocus {
        if row < self.rows.len() {
            self.rows.remove(row);
            self.edited();
        }
        if self.rows.is_empty() {
            BandFocus::Field(BandField::Meter)
        } else {
            BandFocus::Window {
                row: row.min(self.rows.len() - 1),
                col,
            }
        }
    }

    /// The value of `f` as shown.
    pub fn text(&self, f: BandField) -> String {
        match f {
            BandField::Meter => match self.meter {
                BandMeter::Off => "off".into(),
                BandMeter::On => "on, as set below".into(),
                BandMeter::Preset(p) => p.name().into(),
            },
            _ if !self.is_on() => "—".into(),
            BandField::Impulse => match self.impulse {
                ImpulseCorrection::None => "none".into(),
                ImpulseCorrection::Plus5 => "+5 dB".into(),
                ImpulseCorrection::Plus10 => "+10 dB".into(),
            },
            BandField::Tonal => match self.tonal {
                TonalCorrection::None => "none".into(),
                TonalCorrection::Plus3 => "+3 dB".into(),
                TonalCorrection::Plus6 => "+6 dB".into(),
            },
        }
    }

    /// What `f` does, beside it.
    pub fn note(&self, f: BandField) -> String {
        match f {
            BandField::Meter => match self.meter {
                BandMeter::Preset(p) => {
                    let summary = ac2_scene::band_leq::preset_summary(p);
                    let what = summary
                        .split_once(": ")
                        .map_or(summary.as_str(), |(_, w)| w);
                    format!("{what}. {}", ac2_scene::band_leq::preset_source(p))
                }
                _ => "1/3-octave band Leq, one band per window against its limit; a preset \
                      replaces the windows and their limits; informational, not legal advice"
                    .into(),
            },
            BandField::Impulse | BandField::Tonal => {
                "added from now while the character is heard (ac2 does not detect it)".into()
            }
        }
    }

    /// The transfer as it stands, in a line.
    pub fn transfer_text(&self) -> String {
        ac2_scene::band_leq::transfer_summary(self.transfer.as_ref(), &self.shown())
    }

    /// The place the transfer's limits are for, as last named.
    pub fn place(&self) -> &str {
        ac2_scene::band_leq::place_of(self.transfer.as_ref())
    }

    /// A transfer stored since the dialog opened: kept from now.
    pub fn set_transfer(&mut self, t: Option<BandTransferSet>) {
        self.transfer = t;
    }

    /// How the transfer is measured, under it.
    pub fn transfer_hint(&self) -> &'static str {
        "Without a transfer the limits are judged at the mic as typed. To judge at FOH the \
         limits of a place you cannot stay in, T on a band row opens the band transfer step (a \
         steady test signal at FOH, then the same level with the mic at the place, then the \
         place with the system silent). Or `ac2 spl bands transfer`."
    }

    /// The transfer per shown band, when there is one.
    pub fn transfer_bands(&self) -> Vec<String> {
        self.transfer.as_ref().map_or_else(Vec::new, |t| {
            self.shown()
                .into_iter()
                .filter_map(|b| {
                    t.bands
                        .get(b)
                        .map(|x| ac2_scene::band_leq::transfer_band_text(BAND_NOMINAL_HZ[b], x))
                })
                .collect()
        })
    }

    /// The band meter as chosen, checked; `None`: off.
    pub fn config(&self) -> Result<Option<BandLeqConfig>, String> {
        if !self.is_on() {
            return Ok(None);
        }
        let windows = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| r.window(i + 1))
            .collect::<Result<Vec<_>, _>>()?;
        let c = BandLeqConfig {
            windows,
            predicted: self.predicted,
            correction: BandCorrection {
                impulse: self.impulse,
                tonal: self.tonal,
            },
            transfer: self.transfer.clone(),
        };
        c.check()?;
        Ok(Some(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meter() -> BandFocus {
        BandFocus::Field(BandField::Meter)
    }

    fn at(row: usize, col: BandCol) -> BandFocus {
        BandFocus::Window { row, col }
    }

    fn names(s: &BandSection) -> Vec<String> {
        s.rows.iter().map(BandRow::name).collect()
    }

    #[test]
    fn off_to_a_preset_then_edited() {
        let mut s = BandSection::new(None);
        assert_eq!(s.text(BandField::Meter), "off");
        assert_eq!(s.grid().len(), 1, "off: the meter row alone");
        s.cycle(BandFocus::Field(BandField::Impulse), 1);
        assert_eq!(s.impulse, ImpulseCorrection::None);
        assert_eq!(s.config(), Ok(None));
        s.cycle(meter(), 2);
        assert_eq!(s.meter, BandMeter::Preset(BandLeqPreset::Finland545Lf));
        assert_eq!(
            s.text(BandField::Meter),
            "Finland STM 545/2015, low frequencies"
        );
        let note = s.note(BandField::Meter);
        assert!(
            note.starts_with("20–200 Hz LZeq 60 min, night 74 … 32 dB, day 5 dB higher"),
            "{note}"
        );
        assert!(note.contains("not legal advice"));
        // One row per band, each its own limit.
        assert_eq!(s.rows.len(), 11);
        let r = &s.rows[5];
        assert_eq!(
            BandCol::ALL.map(|c| r.cell(c)),
            ["63 Hz", "LZeq 60 min", "Z", "42", "5", "3"].map(String::from)
        );
        // The meter; the 11 windows; the corrections.
        let g = s.grid();
        assert_eq!(g.len(), 14);
        assert!(g[1..12].iter().all(|r| r.len() == 6));
        let c = s.config().expect("valid").expect("on");
        assert_eq!(c, BandLeqPreset::Finland545Lf.apply(None));
        // An edit: the preset's settings become the operator's.
        s.cycle(at(0, BandCol::Weighting), -1);
        assert_eq!(s.meter, BandMeter::On);
        assert_eq!(s.rows[0].cell(BandCol::Length), "LCeq 60 min");
        assert_eq!(
            s.transfer_text(),
            "no band transfer: limits judged at the mic as typed"
        );
        assert!(!s.transfer_hint().contains("bedroom"));
    }

    /// Plus from an empty, off meter: 20 Hz LZeq 60 min; + again the next band up of the
    /// focused row's length and weighting; a band already a window of them is skipped.
    #[test]
    fn plus_adds_the_next_band() {
        let mut s = BandSection::new(None);
        let f = s.add_window(meter()).expect("room");
        assert_eq!(s.meter, BandMeter::On);
        assert_eq!(f, at(0, BandCol::Band));
        assert_eq!(names(&s), ["20 Hz band LZeq 60 min"]);
        assert_eq!(s.grid().len(), 4, "the meter, the row, the corrections");
        let f = s.add_window(f).expect("room");
        assert_eq!(f, at(1, BandCol::Band));
        assert_eq!(names(&s)[1], "25 Hz band LZeq 60 min");
        // 1 min on the first: + after it copies 1 min and takes 25 Hz (free at 1 min).
        for _ in 0..5 {
            s.cycle(at(0, BandCol::Length), -1);
        }
        assert_eq!(s.rows[0].cell(BandCol::Length), "LZeq 1 min");
        s.rows[0].margin = "2".into();
        let f = s.add_window(at(0, BandCol::Limit)).expect("room");
        assert_eq!(f, at(1, BandCol::Band));
        assert_eq!(
            names(&s),
            [
                "20 Hz band LZeq 1 min",
                "25 Hz band LZeq 1 min",
                "25 Hz band LZeq 60 min"
            ]
        );
        assert_eq!(s.rows[1].margin, "2", "the warn margin copied");
        // From the last 60 min row, 25 Hz is taken: 31.5 Hz.
        s.add_window(at(2, BandCol::Band)).expect("room");
        assert_eq!(names(&s)[3], "31.5 Hz band LZeq 60 min");
        *s.text_mut(at(0, BandCol::Limit)).expect("text") = "80".into();
        let c = s.config().expect("valid").expect("on");
        assert_eq!(c.windows.len(), 4);
        let w = c.windows[0];
        assert_eq!(w.band, Hz(20.0));
        assert_eq!(w.duration, Seconds(60.0));
        assert_eq!(w.weighting, Weighting::Z);
        assert_eq!(w.limit, Some(DbSpl(80.0)));
        assert_eq!(w.day_offset, None);
        s.rows[0].day_offset = "5".into();
        let c = s.config().expect("valid").expect("on");
        assert_eq!(c.windows[0].day_offset, Some(Db(5.0)));
        s.rows[1].limit = "loud".into();
        assert_eq!(
            s.config(),
            Err("band window 2 (25 Hz band LZeq 1 min) limit: not a number: \"loud\"".into())
        );
        s.rows[1].limit.clear();
        // The same band twice over one length and weighting is refused.
        s.cycle(at(1, BandCol::Band), -1);
        assert!(
            s.config()
                .is_err_and(|e| e.contains("two band windows of the 20 Hz band")),
            "{:?}",
            s.config()
        );
        // − removes; the focus stays on the column.
        assert_eq!(s.remove_window(1, BandCol::Margin), at(1, BandCol::Margin));
        assert_eq!(s.rows.len(), 3);
        for _ in 0..3 {
            s.remove_window(0, BandCol::Band);
        }
        assert_eq!(s.remove_window(0, BandCol::Band), meter());
        assert!(s.rows.is_empty());
    }

    #[test]
    fn every_band_and_the_cap() {
        let mut s = BandSection::new(None);
        let mut f = s.add_window(meter()).expect("room");
        for _ in 1..BAND_COUNT {
            f = s.add_window(f).expect("room");
        }
        assert_eq!(s.rows.len(), BAND_COUNT);
        assert_eq!(
            s.add_window(f),
            Err("all 28 bands are LZeq 60 min windows already".into())
        );
        s.cycle(at(0, BandCol::Weighting), -1);
        while s.rows.len() < BandLeqConfig::MAX_WINDOWS {
            let n = s.rows.len();
            s.rows.push(BandRow {
                seconds: LENGTHS[n % LENGTHS.len()],
                ..s.rows[n % BAND_COUNT].clone()
            });
        }
        assert!(
            s.add_window(meter())
                .is_err_and(|e| e == "at most 64 band windows per meter")
        );
    }

    /// "+ range…": a row from … to, Add makes one window per band not yet a window.
    #[test]
    fn a_range_adds_single_bands() {
        let mut s = BandSection::new(None);
        s.add_window(meter()).expect("room");
        s.add_window(at(0, BandCol::Band)).expect("room");
        let f = s.open_range(at(1, BandCol::Band));
        assert_eq!(f, BandFocus::Range(RangeCol::From));
        assert_eq!(s.grid()[1], RangeCol::ALL.map(BandFocus::Range).to_vec());
        let r = s.range.expect("open");
        assert_eq!(
            RangeCol::ALL.map(|c| r.cell(c)),
            ["20 Hz", "200 Hz", "LZeq 60 min", "Z"].map(String::from)
        );
        assert_eq!(r.summary(&s.rows), "adds 9 windows, 20–200 Hz LZeq 60 min");
        // To below from: from follows.
        s.cycle(BandFocus::Range(RangeCol::To), -12);
        let r = s.range.expect("open");
        assert_eq!((r.from, r.to), (0, 0));
        assert_eq!(
            s.add_range(),
            Err("every band 20 Hz LZeq 60 min is a window already".into())
        );
        s.cycle(BandFocus::Range(RangeCol::To), 5);
        let f = s.add_range().expect("room");
        assert_eq!(f, at(2, BandCol::Band));
        assert_eq!(s.range, None, "closed");
        assert_eq!(
            names(&s)[2..],
            [
                "31.5 Hz band LZeq 60 min",
                "40 Hz band LZeq 60 min",
                "50 Hz band LZeq 60 min",
                "63 Hz band LZeq 60 min"
            ]
        );
        // Another length: every band anew.
        s.open_range(at(0, BandCol::Band));
        s.cycle(BandFocus::Range(RangeCol::Length), -5);
        s.cycle(BandFocus::Range(RangeCol::To), -9);
        s.add_range().expect("room");
        assert_eq!(
            names(&s)[6..],
            ["20 Hz band LZeq 1 min", "25 Hz band LZeq 1 min"]
        );
        s.config().expect("valid");
        s.open_range(meter());
        assert_eq!(s.close_range(), at(0, BandCol::Band));
        assert_eq!(s.range, None);
    }

    #[test]
    fn a_transfer_and_cli_settings_are_kept() {
        let measured = ac2_proto::samples::band_leq_config();
        let s = BandSection::new(Some(&measured));
        assert_eq!(s.meter, BandMeter::On);
        let c = s.config().expect("valid").expect("on");
        assert_eq!(c, measured);
        assert_eq!(s.rows[11].name(), "1000 Hz band LAeq 15 min");
        assert_eq!(s.transfer_bands().len(), 12);
        assert!(
            s.transfer_text()
                .starts_with("transfer from flat 4 bedroom, 20–200 Hz, 1000 Hz: "),
            "{}",
            s.transfer_text()
        );
        let mut s = BandSection::new(Some(&measured));
        s.cycle(meter(), 1);
        assert_eq!(s.meter, BandMeter::Preset(BandLeqPreset::Finland545Lf));
        let c = s.config().expect("valid").expect("on");
        assert_eq!(c.transfer, measured.transfer, "a preset keeps the transfer");
        assert_eq!(c.correction, measured.correction);
        s.cycle(meter(), -5);
        assert_eq!(s.config(), Ok(None));
    }
}
