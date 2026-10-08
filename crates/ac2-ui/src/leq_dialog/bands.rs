//! The band meter's section of the Leq dialog (`docs/design/band-leq.md`): off, on, or a
//! rule's preset; the bands shown (a range and single bands); a table of band windows like
//! the Leq windows — length, weighting, day offset and warn margin on a row, the shown
//! bands' limits on a sub-row under it, Insert / Delete adding and removing windows; the
//! §13 corrections in force; and the transfer as it stands (measured in its band transfer
//! step or with `ac2 spl bands transfer`). A preset replaces the windows, the bands and the
//! predicted window only; everything stays editable after.

use ac2_proto::model::{
    BAND_COUNT, BAND_NOMINAL_HZ, BandCorrection, BandLeqConfig, BandLeqPreset, BandLimitSet,
    BandTransferSet, BandWindow, ImpulseCorrection, LF_BAND_COUNT, LeqConfig, PredictedWindow,
    TonalCorrection, Weighting, band_index,
};
use ac2_proto::units::{Db, DbSpl, Hz, Seconds};

use super::{LENGTHS, WEIGHTINGS, number_text, weighting_letter};

/// A setting of the band section outside its windows, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandField {
    /// Off, on, or a rule's preset.
    Meter,
    /// The lowest band of the shown range.
    From,
    /// The highest band of the shown range.
    To,
    /// Single bands shown besides the range, typed.
    Also,
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
            BandField::From => "Bands from",
            BandField::To => "… up to",
            BandField::Also => "Also bands",
            BandField::Impulse => "§13 impulse",
            BandField::Tonal => "§13 narrowband",
        }
    }
}

/// A column of a band window row, or of the limits sub-row under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandCol {
    Length,
    Weighting,
    /// How much higher the day limits are; empty: one set day and night.
    DayOffset,
    Margin,
    /// The limit of band `b` (an index into [`BAND_NOMINAL_HZ`]).
    Limit(usize),
}

impl BandCol {
    /// The columns of a window's own row.
    pub const ROW: [BandCol; 4] = [
        BandCol::Length,
        BandCol::Weighting,
        BandCol::DayOffset,
        BandCol::Margin,
    ];

    /// Column heading.
    pub fn title(self) -> String {
        match self {
            BandCol::Length => "Band window".into(),
            BandCol::Weighting => "Weighting".into(),
            BandCol::DayOffset => "Day +dB".into(),
            BandCol::Margin => "Warn within (dB)".into(),
            BandCol::Limit(b) => ac2_scene::band_leq::band_label(BAND_NOMINAL_HZ[b]),
        }
    }

    fn is_text(self) -> bool {
        !matches!(self, BandCol::Length | BandCol::Weighting)
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
            BandFocus::Field(f) => f == BandField::Also,
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
    pub seconds: u32,
    pub weighting: Weighting,
    /// Typed; empty: the limits hold day and night.
    pub day_offset: String,
    /// Typed warn margin.
    pub margin: String,
    /// Typed limit per band (every band, shown or not, so a range edit loses none).
    pub limits: [String; BAND_COUNT],
}

impl BandRow {
    fn of(w: &BandWindow) -> Self {
        Self {
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

    fn new_row() -> Self {
        Self::of(&BandWindow::minutes(60, Weighting::Z))
    }

    /// The cell as the dialog shows it.
    pub fn cell(&self, c: BandCol) -> String {
        match c {
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
        let name = self.cell(BandCol::Length);
        let parse = |t: &str, what: &str| {
            crate::state::parse_number(t, &["db spl", "dbspl", "db"])
                .map_err(|e| format!("band window {n} ({name}) {what}: {e}"))
        };
        let mut limits = [None; BAND_COUNT];
        for (b, t) in self.limits.iter().enumerate() {
            if t.trim().is_empty() {
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
    /// The shown range, indices into [`BAND_NOMINAL_HZ`].
    pub from: usize,
    pub to: usize,
    /// Single bands shown besides the range, typed (`1000, 2 kHz`).
    pub also: String,
    pub impulse: ImpulseCorrection,
    pub tonal: TonalCorrection,
    /// The predicted window: a preset's or the CLI's, kept.
    predicted: Option<PredictedWindow>,
    /// The meter's transfer: every preset keeps it.
    transfer: Option<BandTransferSet>,
}

fn preset_of(c: &BandLeqConfig) -> Option<BandLeqPreset> {
    BandLeqPreset::ALL.into_iter().find(|p| {
        c.windows == p.windows() && c.bands == p.bands() && c.predicted == Some(p.predicted())
    })
}

/// `1000` → 1000 Hz, `2 kHz` → 2000 Hz.
fn parse_band(t: &str) -> Result<usize, String> {
    let l = t.trim().to_ascii_lowercase();
    let (num, scale) = match l.strip_suffix("khz") {
        Some(n) => (n.to_owned(), 1000.0),
        None => (l.strip_suffix("hz").unwrap_or(&l).to_owned(), 1.0),
    };
    let v = crate::state::parse_number(&num, &[]).map_err(|e| format!("also bands: {e}"))?;
    band_index(v * scale).ok_or_else(|| {
        format!(
            "also bands: {:?} is not a 1/3-octave band 20 Hz … 10 kHz",
            t.trim()
        )
    })
}

impl BandSection {
    pub fn new(base: Option<&BandLeqConfig>) -> Self {
        let meter = match base {
            None => BandMeter::Off,
            Some(c) => preset_of(c).map_or(BandMeter::On, BandMeter::Preset),
        };
        let mut s = Self {
            meter,
            rows: base.map_or_else(Vec::new, |c| c.windows.iter().map(BandRow::of).collect()),
            from: 0,
            to: LF_BAND_COUNT - 1,
            also: String::new(),
            impulse: base.map(|c| c.correction.impulse).unwrap_or_default(),
            tonal: base.map(|c| c.correction.tonal).unwrap_or_default(),
            predicted: base.and_then(|c| c.predicted),
            transfer: base.and_then(|c| c.transfer.clone()),
        };
        if let Some(b) = base.and_then(BandLeqConfig::band_indices) {
            s.set_bands(&b);
        }
        s
    }

    /// The range is the first run of adjacent bands; the rest are typed beside it.
    fn set_bands(&mut self, bands: &[usize]) {
        let Some(&first) = bands.first() else {
            return;
        };
        let run = bands
            .iter()
            .enumerate()
            .take_while(|(i, b)| **b == first + i)
            .count();
        self.from = first;
        self.to = first + run - 1;
        self.also = bands[run..]
            .iter()
            .map(|&b| ac2_scene::band_leq::band_label(BAND_NOMINAL_HZ[b]))
            .collect::<Vec<_>>()
            .join(", ");
    }

    /// The single bands typed besides the range, checked.
    fn also_bands(&self) -> Result<Vec<usize>, String> {
        self.also
            .split([',', ';'])
            .filter(|t| !t.trim().is_empty())
            .map(parse_band)
            .collect()
    }

    /// The bands shown, judged and alarmed, low to high (the typed single bands as far as
    /// they parse).
    pub fn shown(&self) -> Vec<usize> {
        let mut b: Vec<usize> = (self.from..=self.to).collect();
        b.extend(
            self.also
                .split([',', ';'])
                .filter_map(|t| parse_band(t).ok()),
        );
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
        let mut g = vec![
            f(BandField::Meter),
            f(BandField::From),
            f(BandField::To),
            f(BandField::Also),
        ];
        let shown = self.shown();
        for row in 0..self.rows.len() {
            g.push(
                BandCol::ROW
                    .iter()
                    .map(|&col| BandFocus::Window { row, col })
                    .collect(),
            );
            g.push(
                shown
                    .iter()
                    .map(|&b| BandFocus::Window {
                        row,
                        col: BandCol::Limit(b),
                    })
                    .collect(),
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
                        if let Some(b) = c.band_indices() {
                            self.set_bands(&b);
                        }
                    }
                    BandMeter::On if self.rows.is_empty() => self.rows.push(BandRow::new_row()),
                    _ => {}
                }
            }
            // The rest set nothing while the meter is off.
            _ if !self.is_on() => {}
            BandFocus::Field(BandField::From) => {
                self.from = idx(self.from, 0, self.to);
                self.edited();
            }
            BandFocus::Field(BandField::To) => {
                self.to = idx(self.to, self.from, BAND_COUNT - 1);
                self.edited();
            }
            BandFocus::Field(BandField::Impulse) => {
                use ImpulseCorrection as I;
                self.impulse = step(&[I::None, I::Plus5, I::Plus10], self.impulse, d);
            }
            BandFocus::Field(BandField::Tonal) => {
                use TonalCorrection as T;
                self.tonal = step(&[T::None, T::Plus3, T::Plus6], self.tonal, d);
            }
            BandFocus::Field(BandField::Also) => {}
            BandFocus::Window { row, col } => {
                let Some(r) = self.rows.get_mut(row) else {
                    return;
                };
                match col {
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
            BandFocus::Field(BandField::Also) => Some(&mut self.also),
            BandFocus::Window { row, col } => {
                let r = self.rows.get_mut(row)?;
                match col {
                    BandCol::DayOffset => Some(&mut r.day_offset),
                    BandCol::Margin => Some(&mut r.margin),
                    BandCol::Limit(b) => r.limits.get_mut(b),
                    BandCol::Length | BandCol::Weighting => None,
                }
            }
            BandFocus::Field(_) => None,
        }
    }

    /// Insert: a new band window after the one at `at` (or at the end), one length longer;
    /// the focus to move to.
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
            col: BandCol::Length,
        })
    }

    /// Delete on band window `row`: removed; the focus to move to.
    pub fn remove_window(&mut self, row: usize, col: BandCol) -> BandFocus {
        if row < self.rows.len() {
            self.rows.remove(row);
            self.edited();
        }
        if self.rows.is_empty() {
            BandFocus::Field(BandField::Also)
        } else {
            let row = row.min(self.rows.len() - 1);
            let col = match col {
                BandCol::Limit(_) => BandCol::Length,
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
            BandField::From => format!(
                "{} Hz",
                ac2_scene::band_leq::band_label(BAND_NOMINAL_HZ[self.from])
            ),
            BandField::To => format!(
                "{} Hz",
                ac2_scene::band_leq::band_label(BAND_NOMINAL_HZ[self.to])
            ),
            BandField::Also => self.also.clone(),
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
                      the windows, limits and bands; informational, not legal advice"
                    .into(),
            },
            BandField::From | BandField::To => format!(
                "shown, judged and alarmed: {}",
                ac2_scene::band_leq::bands_text(&self.shown())
            ),
            BandField::Also => "single bands besides the range, e.g. 1000, 2 kHz".into(),
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
        let mut bands: Vec<usize> = (self.from..=self.to).collect();
        bands.extend(self.also_bands()?);
        bands.sort_unstable();
        bands.dedup();
        let c = BandLeqConfig {
            windows,
            bands: bands.into_iter().map(|b| Hz(BAND_NOMINAL_HZ[b])).collect(),
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
                .starts_with("LZeq 60 min, 20–200 Hz, night 74 … 32 dB")
        );
        assert!(s.note(BandField::Meter).contains("not legal advice"));
        assert_eq!(s.rows.len(), 1);
        assert_eq!(s.rows[0].cell(BandCol::DayOffset), "5");
        // Meter, range, also; a window row and its 11 limits; the corrections.
        let g = s.grid();
        assert_eq!(g.len(), 8);
        assert_eq!(g[5].len(), 11);
        let c = s.config().expect("valid").expect("on");
        assert_eq!(c, BandLeqPreset::Finland545Lf.apply(None));
        // An edit: the preset's settings become the operator's.
        s.cycle(
            BandFocus::Window {
                row: 0,
                col: BandCol::Weighting,
            },
            -1,
        );
        assert_eq!(s.meter, BandMeter::On);
        assert_eq!(s.rows[0].cell(BandCol::Length), "LCeq 60 min");
        s.cycle(BandFocus::Field(BandField::To), -5);
        assert_eq!(s.text(BandField::To), "63 Hz");
        *s.text_mut(BandFocus::Field(BandField::Also)).expect("text") = "1 kHz, 2000".into();
        assert_eq!(
            s.note(BandField::From),
            "shown, judged and alarmed: 20–63 Hz, 1000 Hz, 2000 Hz"
        );
        let c = s.config().expect("valid").expect("on");
        assert_eq!(c.bands.len(), 8);
        assert_eq!(c.windows[0].weighting, Weighting::C);
        // A hidden band keeps its limit.
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
    fn band_windows_are_added_typed_and_removed() {
        let mut s = BandSection::new(None);
        s.cycle(meter(), 1);
        assert_eq!(s.meter, BandMeter::On);
        assert_eq!(s.rows.len(), 1);
        let at = s
            .add_window(BandFocus::Window {
                row: 0,
                col: BandCol::Margin,
            })
            .expect("room");
        assert_eq!(
            at,
            BandFocus::Window {
                row: 1,
                col: BandCol::Length
            }
        );
        assert_eq!(s.rows[1].cell(BandCol::Length), "LZeq 120 min");
        s.cycle(
            BandFocus::Window {
                row: 1,
                col: BandCol::Weighting,
            },
            -2,
        );
        *s.text_mut(BandFocus::Window {
            row: 1,
            col: BandCol::Limit(5),
        })
        .expect("text") = "48".into();
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
            Err("band window 2 (LAeq 120 min) 80 Hz limit: not a number: \"loud\"".into())
        );
        s.rows[1].limits[6].clear();
        s.also = "1100".into();
        assert!(s.config().expect_err("not a band").contains("1100"));
        s.also.clear();
        for _ in 0..LeqConfig::MAX_WINDOWS - 2 {
            s.add_window(meter()).expect("room");
        }
        assert!(s.add_window(meter()).is_err());
        let f = s.remove_window(0, BandCol::Limit(5));
        assert_eq!(
            f,
            BandFocus::Window {
                row: 0,
                col: BandCol::Length
            }
        );
        assert_eq!(s.rows.len(), LeqConfig::MAX_WINDOWS - 1);
    }

    #[test]
    fn a_transfer_and_cli_settings_are_kept() {
        let measured = ac2_proto::samples::band_leq_config();
        let s = BandSection::new(Some(&measured));
        assert_eq!(s.meter, BandMeter::On);
        let c = s.config().expect("valid").expect("on");
        assert_eq!(c, measured);
        assert_eq!(s.text(BandField::Also), "1000");
        assert_eq!(s.transfer_bands().len(), LF_BAND_COUNT + 1);
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
