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
use crate::grid::nearest_column;
use crate::primitives::{
    Color, Dash, FillRect, HAlign, Polyline, Rect, Scene, Stroke, VAlign, Viewport,
};
use crate::tf::LegendEntry;
use crate::theme::Theme;
use crate::time::Freshness;
use crate::trace::TraceKey;
use crate::view::{SpectrumStyle, ViewState};

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

/// The unit a value of `q` in `scale` reads in: `dBFS`, `dB SPL`, `dBFS/Hz`, `dB SPL/Hz`.
pub fn value_unit(scale: LevelScale, q: Quantity) -> &'static str {
    match (scale, q.is_density()) {
        (LevelScale::Dbfs, false) => "dBFS",
        (LevelScale::DbSpl, false) => "dB SPL",
        (LevelScale::Dbfs, true) => "dBFS/Hz",
        (LevelScale::DbSpl, true) => "dB SPL/Hz",
    }
}

/// Caption suffix for a calibrated trace (decisions 7a/7b): the calibration's age when it
/// belongs to this device + input + mic, the mismatch warning when it belongs to another mic
/// or input, and the mic-curve note ([`crate::cal::curve_note`]: which curve was
/// subtracted, or why none was). Uncalibrated says nothing: the axis unit is dBFS. The age
/// is `captured − calibrated_at`, both on the daemon clock
/// (`docs/design/q7-calibration.md` §3), so no client clock offset enters it.
pub fn cal_caption(cal: CalStatus, curve: Option<&str>, captured: WallNs) -> String {
    let mut s = String::new();
    if let CalStatus::Verified { calibrated_at, .. }
    | CalStatus::OtherMicOrInput { calibrated_at, .. } = cal
    {
        let age = captured.0.saturating_sub(calibrated_at.0) as f64 / 1e9;
        if let Some(t) = crate::cal::status_text(cal, age) {
            s.push_str(" · ");
            s.push_str(&t);
        }
    }
    if let Some(c) = curve {
        s.push_str(" · ");
        s.push_str(c);
    }
    s
}

/// What a narrowband level is "per": the FFT bin spacing of its grid.
///
/// The width named is the bin spacing `fs / N`, not the window's equivalent noise bandwidth.
/// With tone normalisation a broadband signal of density `S` reads `S · ENBW` in a bin, and
/// ENBW is 1.5 bins for Hann (1.76 dB above `S · fs / N`), 1.0 for rectangular, about 3.8
/// for flat-top: it is a property of the window, while the spacing is what the FFT length
/// sets and what every spectrum grid, live or stored, carries. The label's job is to say
/// which readings are comparable (same spacing and window: same broadband reading) and how
/// a change of FFT length moves broadband sound (3 dB per doubling, whatever the window);
/// a noise density would need the window's ENBW and is not what this axis claims.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BinWidth {
    /// Not an FFT grid (an imported spectrum on another grid).
    Unknown,
    /// Spacing, Hz.
    Hz(f64),
    /// Curves on different spacings share the axis.
    Mixed,
    /// A multi-resolution spectrum: each column's bandwidth is its window's, wider the
    /// higher the column.
    MultiRes,
}

impl BinWidth {
    pub fn of(bin_hz: Option<f64>) -> Self {
        bin_hz.map_or(BinWidth::Unknown, BinWidth::Hz)
    }
}

/// Axis unit, longest form: `dBFS per 1.46 Hz bin (tone)`, `dB SPL (band)`,
/// `dBFS per 11.7 Hz bin (tone, 1/6 oct smoothed)`, `dBFS/Hz (density, multi-res)`.
pub fn level_unit(scale: LevelScale, q: Quantity, bin: BinWidth) -> String {
    level_unit_forms(scale, q, bin).swap_remove(0)
}

/// The axis unit from its longest form to its shortest, for a pane too narrow for the full
/// one: what goes first is what the pane title or the RTA's own axis also says (`tone`,
/// the smoothing), the scale always stays.
pub fn level_unit_forms(scale: LevelScale, q: Quantity, bin: BinWidth) -> Vec<String> {
    let s = scale_unit(scale);
    let smoothed = match q {
        Quantity::SmoothedTone(f) | Quantity::SmoothedDensity(f) => {
            Some(format!("{} smoothed", format::octave_fraction(f)))
        }
        _ => None,
    };
    if q.is_density() {
        // A density is per hertz whatever the bin width: no bin to name.
        let d = value_unit(scale, q);
        let what = match &smoothed {
            Some(sm) => format!("density, {sm}"),
            None => "density".to_string(),
        };
        let mut v = match bin {
            BinWidth::MultiRes => vec![format!("{d} ({what}, multi-res)")],
            _ => vec![],
        };
        v.push(format!("{d} ({what})"));
        if smoothed.is_some() {
            v.push(format!("{d} (density)"));
        }
        v.push(d.to_string());
        v.dedup();
        return v;
    }
    let tone = match &smoothed {
        Some(sm) => format!("tone, {sm}"),
        None => "tone".to_string(),
    };
    let mut v = match (q, bin) {
        (Quantity::Band, _) => vec![format!("{s} (band)")],
        (_, BinWidth::Unknown) => vec![format!("{s} ({tone})")],
        (_, BinWidth::Hz(b)) => {
            let per = format!("{s} per {} bin", format::freq_readout(b));
            let mut v = vec![format!("{per} ({tone})")];
            if let Some(sm) = &smoothed {
                v.push(format!("{per} ({sm})"));
            }
            v.push(per);
            v.push(format!("{s}/bin"));
            v
        }
        (_, BinWidth::MultiRes) => {
            let mut v = vec![format!("{s} ({tone}, multi-res)")];
            if let Some(sm) = &smoothed {
                v.push(format!("{s} ({sm}, multi-res)"));
            }
            v.push(format!("{s} (multi-res)"));
            v
        }
        (_, BinWidth::Mixed) => {
            let per = format!("{s} per bin, mixed widths");
            vec![format!("{per} ({tone})"), per, format!("{s}/bin")]
        }
    };
    v.push(s.to_string());
    v.dedup();
    v
}

/// What the level axis of a narrowband spectrum of quantity `q` means, for its tooltip and
/// the help.
pub fn narrow_unit_help(q: Quantity, bin: BinWidth) -> String {
    if q.is_density() {
        density_unit_help(bin)
    } else {
        tone_unit_help(bin)
    }
}

/// What a density axis means.
pub fn density_unit_help(bin: BinWidth) -> String {
    let source = if bin == BinWidth::MultiRes {
        "from windows of 0.68 s at the lowest frequencies to 21 ms at the highest"
    } else {
        "per hertz"
    };
    format!(
        "Power spectral density {source}: the power each hertz holds, so noise and music \
         read the same on a fixed or a multi-resolution spectrum, at any FFT length (white \
         noise flat, pink falling 3 dB per octave). A sine reads below its level by \
         10·log10 of the bin's noise bandwidth in hertz (Hann, 1.46 Hz bins: 3.4 dB). Use \
         tone level to read a sine, an RTA for band levels in dB SPL."
    )
}

/// What the level axis of a narrowband spectrum means, for its tooltip and the help.
pub fn tone_unit_help(bin: BinWidth) -> String {
    if bin == BinWidth::MultiRes {
        return "Tone level from windows of 0.68 s at the lowest frequencies to 21 ms at the \
                highest: a sine reads its RMS level at any frequency. Each column's \
                bandwidth grows with its frequency, so broadband sound reads as in a \
                constant fraction of an octave (pink noise flat, white rising 3 dB per \
                octave) between the window changes the dashed lines mark. Use an RTA for \
                band levels in dB SPL."
            .to_string();
    }
    let per = match bin {
        BinWidth::Hz(b) => format!("per FFT bin ({} wide)", format::freq_readout(b)),
        BinWidth::Mixed | BinWidth::Unknown | BinWidth::MultiRes => "per FFT bin".to_string(),
    };
    format!(
        "Level {per}: a sine reads its RMS level. Broadband sound reads lower than its band \
         or total level, and lower with finer bins (3 dB per doubling of the FFT length). \
         Use an RTA for band levels in dB SPL."
    )
}

pub(crate) fn scale_unit(scale: LevelScale) -> &'static str {
    match scale {
        LevelScale::Dbfs => "dBFS",
        LevelScale::DbSpl => "dB SPL",
    }
}

/// `1/3 oct`.
pub fn fraction_label(f: BandFraction) -> String {
    format!("1/{} oct", f.b())
}

/// `A-weighted`, `C-weighted`, `Z (unweighted)`.
pub fn weighting_label(w: Weighting) -> &'static str {
    match w {
        Weighting::A => "A-weighted",
        Weighting::C => "C-weighted",
        Weighting::Z => "Z (unweighted)",
    }
}

pub fn window_label(w: Window) -> &'static str {
    match w {
        Window::Hann => "Hann window",
        Window::BlackmanHarris4 => "Blackman-Harris window",
        Window::FlatTop => "flat-top window",
        Window::Rectangular => "rectangular window",
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
    if !view.unresolved {
        return None;
    }
    let with = || traces.iter().filter(|t| t.bands.is_some());
    let t = with().find(|t| t.selected).or_else(|| with().next())?;
    Some((t.bands?, t.freqs.first().copied().unwrap_or(0.0)))
}

/// A caption from its longest form to its shortest, for a pane too narrow for the full
/// one: without the parenthesised details (`(data sheet 15.0 mV/Pa)`), then without its
/// first part (the window or band, which the pane's title and the unit also say), then its
/// parts from the end off, last nothing. The calibration stays longest: it says whether
/// the levels are dB SPL to be trusted.
pub fn caption_forms(caption: &str) -> Vec<String> {
    let mut v = vec![caption.to_string()];
    if caption.is_empty() {
        return v;
    }
    let mut bare = String::new();
    let mut depth = 0usize;
    for ch in caption.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => bare.push(ch),
            _ => {}
        }
    }
    let parts: Vec<String> = bare
        .split('·')
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .collect();
    v.push(parts.join(" · "));
    let rest = parts.get(1..).unwrap_or_default();
    for n in (1..=rest.len()).rev() {
        v.push(rest[..n].join(" · "));
    }
    v.push(String::new());
    v.dedup();
    v
}

/// The axis unit's forms ([`level_unit_forms`]) for the curves shown, and its help for a
/// narrowband spectrum. Curves that differ only in their bin spacing share `per bin, mixed
/// widths`; any other difference is `mixed units`.
fn unit_forms(traces: &[SpectrumTrace<'_>]) -> (Vec<String>, Option<String>) {
    let Some(first) = traces.first() else {
        return (vec![String::new()], None);
    };
    if traces
        .iter()
        .any(|t| t.scale != first.scale || t.quantity != first.quantity)
    {
        return (vec!["mixed units".to_string()], None);
    }
    let bin = if traces.iter().all(|t| t.bands.is_some()) {
        BinWidth::MultiRes
    } else if traces
        .iter()
        .all(|t| t.bin_hz == first.bin_hz && t.bands.is_none())
    {
        BinWidth::of(first.bin_hz)
    } else {
        BinWidth::Mixed
    };
    let help = first
        .quantity
        .is_narrow()
        .then(|| narrow_unit_help(first.quantity, bin));
    (level_unit_forms(first.scale, first.quantity, bin), help)
}

/// Legend row pitch.
const LEGEND_ROW: f32 = 16.0;
/// Colour swatch of a legend entry and the gap after it.
const SWATCH_W: f32 = 12.0;
const SWATCH_GAP: f32 = 6.0;
/// Gap between two entries on a row.
const ENTRY_GAP: f32 = 16.0;
/// Most of the pane height the legend may take: the curves are what the pane is for.
const LEGEND_SHARE: f32 = 0.25;
const LEGEND_MAX_ROWS: usize = 4;

/// Legend line of one curve: its name and what sets it apart — `stopped`, `STALE 3.2 s`,
/// `offset +3.0 dB` (a spread is never read as a level difference).
fn legend_entry(t: &SpectrumTrace<'_>) -> LegendEntry {
    let mut tags = Vec::new();
    if t.offset_db != 0.0 {
        tags.push(format!("offset {}", format::db_readout(t.offset_db)));
    }
    if let Some(tag) = t.freshness.and_then(|f| f.tag()) {
        tags.push(tag);
    }
    let mut text = t.name.clone();
    for tag in &tags {
        text.push_str(" · ");
        text.push_str(tag);
    }
    LegendEntry {
        key: t.key,
        name: t.name.clone(),
        tags,
        text,
        stale: t.is_stale(),
        selected: t.selected,
    }
}

/// Width an entry takes on a row: swatch, gap, text.
fn entry_width(text: &str, font: f32) -> f32 {
    SWATCH_W + SWATCH_GAP + text_width(text, font)
}

/// Flows `texts` into rows `width` wide; each item's row and x offset. An item wider than
/// a row is cut to fit it.
fn flow(texts: &[String], width: f32, font: f32) -> Vec<(usize, f32, String)> {
    let mut out = Vec::with_capacity(texts.len());
    let (mut row, mut x) = (0usize, 0.0f32);
    for t in texts {
        let t = cut_to(t, width - SWATCH_W - SWATCH_GAP, font);
        let w = entry_width(&t, font);
        if x > 0.0 && x + w > width {
            row += 1;
            x = 0.0;
        }
        out.push((row, x, t));
        x += w + ENTRY_GAP;
    }
    out
}

/// The legend laid out in at most `max_rows` rows `width` wide: every entry in full when
/// that fits, else names only, else the names that fit and `+N more` closing the last row.
/// Items are (entry index or `None` for the `+N more` note, row, x, text).
fn legend_layout(
    entries: &[LegendEntry],
    width: f32,
    max_rows: usize,
    font: f32,
) -> Vec<(Option<usize>, usize, f32, String)> {
    if entries.is_empty() || max_rows == 0 {
        return Vec::new();
    }
    let rows = |f: &[(usize, f32, String)]| f.last().map_or(0, |l| l.0 + 1);
    let indexed = |f: Vec<(usize, f32, String)>| {
        f.into_iter()
            .enumerate()
            .map(|(i, (r, x, t))| (Some(i), r, x, t))
            .collect()
    };
    let full: Vec<String> = entries.iter().map(|e| e.text.clone()).collect();
    let f = flow(&full, width, font);
    if rows(&f) <= max_rows {
        return indexed(f);
    }
    let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    let f = flow(&names, width, font);
    if rows(&f) <= max_rows {
        return indexed(f);
    }
    let mut kept: Vec<(usize, f32, String)> = f
        .into_iter()
        .take_while(|(r, _, _)| *r < max_rows)
        .collect();
    // Room on the last row for the note of what is left out.
    loop {
        let more = format!("+{} more", entries.len() - kept.len());
        let end = kept
            .last()
            .map_or(0.0, |(_, x, t)| x + entry_width(t, font) + ENTRY_GAP);
        let last_row = kept.last().map_or(0, |k| k.0);
        if kept.is_empty() || end + text_width(&more, font) <= width {
            let mut out: Vec<_> = indexed(kept);
            out.push((None, last_row, end, more));
            return out;
        }
        kept.pop();
    }
}

pub fn spectrum_scene(
    traces: &[SpectrumTrace<'_>],
    status: &Status,
    view: &ViewState,
    theme: &Theme,
    size: Viewport,
) -> SpectrumScene {
    spectrum_scene_in(traces, status, view, theme, size, MARGINS.right)
}

/// [`spectrum_scene`] with `right` logical pixels right of the plot: room for what a
/// composed view draws beside a plot under it, so the two frequency axes line up.
pub(crate) fn spectrum_scene_in(
    traces: &[SpectrumTrace<'_>],
    status: &Status,
    view: &ViewState,
    theme: &Theme,
    size: Viewport,
    right: f32,
) -> SpectrumScene {
    let mut c = Canvas::new(size, theme);
    let plot_w = (size.width - MARGINS.left - right).max(1.0);
    let strip = canvas::banner_strip(&mut c, status, MARGINS.left, plot_w, size, theme);
    let font = theme.small_font_size;
    // The legend takes rows of its own between the banner strip and the plot, so it never
    // covers a curve; a short pane gives it fewer rows.
    let legend: Vec<LegendEntry> = traces.iter().map(legend_entry).collect();
    let free = size.height - strip.rect.bottom() - MARGINS.top - MARGINS.bottom;
    let max_rows =
        ((free * LEGEND_SHARE / LEGEND_ROW).floor().max(0.0) as usize).min(LEGEND_MAX_ROWS);
    let placed = legend_layout(&legend, plot_w, max_rows, font);
    let legend_rows = placed.iter().map(|p| p.1 + 1).max().unwrap_or(0);
    let legend_rect = Rect::new(
        MARGINS.left,
        strip.rect.bottom() + if legend_rows > 0 { 4.0 } else { 0.0 },
        plot_w,
        legend_rows as f32 * LEGEND_ROW,
    );
    let plot = canvas::plot_area(
        size,
        legend_rect.bottom() - if legend_rows > 0 { 4.0 } else { 0.0 },
        right,
    );
    let mut legend_shown = Vec::with_capacity(placed.len());
    let mut legend_hits = Vec::with_capacity(placed.len());
    for (i, row, x, text) in placed {
        let y = legend_rect.y + (row as f32 + 0.5) * LEGEND_ROW;
        let x = legend_rect.x + x;
        if let Some(e) = i.and_then(|i| legend.get(i)) {
            legend_hits.push(crate::legend::LegendHit {
                key: e.key,
                rect: Rect::new(
                    x,
                    y - LEGEND_ROW / 2.0,
                    entry_width(&text, font),
                    LEGEND_ROW,
                ),
            });
        }
        let (tx, color) = match i.and_then(|i| traces.get(i).zip(legend.get(i))) {
            Some((t, e)) => {
                let a = if e.stale { theme.stale_alpha } else { 1.0 };
                let color = t.color.with_alpha(a);
                crate::tf::legend_swatch(
                    &mut c.overlay,
                    x,
                    y,
                    SWATCH_W,
                    color,
                    e.selected,
                    None,
                    theme,
                );
                (
                    x + SWATCH_W + SWATCH_GAP,
                    if e.stale { theme.text_dim } else { theme.text },
                )
            }
            None => (x, theme.text_dim),
        };
        c.overlay.labels.push(label(
            text.clone(),
            [tx, y],
            anchor(HAlign::Left, VAlign::Center),
            font,
            color,
        ));
        legend_shown.push(text);
    }
    let full_caption = traces.first().map_or(String::new(), |t| t.caption.clone());
    // The unit (left) and the caption (right) share the plot's top line. The caption takes
    // its longest form that leaves the unit its compact form room (`dB SPL/bin`: a bin level
    // is no band level, which the bare scale would no longer say), then the unit its longest
    // form beside it: the two never overlap, and what the axis means goes last.
    let (forms, unit_help) = unit_forms(traces);
    let compact = forms
        .len()
        .checked_sub(2)
        .and_then(|i| forms.get(i))
        .or(forms.last())
        .map_or(0.0, |f| text_width(f, font));
    let shortest = forms.last().map_or(0.0, |f| text_width(f, font));
    let room_beside = |caption: &str| {
        plot.w
            - 12.0
            - if caption.is_empty() {
                0.0
            } else {
                text_width(caption, font) + 12.0
            }
    };
    let caption_forms = caption_forms(&full_caption);
    let caption = caption_forms
        .iter()
        .find(|c| room_beside(c) >= compact)
        .or_else(|| caption_forms.iter().find(|c| room_beside(c) >= shortest))
        .cloned()
        .unwrap_or_default();
    let room = room_beside(&caption);
    let unit = forms
        .iter()
        .find(|f| text_width(f, font) <= room)
        .or(forms.last())
        .cloned()
        .unwrap_or_default();
    let x_axis = axis::freq_axis(view.freq.range(), plot.x, plot.right());
    let y_axis = axis::linear_axis(view.spectrum.level, plot.bottom(), plot.y, &unit);
    canvas::pane_frame(
        &mut c,
        plot,
        &x_axis,
        &y_axis,
        true,
        &unit,
        view.chrome,
        theme,
    );
    c.base.labels.push(label(
        caption.clone(),
        [plot.right() - 6.0, plot.y + 4.0],
        anchor(HAlign::Right, VAlign::Top),
        theme.small_font_size,
        theme.text_dim,
    ));
    let (xm, ym) = (x_axis.mapping, y_axis.mapping);
    let marked = marked_bands(traces, view);
    let stitches = marked.map_or_else(Vec::new, |(b, _)| crate::stitch::stitches(b));
    let unresolved = marked.map_or_else(Vec::new, |(b, first)| {
        crate::unresolved::marks(
            crate::unresolved::Source {
                unresolved: &b.unresolved,
                fill: crate::unresolved::Fill::Steps,
            },
            first,
            None,
        )
    });
    crate::unresolved::draw(&mut c, plot, &xm, &unresolved, theme);
    crate::stitch::draw(&mut c, plot, &xm, &stitches, true, theme);

    for t in traces {
        let dim = if t.is_stale() { theme.stale_alpha } else { 1.0 };
        let color = t.color.with_alpha(dim);
        let n = t.freqs.len().min(t.level.len());
        let cols = visible_columns(&t.freqs[..n], xm.range.lo, xm.range.hi);
        // Bars are band power (RTA). A narrowband bin is a tone level, and thousands of bins
        // share a pixel at the top of a log axis: drawn as bars, a single-bin tone becomes a
        // sub-pixel sliver. Tone traces are always the max-per-pixel line.
        let style = if t.quantity.is_narrow() {
            SpectrumStyle::Line
        } else {
            view.spectrum.style
        };
        match style {
            SpectrumStyle::Bars => {
                for i in cols.clone() {
                    let v = t.drawn_value(i);
                    let Some(&(e0, e1)) = t.edges.get(i) else {
                        continue;
                    };
                    let y = ym.to_px(v).max(plot.y);
                    if !y.is_finite() || y >= plot.bottom() {
                        continue;
                    }
                    let (x0, x1) = (xm.to_px(e0.max(xm.range.lo * 1e-3)), xm.to_px(e1));
                    // A one-pixel gap between bars when they are wide enough to show it.
                    let gap = if x1 - x0 > 4.0 { 1.0 } else { 0.0 };
                    c.data.rects.push(FillRect {
                        rect: Rect::new(
                            x0 + gap / 2.0,
                            y,
                            (x1 - x0 - gap).max(0.5),
                            plot.bottom() - y,
                        ),
                        color: color.with_alpha(theme.bar_alpha),
                        clip: Some(plot),
                    });
                }
            }
            SpectrumStyle::Line => {
                let xs: Vec<f32> = t.freqs[cols.clone()].iter().map(|f| xm.to_px(*f)).collect();
                let ys: Vec<f32> = cols.clone().map(|i| ym.to_px(t.drawn_value(i))).collect();
                let (xs, ys) = max_per_pixel(&xs, &ys);
                let (points, _) = gapped(&xs, &ys, None, |_, _| false);
                if !points.is_empty() {
                    c.data.polylines.push(Polyline {
                        points,
                        alpha: vec![],
                        stroke: Stroke::solid(
                            color,
                            theme.trace_width
                                * if t.selected {
                                    crate::tf::SELECTED_WIDTH
                                } else {
                                    1.0
                                },
                        ),
                        clip: Some(plot),
                    });
                }
            }
        }
        if view.spectrum.peak_hold && t.peak.is_some() {
            let pc = color.with_alpha(theme.peak_alpha);
            let points = match style {
                // A cap across each band.
                SpectrumStyle::Bars => {
                    let mut pts = Vec::new();
                    for i in cols.clone() {
                        let (Some(p), Some(&(e0, e1))) = (t.peak_value(i), t.edges.get(i)) else {
                            continue;
                        };
                        let y = ym.to_px(p);
                        if !y.is_finite() {
                            continue;
                        }
                        if !pts.is_empty() {
                            pts.push([f32::NAN, f32::NAN]);
                        }
                        pts.push([xm.to_px(e0.max(xm.range.lo * 1e-3)), y]);
                        pts.push([xm.to_px(e1), y]);
                    }
                    pts
                }
                SpectrumStyle::Line => {
                    let xs: Vec<f32> = t.freqs[cols.clone()].iter().map(|f| xm.to_px(*f)).collect();
                    let ys: Vec<f32> = cols
                        .clone()
                        .map(|i| t.peak_value(i).map_or(f32::NAN, |p| ym.to_px(p)))
                        .collect();
                    let (xs, ys) = max_per_pixel(&xs, &ys);
                    gapped(&xs, &ys, None, |_, _| false).0
                }
            };
            if !points.is_empty() {
                c.data.polylines.push(Polyline {
                    points,
                    alpha: vec![],
                    stroke: Stroke {
                        color: pc,
                        width: (theme.trace_width * 0.75).max(1.0),
                        dash: (style == SpectrumStyle::Line).then_some(Dash {
                            on: 4.0,
                            off: 3.0,
                            offset: 0.0,
                        }),
                    },
                    clip: Some(plot),
                });
            }
        }
    }

    let cursor_hz = view.cursor_hz.filter(|_| view.chrome.cursor());
    let cursor = cursor_hz.and_then(|hz| {
        let first = traces.first()?;
        let i = nearest_column(first.freqs, hz)?;
        let f = first.freqs[i];
        let rows = traces
            .iter()
            .filter_map(|t| {
                let j = nearest_column(t.freqs, hz)?;
                let v = t.value(j);
                let s = format::level(v);
                let s = if v.is_finite() {
                    format!("{s} {}", value_unit(t.scale, t.quantity))
                } else {
                    s
                };
                // An offset trace reads its displayed (offset) level, and says so.
                let name = if t.offset_db != 0.0 {
                    offset_note(&t.name, t.offset_db)
                } else {
                    t.name.clone()
                };
                Some((name, s))
            })
            .collect();
        let mut freq = format::freq_readout(f);
        if let Some((b, _)) = marked {
            if let Some(w) = crate::stitch::windows_at(b, f) {
                freq = format!("{freq} · {w}");
            }
            if let Some(m) = crate::unresolved::at(&unresolved, f) {
                freq = format!("{freq} · {}", m.note);
            }
        }
        Some(SpectrumCursor {
            freq_hz: f,
            freq,
            rows,
        })
    });
    if let Some(cur) = &cursor {
        canvas::vline(&mut c.overlay, plot, xm.to_px(cur.freq_hz), theme.cursor);
        let mut lines = vec![cur.freq.clone()];
        lines.extend(cur.rows.iter().map(|(n, v)| format!("{n}  {v}")));
        c.overlay.labels.push(label(
            lines.join("\n"),
            [plot.right() - 8.0, plot.y + 22.0],
            anchor(HAlign::Right, VAlign::Top),
            theme.small_font_size,
            theme.text,
        ));
    }

    SpectrumScene {
        scene: c.into_scene(size),
        plot,
        x_axis,
        y_axis,
        unit_rect: Rect::new(
            plot.x + 6.0,
            plot.y + 4.0,
            text_width(&unit, font),
            font * 1.25,
        ),
        unit,
        unit_help,
        caption,
        cursor,
        legend,
        legend_shown,
        legend_rect,
        legend_hits,
        strip: strip.rect,
        banners: strip.rows,
        stitches,
        unresolved,
    }
}

#[cfg(test)]
mod tests;
