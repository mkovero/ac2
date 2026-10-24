//! The Programme pane's display truth: what each input's bar reads, what the chip beside a
//! pair's goniometer says, the loudness column ([`loudness`]), and where the parts sit.
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

/// The meter's dynamics, as the rig setting names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ballistic {
    TruePeak,
    Type1,
    Type2,
    Vu,
}

impl Ballistic {
    /// The word on the pane: `true peak`, `Type I`, `Type II`, `VU`.
    pub fn word(self) -> &'static str {
        match self {
            Ballistic::TruePeak => "true peak",
            Ballistic::Type1 => "Type I",
            Ballistic::Type2 => "Type II",
            Ballistic::Vu => "VU",
        }
    }
}

/// Said under a bar when VU is chosen but the input cannot offer it.
pub const VU_FALLBACK: &str = "VU needs a line calibration · true peak shown";

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
    /// is chosen but not offered.
    pub fn reads(&self, ballistic: Ballistic) -> Ballistic {
        if self.note(ballistic).is_some() {
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

    /// The Inputs-list line under the row's meter, readouts only, never empty separators,
    /// the reading's unit said once (the reading and its max share it), so the line fits the
    /// sidebar: `Type I −12.3 · max −3.1 dBFS · −1.2 dBTP · 2 overs · CLIP`,
    /// `true peak −12.3 · max −3.1 dBTP`.
    pub fn row_text(&self, ballistic: Ballistic) -> String {
        let reads = self.reads(ballistic);
        let unit = self.unit(ballistic);
        let ppm = self.scale.number(self.ppm);
        let max = self.scale.number(self.max);
        // The unit goes after the last number on the line.
        let with_unit = |n: &str, last: bool| {
            if last {
                format!("{n} {unit}")
            } else {
                n.to_owned()
            }
        };
        let reading = format!(
            "{} {} · max {}",
            reads.word(),
            ppm.as_deref()
                .map_or(NO_VALUE.to_owned(), |p| with_unit(p, max.is_none())),
            max.as_deref()
                .map_or(NO_VALUE.to_owned(), |m| with_unit(m, true)),
        );
        let mut parts = vec![reading];
        // A true-peak reading already is the dBTP number.
        if reads != Ballistic::TruePeak {
            parts.push(self.true_peak_text());
        }
        parts.extend(
            [self.overs_text(), self.clip_text().to_owned()]
                .into_iter()
                .filter(|t| !t.is_empty()),
        );
        parts.join(" · ")
    }

    /// The note under the bar when the rig's `ballistic` is one this input cannot read.
    pub fn note(&self, ballistic: Ballistic) -> Option<&'static str> {
        (ballistic == Ballistic::Vu && !self.offers_vu).then_some(VU_FALLBACK)
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
    pub inputs: Vec<ProgrammeInput>,
    pub pair: Option<ProgrammePair>,
    pub ballistic: Ballistic,
    /// The loudness column; `None` shows none.
    pub loudness: Option<loudness::LoudnessColumn>,
}

impl ProgrammePane {
    /// The pane's title line: the ballistic word, and the pair's name with its correlator
    /// window: `true peak · Main L + Main R · 0.3 s`.
    pub fn title(&self) -> String {
        let mut s = self.ballistic.word().to_owned();
        if let Some(p) = &self.pair {
            s.push_str(&format!(" · {} · {}", p.name, window_text(p.window_s)));
        }
        s
    }

    pub fn layout(&self, rect: Rect) -> ProgrammeLayout {
        layout(
            rect,
            self.pair.is_some(),
            self.loudness.is_some(),
            self.inputs.len(),
        )
    }
}

/// Height of the correlation chip strip, logical pixels.
pub const CHIP_H: f32 = 28.0;
/// Width of the chip when it stands alone beside the bars (short pane).
pub const CHIP_W: f32 = 96.0;
/// Below this height the goniometer has no room (it needs a square of at least ~120 px
/// above the chip) and the pane shows bars plus the chip only.
pub const SHORT_PANE_H: f32 = 150.0;
/// Width of the loudness column at the pane's right edge: `since 2 Oct 20:14 · R resets` on
/// one line.
pub const LOUDNESS_W: f32 = 190.0;
/// A bar's column keeps its readouts whole (`max −11.8 dBTP`) at this width.
pub const BAR_MIN_W: f32 = 96.0;
/// The dBFS scale left of the bars.
pub const BAR_SCALE_W: f32 = 32.0;
/// The narrowest goniometer worth drawing beside the loudness column.
pub const GONIO_MIN_W: f32 = 120.0;

/// Where the parts go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgrammeLayout {
    /// The goniometer's rect (a pair, tall enough); the figure is [`crate::gonio::square`]
    /// of it.
    pub gonio: Option<Rect>,
    /// The correlation chip (a pair).
    pub correlation: Option<Rect>,
    pub bars: Rect,
    /// The loudness column (the pane has one and room for it).
    pub loudness: Option<Rect>,
}

/// Bars on the right, the goniometer with the correlation chip under it on the left, split
/// 1:1 (the MSD main screen); bars alone without a pair; on a pane shorter than
/// [`SHORT_PANE_H`] the chip stands [`CHIP_W`] wide on the left and the bars take the rest.
///
/// The loudness column, when `loudness`, takes [`LOUDNESS_W`] at the right edge only if the
/// rest keeps the `inputs` bars their readouts ([`BAR_MIN_W`] each) and a pair its
/// goniometer ([`GONIO_MIN_W`]) or chip: a narrow pane drops the column first. With the
/// column the bars take what they need and the goniometer the rest, up to half.
pub fn layout(rect: Rect, has_pair: bool, loudness: bool, inputs: usize) -> ProgrammeLayout {
    let short = rect.h < SHORT_PANE_H;
    let bars_min = BAR_SCALE_W + inputs.max(1) as f32 * BAR_MIN_W;
    let beside = match (has_pair, short) {
        (false, _) => 0.0,
        (true, true) => CHIP_W,
        (true, false) => GONIO_MIN_W,
    };
    let column = (loudness && rect.w - LOUDNESS_W >= bars_min + beside)
        .then(|| Rect::new(rect.right() - LOUDNESS_W, rect.y, LOUDNESS_W, rect.h));
    let rect = match column {
        Some(c) => Rect::new(rect.x, rect.y, rect.w - c.w, rect.h),
        None => rect,
    };
    if !has_pair {
        return ProgrammeLayout {
            gonio: None,
            correlation: None,
            bars: rect,
            loudness: column,
        };
    }
    if short {
        let w = CHIP_W.min(rect.w / 2.0);
        return ProgrammeLayout {
            gonio: None,
            correlation: Some(Rect::new(rect.x, rect.y, w, rect.h)),
            bars: Rect::new(rect.x + w, rect.y, rect.w - w, rect.h),
            loudness: column,
        };
    }
    let gonio_w = match column {
        Some(_) => (rect.w - bars_min).min(rect.w / 2.0),
        None => rect.w / 2.0,
    };
    ProgrammeLayout {
        gonio: Some(Rect::new(rect.x, rect.y, gonio_w, rect.h - CHIP_H)),
        correlation: Some(Rect::new(rect.x, rect.bottom() - CHIP_H, gonio_w, CHIP_H)),
        bars: Rect::new(rect.x + gonio_w, rect.y, rect.w - gonio_w, rect.h),
        loudness: column,
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

mod draw;
pub mod loudness;
pub use draw::{ProgrammeScene, programme_scene};

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
         VU only on a calibrated line input";
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
}
