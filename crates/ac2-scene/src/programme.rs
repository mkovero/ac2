//! The Programme pane's display truth: what each input's bar reads, what the chip beside a
//! pair's goniometer says, the bars under a loudness ballistic ([`sources`], [`bars_of`]),
//! the loudness plate ([`loudness_plate`]), and where the parts sit ([`dock`], [`layout`]).
//!
//! The wire carries every level in dBFS; the scale an input is shown on
//! ([`Scale`]) converts for the text only, so a line input reads `+4.0 dBu` while its bar
//! fills exactly as the dBFS bar beside it: the bar is 60 dB from full scale down,
//! whatever the unit (`meter.rs` sets that span). Vocabulary: `dBTP` is the true peak,
//! `over` an event over the rig's threshold, `CLIP` samples at full scale, `max` the daemon's
//! hold since reset — four quantities, four words, never swapped.

use crate::format::{self, NO_VALUE};
use crate::meter::FLOOR_DBFS;
use crate::primitives::Rect;
use crate::view::{PlotChrome, ProgrammeMode};

/// The meter's dynamics, as the rig setting names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ballistic {
    TruePeak,
    Type1,
    Type2,
    Vu,
    /// BS.1770 momentary loudness of the bar's source (400 ms), LUFS. Line and digital
    /// sources only; an input without a source reads its true peak.
    Momentary,
    /// BS.1770 short-term loudness of the bar's source (3 s), LUFS.
    ShortTerm,
}

impl Ballistic {
    /// The word on the pane: `true peak`, `Type I`, `Type II`, `VU`, `Mom loudness`,
    /// `Short loudness` (never M / S, which are the goniometer's).
    pub fn word(self) -> &'static str {
        match self {
            Ballistic::TruePeak => "true peak",
            Ballistic::Type1 => "Type I",
            Ballistic::Type2 => "Type II",
            Ballistic::Vu => "VU",
            Ballistic::Momentary => "Mom loudness",
            Ballistic::ShortTerm => "Short loudness",
        }
    }

    /// A loudness window: the bars are sources, not inputs.
    pub fn is_loudness(self) -> bool {
        matches!(self, Ballistic::Momentary | Ballistic::ShortTerm)
    }

    /// What an input's own bar reads under this rig ballistic, as the Inputs list's header
    /// names it: the true peak under a loudness ballistic (loudness is a source's, in the
    /// Programme pane), else the ballistic. An input that cannot read VU says so on its
    /// own row.
    pub fn inputs_read(self) -> Ballistic {
        if self.is_loudness() {
            Ballistic::TruePeak
        } else {
            self
        }
    }
}

/// Said under a bar when VU is chosen but the input cannot offer it: what the bar reads
/// first, so a cut to the bar's width loses the reason, never the reading.
pub const VU_FALLBACK: &str = "true peak shown · VU needs a line calibration";

/// How an input's levels are shown. The wire value is dBFS; `fs` is what 0 dBFS is on the
/// scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scale {
    Dbfs,
    /// A calibrated line input: 0 dBFS is `fs_dbu`.
    Dbu {
        fs_dbu: f32,
    },
    /// A calibrated mic: 0 dBFS is `fs_db_spl`.
    DbSpl {
        fs_db_spl: f32,
    },
}

impl Scale {
    /// A dBFS level as this scale's number without its unit: `−12.3`, `+4.0`, `94.0`;
    /// none for no level or silence.
    fn number(self, dbfs: Option<f32>) -> Option<String> {
        let v = f64::from(dbfs.filter(|v| v.is_finite())?);
        Some(match self {
            Scale::Dbfs => format::signed(v, 1),
            Scale::Dbu { fs_dbu } => format::signed(v + f64::from(fs_dbu), 1),
            Scale::DbSpl { fs_db_spl } => format::level(v + f64::from(fs_db_spl)),
        })
    }
}

/// Bar fill `0 … 1` for a dBFS level over the 60 dB span under full scale (as the input
/// meters); none or silence is empty.
pub fn bar_fill(dbfs: Option<f32>) -> f32 {
    match dbfs {
        Some(v) if v.is_finite() => {
            ((f64::from(v) - FLOOR_DBFS) / -FLOOR_DBFS).clamp(0.0, 1.0) as f32
        }
        _ => 0.0,
    }
}

/// One input's row, levels in dBFS as the frame carries them.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgrammeInput {
    pub label: String,
    pub scale: Scale,
    /// The chosen ballistic's reading (or the true peak where VU is not offered).
    pub ppm: Option<f32>,
    /// True peak, dBTP, this frame.
    pub true_peak_dbtp: Option<f32>,
    /// The daemon's hold since the last reset.
    pub max: Option<f32>,
    pub overs: u32,
    pub clipped: bool,
    /// The app's flying peak ([`FlyingPeak`]).
    pub flying_peak: Option<f32>,
    /// A calibrated line input: the one kind that can read VU.
    pub offers_vu: bool,
}

impl ProgrammeInput {
    pub fn bar_fill(&self) -> f32 {
        bar_fill(self.ppm)
    }

    pub fn flying_peak_fill(&self) -> f32 {
        bar_fill(self.flying_peak)
    }

    /// The ballistic this input reads under the rig's `ballistic`: the true peak where VU
    /// is chosen but not offered, and under a loudness ballistic (loudness is a source's,
    /// never one input's).
    pub fn reads(&self, ballistic: Ballistic) -> Ballistic {
        if ballistic.is_loudness() || self.note(ballistic).is_some() {
            Ballistic::TruePeak
        } else {
            ballistic
        }
    }

    /// The unit of the reading and its max: a digital true peak is `dBTP`, any other digital
    /// reading `dBFS`; a calibrated scale keeps its own (`dBu`, `dB SPL`).
    pub fn unit(&self, ballistic: Ballistic) -> &'static str {
        match self.scale {
            Scale::Dbfs if self.reads(ballistic) == Ballistic::TruePeak => "dBTP",
            Scale::Dbfs => "dBFS",
            Scale::Dbu { .. } => "dBu",
            Scale::DbSpl { .. } => "dB SPL",
        }
    }

    /// `−12.3 dBFS`, `−1.2 dBTP`, `+4.0 dBu`, `94.0 dB SPL`, `—`.
    pub fn ppm_text(&self, ballistic: Ballistic) -> String {
        match self.scale.number(self.ppm) {
            Some(n) => format!("{n} {}", self.unit(ballistic)),
            None => NO_VALUE.to_owned(),
        }
    }

    /// `−1.2 dBTP`: the true peak is a digital quantity and keeps its own unit on every
    /// scale.
    pub fn true_peak_text(&self) -> String {
        match self.true_peak_dbtp {
            Some(v) if v.is_finite() => format!("{} dBTP", format::signed(f64::from(v), 1)),
            _ => NO_VALUE.to_owned(),
        }
    }

    /// The max holds the chosen ballistic's reading, so it carries that unit:
    /// `max −3.1 dBFS`, `max −3.1 dBTP`, `max —`.
    pub fn max_text(&self, ballistic: Ballistic) -> String {
        match self.scale.number(self.max) {
            Some(n) => format!("max {n} {}", self.unit(ballistic)),
            None => format!("max {NO_VALUE}"),
        }
    }

    /// `2 overs`, `1 over`; empty when none.
    pub fn overs_text(&self) -> String {
        match self.overs {
            0 => String::new(),
            1 => "1 over".to_owned(),
            n => format!("{n} overs"),
        }
    }

    /// `CLIP` while samples sit at full scale; empty otherwise.
    pub fn clip_text(&self) -> &'static str {
        if self.clipped { "CLIP" } else { "" }
    }

    /// The note under the bar when the rig's `ballistic` is one this input cannot read.
    pub fn note(&self, ballistic: Ballistic) -> Option<&'static str> {
        (ballistic == Ballistic::Vu && !self.offers_vu).then_some(VU_FALLBACK)
    }
}

/// Characters of a level's slot: the widest reading is `−120.0`.
pub const NUMBER_SLOT: usize = 6;
/// Characters of the overs slot: `9999 overs`.
pub const OVERS_SLOT: usize = 10;
/// Characters of a pair's correlation slot: the widest is `right quiet`.
pub const CORRELATION_SLOT: usize = 11;

/// `n` right-aligned in a [`NUMBER_SLOT`] (wider numbers keep every digit).
pub(crate) fn number_slot(n: &str) -> String {
    format!("{n:>NUMBER_SLOT$}")
}

/// `   2 overs`, `   1 over `, blank when none; past 9999 in thousands (`120k overs`), so the
/// count never outgrows its [`OVERS_SLOT`].
pub fn overs_slot(n: u32) -> String {
    let count = match n {
        0 => return " ".repeat(OVERS_SLOT),
        1..=9999 => n.to_string(),
        10_000..=999_999 => format!("{}k", n / 1000),
        _ => "1M+".to_owned(),
    };
    let word = if n == 1 { "over " } else { "overs" };
    format!("{count:>4} {word}")
}

/// A pair's line in the Inputs list: `correlation +0.87      `, `correlation right quiet`, the
/// value left in a fixed [`CORRELATION_SLOT`] so the chip keeps its width.
pub fn correlation_line(c: Correlation) -> CorrelationChip {
    let chip = c.chip();
    CorrelationChip {
        text: format!("correlation {:<CORRELATION_SLOT$}", chip.text),
        warning: chip.warning,
    }
}

/// Hold time of the flying peak, seconds.
pub const FLYING_HOLD_S: f64 = 1.0;
/// Fall rate after the hold, dB per second.
pub const FLYING_FALL_DB_PER_S: f64 = 20.0;

/// The marker above a bar: holds the highest recent peak for [`FLYING_HOLD_S`], then falls
/// at [`FLYING_FALL_DB_PER_S`] until a new peak lifts it or it drops under the bar's floor.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FlyingPeak {
    value: Option<f64>,
    held_until: f64,
    last: Option<f64>,
}

impl FlyingPeak {
    /// Feeds this frame's peak (dBFS, the chosen ballistic's reading) at display time `now`
    /// and returns the marker's level, `None` when there is nothing to show.
    pub fn update(&mut self, now: f64, peak: Option<f32>) -> Option<f32> {
        let dt = self.last.map_or(0.0, |t| (now - t).max(0.0));
        self.last = Some(now);
        if let Some(v) = self.value
            && now > self.held_until
        {
            self.value = Some(v - FLYING_FALL_DB_PER_S * dt);
        }
        let peak = peak.map(f64::from).filter(|v| v.is_finite());
        if let Some(p) = peak
            && self.value.is_none_or(|v| p >= v)
        {
            self.value = Some(p);
            self.held_until = now + FLYING_HOLD_S;
        }
        if self.value.is_some_and(|v| v < FLOOR_DBFS) {
            self.value = None;
        }
        self.value.map(|v| v as f32)
    }

    pub fn value(&self) -> Option<f32> {
        self.value.map(|v| v as f32)
    }
}

/// Which side of a pair is under the gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Both,
}

/// A pair's correlation as the frame carries it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Correlation {
    /// `−1 … 1`.
    Value(f32),
    /// Gated: a side is too quiet for the number to mean anything.
    Quiet(Side),
    /// No frame yet.
    NoData,
}

/// What the chip says and whether it says it in the warning colour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorrelationChip {
    pub text: String,
    /// Negative correlation: a sub fed the sum will cancel.
    pub warning: bool,
}

impl Correlation {
    /// `+0.87`, `−0.42` (warning), `right quiet`, `both quiet`, `—`.
    pub fn chip(self) -> CorrelationChip {
        match self {
            Correlation::Value(v) if v.is_finite() => CorrelationChip {
                text: format::signed(f64::from(v.clamp(-1.0, 1.0)), 2),
                warning: v < 0.0,
            },
            Correlation::Value(_) | Correlation::NoData => CorrelationChip {
                text: NO_VALUE.to_owned(),
                warning: false,
            },
            Correlation::Quiet(side) => CorrelationChip {
                text: match side {
                    Side::Left => "left quiet",
                    Side::Right => "right quiet",
                    Side::Both => "both quiet",
                }
                .to_owned(),
                warning: false,
            },
        }
    }
}

/// The correlator window as the rig setting reads: `0.3 s`, `1 s` (never *Fast* / *Slow*,
/// which are the SPL meter's 125 ms and 1 s).
pub fn window_text(window_s: f32) -> String {
    let w = f64::from(window_s);
    if !w.is_finite() || w <= 0.0 {
        return NO_VALUE.to_owned();
    }
    if (w - w.round()).abs() < 1e-6 {
        format!("{} s", w.round() as i64)
    } else {
        format!("{} s", format::fixed(w, 1))
    }
}

/// The pair a Programme pane shows.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgrammePair {
    pub name: String,
    pub correlation: Correlation,
    pub window_s: f32,
}

/// Everything the pane displays.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgrammePane {
    /// One per pane input, under every ballistic: the Inputs list and preview rows read
    /// these.
    pub inputs: Vec<ProgrammeInput>,
    /// What the pane draws, left to right ([`bars_of`]).
    pub bars: Vec<ProgrammeBar>,
    pub pair: Option<ProgrammePair>,
    pub ballistic: Ballistic,
    /// The rig's loudness target, LUFS: where a loudness bar's colour turns and its dashed
    /// line sits.
    pub loudness_target: f64,
    /// The loudness plate's data: present iff the pane's inputs resolve to exactly one
    /// source ([`loudness::plate_readout`]).
    pub loudness: Option<LoudnessReadout>,
    /// Where the loudness plate sits (`at: None` docks it) and its text scale.
    pub plate: crate::plate::PlatePlace,
    /// The plate under the pointer (lit, its grip bright).
    pub plate_hover: Option<crate::plate::PlateHover>,
    /// The view the operator stepped the pane to (G); see [`Self::shown_view`].
    pub view: ProgrammeMode,
}

impl ProgrammePane {
    /// The view drawn: the stored one, but the Meters view on a pane that meters no pair,
    /// where there is no goniometer (the stored view is kept for when it meters one again).
    pub fn shown_view(&self) -> ProgrammeMode {
        match self.view {
            ProgrammeMode::Goniometer if self.pair.is_some() => ProgrammeMode::Goniometer,
            _ => ProgrammeMode::Meters,
        }
    }

    /// Whether any bar reads an input (a peak bar).
    pub fn has_peak_bar(&self) -> bool {
        self.bars
            .iter()
            .any(|b| matches!(b, ProgrammeBar::Peak { .. }))
    }

    /// Whether any bar reads a loudness source.
    pub fn has_source_bar(&self) -> bool {
        self.bars
            .iter()
            .any(|b| matches!(b, ProgrammeBar::Source(_)))
    }

    /// What the bars read, as the title's first word: the ballistic, unless no bar can read
    /// it (a pane of mics under a loudness ballistic, of inputs without a line calibration
    /// under VU), when the bars read the true peak and the title says so.
    pub fn reads(&self) -> Ballistic {
        let offered = match self.ballistic {
            b if b.is_loudness() => self.has_source_bar(),
            Ballistic::Vu => self.bars.iter().any(|b| match b {
                ProgrammeBar::Peak { input, .. } => input.offers_vu,
                ProgrammeBar::Source(_) => false,
            }),
            _ => true,
        };
        if offered {
            self.ballistic
        } else {
            Ballistic::TruePeak
        }
    }

    /// The pane's title line. Meters: what the bars read, the target after a loudness
    /// word, and the pair's name with its correlator window (the Meters view carries the
    /// chip): `true peak · Main L + Main R · 0.3 s`, `Short loudness · target −23 LUFS ·
    /// Main · 1 s`. Goniometer: `goniometer · Main · 1 s`, the window being the chip's.
    pub fn title(&self) -> String {
        let pair = self
            .pair
            .as_ref()
            .map(|p| format!(" · {} · {}", p.name, window_text(p.window_s)));
        if self.shown_view() == ProgrammeMode::Goniometer {
            return format!("goniometer{}", pair.unwrap_or_default());
        }
        let reads = self.reads();
        let mut s = reads.word().to_owned();
        if reads.is_loudness() {
            s.push_str(&format!(
                " · target {}",
                rig::target_text(self.loudness_target)
            ));
        }
        s.push_str(&pair.unwrap_or_default());
        s
    }

    /// The scale columns' total width at the plot font `small`: the dBFS column before the
    /// peak bars (always under a peak ballistic), the LUFS column before the source bars.
    pub fn scale_cols_w(&self, small: f32) -> f32 {
        let peak = self.has_peak_bar() || !self.has_source_bar();
        let mut w = 0.0;
        if peak {
            w += scale_w(ScaleKind::Dbfs, small);
        }
        if self.has_source_bar() {
            w += scale_w(ScaleKind::Lufs, small);
        }
        w
    }

    /// Where the parts go in `rect` at the plot fonts `font` and `small` and the **T** step
    /// `chrome`. Goniometer: the square and its chip. Meters: the dock when the pane has a
    /// plate where the pane puts it and the plate shows (at *traces only* it is hidden, and
    /// its room goes to the bars), else the bars on the whole rect (a floating plate
    /// reserves nothing).
    pub fn layout(&self, rect: Rect, font: f32, small: f32, chrome: PlotChrome) -> ProgrammeLayout {
        if self.shown_view() == ProgrammeMode::Goniometer {
            return goniometer(rect, chip_min_w(font, small));
        }
        let has_pair = self.pair.is_some();
        let docks =
            self.loudness.is_some() && self.plate.at.is_none() && chrome != PlotChrome::Bare;
        if docks {
            let need = bars_min(self.bars.len(), self.scale_cols_w(small));
            dock(rect, has_pair, need, small)
        } else {
            layout(rect, has_pair)
        }
    }
}

/// What a Programme pane says when inputs it names are not in the session (or no session
/// is open): `input 3 not in this session`, `inputs 3+4 not in this session`.
pub fn missing_text(inputs: &[u16]) -> String {
    let n: Vec<String> = inputs
        .iter()
        .map(|c| (u32::from(*c) + 1).to_string())
        .collect();
    match n.len() {
        1 => format!("input {} not in this session", n[0]),
        _ => format!("inputs {} not in this session", n.join("+")),
    }
}

/// A Programme pane that names nothing to meter (a layout from elsewhere): its list picks
/// the inputs.
pub const NO_METERS: &str = "no inputs chosen: the pane's list picks a pair or inputs";

/// A Programme row of a pane's list: `meter  Main L + Main R`, `meter  inputs 1–8`.
pub fn menu_row(what: &str) -> String {
    format!("meter  {what}")
}

/// What a Programme row of several inputs is called: `inputs 1, 2, 5`.
pub fn inputs_text(inputs: &[u16]) -> String {
    let n: Vec<String> = inputs
        .iter()
        .map(|c| (u32::from(*c) + 1).to_string())
        .collect();
    format!("inputs {}", n.join(", "))
}

mod bars;
mod draw;
pub mod form;
mod gonio_view;
mod layout;
pub mod loudness;
pub mod loudness_plate;
mod meters;
mod row;
pub use bars::{
    LUFS_LOW, LUFS_SPAN, ProgrammeBar, Resolved, SourceBar, TRUE_PEAK_SHOWN, bars_of, floor_text,
    lufs_fill, on_lufs_span, peak_bars, sources,
};
pub use draw::{ProgrammeScene, programme_scene};
pub use layout::{
    BAR_MIN_H, BAR_MIN_W, CHIP_H, Docked, FIT_SCALE_MAX, FIT_SCALE_STEP, GONIO_MIN_W,
    ProgrammeLayout, ScaleKind, bars_min, chip_min_w, dock, fit_scale, goniometer, layout, scale_w,
};
pub use loudness::LoudnessReadout;
pub use row::{
    INPUTS_LOUDNESS_TIP, RowBlock, RowLine, RowSide, UNIT_SLOT, WORD_SLOT, inputs_header, row_block,
};

#[cfg(test)]
mod tests;

/// The rig's programme-meter rows on the Rig monitor page: title, the value as shown, and a
/// note under it.
pub mod rig {
    use crate::format;

    pub const BALLISTIC_TITLE: &str = "Programme meters read";
    pub const WINDOW_TITLE: &str = "Correlation window";
    pub const OVER_TITLE: &str = "An over is above";
    pub const GATE_TITLE: &str = "Correlation gate";

    pub const BALLISTIC_NOTE: &str = "the rig's choice: every app on it reads the same bar · \
         VU only on a calibrated line input · loudness on line and digital inputs";
    pub const WINDOW_NOTE: &str = "how long the correlator averages";
    pub const OVER_NOTE: &str = "true peak above this counts an over";
    pub const GATE_NOTE: &str = "a side quieter than this reads quiet, with no correlation";

    /// One ←/→ step of the over threshold, dB.
    pub const OVER_STEP_DB: f64 = 0.5;
    /// One ←/→ step of the correlation gate, dB.
    pub const GATE_STEP_DB: f64 = 5.0;

    /// `−1.0 dBTP`.
    pub fn over_text(dbtp: f64) -> String {
        format!("{} dBTP", format::signed(dbtp, 1))
    }

    /// `−60 dBFS`.
    pub fn gate_text(dbfs: f64) -> String {
        format!("{} dBFS", format::signed(dbfs, 0))
    }

    pub const TARGET_TITLE: &str = "Loudness target";
    /// The target colours the bars, so it is the rig's: two apps on one rig must light the
    /// same programme the same way.
    pub const TARGET_NOTE: &str = "loudness bars are lighter above it · the rig's: every app \
         on it reads the same · R128 −23, streaming often −14";
    /// One ←/→ step of the loudness target, LU.
    pub const TARGET_STEP_LU: f64 = 1.0;

    /// `−23 LUFS`: the target is set in whole LU.
    pub fn target_text(lufs: f64) -> String {
        format!("{} LUFS", format::fixed(lufs, 0))
    }
}
