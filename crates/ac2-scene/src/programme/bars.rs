//! What a Programme pane's bars read (docs/design/programme-meter.md §6.2 *What a bar
//! reads*): under a peak ballistic one bar per input; under a loudness ballistic one bar
//! per loudness source, a pair's being both channels summed (BS.1770 defines loudness of a
//! programme, one figure for all its channels: a side K-weighted alone is not the loudness
//! of anything delivered), and the true peak, with its reason, for an input without one.

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

/// One side of a source bar's flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SideFlags {
    pub overs: u32,
    pub clipped: bool,
}

/// A loudness source's bar.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceBar {
    /// `Main`, `Vox mono`.
    pub name: String,
    /// LUFS of the ballistic's window; `None` for −∞ (no window wholly after the start).
    pub reading: Option<f64>,
    /// The window's max since the reset; `None` before one.
    pub max: Option<f64>,
    /// dBTP, the higher of the source's inputs this frame.
    pub true_peak: Option<f64>,
    /// Per input, left first.
    pub sides: Vec<SideFlags>,
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

    /// `−1.2 dBTP` (the `TP` word sits apart), `—`.
    pub fn true_peak_value_text(&self) -> String {
        match self.true_peak.filter(|v| v.is_finite()) {
            Some(v) => format!("{} dBTP", format::signed(v, 1)),
            None => NO_VALUE.to_owned(),
        }
    }

    /// Whether any side holds samples at full scale.
    pub fn clipped(&self) -> bool {
        self.sides.iter().any(|s| s.clipped)
    }

    /// A pair's flags named by side, left first, the overs before CLIP: `1 over L`,
    /// `2 overs L · 1 over R`, `2 overs L · CLIP L · CLIP R`; counts past 999 read `999+`.
    /// A mono source's are its input's own (`2 overs`, with CLIP apart).
    pub fn flags_text(&self) -> String {
        let count = |n: u32| match n {
            1 => "1 over".to_owned(),
            2..=999 => format!("{n} overs"),
            _ => "999+ overs".to_owned(),
        };
        if let [one] = self.sides.as_slice() {
            return if one.overs == 0 {
                String::new()
            } else {
                count(one.overs)
            };
        }
        let side = |k: usize| if k == 0 { "L" } else { "R" };
        let overs = self
            .sides
            .iter()
            .enumerate()
            .filter(|(_, s)| s.overs > 0)
            .map(|(k, s)| format!("{} {}", count(s.overs), side(k)));
        let clips = self
            .sides
            .iter()
            .enumerate()
            .filter(|(_, s)| s.clipped)
            .map(|(k, _)| format!("CLIP {}", side(k)));
        overs.chain(clips).collect::<Vec<_>>().join(" · ")
    }
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
/// input (a pair pane keeps its L and R); under a loudness one, a bar per distinct source
/// at its first input and the true peak with its reason for an input without one.
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
    let mut seen: Vec<LoudnessSource> = Vec::new();
    let mut bars = Vec::new();
    for r in resolved {
        match r {
            Resolved::Reason { input: c, reason } => bars.push(ProgrammeBar::Peak {
                input: input(*c),
                note: Some(format!("{TRUE_PEAK_SHOWN} · {reason}")),
            }),
            Resolved::Source { source, .. } if seen.contains(source) => {}
            Resolved::Source { source, .. } => {
                seen.push(*source);
                bars.push(ProgrammeBar::Source(source_bar(*source, ballistic, f)));
            }
        }
    }
    bars
}

fn source_bar(source: LoudnessSource, ballistic: Ballistic, f: &LoudnessFacts<'_>) -> SourceBar {
    let sides: Vec<Option<ProgrammeInput>> = match source {
        LoudnessSource::Pair { left, right } => vec![(f.input)(left), (f.input)(right)],
        LoudnessSource::Input { channel } => vec![(f.input)(channel)],
    };
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
        true_peak: sides
            .iter()
            .flatten()
            .filter_map(|i| i.true_peak_dbtp.filter(|v| v.is_finite()))
            .map(f64::from)
            .reduce(f64::max),
        sides: sides
            .iter()
            .map(|i| {
                i.as_ref().map_or_else(SideFlags::default, |i| SideFlags {
                    overs: i.overs,
                    clipped: i.clipped,
                })
            })
            .collect(),
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
