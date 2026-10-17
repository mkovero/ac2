//! Spectrum and RTA view: bars or lines per band / bin, peak-hold overlay, unit on the axis.
//!
//! Units (decision 4b): a narrowband spectrum shows **tone level** (a sine reads its RMS
//! level regardless of FFT length), an RTA shows **band power**; the axis says which, so
//! the two are never read against each other by mistake. A smoothed narrowband spectrum is
//! a fractional-octave power average of tone levels — a sine reads lower by however many
//! bins the kernel spreads it over — so its axis says that too.

use ac2_proto::frame::{RtaFrame, SpecBands, SpecFrame, ValidityMask};
use ac2_proto::model::{
    BandFraction, CalStatus, LevelScale, SmoothingFraction, SpecLevel, Weighting, Window,
};
use ac2_proto::units::WallNs;

use crate::axis::{self, Axis};
use crate::banner::{BannerRow, Status};
use crate::canvas::{
    self, Canvas, MARGINS, anchor, cut_to, gapped, label, text_width, visible_columns,
};
use crate::format;
use crate::grid::column_at;
use crate::primitives::{
    Color, Dash, FillRect, HAlign, Polyline, Rect, Scene, Stroke, VAlign, Viewport,
};
use crate::tf::LegendEntry;
use crate::theme::Theme;
use crate::time::Freshness;
use crate::trace::TraceKey;
use crate::view::{SpectrumStyle, ViewState};

mod draw;
mod legend;
mod units;
pub use draw::spectrum_scene;
pub(crate) use draw::spectrum_scene_in;
pub use legend::caption_forms;
use legend::{
    LEGEND_MAX_ROWS, LEGEND_ROW, LEGEND_SHARE, SWATCH_GAP, SWATCH_W, entry_width, legend_entry,
    legend_layout, unit_forms,
};
pub use units::{
    BinWidth, cal_caption, density_unit_help, fraction_label, level_unit, level_unit_forms,
    narrow_unit_help, tone_unit_help, value_unit, weighting_label, window_label,
};

/// What a level value means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quantity {
    /// Level of a sinusoid in the bin (narrowband spectrum).
    Tone,
    /// Tone levels power-averaged over a fractional-octave kernel (smoothed spectrum).
    SmoothedTone(SmoothingFraction),
    /// Power spectral density (narrowband spectrum, per hertz).
    Density,
    /// Densities averaged over a fractional-octave kernel (smoothed spectrum).
    SmoothedDensity(SmoothingFraction),
    /// Power in the band (RTA).
    Band,
}

impl Quantity {
    /// A narrowband spectrum's quantity at `level` under `smoothing`.
    pub fn narrow(level: SpecLevel, smoothing: Option<SmoothingFraction>) -> Self {
        match (level, smoothing) {
            (SpecLevel::Tone, None) => Quantity::Tone,
            (SpecLevel::Tone, Some(f)) => Quantity::SmoothedTone(f),
            (SpecLevel::Density, None) => Quantity::Density,
            (SpecLevel::Density, Some(f)) => Quantity::SmoothedDensity(f),
        }
    }

    /// A narrowband spectrum (tone level or density), not RTA bands.
    pub fn is_narrow(self) -> bool {
        self != Quantity::Band
    }

    /// A density, smoothed or not.
    pub fn is_density(self) -> bool {
        matches!(self, Quantity::Density | Quantity::SmoothedDensity(_))
    }
}

/// One spectrum or RTA trace.
#[derive(Clone, Debug)]
pub struct SpectrumTrace<'a> {
    pub key: TraceKey,
    pub name: String,
    pub color: Color,
    pub freqs: &'a [f64],
    /// Band edges per column ([`crate::grid::column_edges`]), for bars.
    pub edges: &'a [(f64, f64)],
    pub level: &'a [f32],
    /// The levels the curve is drawn at while it glides to `level` ([`crate::curve_ease`]);
    /// `None`: drawn at `level`. Cursor readouts and peak hold read `level`, the frame itself.
    pub drawn: Option<&'a [f32]>,
    pub validity: Option<&'a [ValidityMask]>,
    /// Peak-hold values, column-aligned ([`PeakHold`]).
    pub peak: Option<&'a [f32]>,
    pub scale: LevelScale,
    pub quantity: Quantity,
    /// FFT bin spacing of a narrowband trace's grid ([`crate::grid::bin_spacing`]): its
    /// levels are per bin, and the axis names the width.
    pub bin_hz: Option<f64>,
    /// `1/3 oct · A-weighted`, `Hann window`.
    pub caption: String,
    pub freshness: Option<Freshness>,
    /// Display offset, dB, added to the level and the peak hold: traces spread apart to
    /// compare their shapes. The legend and the cursor name every offset trace, so a
    /// spread is never read as a level difference.
    pub offset_db: f64,
    /// The selected stored trace: marked in the legend and drawn with a thicker line.
    pub selected: bool,
    /// A live multi-resolution spectrum's bands: where its windows change, and which
    /// columns resolve less than their grid.
    pub bands: Option<&'a SpecBands>,
}

/// ` · stopped` after a stopped measurement's caption: its curve is the final result.
fn stopped_caption(f: Freshness) -> &'static str {
    if f.is_stopped() { " · stopped" } else { "" }
}

/// What the spectrum plot writes for a trace drawn with a display offset:
/// `Main L S1 · offset +3.0 dB`.
pub fn offset_note(name: &str, offset_db: f64) -> String {
    format!("{name} · offset {}", format::db_readout(offset_db))
}

impl<'a> SpectrumTrace<'a> {
    /// A live RTA trace; `captured` is the frame's capture time (for the calibration age),
    /// `curve` the mic-curve note of its input.
    #[allow(clippy::too_many_arguments)]
    pub fn rta(
        frame: &'a RtaFrame,
        captured: WallNs,
        curve: Option<&str>,
        freqs: &'a [f64],
        edges: &'a [(f64, f64)],
        name: impl Into<String>,
        color: Color,
        freshness: Freshness,
    ) -> Self {
        Self {
            key: TraceKey::Live(frame.meas),
            name: name.into(),
            color,
            freqs,
            edges,
            level: &frame.level,
            drawn: None,
            validity: Some(&frame.validity),
            peak: None,
            scale: frame.meta.scale,
            quantity: Quantity::Band,
            bin_hz: None,
            caption: format!(
                "{} · {}{}{}",
                fraction_label(frame.meta.fraction),
                weighting_label(frame.meta.weighting),
                cal_caption(frame.meta.cal, curve, captured),
                stopped_caption(freshness)
            ),
            freshness: Some(freshness),
            offset_db: 0.0,
            selected: false,
            bands: None,
        }
    }

    /// A live narrowband spectrum; `captured` is the frame's capture time, `curve` the
    /// mic-curve note of its input. Its columns are display columns of FFT bins, each the
    /// highest tone level among its bins (the mean density, a density); a gap is NaN.
    #[allow(clippy::too_many_arguments)]
    pub fn spectrum(
        frame: &'a SpecFrame,
        captured: WallNs,
        curve: Option<&str>,
        freqs: &'a [f64],
        edges: &'a [(f64, f64)],
        name: impl Into<String>,
        color: Color,
        freshness: Freshness,
    ) -> Self {
        Self {
            key: TraceKey::Live(frame.meas),
            name: name.into(),
            color,
            freqs,
            edges,
            level: &frame.level,
            drawn: None,
            validity: None,
            peak: None,
            scale: frame.meta.scale,
            quantity: Quantity::narrow(frame.meta.level, frame.meta.smoothing),
            bin_hz: None,
            // The axis unit says whether the level is smoothed (`level_unit`).
            caption: format!(
                "{}{}{}{}",
                window_label(frame.meta.window),
                frame.meta.bands.as_ref().map_or(String::new(), |b| format!(
                    " · {}",
                    format::spec_resolution(ac2_proto::model::SpecResolution::Multi {
                        grid: b.unresolved.resolution
                    })
                )),
                cal_caption(frame.meta.cal, curve, captured),
                stopped_caption(freshness)
            ),
            freshness: Some(freshness),
            offset_db: 0.0,
            selected: false,
            bands: frame.meta.bands.as_deref(),
        }
    }

    fn value(&self, i: usize) -> f64 {
        let ok = self
            .validity
            .is_none_or(|v| v.get(i).is_some_and(|m| *m == ValidityMask::NONE));
        match self.level.get(i) {
            Some(l) if ok && l.is_finite() => f64::from(*l) + self.offset_db,
            _ => f64::NAN,
        }
    }

    /// The level the curve is drawn at in column `i`: [`Self::value`], or where its glide
    /// to it has got to.
    fn drawn_value(&self, i: usize) -> f64 {
        let Some(drawn) = self.drawn else {
            return self.value(i);
        };
        let ok = self
            .validity
            .is_none_or(|v| v.get(i).is_some_and(|m| *m == ValidityMask::NONE));
        match drawn.get(i) {
            Some(l) if ok && l.is_finite() => f64::from(*l) + self.offset_db,
            _ => f64::NAN,
        }
    }

    /// Peak-hold value at column `i`, with the display offset.
    fn peak_value(&self, i: usize) -> Option<f64> {
        let p = self.peak?.get(i)?;
        Some(f64::from(*p) + self.offset_db)
    }

    fn is_stale(&self) -> bool {
        self.freshness.is_some_and(|f| f.is_stale())
    }
}

/// Peak hold with optional decay, kept by the UI between frames. Display state like a
/// zoom, not DSP: it only remembers what was shown.
#[derive(Clone, Debug, PartialEq)]
pub struct PeakHold {
    /// dB per second; 0 = hold until reset.
    pub decay_db_per_s: f32,
    values: Vec<f32>,
}

impl PeakHold {
    pub fn new(decay_db_per_s: f32) -> Self {
        Self {
            decay_db_per_s,
            values: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.values.clear();
    }

    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// Folds in a frame `dt_s` seconds after the previous one. A column count change
    /// (new grid) restarts the hold; an invalid column only decays.
    pub fn update(&mut self, level: &[f32], validity: Option<&[ValidityMask]>, dt_s: f32) {
        if self.values.len() != level.len() {
            self.values = vec![f32::NAN; level.len()];
        }
        let decay = self.decay_db_per_s * dt_s.max(0.0);
        for (i, p) in self.values.iter_mut().enumerate() {
            let ok = validity.is_none_or(|v| v.get(i).is_some_and(|m| *m == ValidityMask::NONE));
            let l = if ok { level[i] } else { f32::NAN };
            let decayed = *p - decay;
            *p = match (decayed.is_finite(), l.is_finite()) {
                (true, true) => decayed.max(l),
                (true, false) => decayed,
                (false, true) => l,
                (false, false) => f32::NAN,
            };
        }
    }
}

/// Thins a line to one point per pixel column (`xs` ascending, `ys` in pixels, NaN = gap):
/// the highest point among the columns falling in that pixel, at its own x. A narrowband
/// spectrum has far more bins than pixels; drawing every bin wastes vertices, and averaging
/// or picking any bin would let a single-bin tone fall out of the picture. A pixel whose
/// columns are all gaps stays a gap; a gap narrower than a pixel next to valid columns is
/// not visible and is dropped.
pub fn max_per_pixel(xs: &[f32], ys: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let n = xs.len().min(ys.len());
    let mut ox = Vec::with_capacity(n.min(4096));
    let mut oy = Vec::with_capacity(n.min(4096));
    let mut i = 0;
    while i < n {
        let px = xs[i].floor();
        let mut best: Option<usize> = None;
        let mut j = i;
        while j < n && (j == i || xs[j].floor() == px) {
            if ys[j].is_finite() && best.is_none_or(|b| ys[j] < ys[b]) {
                best = Some(j);
            }
            j += 1;
        }
        match best {
            Some(b) => {
                ox.push(xs[b]);
                oy.push(ys[b]);
            }
            None => {
                ox.push(xs[i]);
                oy.push(f32::NAN);
            }
        }
        i = j;
    }
    (ox, oy)
}

/// Cursor values on the spectrum.
#[derive(Clone, Debug, PartialEq)]
pub struct SpectrumCursor {
    pub freq_hz: f64,
    pub freq: String,
    /// `(trace name, "−23.5 dBFS")`.
    pub rows: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpectrumScene {
    pub scene: Scene,
    pub plot: Rect,
    pub x_axis: Axis,
    pub y_axis: Axis,
    /// Axis unit as drawn ([`level_unit_forms`]: the longest that fits).
    pub unit: String,
    /// Where the unit is drawn, and what it means ([`tone_unit_help`]) for a narrowband
    /// spectrum, for a tooltip over it.
    pub unit_rect: Rect,
    pub unit_help: Option<String>,
    /// The caption as drawn ([`caption_forms`]: the longest that leaves the unit room).
    pub caption: String,
    pub cursor: Option<SpectrumCursor>,
    /// One entry per curve, in drawing order: name, colour and tags (`stopped`,
    /// `STALE 3.2 s`, `offset +3.0 dB`). Drawn in rows above the plot, never over a curve.
    pub legend: Vec<LegendEntry>,
    /// The legend as drawn: entry texts, shortened to fit (`+2 more` when entries do not).
    pub legend_shown: Vec<String>,
    /// The band the legend takes above the plot; zero height without one.
    pub legend_rect: Rect,
    /// Where each drawn legend entry is, for a click to select its curve.
    pub legend_hits: Vec<crate::legend::LegendHit>,
    /// Banner strip above the plot; zero height when no banner is up.
    pub strip: Rect,
    pub banners: Vec<BannerRow>,
    /// Where the marked multi-resolution spectrum's windows change (the selected curve's,
    /// else the first's that has bands); empty when the marker is off.
    pub stitches: Vec<crate::stitch::Stitch>,
    /// Its columns finer than the window behind them resolves, shaded.
    pub unresolved: Vec<crate::unresolved::Mark>,
}

/// The bands of the selected curve that has them, else of the first that does; `None`
/// when the valid-resolution marker is off.
pub(crate) fn marked_bands<'a>(
    traces: &[SpectrumTrace<'a>],
    view: &ViewState,
) -> Option<(&'a SpecBands, f64)> {
    if !view.marks() {
        return None;
    }
    let with = || traces.iter().filter(|t| t.bands.is_some());
    let t = with().find(|t| t.selected).or_else(|| with().next())?;
    Some((t.bands?, t.freqs.first().copied().unwrap_or(0.0)))
}

#[cfg(test)]
mod tests;
