//! An alignment of two systems in their overlap, as the app and the CLI say it
//! (`docs/design/alignment.md`): which one to delay and by how much, the polarity, how well
//! it fits, how fully the two then add; the band prefilled from where both curves are
//! alike; and the alignment view ([`view`]). The daemon fits; this only words and draws.

use ac2_proto::model::{
    AlignBand, AlignCandidate, AlignFit, AlignNoFit, AlignOutcome, AlignSide, AlignState,
};

use crate::format;
use crate::readout::delay_readout;

pub mod view;

/// Between the two systems' names in an alignment's expression: `Sub ↔ Main`.
pub const SYMBOL: &str = "↔";

/// How far apart in level two curves may be and still overlap, dB: within 6 dB a sum can
/// gain or lose more than 3.5 dB by phase, so their alignment matters there.
pub const OVERLAP_DB: f64 = 6.0;
/// How far below its own peak a curve still counts as playing, dB.
pub const PLAYING_DB: f64 = 20.0;

/// `60–120 Hz`; `800 Hz–1.80 kHz` across the kilohertz.
pub fn band_text(b: AlignBand) -> String {
    if b.hi.0 < 1000.0 {
        format!(
            "{}–{} Hz",
            format::freq_tick(b.lo.0),
            format::freq_tick(b.hi.0)
        )
    } else {
        format!(
            "{}–{}",
            format::freq_readout(b.lo.0),
            format::freq_readout(b.hi.0)
        )
    }
}

/// The name of the one `c` delays.
pub fn delayed<'a>(c: &AlignCandidate, a: &'a str, b: &'a str) -> &'a str {
    match c.delayed {
        AlignSide::A => a,
        AlignSide::B => b,
    }
}

/// `Delay Main by 4.21 ms · 1.45 m @ 20 °C · polarity normal`; `… · invert Main`.
pub fn suggestion(c: &AlignCandidate, a: &str, b: &str, temp_c: f64) -> String {
    let who = delayed(c, a, b);
    let polarity = if c.inverted {
        format!("invert {who}")
    } else {
        "polarity normal".into()
    };
    format!(
        "Delay {who} by {} · {polarity}",
        delay_readout(c.delay.0, temp_c)
    )
}

/// `Main +9.80 ms inverted, ±41°`; `Sub +1.20 ms, ±14°`.
pub fn short(c: &AlignCandidate, a: &str, b: &str) -> String {
    format!(
        "{} +{}{}, ±{}°",
        delayed(c, a, b),
        format::delay(c.delay.0),
        if c.inverted { " inverted" } else { "" },
        format::fixed(c.residual_deg, 0)
    )
}

/// The aligned sum's legend note: `aligned: Main +4.21 ms`, `aligned: Main +0.00 ms
/// inverted`.
pub fn legend_note(fit: &AlignFit, a: &str, b: &str) -> String {
    let c = &fit.best;
    format!(
        "aligned: {} +{}{}",
        delayed(c, a, b),
        format::delay(c.delay.0),
        if c.inverted { " inverted" } else { "" }
    )
}

/// Why there is no fit, in the band's words.
pub fn no_fit(why: AlignNoFit, band: AlignBand) -> String {
    let b = band_text(band);
    match why {
        AlignNoFit::FewOperands => "waiting for both curves (see the banner)".into(),
        AlignNoFit::FewColumns { found } => format!(
            "{found} {} with both curves in {b}: widen the band, or check both reach it",
            if found == 1 { "column" } else { "columns" }
        ),
        AlignNoFit::NoWeight => format!("no coherent signal from both curves in {b}"),
    }
}

/// The readout plate: a headline (what to do, or `No fit`) and the lines under it.
#[derive(Clone, Debug, PartialEq)]
pub struct Readout {
    pub headline: String,
    pub lines: Vec<String>,
    /// A fit was found (the headline says what to do).
    pub fit: bool,
}

/// The readout of `state` for systems `a` and `b` aligned in `band`.
pub fn readout(state: &AlignState, a: &str, b: &str, band: AlignBand, temp_c: f64) -> Readout {
    match &state.outcome {
        AlignOutcome::NoFit { why } => Readout {
            headline: "No fit".into(),
            lines: vec![no_fit(*why, band)],
            fit: false,
        },
        AlignOutcome::Fit { fit } => {
            let mut lines = vec![format!(
                "residual ±{}° over {} · summation {} → {} (worst {} → {})",
                format::fixed(fit.best.residual_deg, 0),
                band_text(band),
                format::db_readout(fit.before.mean_db),
                format::db_readout(fit.after.mean_db),
                format::db_readout(fit.before.worst_db),
                format::db_readout(fit.after.worst_db),
            )];
            if let Some(o) = &fit.other_polarity {
                lines.push(format!("other polarity: {}", short(o, a, b)));
            }
            if let Some(r) = &fit.runner_up {
                lines.push(format!(
                    "also fits: {} — the band is too narrow to tell: pick by distance",
                    short(r, a, b)
                ));
            }
            Readout {
                headline: suggestion(&fit.best, a, b, temp_c),
                lines,
                fit: true,
            }
        }
    }
}

/// `v` to two significant digits: a prefilled band edge reads as a round number.
fn round2(v: f64) -> f64 {
    let mag = v.abs().log10().floor() as i32;
    let scale = 10f64.powi(1 - mag);
    (v * scale).round() / scale
}

/// Where two curves overlap: the widest run of columns where both play (within
/// [`PLAYING_DB`] of their own peak) and are within [`OVERLAP_DB`] of each other, edges to
/// two significant digits. `None` without three such columns in a row. The dialog
/// prefills the band with it; the user names the band.
pub fn overlap(freqs: &[f64], a_db: &[f32], b_db: &[f32]) -> Option<(f64, f64)> {
    let peak = |v: &[f32]| {
        v.iter()
            .copied()
            .filter(|x| x.is_finite())
            .fold(f32::NEG_INFINITY, f32::max)
    };
    let (pa, pb) = (peak(a_db), peak(b_db));
    if !(pa.is_finite() && pb.is_finite()) {
        return None;
    }
    let ok = |i: usize| {
        let (x, y) = (a_db[i], b_db[i]);
        x.is_finite()
            && y.is_finite()
            && f64::from(pa - x) <= PLAYING_DB
            && f64::from(pb - y) <= PLAYING_DB
            && f64::from((x - y).abs()) <= OVERLAP_DB
    };
    let n = freqs.len().min(a_db.len()).min(b_db.len());
    let mut best: Option<(usize, usize)> = None;
    let mut start = None;
    for i in 0..=n {
        if i < n && ok(i) {
            start.get_or_insert(i);
            continue;
        }
        if let Some(s) = start.take()
            && best.is_none_or(|(bs, be)| i - s > be - bs)
        {
            best = Some((s, i));
        }
    }
    let (s, e) = best.filter(|(s, e)| e - s >= 3)?;
    let (lo, hi) = (round2(freqs[s]), round2(freqs[e - 1]));
    (lo < hi).then_some((lo, hi))
}

#[cfg(test)]
mod tests;
