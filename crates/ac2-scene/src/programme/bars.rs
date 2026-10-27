//! What a Programme pane's bars read (docs/design/programme-meter.md §6.2 *What a bar
//! reads*): under a peak ballistic one bar per input; under a loudness ballistic one bar
//! per input reading its true peak, then one per loudness source, a pair's being both
//! channels summed (BS.1770 defines loudness of a programme, one figure for all its
//! channels: a side K-weighted alone is not the loudness of anything delivered); an input
//! without a source says why under its true-peak bar (§6.3 *The Meters view*).

use ac2_proto::frame::LoudnessSource;

use super::loudness::{
    LoudnessFacts, MIC_TEXT, RATE_TEXT, partner_text, rate_has_loudness, source_name,
};
use super::{Ballistic, ProgrammeInput, Scale};
use crate::format::{self, NO_VALUE};

/// A pane input resolved to its loudness source, or to why it has none.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolved {
    Source {
        input: u16,
        source: LoudnessSource,
    },
    /// §6.1's reason, in its order: the rate, a mic, a pair partner not captured.
    Reason {
        input: u16,
        reason: String,
    },
}

impl Resolved {
    pub fn input(&self) -> u16 {
        match self {
            Resolved::Source { input, .. } | Resolved::Reason { input, .. } => *input,
        }
    }

    pub fn source(&self) -> Option<LoudnessSource> {
        match self {
            Resolved::Source { source, .. } => Some(*source),
            Resolved::Reason { .. } => None,
        }
    }
}

/// Each of `shown` (the pane's inputs, in order) resolved to its source: the input's pair
/// when it is half of a declared one (whether or not the pane names the other half), else
/// the input alone. The one place an input is mapped to a source, so the bars and the plate
/// never disagree on it.
pub fn sources(shown: &[u16], f: &LoudnessFacts<'_>) -> Vec<Resolved> {
    shown
        .iter()
        .map(|&input| {
            let reason = |reason: String| Resolved::Reason { input, reason };
            if !rate_has_loudness(f.rate_hz) {
                return reason(RATE_TEXT.to_owned());
            }
            let (source, sides) = match f.pairs.iter().find(|p| p.left == input || p.right == input)
            {
                Some(p) => (
                    LoudnessSource::Pair {
                        left: p.left,
                        right: p.right,
                    },
                    vec![p.left, p.right],
                ),
                None => (LoudnessSource::Input { channel: input }, vec![input]),
            };
            if sides.iter().any(|&c| (f.is_mic)(c)) {
                return reason(MIC_TEXT.to_owned());
            }
            if let Some(&c) = sides.iter().find(|c| !f.captured.contains(c)) {
                return reason(partner_text(c, (f.own_name)(c).as_deref()));
            }
            Resolved::Source { input, source }
        })
        .collect()
}

/// A loudness source's bar. Its sides' true peak, overs and CLIP are their own bars' (the
/// true-peak block), and the plate's TP max covers the whole source.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceBar {
    /// `Main`, `Vox mono`.
    pub name: String,
    /// LUFS of the ballistic's window; `None` for −∞ (no window wholly after the start).
    pub reading: Option<f64>,
    /// The window's max since the reset; `None` before one.
    pub max: Option<f64>,
}

impl SourceBar {
    /// `−23.1 LUFS`, `—`.
    pub fn reading_text(&self) -> String {
        lufs_text(self.reading)
    }

    /// `−20.1 LUFS` (the `max` word sits apart), `—`.
    pub fn max_value_text(&self) -> String {
        lufs_text(self.max)
    }

    /// Whether the reading lies under the span's floor at `target`, where the bar is
    /// empty although the programme is not silent. A reading of none (no window yet) is
    /// not: `—` already says *nothing yet*.
    pub fn below_floor(&self, target: f64) -> bool {
        self.reading
            .is_some_and(|v| v.is_finite() && v < target + LUFS_LOW)
    }
}

/// `below −41`: what an empty LUFS bar says when its reading lies under the floor of the
/// span at `target`.
pub fn floor_text(target: f64) -> String {
    format!("below {}", format::fixed(target + LUFS_LOW, 0))
}

fn lufs_text(v: Option<f64>) -> String {
    match v.filter(|v| v.is_finite()) {
        Some(v) => format!("{} LUFS", format::fixed(v, 1)),
        None => NO_VALUE.to_owned(),
    }
}

/// What a bar of the pane reads.
#[derive(Clone, Debug, PartialEq)]
pub enum ProgrammeBar {
    /// One input's peak bar: every peak ballistic, and an input without a source under a
    /// loudness one, with the reason in `note`. Under a peak ballistic `note` is the VU
    /// fallback where it applies.
    Peak {
        input: ProgrammeInput,
        note: Option<String>,
    },
    /// A loudness source's bar.
    Source(SourceBar),
}

/// The words before a sourceless input's reason: what the bar reads comes first, so a cut
/// to the bar's width loses the reason, never the reading.
pub const TRUE_PEAK_SHOWN: &str = "true peak shown";

/// A bar fill `0 … 1` over the EBU +9 span: 0 at `target` −18 LU, 1 at `target` +9 LU,
/// clamped; −∞ or none is empty.
pub fn lufs_fill(reading: Option<f64>, target: f64) -> f32 {
    match reading {
        Some(v) if v.is_finite() => ((v - (target + LUFS_LOW)) / LUFS_SPAN).clamp(0.0, 1.0) as f32,
        _ => 0.0,
    }
}

/// The LUFS span's bottom below the target, LU (EBU +9 scale).
pub const LUFS_LOW: f64 = -18.0;
/// The span, LU: target −18 … target +9.
pub const LUFS_SPAN: f64 = 27.0;

/// Whether `v` lies on the LUFS span (a max outside it draws no tick: clamped, it would
/// read as the span's edge).
pub fn on_lufs_span(v: f64, target: f64) -> bool {
    (target + LUFS_LOW..=target + LUFS_LOW + LUFS_SPAN).contains(&v)
}

/// One peak bar per input, notes as `ballistic` gives them: the pane's bars with no levels
/// frame to resolve sources from.
pub fn peak_bars(inputs: &[ProgrammeInput], ballistic: Ballistic) -> Vec<ProgrammeBar> {
    inputs
        .iter()
        .map(|i| ProgrammeBar::Peak {
            input: i.clone(),
            note: i.note(ballistic).map(str::to_owned),
        })
        .collect()
}

/// The bars of a pane whose inputs resolved to `resolved`: under a peak ballistic one per
/// input (a pair pane keeps its L and R). Under a loudness one, the true-peak block (a bar
/// per input the pane names, in its order, a sourceless one with its reason) and then the
/// LUFS block (a bar per distinct source, in the order the pane first names one of its
/// inputs): the inputs read their true peak, which each one has, and loudness belongs to a
/// source, never to one side of it.
pub fn bars_of(
    resolved: &[Resolved],
    ballistic: Ballistic,
    f: &LoudnessFacts<'_>,
) -> Vec<ProgrammeBar> {
    let input = |c: u16| (f.input)(c).unwrap_or_else(|| empty_input((f.label)(c)));
    if !ballistic.is_loudness() {
        let inputs: Vec<ProgrammeInput> = resolved.iter().map(|r| input(r.input())).collect();
        return peak_bars(&inputs, ballistic);
    }
    let mut bars: Vec<ProgrammeBar> = resolved
        .iter()
        .map(|r| ProgrammeBar::Peak {
            input: input(r.input()),
            note: match r {
                Resolved::Reason { reason, .. } => Some(format!("{TRUE_PEAK_SHOWN} · {reason}")),
                Resolved::Source { .. } => None,
            },
        })
        .collect();
    let mut seen: Vec<LoudnessSource> = Vec::new();
    for source in resolved.iter().filter_map(Resolved::source) {
        if !seen.contains(&source) {
            seen.push(source);
            bars.push(ProgrammeBar::Source(source_bar(source, ballistic, f)));
        }
    }
    bars
}

fn source_bar(source: LoudnessSource, ballistic: Ballistic, f: &LoudnessFacts<'_>) -> SourceBar {
    let finite = |v: f64| Some(v).filter(|v| v.is_finite());
    let (reading, max) = match f.sources.iter().find(|l| l.source == source) {
        Some(l) if ballistic == Ballistic::Momentary => {
            (finite(l.momentary.0), finite(l.max_momentary.0))
        }
        Some(l) => (finite(l.short_term.0), finite(l.max_short_term.0)),
        None => (None, None),
    };
    SourceBar {
        name: source_name(source, f),
        reading,
        max,
    }
}

/// An input the facts know no readings of: its bar is there, empty.
fn empty_input(label: String) -> ProgrammeInput {
    ProgrammeInput {
        label,
        scale: Scale::Dbfs,
        ppm: None,
        true_peak_dbtp: None,
        max: None,
        overs: 0,
        clipped: false,
        flying_peak: None,
        offers_vu: false,
    }
}
