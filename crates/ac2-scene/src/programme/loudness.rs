//! The Programme pane's loudness facts and the loudness plate's data
//! (docs/design/programme-meter.md §6.1, §6.2): what the resolution of a pane's inputs to
//! sources rests on ([`LoudnessFacts`], used by [`super::sources`]), why an input has none,
//! and one source's readouts as the plate shows them ([`LoudnessReadout`]).
//!
//! The daemon decides the sources (`ac2_proto::cal::loudness_sources`); the pane resolves its
//! inputs by the same rule, so a reason names the real cause. The readouts are on the
//! absolute scale only (LUFS, LU for the range): ac2 has no relative (EBU Mode) readout, so
//! nothing here is worded as one.

use ac2_proto::frame::{LoudnessSource, SourceLoudness};
use ac2_proto::units::WallNs;

use super::{ProgrammeInput, Resolved};
use crate::format::{self, NO_VALUE};

/// BS.1770 is defined on 10 ms sub-blocks and a K-weighting that follows the 48 kHz curve to
/// 19.2 kHz: the daemon meters loudness only where a sub-block is a whole number of samples
/// and the curve fits under Nyquist, and not above the rates its filters are checked at.
pub const RATE_TEXT: &str = "loudness needs a rate of 44.1 to 192 kHz, a multiple of 100 Hz";
/// A LUFS figure on a measurement mic would follow its preamp gain and be misread beside
/// its dB SPL.
pub const MIC_TEXT: &str = "no loudness on a mic input";
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

/// What the resolution rests on: the session's rate, pairs, captured inputs and the latest
/// frame's loudness, with how inputs are named, what kind they are and what they read.
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
    /// Every captured input's peak readings as the frame carries them, for the bar of each
    /// input the pane names. A side the pane does not name has no bar, so its overs and
    /// CLIP are not on this pane: the plate's TP max is all the pane keeps of it.
    pub input: &'a dyn Fn(u16) -> Option<ProgrammeInput>,
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

/// One source's readouts as the plate words them. A source the frame does not carry yet
/// reads −∞ / `None` everywhere ([`LoudnessReadout::empty`]), so the plate is there before
/// its first value and never pops in a frame later.
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
    /// Tech 3341's maximum since the reset of each window.
    pub max_momentary: f64,
    pub max_short_term: f64,
    /// `since 20:14 · R resets`; blank before the frame carries the source.
    pub since: String,
}

impl LoudnessReadout {
    /// A source the frame does not carry yet.
    pub fn empty(title: String) -> Self {
        Self {
            title,
            momentary: f64::NEG_INFINITY,
            short_term: f64::NEG_INFINITY,
            integrated: None,
            range: None,
            duration_s: 0.0,
            max_true_peak: f64::NEG_INFINITY,
            max_momentary: f64::NEG_INFINITY,
            max_short_term: f64::NEG_INFINITY,
            since: String::new(),
        }
    }

    fn of(title: String, l: &SourceLoudness, since: String) -> Self {
        Self {
            title,
            momentary: l.momentary.0,
            short_term: l.short_term.0,
            integrated: l.integrated.map(|v| v.0),
            range: l.range.map(|v| v.0),
            duration_s: l.duration.0,
            max_true_peak: l.max_true_peak.0,
            max_momentary: l.max_momentary.0,
            max_short_term: l.max_short_term.0,
            since,
        }
    }
}

/// A plate value: one decimal, `—` for −∞ or none.
pub fn value_text(v: Option<f64>) -> String {
    match v {
        Some(v) if v.is_finite() => format::fixed(v, 1),
        _ => NO_VALUE.to_owned(),
    }
}

/// `since 20:14 · R resets` (the SPL meter's wording), `since 20:14` with no reset key.
pub fn since_text(clock: &str, reset_key: Option<&str>) -> String {
    match reset_key {
        Some(k) => format!("since {clock} · {k} resets"),
        None => format!("since {clock}"),
    }
}

/// What a source is called on its bar and plate: the pair's name, an input's with `mono`.
pub fn source_name(source: LoudnessSource, f: &LoudnessFacts<'_>) -> String {
    match source {
        LoudnessSource::Pair { left, right } => f
            .pairs
            .iter()
            .find(|p| p.left == left && p.right == right)
            .map_or_else(
                || crate::meter::pair_name(&(f.label)(left), &(f.label)(right)),
                |p| p.name.clone(),
            ),
        LoudnessSource::Input { channel } => format!("{} mono", (f.label)(channel)),
    }
}

/// The loudness plate's data for a pane whose inputs resolved to `resolved`: present iff
/// they name exactly one distinct source, whether or not the frame carries it yet. Several
/// sources have no single programme to read; none has nothing.
pub fn plate_readout(resolved: &[Resolved], f: &LoudnessFacts<'_>) -> Option<LoudnessReadout> {
    let mut sources = resolved.iter().filter_map(Resolved::source);
    let first = sources.next()?;
    if sources.any(|s| s != first) {
        return None;
    }
    let title = source_name(first, f);
    Some(match f.sources.iter().find(|l| l.source == first) {
        Some(l) => LoudnessReadout::of(title, l, since_text(&(f.clock)(l.since), f.reset_key)),
        None => LoudnessReadout::empty(title),
    })
}
