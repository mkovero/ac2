//! An Inputs-list row (docs/design/programme-meter.md §6.3 *The Inputs list rows*): one
//! quantity per bar, the reading the Programme pane's bar of that input reads, with its
//! unit beside it, and every text in a fixed slot of a monospace line, so only the bar fill
//! and the tick move as levels, overs and CLIP come and go.

use super::{
    Ballistic, Correlation, CorrelationChip, ProgrammeInput, correlation_line, number_slot,
    overs_slot,
};
use crate::format::NO_VALUE;
use crate::meter::{HOT_PEAK_DBFS, MeterState, PAIR_MARKS, SIGNAL_DBFS};

/// Characters of a unit's slot: the widest is `dB SPL`.
pub const UNIT_SLOT: usize = 6;
/// Characters of a line's word: the widest is `true peak`.
pub const WORD_SLOT: usize = 9;
/// Characters of the CLIP slot.
const CLIP_SLOT: usize = 4;

/// Said in the header's hover tip under a loudness ballistic, where the rows read the true
/// peak: too long for the header line, which must not wrap and push the rows down.
pub const INPUTS_LOUDNESS_TIP: &str = "loudness is metered per programme in the Programme pane";

/// The Inputs header after the strong `Inputs`, from the rig's ballistic (not the frame's,
/// so it reads before any frame): `true peak · max since R`, `Type I · max since R`.
pub fn inputs_header(ballistic: Ballistic, reset_key: &str) -> String {
    format!("{} · max since {reset_key}", ballistic.inputs_read().word())
}

/// A line of text, and whether it reads in the fault colour (CLIP, overs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowLine {
    pub text: String,
    pub flagged: bool,
}

/// One side's bar line: `<m> <bar> <number> <unit> <CLIP>`.
#[derive(Clone, Debug, PartialEq)]
pub struct RowSide {
    /// `L`, `R`, or a blank on a mono row: one character on every row, so every number
    /// column lines up down the list.
    pub mark: &'static str,
    /// The reading's bar fill, `0 … 1` (60 dB under full scale).
    pub fill: f32,
    /// The flying peak's place, `0 … 1`; `None` draws no tick.
    pub tick: Option<f32>,
    /// What colour the bar takes, as a pane's peak bar: level, hot, CLIP; no data.
    pub state: MeterState,
    /// The text right of the bar: the number and unit, then CLIP's slot.
    pub reading: RowLine,
}

/// A mono row's or a pair's lines under the label.
#[derive(Clone, Debug, PartialEq)]
pub struct RowBlock {
    /// One bar line per side, left first.
    pub sides: Vec<RowSide>,
    /// Each side's max (with its overs), then each side's true peak where the bar reads
    /// something else.
    pub lines: Vec<RowLine>,
    /// A pair's correlation: `  correlation +0.31`, in the warning colours when negative.
    pub correlation: Option<CorrelationChip>,
    /// Why a row cannot read the rig's ballistic: `  true peak shown · VU needs …`.
    pub note: Option<String>,
}

fn state(i: &ProgrammeInput) -> MeterState {
    let Some(v) = i.ppm else {
        return MeterState::NoData;
    };
    let v = f64::from(v);
    if i.clipped {
        MeterState::Clip
    } else if v.is_finite() && v >= HOT_PEAK_DBFS {
        MeterState::Hot
    } else if v.is_finite() && v >= SIGNAL_DBFS {
        MeterState::Signal
    } else {
        MeterState::Silent
    }
}

/// The lines of a row metering `inputs` (one, or a pair's left and right) under the rig's
/// `ballistic`, with the pair's `correlation`. The words and helpers are the Programme
/// pane's, so the two surfaces cannot drift. A row with no reading (no frame yet) keeps
/// every slot, each number reading `—`.
pub fn row_block(
    inputs: &[ProgrammeInput],
    ballistic: Ballistic,
    correlation: Option<Correlation>,
) -> RowBlock {
    let pair = inputs.len() > 1;
    let mark = |k: usize| match (pair, k) {
        (false, _) => " ",
        (true, 0) => PAIR_MARKS.0,
        (true, _) => PAIR_MARKS.1,
    };
    let slot = |v: Option<String>| number_slot(v.as_deref().unwrap_or(NO_VALUE));
    let unit = |u: &str| format!("{u:<UNIT_SLOT$}");
    let sides = inputs
        .iter()
        .enumerate()
        .map(|(k, i)| {
            let clip = if i.clipped { "CLIP" } else { "" };
            RowSide {
                mark: mark(k),
                fill: i.bar_fill(),
                tick: i
                    .flying_peak
                    .filter(|v| v.is_finite())
                    .map(|_| i.flying_peak_fill()),
                state: state(i),
                reading: RowLine {
                    text: format!(
                        "{} {} {clip:<CLIP_SLOT$}",
                        slot(i.scale.number(i.ppm)),
                        unit(i.unit(ballistic))
                    ),
                    flagged: i.clipped,
                },
            }
        })
        .collect();
    let mut lines: Vec<RowLine> = inputs
        .iter()
        .enumerate()
        .map(|(k, i)| RowLine {
            text: format!(
                "{} {:<WORD_SLOT$} {} {} {}",
                mark(k),
                "max",
                slot(i.scale.number(i.max)),
                unit(i.unit(ballistic)),
                overs_slot(i.overs)
            ),
            flagged: i.overs > 0,
        })
        .collect();
    // A true-peak bar already reads the dBTP number; a second line would repeat it.
    lines.extend(
        inputs
            .iter()
            .enumerate()
            .filter(|(_, i)| i.reads(ballistic) != Ballistic::TruePeak)
            .map(|(k, i)| {
                let tp = i
                    .true_peak_dbtp
                    .filter(|v| v.is_finite())
                    .map(|v| crate::format::signed(f64::from(v), 1));
                RowLine {
                    text: format!(
                        "{} {:<WORD_SLOT$} {} {} {:OVERS$}",
                        mark(k),
                        Ballistic::TruePeak.word(),
                        slot(tp),
                        unit("dBTP"),
                        "",
                        OVERS = super::OVERS_SLOT
                    ),
                    flagged: false,
                }
            }),
    );
    let correlation = correlation.filter(|_| pair).map(|c| {
        let chip = correlation_line(c);
        CorrelationChip {
            text: format!("  {}", chip.text),
            warning: chip.warning,
        }
    });
    let note = inputs
        .iter()
        .find_map(|i| i.note(ballistic))
        .map(|n| format!("  {n}"));
    RowBlock {
        sides,
        lines,
        correlation,
        note,
    }
}
