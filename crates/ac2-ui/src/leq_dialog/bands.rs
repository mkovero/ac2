//! The band meter's section of the Leq dialog (`docs/design/band-leq.md`): off, on, or a
//! rule's preset; a table of band windows like the Leq windows — each its bands (one band
//! or a range), length, weighting, limit, day offset and warn margin on a row, a range's
//! per-band limits on a sub-row under it, + / Insert and − / Delete adding and removing
//! windows; the §13 corrections in force; and the transfer as it stands (measured in its
//! band transfer step or with `ac2 spl bands transfer`). A preset replaces the windows and
//! the predicted window only; everything stays editable after.

use ac2_proto::model::{
    BAND_COUNT, BAND_NOMINAL_HZ, BandCorrection, BandLeqConfig, BandLeqPreset, BandLimitSet,
    BandRange, BandTransferSet, BandWindow, ImpulseCorrection, LeqConfig, PredictedWindow,
    TonalCorrection, Weighting,
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

/// A column of a band window row, or of the limits sub-row under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandCol {
    /// The window's (lowest) band.
    Band,
    /// Its highest band: the same for a single-band window.
    UpTo,
    Length,
    Weighting,
    /// How much higher the day limits are; empty: one set day and night.
    DayOffset,
    Margin,
    /// The limit of band `b` (an index into [`BAND_NOMINAL_HZ`]): on the row of a
    /// single-band window, on the sub-row under a range's.
    Limit(usize),
}

impl BandCol {
    /// Column heading.
    pub fn title(self) -> String {
        match self {
            BandCol::Band => "Band".into(),
            BandCol::UpTo => "Up to".into(),
            BandCol::Length => "Band window".into(),
            BandCol::Weighting => "Weighting".into(),
            BandCol::DayOffset => "Day +dB".into(),
            BandCol::Margin => "Warn within (dB)".into(),
            BandCol::Limit(b) => ac2_scene::band_leq::band_label(BAND_NOMINAL_HZ[b]),
        }
    }

    fn is_text(self) -> bool {
        !matches!(
            self,
            BandCol::Band | BandCol::UpTo | BandCol::Length | BandCol::Weighting
        )
    }
}

/// Where the focus is in the band section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandFocus {
    Field(BandField),
    /// Band window `row`, column `col` (a [`BandCol::Limit`] is on its sub-row).
    Window {
        row: usize,
        col: BandCol,
    },
}

impl BandFocus {
    /// The focus is on typed text.
    pub fn is_text(self) -> bool {
        match self {
            BandFocus::Field(_) => false,
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

/// One band window being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct BandRow {
    /// Lowest and highest band, indices into [`BAND_NOMINAL_HZ`] (equal: one band).
    pub low: usize,
    pub high: usize,
    pub seconds: u32,
    pub weighting: Weighting,
    /// Typed; empty: the limits hold day and night.
    pub day_offset: String,
    /// Typed warn margin.
    pub margin: String,
    /// Typed limit per band (every band, in the window or not, so a range edit loses none).
    pub limits: [String; BAND_COUNT],
}

impl BandRow {
    fn of(w: &BandWindow) -> Self {
        let r = w.bands.indices().unwrap_or(0..=0);
        Self {
            low: *r.start(),
            high: *r.end(),
            seconds: w.seconds().unwrap_or(3600),
            weighting: w.weighting,
            day_offset: w
                .limits
                .day_offset()
                .map_or_else(String::new, |o| number_text(o.0)),
            margin: number_text(w.warn_margin.0),
            limits: std::array::from_fn(|b| {
                w.limits.night()[b].map_or_else(String::new, |l| number_text(l.0))
            }),
        }
    }

    /// A new window: the lowest band alone, LZeq 60 min.
    fn new_row() -> Self {
        Self::of(&BandWindow::minutes(
            BandRange::single(Hz(BAND_NOMINAL_HZ[0])),
            60,
            Weighting::Z,
        ))
    }

    /// Its bands.
    pub fn range(&self) -> BandRange {
        BandRange::of_indices(self.low, self.high)
    }

    /// One band, its limit on the row.
    pub fn is_single(&self) -> bool {
        self.low == self.high
    }

    /// The cells of its row, left to right: a single band's limit among them, a range's on
    /// the sub-row ([`Self::sub_row`]).
    pub fn row_cols(&self) -> Vec<BandCol> {
        let mut c = vec![
            BandCol::Band,
            BandCol::UpTo,
            BandCol::Length,
            BandCol::Weighting,
        ];
        if self.is_single() {
            c.push(BandCol::Limit(self.low));
        }
        c.extend([BandCol::DayOffset, BandCol::Margin]);
        c
    }

    /// The limits sub-row of a range: one cell per band; none for a single band.
    pub fn sub_row(&self) -> Vec<BandCol> {
        if self.is_single() {
            return Vec::new();
        }
        (self.low..=self.high).map(BandCol::Limit).collect()
    }

    /// The window as named everywhere: `20 Hz LZeq 1 min`.
    pub fn name(&self) -> String {
        ac2_scene::band_leq::ranged_window_name(
            &self.range(),
            f64::from(self.seconds),
            self.weighting,
        )
    }

    /// The cell as the dialog shows it.
    pub fn cell(&self, c: BandCol) -> String {
        let hz = |b: usize| format!("{} Hz", ac2_scene::band_leq::band_label(BAND_NOMINAL_HZ[b]));
        match c {
            BandCol::Band => hz(self.low),
            BandCol::UpTo if self.is_single() => "this band only".into(),
            BandCol::UpTo => format!("up to {}", hz(self.high)),
            BandCol::Length => {
                ac2_scene::band_leq::window_name(f64::from(self.seconds), self.weighting)
            }
            BandCol::Weighting => weighting_letter(self.weighting).to_owned(),
            BandCol::DayOffset => self.day_offset.clone(),
            BandCol::Margin => self.margin.clone(),
            BandCol::Limit(b) => self.limits[b].clone(),
        }
    }

    fn window(&self, n: usize) -> Result<BandWindow, String> {
        let name = self.name();
        let parse = |t: &str, what: &str| {
            crate::state::parse_number(t, &["db spl", "dbspl", "db"])
                .map_err(|e| format!("band window {n} ({name}) {what}: {e}"))
        };
        let mut limits = [None; BAND_COUNT];
        // A limit typed for a band outside the window is kept as typed, not sent.
        for (b, t) in self.limits.iter().enumerate() {
            if t.trim().is_empty() || !(self.low..=self.high).contains(&b) {
                continue;
            }
            let hz = ac2_scene::band_leq::band_label(BAND_NOMINAL_HZ[b]);
            let v = parse(t, &format!("{hz} Hz limit"))?;
            if !(0.0..=150.0).contains(&v) {
                return Err(format!(
                    "band window {n} ({name}): a band limit is 0 … 150 dB SPL (empty for none)"
                ));
            }
            limits[b] = Some(DbSpl(v));
        }
        let limits = if self.day_offset.trim().is_empty() {
            BandLimitSet::Always { limits }
        } else {
            let o = parse(&self.day_offset, "day offset")?;
            if !(-30.0..=30.0).contains(&o) {
                return Err(format!(
                    "band window {n} ({name}): the day offset is −30 … 30 dB (empty: the same \
                     limits day and night)"
                ));
            }
            BandLimitSet::NightDay {
                night: limits,
                day_offset: Db(o),
            }
        };
        let margin = parse(&self.margin, "warn margin")?;
        if !(0.0..=20.0).contains(&margin) {
            return Err(format!(
                "band window {n} ({name}): the warn margin is 0 … 20 dB"
            ));
        }
        Ok(BandWindow {
            bands: self.range(),
            duration: Seconds(f64::from(self.seconds)),
            weighting: self.weighting,
            limits,
            warn_margin: Db(margin),
        })
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
        }
    }

    /// Every window's bands together, low to high, each once.
    pub fn shown(&self) -> Vec<usize> {
        let mut b: Vec<usize> = self.rows.iter().flat_map(|r| r.low..=r.high).collect();
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
        for (row, r) in self.rows.iter().enumerate() {
            let cells = |cols: Vec<BandCol>| -> Vec<BandFocus> {
                cols.into_iter()
                    .map(|col| BandFocus::Window { row, col })
                    .collect()
            };
            g.push(cells(r.row_cols()));
            let sub = r.sub_row();
            if !sub.is_empty() {
                g.push(cells(sub));
            }
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
                    _ => {}
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
            BandFocus::Window { row, col } => {
                let Some(r) = self.rows.get_mut(row) else {
                    return;
                };
                match col {
                    // A single band moves as one; a range keeps its top at or above.
                    BandCol::Band => {
                        let single = r.is_single();
                        r.low = idx(r.low, 0, BAND_COUNT - 1);
                        r.high = if single { r.low } else { r.high.max(r.low) };
                    }
                    BandCol::UpTo => r.high = idx(r.high, r.low, BAND_COUNT - 1),
                    BandCol::Length => {
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
                    BandCol::Weighting => r.weighting = step(&WEIGHTINGS, r.weighting, d),
                    _ => return,
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
                    BandCol::DayOffset => Some(&mut r.day_offset),
                    BandCol::Margin => Some(&mut r.margin),
                    BandCol::Limit(b) => r.limits.get_mut(b),
                    BandCol::Band | BandCol::UpTo | BandCol::Length | BandCol::Weighting => None,
                }
            }
            BandFocus::Field(_) => None,
        }
    }

    /// Insert, or the plus at the heading: a new band window after the one at `at` (or at
    /// the end), on the same bands one length longer (the first: [`BandRow::new_row`]); the
    /// focus to move to: its band.
    pub fn add_window(&mut self, at: BandFocus) -> Result<BandFocus, String> {
        if self.rows.len() >= LeqConfig::MAX_WINDOWS {
            return Err(format!(
                "at most {} band windows per meter",
                LeqConfig::MAX_WINDOWS
            ));
        }
        if !self.is_on() {
            self.meter = BandMeter::On;
        }
        self.edited();
        let after = match at {
            BandFocus::Window { row, .. } => row.min(self.rows.len().saturating_sub(1)),
            BandFocus::Field(_) => self.rows.len().saturating_sub(1),
        };
        let first = self.rows.is_empty();
        let from = self
            .rows
            .get(after)
            .cloned()
            .unwrap_or_else(BandRow::new_row);
        let seconds = if first {
            from.seconds
        } else {
            LENGTHS
                .iter()
                .copied()
                .find(|&s| s > from.seconds)
                .unwrap_or(from.seconds)
        };
        let row = if first { 0 } else { after + 1 };
        self.rows.insert(
            row,
            // Its own limits, typed: none yet, so one set until a day offset is typed.
            BandRow {
                seconds,
                day_offset: String::new(),
                limits: Default::default(),
                ..from
            },
        );
        Ok(BandFocus::Window {
            row,
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
            let row = row.min(self.rows.len() - 1);
            let col = match col {
                BandCol::Limit(_) => BandCol::Band,
                c => c,
            };
            BandFocus::Window { row, col }
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
                _ => "1/3-octave band Leq per window against per-band limits; a preset replaces \
                      the windows and their limits; informational, not legal advice"
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
        assert!(
            s.note(BandField::Meter)
                .starts_with("20–200 Hz LZeq 60 min, night 74 … 32 dB")
        );
        assert!(s.note(BandField::Meter).contains("not legal advice"));
        assert_eq!(s.rows.len(), 1);
        assert_eq!(s.rows[0].cell(BandCol::Band), "20 Hz");
        assert_eq!(s.rows[0].cell(BandCol::UpTo), "up to 200 Hz");
        assert_eq!(s.rows[0].cell(BandCol::DayOffset), "5");
        // The meter; the window's row (no limit on it) and its 11 limits; the corrections.
        let g = s.grid();
        assert_eq!(g.len(), 5);
        assert_eq!(g[1].len(), 6);
        assert_eq!(g[2].len(), 11);
        let c = s.config().expect("valid").expect("on");
        assert_eq!(c, BandLeqPreset::Finland545Lf.apply(None));
        // An edit: the preset's settings become the operator's.
        s.cycle(at(0, BandCol::Weighting), -1);
        assert_eq!(s.meter, BandMeter::On);
        assert_eq!(s.rows[0].cell(BandCol::Length), "LCeq 60 min");
        s.cycle(at(0, BandCol::UpTo), -5);
        assert_eq!(s.rows[0].cell(BandCol::UpTo), "up to 63 Hz");
        let c = s.config().expect("valid").expect("on");
        assert_eq!(c.windows[0].bands.len(), 6);
        assert_eq!(c.windows[0].weighting, Weighting::C);
        // A band out of the window keeps its typed limit for when it comes back.
        assert_eq!(c.windows[0].limits.night()[8], None);
        s.cycle(at(0, BandCol::UpTo), 5);
        let c = s.config().expect("valid").expect("on");
        assert_eq!(
            c.windows[0].limits.night()[8],
            BandLeqPreset::Finland545Lf.windows()[0].limits.night()[8]
        );
        assert_eq!(
            s.transfer_text(),
            "no band transfer: limits judged at the mic as typed"
        );
        assert!(!s.transfer_hint().contains("bedroom"));
    }

    #[test]
    fn a_single_band_window_has_its_limit_on_its_row() {
        let mut s = BandSection::new(None);
        // + on the band section of a meter that is off: on, with a window.
        let f = s.add_window(meter()).expect("room");
        assert_eq!(s.meter, BandMeter::On);
        assert_eq!(f, at(0, BandCol::Band));
        let r = &s.rows[0];
        assert_eq!(r.cell(BandCol::Band), "20 Hz");
        assert_eq!(r.cell(BandCol::UpTo), "this band only");
        assert_eq!(
            r.row_cols(),
            vec![
                BandCol::Band,
                BandCol::UpTo,
                BandCol::Length,
                BandCol::Weighting,
                BandCol::Limit(0),
                BandCol::DayOffset,
                BandCol::Margin,
            ]
        );
        assert_eq!(s.grid().len(), 4, "the meter, the row, the corrections");
        for _ in 0..5 {
            s.cycle(at(0, BandCol::Length), -1);
        }
        assert_eq!(s.rows[0].cell(BandCol::Length), "LZeq 1 min");
        *s.text_mut(at(0, BandCol::Limit(0))).expect("text") = "80".into();
        let c = s.config().expect("valid").expect("on");
        assert_eq!(c.windows.len(), 1);
        let w = c.windows[0];
        assert_eq!(w.bands, BandRange::single(Hz(20.0)));
        assert_eq!(w.duration, Seconds(60.0));
        assert_eq!(w.weighting, Weighting::Z);
        assert_eq!(w.limits.night()[0], Some(DbSpl(80.0)));
        // A single band moves as one.
        s.cycle(at(0, BandCol::Band), 5);
        assert_eq!(s.rows[0].range(), BandRange::single(Hz(63.0)));
        assert_eq!(s.rows[0].name(), "63 Hz LZeq 1 min");
        // − removes it: the focus goes back to the meter.
        assert_eq!(s.remove_window(0, BandCol::Margin), meter());
        assert!(s.rows.is_empty());
    }

    #[test]
    fn band_windows_are_added_typed_and_removed() {
        let mut s = BandSection::new(None);
        s.cycle(meter(), 1);
        assert_eq!(s.meter, BandMeter::On);
        assert_eq!(s.rows.len(), 1);
        s.cycle(at(0, BandCol::UpTo), 10);
        let f = s.add_window(at(0, BandCol::Margin)).expect("room");
        assert_eq!(f, at(1, BandCol::Band));
        assert_eq!(s.rows[1].name(), "20–200 Hz LZeq 120 min");
        s.cycle(at(1, BandCol::Weighting), -2);
        *s.text_mut(at(1, BandCol::Limit(5))).expect("text") = "48".into();
        let c = s.config().expect("valid").expect("on");
        assert_eq!(c.windows.len(), 2);
        assert_eq!(c.windows[1].weighting, Weighting::A);
        assert_eq!(
            c.windows[1].limits,
            BandLimitSet::Always {
                limits: std::array::from_fn(|b| (b == 5).then_some(DbSpl(48.0)))
            }
        );
        s.rows[1].day_offset = "5".into();
        let c = s.config().expect("valid").expect("on");
        assert_eq!(c.windows[1].limits.day_offset(), Some(Db(5.0)));
        s.rows[1].limits[6] = "loud".into();
        assert_eq!(
            s.config(),
            Err(
                "band window 2 (20–200 Hz LAeq 120 min) 80 Hz limit: not a number: \"loud\"".into()
            )
        );
        s.rows[1].limits[6].clear();
        for _ in 0..LeqConfig::MAX_WINDOWS - 2 {
            s.add_window(meter()).expect("room");
        }
        assert!(s.add_window(meter()).is_err());
        let f = s.remove_window(0, BandCol::Limit(5));
        assert_eq!(f, at(0, BandCol::Band));
        assert_eq!(s.rows.len(), LeqConfig::MAX_WINDOWS - 1);
    }

    #[test]
    fn a_transfer_and_cli_settings_are_kept() {
        let measured = ac2_proto::samples::band_leq_config();
        let s = BandSection::new(Some(&measured));
        assert_eq!(s.meter, BandMeter::On);
        let c = s.config().expect("valid").expect("on");
        assert_eq!(c, measured);
        assert_eq!(s.rows[1].name(), "1000 Hz LAeq 15 min");
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
