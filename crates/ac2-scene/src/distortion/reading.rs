//! What a distortion curve reads at a column, its texts, the sweep summary and the legend
//! entries: the values the view, its cursor and the CLI print.

use super::*;

/// What a distortion curve says at one column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reading {
    /// Not measured there (outside the band this order can be measured in).
    NotMeasured,
    /// Within the noise: below this floor (dB re fundamental).
    BelowFloor(f64),
    /// Distortion, dB re fundamental.
    Level(f64),
}

/// The reading of `c` at column `i` under the analysis' margin `margin_db`.
pub fn reading(c: &DistortionCurve, i: usize, margin_db: f64) -> Reading {
    let level = c.level_db.get(i).map_or(f64::NAN, |v| f64::from(*v));
    let floor = c.floor_db.get(i).map_or(f64::NAN, |v| f64::from(*v));
    if !level.is_finite() {
        Reading::NotMeasured
    } else if c.valid(i, ac2_proto::units::Db(margin_db)) {
        Reading::Level(level)
    } else {
        Reading::BelowFloor(floor)
    }
}

/// dB re fundamental as percent.
pub fn percent(db: f64) -> f64 {
    100.0 * 10f64.powf(db / 20.0)
}

/// Percent of the fundamental as dB re it (the inverse of [`percent`]); NaN for a value
/// that is not a positive ratio.
pub fn db_of_percent(p: f64) -> f64 {
    if p > 0.0 {
        20.0 * (p / 100.0).log10()
    } else {
        f64::NAN
    }
}

/// `1.00 %`, `0.0316 %`: three significant digits.
pub fn percent_text(db: f64) -> String {
    let p = percent(db);
    if !p.is_finite() || p <= 0.0 {
        return format::NO_VALUE.to_string();
    }
    let decimals = (2 - p.log10().floor() as i32).clamp(0, 6) as usize;
    format!("{} %", format::fixed(p, decimals))
}

/// `−40.1 dB`.
pub fn db_text(db: f64) -> String {
    if db.is_finite() {
        format!("{} dB", format::fixed(db, 1))
    } else {
        format::NO_VALUE.to_string()
    }
}

/// A value in `unit`.
pub fn value_text(db: f64, unit: DistortionUnit) -> String {
    match unit {
        DistortionUnit::Db => db_text(db),
        DistortionUnit::Percent => percent_text(db),
    }
}

/// `−40.1 dB`, `< −72.0 dB` (within the noise), `—` (not measured).
pub fn reading_text(r: Reading, unit: DistortionUnit) -> String {
    match r {
        Reading::NotMeasured => format::NO_VALUE.to_string(),
        Reading::BelowFloor(f) if f.is_finite() => format!("< {}", value_text(f, unit)),
        Reading::BelowFloor(_) => "< floor".to_string(),
        Reading::Level(v) => value_text(v, unit),
    }
}

/// The highest valid point of an order: (frequency, dB re fundamental).
pub fn peak(c: &DistortionCurve, freqs: &[f64], margin_db: f64) -> Option<(f64, f64)> {
    (0..freqs.len())
        .filter_map(|i| match reading(c, i, margin_db) {
            Reading::Level(v) => Some((freqs[i], v)),
            _ => None,
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

/// The reading at `hz`: the nearest column within 1/12 octave, else not measured.
pub fn reading_at(c: &DistortionCurve, freqs: &[f64], hz: f64, margin_db: f64) -> Reading {
    match nearest_column(freqs, hz) {
        Some(i) if (freqs[i] / hz).log2().abs() <= 1.0 / 12.0 => reading(c, i, margin_db),
        _ => Reading::NotMeasured,
    }
}

/// A measured harmonic less than this far above the reference input's own is flagged, dB.
/// Where the loopback shares the measured path's converters, their distortion is in both:
/// 6 dB above it, the converters' share adds at most 1 dB to the measured level (power sum,
/// 10·log10(1 + 10^−0.6)); closer, the system's own harmonic cannot be told from theirs.
pub const REFERENCE_BOUND_DB: f64 = 6.0;

/// Where one order of the measurement reads within [`REFERENCE_BOUND_DB`] of the reference's
/// own valid level.
#[derive(Clone, Debug, PartialEq)]
pub struct NearReference {
    pub order: u8,
    /// Columns flagged.
    pub columns: usize,
    /// Lowest and highest flagged fundamental, Hz.
    pub span_hz: (f64, f64),
}

/// Short summary of a sweep: THD at a few frequencies, each order's highest point.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    /// `(frequency, THD reading)`.
    pub thd: Vec<(f64, Reading)>,
    /// `(order, highest valid point (frequency, dB))`.
    pub peaks: Vec<(u8, Option<(f64, f64)>)>,
    /// The reference input's own orders, as [`Summary::peaks`]; empty when the sweep has
    /// none.
    pub reference_peaks: Vec<(u8, Option<(f64, f64)>)>,
    /// Orders that read within [`REFERENCE_BOUND_DB`] of the reference's own somewhere.
    pub near_reference: Vec<NearReference>,
}

/// THD at 100 Hz, 1 kHz and 10 kHz and every order's highest point, the reference's own,
/// and where the measurement comes near those.
pub fn summary(s: &SweepData, freqs: &[f64]) -> Summary {
    let m = s.info.floor_margin.0;
    let reference = s.reference_harmonics.as_deref().unwrap_or_default();
    Summary {
        thd: [100.0, 1000.0, 10_000.0]
            .into_iter()
            .map(|f| (f, reading_at(&s.thd, freqs, f, m)))
            .collect(),
        peaks: s
            .harmonics
            .iter()
            .map(|h| (h.order, peak(&h.curve, freqs, m)))
            .collect(),
        reference_peaks: reference
            .iter()
            .map(|h| (h.order, peak(&h.curve, freqs, m)))
            .collect(),
        near_reference: s
            .harmonics
            .iter()
            .filter_map(|h| {
                let r = reference.iter().find(|r| r.order == h.order)?;
                let near: Vec<f64> = (0..freqs.len())
                    .filter(
                        |&i| match (reading(&h.curve, i, m), reading(&r.curve, i, m)) {
                            (Reading::Level(v), Reading::Level(rv)) => v < rv + REFERENCE_BOUND_DB,
                            _ => false,
                        },
                    )
                    .map(|i| freqs[i])
                    .collect();
                Some(NearReference {
                    order: h.order,
                    columns: near.len(),
                    span_hz: (*near.first()?, *near.last()?),
                })
            })
            .collect(),
    }
}

/// `H2`, …, `THD`.
pub fn order_name(order: u8) -> String {
    format!("H{order}")
}

/// `reference H2`: the reference input's own order.
pub fn reference_order_name(order: u8) -> String {
    format!("reference {}", order_name(order))
}

/// `H2 within 6 dB of reference H2 at 12 points, 100 Hz – 1.00 kHz`.
pub fn near_reference_text(n: &NearReference) -> String {
    let (lo, hi) = n.span_hz;
    let span = if lo == hi {
        format!("at {}", format::freq_readout(lo))
    } else {
        format!(
            "at {} points, {} – {}",
            n.columns,
            format::freq_readout(lo),
            format::freq_readout(hi)
        )
    };
    format!(
        "{} within {} dB of {} {span}",
        order_name(n.order),
        format::fixed(REFERENCE_BOUND_DB, 0),
        reference_order_name(n.order)
    )
}

/// One sweep trace to draw.
#[derive(Clone, Copy, Debug)]
pub struct SweepView<'a> {
    pub data: &'a TraceData,
    /// Column frequencies of its grid.
    pub freqs: &'a [f64],
    /// The colour its fundamental is drawn in ([`crate::families`]).
    pub color: Color,
}

/// How a legend entry shows what it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegendMark {
    /// The name in the curve's colour.
    Name,
    /// A dashed sample: an order within the noise, drawn at its floor in its own colour.
    Dashed,
    /// A shaded sample: under the lowest order's floor.
    Shade,
    /// A dotted sample: the reference input's own orders, each in its order's colour.
    Dotted,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LegendEntry {
    pub name: String,
    pub color: Color,
    pub mark: LegendMark,
}

/// Legend name of the dashed floor lines.
pub const BELOW_FLOOR: &str = "< floor";
/// Legend name of the shading under every order's floor.
pub const NOISE: &str = "noise";
/// Legend name of the dotted lines of the reference input's own orders: each is
/// [`reference_order_name`] in the cursor readout and the notes.
pub const REFERENCE: &str = "reference";

/// What showing or hiding the reference input's own orders says: `reference harmonics
/// hidden`.
pub fn reference_toast(shown: bool) -> String {
    let state = if shown { "shown" } else { "hidden" };
    format!("{REFERENCE} harmonics {state}")
}
