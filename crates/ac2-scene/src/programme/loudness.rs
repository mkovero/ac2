//! The Programme pane's loudness column (docs/design/programme-meter.md §6.1): which of the
//! daemon's loudness sources a pane shows, the readout lines, and what it says instead when
//! there is no source to show.
//!
//! The daemon decides the sources (`ac2_proto::cal::loudness_sources`); this module picks one
//! for a pane by the same rule, so a message names the real reason a pane has none. The
//! readouts are on the absolute scale only (LUFS, LU for the range): ac2 has no relative
//! (EBU Mode) readout, so nothing here is worded as one.

use ac2_proto::frame::{LoudnessSource, SourceLoudness};
use ac2_proto::units::WallNs;

use crate::format::{self, NO_VALUE};

/// BS.1770 is defined on 10 ms sub-blocks and a K-weighting that follows the 48 kHz curve to
/// 19.2 kHz: the daemon meters loudness only where a sub-block is a whole number of samples
/// and the curve fits under Nyquist, and not above the rates its filters are checked at.
pub const RATE_TEXT: &str = "loudness needs a rate of 44.1 to 192 kHz, a multiple of 100 Hz";
/// A LUFS figure on a measurement mic would follow its preamp gain and be misread beside
/// its dB SPL.
pub const MIC_TEXT: &str = "no loudness on a mic input";
/// A pane of several inputs that are not exactly one declared pair has no single programme.
pub const ONE_INPUT_OR_PAIR: &str = "loudness: one input or a pair";
/// What integrated loudness and the range read before a block passes their gates (the CLI's
/// word too).
pub const WAITING: &str = "— (waiting for programme)";
/// Tech 3342 asks for 60 s of programme before a range is more than a first impression.
pub const RANGE_SETTLED_S: f64 = 60.0;

/// Whether the daemon meters loudness at `rate_hz`: 44.1 to 192 kHz, a multiple of 100 Hz.
pub fn rate_has_loudness(rate_hz: u32) -> bool {
    (44_100..=192_000).contains(&rate_hz) && rate_hz.is_multiple_of(100)
}

/// `loudness: Main R (input 4) not captured`, `loudness: input 4 not captured` when the
/// input has no name of its own.
pub fn partner_text(channel: u16, name: Option<&str>) -> String {
    let n = u32::from(channel) + 1;
    match name.map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => format!("loudness: {name} (input {n}) not captured"),
        None => format!("loudness: input {n} not captured"),
    }
}

/// A declared pair as the pane names it.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedPair {
    pub left: u16,
    pub right: u16,
    /// The pair's name, or its sides' names joined (`crate::meter::pair_name`).
    pub name: String,
}

/// What the column's choice rests on: the session's rate, pairs, captured inputs and the
/// latest frame's loudness, with how inputs are named and what kind they are.
pub struct LoudnessFacts<'a> {
    pub rate_hz: u32,
    pub pairs: &'a [NamedPair],
    pub captured: &'a [u16],
    /// The latest levels frame's sources; empty with no frame.
    pub sources: &'a [SourceLoudness],
    /// Whether an input is declared a mic (undeclared inputs are).
    pub is_mic: &'a dyn Fn(u16) -> bool,
    /// What a captured input is called in the pane (its bar's label).
    pub label: &'a dyn Fn(u16) -> String,
    /// The name of an input that may not be captured, `None` when it has none of its own.
    pub own_name: &'a dyn Fn(u16) -> Option<String>,
    /// The local time of a reset, `20:14`.
    pub clock: &'a dyn Fn(WallNs) -> String,
    /// The key that resets the pane's meters, for the since line.
    pub reset_key: Option<&'a str>,
}

impl std::fmt::Debug for LoudnessFacts<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoudnessFacts")
            .field("rate_hz", &self.rate_hz)
            .field("pairs", &self.pairs)
            .field("captured", &self.captured)
            .field("sources", &self.sources)
            .field("reset_key", &self.reset_key)
            .finish_non_exhaustive()
    }
}

/// What the loudness column shows.
#[derive(Clone, Debug, PartialEq)]
pub enum LoudnessColumn {
    /// Why the pane has no loudness.
    Message(String),
    Readout(LoudnessReadout),
}

/// One source's readouts as the pane words them.
#[derive(Clone, Debug, PartialEq)]
pub struct LoudnessReadout {
    /// The pair's name, or the input's with `mono`: BS.1770 weighs one channel 1, so a mono
    /// signal on one input reads 3 LU below the same signal on both inputs of a pair.
    pub title: String,
    pub momentary: f64,
    pub short_term: f64,
    pub integrated: Option<f64>,
    pub range: Option<f64>,
    /// Programme metered since the reset, s.
    pub duration_s: f64,
    pub max_true_peak: f64,
    /// `since 20:14 · R resets`.
    pub since: String,
}

impl LoudnessReadout {
    /// `Mom −23.1`.
    pub fn momentary_text(&self) -> String {
        format!("Mom {}", format::fixed(self.momentary, 1))
    }

    /// `Short −22.8`.
    pub fn short_term_text(&self) -> String {
        format!("Short {}", format::fixed(self.short_term, 1))
    }

    /// `Int −23.0 LUFS`, `Int — (waiting for programme)` before a block passed the gate.
    pub fn integrated_text(&self) -> String {
        match self.integrated {
            Some(v) if v.is_finite() => format!("Int {} LUFS", format::fixed(v, 1)),
            Some(_) => format!("Int {NO_VALUE}"),
            None => format!("Int {WAITING}"),
        }
    }

    /// `LRA 6.2 LU`, `LRA 6.2 LU (settling)` for the first minute, `LRA — (waiting for
    /// programme)` before a value passed the gate.
    pub fn range_text(&self) -> String {
        match self.range {
            None => format!("LRA {WAITING}"),
            Some(r) if !r.is_finite() => format!("LRA {NO_VALUE}"),
            Some(r) if self.duration_s < RANGE_SETTLED_S => {
                format!("LRA {} LU (settling)", format::fixed(r, 1))
            }
            Some(r) => format!("LRA {} LU", format::fixed(r, 1)),
        }
    }

    /// `TP max −1.2 dBTP`, `TP max —` before the first metered block.
    pub fn true_peak_text(&self) -> String {
        if self.max_true_peak.is_finite() {
            format!("TP max {} dBTP", format::fixed(self.max_true_peak, 1))
        } else {
            format!("TP max {NO_VALUE}")
        }
    }

    /// The readout lines top to bottom, the since line apart (it is chrome).
    pub fn lines(&self) -> [String; 5] {
        [
            self.momentary_text(),
            self.short_term_text(),
            self.integrated_text(),
            self.range_text(),
            self.true_peak_text(),
        ]
    }
}

/// `since 20:14 · R resets` (the SPL meter's wording), `since 20:14` with no reset key.
pub fn since_text(clock: &str, reset_key: Option<&str>) -> String {
    match reset_key {
        Some(k) => format!("since {clock} · {k} resets"),
        None => format!("since {clock}"),
    }
}

/// The column of a pane showing `shown` (a pair pane: its two inputs). `None`: nothing to
/// show, as when the frame does not carry the source yet — a wrong reason would be read as
/// the truth. When several messages apply the first wins: the rate, a mic, a pair partner
/// not captured, then *one input or a pair*. The mic and partner messages concern the
/// source the pane would show, so a pane of several inputs only ever says the last.
pub fn loudness_column(shown: &[u16], f: &LoudnessFacts<'_>) -> Option<LoudnessColumn> {
    let pair_of = |c: u16| f.pairs.iter().find(|p| p.left == c || p.right == c);
    let candidate = match *shown {
        [] => return None,
        [c] => Some(pair_of(c).ok_or(c)),
        [a, b] => f
            .pairs
            .iter()
            .find(|p| (p.left, p.right) == (a, b) || (p.left, p.right) == (b, a))
            .map(Ok),
        _ => None,
    };
    if !rate_has_loudness(f.rate_hz) {
        return Some(LoudnessColumn::Message(RATE_TEXT.to_owned()));
    }
    let Some(candidate) = candidate else {
        return Some(LoudnessColumn::Message(ONE_INPUT_OR_PAIR.to_owned()));
    };
    let (source, inputs, title) = match candidate {
        Ok(p) => (
            LoudnessSource::Pair {
                left: p.left,
                right: p.right,
            },
            vec![p.left, p.right],
            p.name.clone(),
        ),
        Err(c) => (
            LoudnessSource::Input { channel: c },
            vec![c],
            format!("{} mono", (f.label)(c)),
        ),
    };
    if inputs.iter().any(|&c| (f.is_mic)(c)) {
        return Some(LoudnessColumn::Message(MIC_TEXT.to_owned()));
    }
    if let Some(&c) = inputs.iter().find(|c| !f.captured.contains(c)) {
        return Some(LoudnessColumn::Message(partner_text(
            c,
            (f.own_name)(c).as_deref(),
        )));
    }
    let l = f.sources.iter().find(|l| l.source == source)?;
    Some(LoudnessColumn::Readout(LoudnessReadout {
        title,
        momentary: l.momentary.0,
        short_term: l.short_term.0,
        integrated: l.integrated.map(|v| v.0),
        range: l.range.map(|v| v.0),
        duration_s: l.duration.0,
        max_true_peak: l.max_true_peak.0,
        since: since_text(&(f.clock)(l.since), f.reset_key),
    }))
}

#[cfg(test)]
mod tests;
