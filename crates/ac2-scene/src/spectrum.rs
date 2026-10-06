//! Spectrum and RTA view: bars or lines per band / bin, peak-hold overlay, unit on the axis.
//!
//! Units (decision 4b): a narrowband spectrum shows **tone level** (a sine reads its RMS
//! level regardless of FFT length), an RTA shows **band power**; the axis says which, so
//! the two are never read against each other by mistake. A smoothed narrowband spectrum is
//! a fractional-octave power average of tone levels — a sine reads lower by however many
//! bins the kernel spreads it over — so its axis says that too.

use ac2_proto::frame::{RtaFrame, SpecFrame, ValidityMask};
use ac2_proto::model::{BandFraction, CalStatus, LevelScale, SmoothingFraction, Weighting, Window};
use ac2_proto::units::WallNs;

use crate::axis::{self, Axis};
use crate::banner::{BannerRow, Status};
use crate::canvas::{self, Canvas, MARGINS, anchor, gapped, label, text_width, visible_columns};
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
    /// Power in the band (RTA).
    Band,
}

impl Quantity {
    /// A narrowband spectrum's quantity under `smoothing`.
    pub fn tone(smoothing: Option<SmoothingFraction>) -> Self {
        smoothing.map_or(Quantity::Tone, Quantity::SmoothedTone)
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
}

impl BinWidth {
    pub fn of(bin_hz: Option<f64>) -> Self {
        bin_hz.map_or(BinWidth::Unknown, BinWidth::Hz)
    }
}

/// Axis unit, longest form: `dBFS per 1.46 Hz bin (tone)`, `dB SPL (band)`,
/// `dBFS per 11.7 Hz bin (tone, 1/6 oct smoothed)`.
pub fn level_unit(scale: LevelScale, q: Quantity, bin: BinWidth) -> String {
    level_unit_forms(scale, q, bin).swap_remove(0)
}

/// The axis unit from its longest form to its shortest, for a pane too narrow for the full
/// one: what goes first is what the pane title or the RTA's own axis also says (`tone`,
/// the smoothing), the scale always stays.
pub fn level_unit_forms(scale: LevelScale, q: Quantity, bin: BinWidth) -> Vec<String> {
    let s = scale_unit(scale);
    let smoothed = match q {
        Quantity::SmoothedTone(f) => Some(format!("{} smoothed", format::octave_fraction(f))),
        _ => None,
    };
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
        (_, BinWidth::Mixed) => {
            let per = format!("{s} per bin, mixed widths");
            vec![format!("{per} ({tone})"), per, format!("{s}/bin")]
        }
    };
    v.push(s.to_string());
    v.dedup();
    v
}

/// What the level axis of a narrowband spectrum means, for its tooltip and the help.
pub fn tone_unit_help(bin: BinWidth) -> String {
    let per = match bin {
        BinWidth::Hz(b) => format!("per FFT bin ({} wide)", format::freq_readout(b)),
        BinWidth::Mixed | BinWidth::Unknown => "per FFT bin".to_string(),
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
        }
    }

    /// A live narrowband spectrum; `captured` is the frame's capture time, `curve` the
    /// mic-curve note of its input. Its columns are display columns of FFT bins, each the
    /// highest tone level among its bins; a gap is NaN.
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
            validity: None,
            peak: None,
            scale: frame.meta.scale,
            quantity: Quantity::tone(frame.meta.smoothing),
            bin_hz: None,
            // The axis unit says whether the level is smoothed (`level_unit`).
            caption: format!(
                "{}{}{}",
                window_label(frame.meta.window),
                cal_caption(frame.meta.cal, curve, captured),
                stopped_caption(freshness)
            ),
            freshness: Some(freshness),
            offset_db: 0.0,
            selected: false,
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
    /// Banner strip above the plot; zero height when no banner is up.
    pub strip: Rect,
    pub banners: Vec<BannerRow>,
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
    let bin = if traces.iter().all(|t| t.bin_hz == first.bin_hz) {
        BinWidth::of(first.bin_hz)
    } else {
        BinWidth::Mixed
    };
    let help = (first.quantity != Quantity::Band).then(|| tone_unit_help(bin));
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
    let stale = t.is_stale();
    match t.freshness {
        Some(f) if stale => tags.push(format!("STALE {}", format::age(f.age_s()))),
        Some(f) if f.is_stopped() => tags.push("stopped".to_string()),
        _ => {}
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
        stale,
        selected: t.selected,
    }
}

/// Width an entry takes on a row: swatch, gap, text.
fn entry_width(text: &str, font: f32) -> f32 {
    SWATCH_W + SWATCH_GAP + text_width(text, font)
}

/// `text` cut to `width` with an ellipsis (no font metrics: [`text_width`]'s generous
/// advance per character).
fn cut_to(text: &str, width: f32, font: f32) -> String {
    if text_width(text, font) <= width {
        return text.to_string();
    }
    let n = (width / text_width("x", font)).floor() as usize;
    let mut s: String = text.chars().take(n.saturating_sub(1)).collect();
    s.push('…');
    s
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
    for (i, row, x, text) in placed {
        let y = legend_rect.y + (row as f32 + 0.5) * LEGEND_ROW;
        let x = legend_rect.x + x;
        let (tx, color) = match i.and_then(|i| traces.get(i).zip(legend.get(i))) {
            Some((t, e)) => {
                let a = if e.stale { theme.stale_alpha } else { 1.0 };
                let color = t.color.with_alpha(a);
                crate::tf::legend_swatch(&mut c, x, y, SWATCH_W, color, e.selected, None, theme);
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
    canvas::pane_frame(&mut c, plot, &x_axis, &y_axis, true, &unit, theme);
    c.base.labels.push(label(
        caption.clone(),
        [plot.right() - 6.0, plot.y + 4.0],
        anchor(HAlign::Right, VAlign::Top),
        theme.small_font_size,
        theme.text_dim,
    ));
    let (xm, ym) = (x_axis.mapping, y_axis.mapping);

    for t in traces {
        let dim = if t.is_stale() { theme.stale_alpha } else { 1.0 };
        let color = t.color.with_alpha(dim);
        let n = t.freqs.len().min(t.level.len());
        let cols = visible_columns(&t.freqs[..n], xm.range.lo, xm.range.hi);
        // Bars are band power (RTA). A narrowband bin is a tone level, and thousands of bins
        // share a pixel at the top of a log axis: drawn as bars, a single-bin tone becomes a
        // sub-pixel sliver. Tone traces are always the max-per-pixel line.
        let style = match t.quantity {
            Quantity::Tone | Quantity::SmoothedTone(_) => SpectrumStyle::Line,
            Quantity::Band => view.spectrum.style,
        };
        match style {
            SpectrumStyle::Bars => {
                for i in cols.clone() {
                    let v = t.value(i);
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
                let ys: Vec<f32> = cols.clone().map(|i| ym.to_px(t.value(i))).collect();
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

    let cursor = view.cursor_hz.and_then(|hz| {
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
                    format!("{s} {}", scale_unit(t.scale))
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
        Some(SpectrumCursor {
            freq_hz: f,
            freq: format::freq_readout(f),
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
        strip: strip.rect,
        banners: strip.rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axis::Range;
    use crate::grid::{column_edges, column_frequencies};
    use crate::view::SpectrumView;
    use ac2_proto::GridDef;
    use ac2_proto::units::{Hz, MeasId};

    const SIZE: Viewport = Viewport {
        width: 800.0,
        height: 400.0,
    };

    fn third_octaves() -> GridDef {
        let centres = (-17..=13)
            .map(|x| Hz(1000.0 * 10f64.powf(f64::from(x) / 10.0)))
            .collect();
        GridDef::IecBands {
            fraction: BandFraction::Third,
            centres,
        }
    }

    fn rta_trace<'a>(
        f: &'a [f64],
        e: &'a [(f64, f64)],
        level: &'a [f32],
        scale: LevelScale,
    ) -> SpectrumTrace<'a> {
        SpectrumTrace {
            key: TraceKey::Live(MeasId(1)),
            name: "RTA".into(),
            color: Color::WHITE,
            freqs: f,
            edges: e,
            level,
            validity: None,
            peak: None,
            scale,
            quantity: Quantity::Band,
            bin_hz: None,
            caption: format!(
                "{} · {}",
                fraction_label(BandFraction::Third),
                weighting_label(Weighting::A)
            ),
            freshness: None,
            offset_db: 0.0,
            selected: false,
        }
    }

    #[test]
    fn calibration_captions() {
        const H: u64 = 3_600_000_000_000;
        let now = WallNs(100 * H);
        assert_eq!(cal_caption(CalStatus::Uncalibrated, None, now), "");
        assert_eq!(
            cal_caption(CalStatus::Uncalibrated, Some("mic curve: MM1 90°"), now),
            " · mic curve: MM1 90°"
        );
        let at = WallNs(97 * H - 59_000_000_000);
        assert_eq!(
            cal_caption(
                CalStatus::Verified {
                    calibrated_at: at,
                    basis: ac2_proto::model::CalBasis::Acoustic {
                        calibrator_level: ac2_proto::units::DbSpl(94.0)
                    }
                },
                None,
                now
            ),
            " · cal 94 dB · 3 h ago"
        );
        assert_eq!(
            cal_caption(
                CalStatus::Verified {
                    calibrated_at: at,
                    basis: ac2_proto::model::CalBasis::Acoustic {
                        calibrator_level: ac2_proto::units::DbSpl(94.0)
                    }
                },
                Some("mic curve off"),
                now
            ),
            " · cal 94 dB · 3 h ago · mic curve off"
        );
        assert_eq!(
            cal_caption(
                CalStatus::Verified {
                    calibrated_at: WallNs(100 * H - 600_000_000_000),
                    basis: ac2_proto::model::CalBasis::Acoustic {
                        calibrator_level: ac2_proto::units::DbSpl(94.0)
                    }
                },
                None,
                now
            ),
            " · cal 94 dB · 10 min ago"
        );
        // A calibration stamped after the frame (clock stepped back) is not in the future.
        assert_eq!(
            cal_caption(
                CalStatus::Verified {
                    calibrated_at: WallNs(101 * H),
                    basis: ac2_proto::model::CalBasis::Acoustic {
                        calibrator_level: ac2_proto::units::DbSpl(94.0)
                    }
                },
                None,
                now
            ),
            " · cal 94 dB · just now"
        );
        // The mismatch says so instead of an age: the age would be another mic's.
        assert_eq!(
            cal_caption(
                CalStatus::OtherMicOrInput {
                    calibrated_at: at,
                    basis: ac2_proto::model::CalBasis::Acoustic {
                        calibrator_level: ac2_proto::units::DbSpl(94.0)
                    }
                },
                Some("mic curve: MM1 0°"),
                now
            ),
            " · cal from other mic / input · mic curve: MM1 0°"
        );
    }

    #[test]
    fn live_traces_carry_the_calibration_in_their_caption() {
        use ac2_proto::frame::{RtaMeta, SpecMeta};
        const H: u64 = 3_600_000_000_000;
        let cal = CalStatus::Verified {
            calibrated_at: WallNs(H),
            basis: ac2_proto::model::CalBasis::Acoustic {
                calibrator_level: ac2_proto::units::DbSpl(94.0),
            },
        };
        let rta = RtaFrame {
            meas: MeasId(1),
            meta: RtaMeta {
                fraction: BandFraction::Third,
                weighting: Weighting::A,
                scale: LevelScale::DbSpl,
                cal,
                mic_curve: true,
            },
            level: vec![],
            validity: vec![],
        };
        let fresh = Freshness::from_age(0.0);
        let t = SpectrumTrace::rta(
            &rta,
            WallNs(3 * H),
            Some("mic curve: MM1 90°"),
            &[],
            &[],
            "RTA",
            Color::WHITE,
            fresh,
        );
        assert_eq!(
            t.caption,
            "1/3 oct · A-weighted · cal 94 dB · 2 h ago · mic curve: MM1 90°"
        );
        let spec = SpecFrame {
            meas: MeasId(2),
            meta: SpecMeta {
                window: Window::Hann,
                scale: LevelScale::DbSpl,
                cal,
                mic_curve: false,
                smoothing: None,
            },
            level: vec![],
        };
        let t = SpectrumTrace::spectrum(
            &spec,
            WallNs(25 * H),
            None,
            &[],
            &[],
            "FFT",
            Color::WHITE,
            fresh,
        );
        assert_eq!(t.caption, "Hann window · cal 94 dB · 1 d ago");
        assert_eq!(t.quantity, Quantity::Tone);
        let mut smoothed = spec.clone();
        smoothed.meta.smoothing = Some(SmoothingFraction::Sixth);
        let t = SpectrumTrace::spectrum(
            &smoothed,
            WallNs(25 * H),
            None,
            &[],
            &[],
            "FFT",
            Color::WHITE,
            fresh,
        );
        assert_eq!(t.caption, "Hann window · cal 94 dB · 1 d ago");
        assert_eq!(t.quantity, Quantity::SmoothedTone(SmoothingFraction::Sixth));
    }

    #[test]
    fn unit_labels() {
        use BinWidth as B;
        let bin = B::Hz(48_000.0 / 32_768.0);
        assert_eq!(
            level_unit(LevelScale::Dbfs, Quantity::Tone, bin),
            "dBFS per 1.46 Hz bin (tone)"
        );
        assert_eq!(
            level_unit(LevelScale::Dbfs, Quantity::Tone, B::Unknown),
            "dBFS (tone)"
        );
        assert_eq!(
            level_unit(LevelScale::Dbfs, Quantity::Band, B::Unknown),
            "dBFS (band)"
        );
        assert_eq!(
            level_unit(LevelScale::DbSpl, Quantity::Band, B::Unknown),
            "dB SPL (band)"
        );
        assert_eq!(
            level_unit(
                LevelScale::Dbfs,
                Quantity::SmoothedTone(SmoothingFraction::Sixth),
                B::Hz(48_000.0 / 4096.0)
            ),
            "dBFS per 11.7 Hz bin (tone, 1/6 oct smoothed)"
        );
        assert_eq!(
            level_unit_forms(
                LevelScale::DbSpl,
                Quantity::tone(Some(SmoothingFraction::Third)),
                bin
            ),
            [
                "dB SPL per 1.46 Hz bin (tone, 1/3 oct smoothed)",
                "dB SPL per 1.46 Hz bin (1/3 oct smoothed)",
                "dB SPL per 1.46 Hz bin",
                "dB SPL/bin",
                "dB SPL",
            ]
        );
        assert_eq!(
            level_unit_forms(LevelScale::Dbfs, Quantity::Tone, B::Mixed),
            [
                "dBFS per bin, mixed widths (tone)",
                "dBFS per bin, mixed widths",
                "dBFS/bin",
                "dBFS",
            ]
        );
        assert_eq!(
            level_unit_forms(LevelScale::DbSpl, Quantity::Band, B::Unknown),
            ["dB SPL (band)", "dB SPL"]
        );
        assert_eq!(fraction_label(BandFraction::TwentyFourth), "1/24 oct");
        assert_eq!(window_label(Window::FlatTop), "flat-top window");
    }

    #[test]
    fn bars_cover_band_edges() {
        let g = third_octaves();
        let (f, e) = (column_frequencies(&g), column_edges(&g));
        let level: Vec<f32> = vec![-40.0; f.len()];
        let t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
        let s = spectrum_scene(
            &[t],
            &Status::default(),
            &ViewState::default(),
            &Theme::dark(),
            SIZE,
        );
        assert_eq!(s.unit, "dBFS (band)");
        assert_eq!(s.caption, "1/3 oct · A-weighted");
        assert_eq!(s.y_axis.title, "dBFS (band)");
        // 20 Hz … 20 kHz shows 31 bands (the 20 Hz band is partly visible).
        let bars = &s.scene.layers[1].rects;
        assert_eq!(bars.len(), 31);
        // The 1 kHz bar spans its band edges and reaches −40 dB.
        let xm = s.x_axis.mapping;
        let k = f.iter().position(|x| *x == 1000.0).expect("1k");
        let bar = bars
            .iter()
            .find(|b| b.rect.x <= xm.to_px(1000.0) && b.rect.right() >= xm.to_px(1000.0))
            .expect("bar");
        assert!((bar.rect.x - (xm.to_px(e[k].0) + 0.5)).abs() < 1e-3);
        assert!((bar.rect.y - s.y_axis.mapping.to_px(-40.0)).abs() < 1e-3);
        assert!((bar.rect.bottom() - s.plot.bottom()).abs() < 1e-3);
    }

    #[test]
    fn line_style_gaps_and_cursor() {
        let g = third_octaves();
        let (f, e) = (column_frequencies(&g), column_edges(&g));
        let mut level: Vec<f32> = vec![-40.0; f.len()];
        level[15] = f32::NAN;
        let mut validity = vec![ValidityMask::NONE; f.len()];
        validity[20] = ValidityMask::INSUFFICIENT_RESOLUTION;
        let mut t = rta_trace(&f, &e, &level, LevelScale::DbSpl);
        t.validity = Some(&validity);
        let view = ViewState {
            spectrum: SpectrumView {
                style: SpectrumStyle::Line,
                level: Range::new(-60.0, 0.0),
                peak_hold: false,
                ..SpectrumView::default()
            },
            cursor_hz: Some(1000.0),
            ..ViewState::default()
        };
        let s = spectrum_scene(&[t], &Status::default(), &view, &Theme::dark(), SIZE);
        let line = &s.scene.layers[1].polylines[0];
        assert_eq!(crate::canvas::tests::segments(&line.points).len(), 3);
        let cur = s.cursor.expect("cursor");
        assert_eq!(cur.freq, "1.00 kHz");
        assert_eq!(
            cur.rows,
            vec![("RTA".to_string(), "−40.0 dB SPL".to_string())]
        );
    }

    /// A trace spread by a display offset is drawn that much higher, its peak hold too, and
    /// the plot and the cursor name the offset; an unshifted trace adds no note.
    #[test]
    fn offset_traces_move_and_say_so() {
        let g = third_octaves();
        let (f, e) = (column_frequencies(&g), column_edges(&g));
        let level = vec![-40.0f32; f.len()];
        let peak = vec![-30.0f32; f.len()];
        let a = rta_trace(&f, &e, &level, LevelScale::Dbfs);
        let mut b = rta_trace(&f, &e, &level, LevelScale::Dbfs);
        b.name = "Main L S1".into();
        b.offset_db = 3.0;
        b.peak = Some(&peak);
        let view = ViewState {
            spectrum: SpectrumView {
                style: SpectrumStyle::Line,
                level: Range::new(-60.0, 0.0),
                peak_hold: true,
                ..SpectrumView::default()
            },
            cursor_hz: Some(1000.0),
            ..ViewState::default()
        };
        let s = spectrum_scene(&[a, b], &Status::default(), &view, &Theme::dark(), SIZE);
        let texts: Vec<&str> = s.legend.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["RTA", "Main L S1 · offset +3.0 dB"]);
        let ym = s.y_axis.mapping;
        let lines = &s.scene.layers[1].polylines;
        let y_of = |l: &Polyline| l.points.iter().find(|p| p[1].is_finite()).map(|p| p[1]);
        assert_eq!(y_of(&lines[0]), Some(ym.to_px(-40.0)));
        assert_eq!(y_of(&lines[1]), Some(ym.to_px(-37.0)));
        assert_eq!(
            y_of(&lines[2]),
            Some(ym.to_px(-27.0)),
            "peak hold moves too"
        );
        let cur = s.cursor.expect("cursor");
        assert_eq!(
            cur.rows,
            [
                ("RTA".to_string(), "−40.0 dBFS".to_string()),
                (
                    "Main L S1 · offset +3.0 dB".to_string(),
                    "−37.0 dBFS".to_string()
                ),
            ]
        );
        assert!(
            s.scene
                .layers
                .iter()
                .flat_map(|l| &l.labels)
                .any(|l| l.text == "Main L S1 · offset +3.0 dB")
        );
    }

    #[test]
    fn mixed_units_are_flagged() {
        let g = third_octaves();
        let (f, e) = (column_frequencies(&g), column_edges(&g));
        let level = vec![-40.0f32; f.len()];
        let a = rta_trace(&f, &e, &level, LevelScale::Dbfs);
        let b = rta_trace(&f, &e, &level, LevelScale::DbSpl);
        let s = spectrum_scene(
            &[a, b],
            &Status::default(),
            &ViewState::default(),
            &Theme::dark(),
            SIZE,
        );
        assert_eq!(s.unit, "mixed units");
    }

    /// A spectrum's axis names its bin width; a narrower pane shortens the unit to what
    /// fits beside the caption, down to the bare scale, and never runs it into the caption.
    #[test]
    fn tone_unit_names_the_bin_and_fits_narrow_panes() {
        let g = GridDef::LogBins {
            fs: Hz(48_000.0),
            n: 32_768,
            ppo: 96,
        };
        let cols = crate::grid::columns(&g);
        let level = vec![-60.0f32; cols.freqs.len()];
        let mut t = SpectrumTrace {
            key: TraceKey::Stored(ac2_proto::units::TraceId(1)),
            name: "Main L".into(),
            color: Color::WHITE,
            freqs: &cols.freqs,
            edges: &cols.edges,
            level: &level,
            validity: None,
            peak: None,
            scale: LevelScale::DbSpl,
            quantity: Quantity::tone(Some(SmoothingFraction::Third)),
            bin_hz: cols.bin_hz,
            caption: "Hann window".into(),
            freshness: None,
            offset_db: 0.0,
            selected: false,
        };
        let theme = Theme::dark();
        let scene = |traces: &[SpectrumTrace<'_>], w: f32| {
            spectrum_scene(
                traces,
                &Status::default(),
                &ViewState::default(),
                &theme,
                Viewport {
                    width: w,
                    height: 300.0,
                },
            )
        };
        let wide = scene(std::slice::from_ref(&t), 900.0);
        assert_eq!(wide.unit, "dB SPL per 1.46 Hz bin (tone, 1/3 oct smoothed)");
        let help = wide.unit_help.as_deref().expect("help");
        assert!(help.contains("1.46 Hz") && help.contains("RTA"), "{help}");
        let mut seen = Vec::new();
        for w in [900.0, 480.0, 440.0, 400.0, 330.0, 260.0, 200.0, 120.0] {
            let s = scene(std::slice::from_ref(&t), w);
            assert_unit_clear_of_caption(&s, w);
            seen.push((s.unit, s.caption));
        }
        seen.dedup();
        let u = |a: &str, c: &str| (a.to_string(), c.to_string());
        assert_eq!(
            seen,
            [
                u(
                    "dB SPL per 1.46 Hz bin (tone, 1/3 oct smoothed)",
                    "Hann window"
                ),
                u("dB SPL per 1.46 Hz bin (1/3 oct smoothed)", "Hann window"),
                u("dB SPL per 1.46 Hz bin", "Hann window"),
                u("dB SPL/bin", "Hann window"),
                // The window goes before the unit shrinks to the bare scale.
                u("dB SPL/bin", ""),
                u("dB SPL", ""),
            ]
        );
        // Spectra on two FFT lengths share the axis as per bin, mixed widths.
        let g2 = GridDef::LogBins {
            fs: Hz(48_000.0),
            n: 4096,
            ppo: 96,
        };
        let cols2 = crate::grid::columns(&g2);
        let level2 = vec![-60.0f32; cols2.freqs.len()];
        let mut u = t.clone();
        u.freqs = &cols2.freqs;
        u.edges = &cols2.edges;
        u.level = &level2;
        u.bin_hz = cols2.bin_hz;
        t.quantity = Quantity::Tone;
        u.quantity = Quantity::Tone;
        let s = scene(&[t, u], 900.0);
        assert_eq!(s.unit, "dB SPL per bin, mixed widths (tone)");
    }

    /// The unit's label and the drawn caption never overlap.
    fn assert_unit_clear_of_caption(s: &SpectrumScene, w: f32) {
        if s.caption.is_empty() {
            return;
        }
        let caption = s
            .scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .find(|l| l.text == s.caption)
            .expect("caption");
        let cap = crate::canvas::tests::label_box(caption);
        assert!(
            s.unit_rect.right() < cap.x,
            "{w}: {:?} vs {:?} at {cap:?}",
            s.unit,
            s.caption
        );
    }

    /// The rig's calibration caption (an electrical calibration with its data sheet and a
    /// mic curve) beside the per-bin unit: from 300 to 1300 px the two never overlap; the
    /// caption gives up the data sheet, then the window, then its tail, and the unit keeps
    /// at least `dB SPL/bin`.
    #[test]
    fn a_long_calibration_caption_leaves_the_unit_room() {
        let g = GridDef::LogBins {
            fs: Hz(48_000.0),
            n: 32_768,
            ppo: 96,
        };
        let cols = crate::grid::columns(&g);
        let level = vec![-60.0f32; cols.freqs.len()];
        let rig = "Hann window · electrical cal 1 kHz (data sheet 15.0 mV/Pa) ±1 dB · 2 d ago \
                   · mic curve: MM1 34804 90°";
        let t = SpectrumTrace {
            key: TraceKey::Stored(ac2_proto::units::TraceId(1)),
            name: "Mic".into(),
            color: Color::WHITE,
            freqs: &cols.freqs,
            edges: &cols.edges,
            level: &level,
            validity: None,
            peak: None,
            scale: LevelScale::DbSpl,
            quantity: Quantity::Tone,
            bin_hz: cols.bin_hz,
            caption: rig.into(),
            freshness: None,
            offset_db: 0.0,
            selected: false,
        };
        let theme = Theme::dark();
        let mut captions = Vec::new();
        for w in (300..=1300).step_by(50) {
            let w = w as f32;
            let size = Viewport {
                width: w,
                height: 300.0,
            };
            let s = spectrum_scene(
                std::slice::from_ref(&t),
                &Status::default(),
                &ViewState::default(),
                &theme,
                size,
            );
            assert_unit_clear_of_caption(&s, w);
            assert_ne!(s.unit, "dB SPL", "{w}");
            captions.push(s.caption);
        }
        captions.dedup();
        assert_eq!(captions.first().map(String::as_str), Some(""));
        assert!(
            captions.contains(&"electrical cal 1 kHz ±1 dB".to_string()),
            "{captions:?}"
        );
        assert_eq!(captions.last().map(String::as_str), Some(rig));
        assert_eq!(
            caption_forms(rig),
            [
                rig,
                "Hann window · electrical cal 1 kHz ±1 dB · 2 d ago · mic curve: MM1 34804 90°",
                "electrical cal 1 kHz ±1 dB · 2 d ago · mic curve: MM1 34804 90°",
                "electrical cal 1 kHz ±1 dB · 2 d ago",
                "electrical cal 1 kHz ±1 dB",
                "",
            ]
        );
    }

    #[test]
    fn peak_hold_decays_and_restarts() {
        let mut p = PeakHold::new(10.0);
        p.update(&[-20.0, -30.0], None, 0.0);
        assert_eq!(p.values(), [-20.0, -30.0]);
        p.update(&[-40.0, -25.0], None, 0.5);
        assert_eq!(p.values(), [-25.0, -25.0]);
        // Invalid column only decays.
        p.update(
            &[0.0, -60.0],
            Some(&[ValidityMask::SETTLING, ValidityMask::NONE]),
            0.1,
        );
        assert_eq!(p.values(), [-26.0, -26.0]);
        // New grid restarts.
        p.update(&[-50.0], None, 0.1);
        assert_eq!(p.values(), [-50.0]);
        let mut hold = PeakHold::new(0.0);
        hold.update(&[-10.0], None, 1.0);
        hold.update(&[-90.0], None, 100.0);
        assert_eq!(hold.values(), [-10.0]);
        hold.reset();
        assert!(hold.values().is_empty());
    }

    #[test]
    fn peak_overlay_drawn_when_enabled() {
        let g = third_octaves();
        let (f, e) = (column_frequencies(&g), column_edges(&g));
        let level = vec![-40.0f32; f.len()];
        let peak = vec![-20.0f32; f.len()];
        let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
        t.peak = Some(&peak);
        let mut view = ViewState::default();
        let s = spectrum_scene(
            &[t.clone()],
            &Status::default(),
            &view,
            &Theme::dark(),
            SIZE,
        );
        assert!(s.scene.layers[1].polylines.is_empty());
        view.spectrum.peak_hold = true;
        let s = spectrum_scene(&[t], &Status::default(), &view, &Theme::dark(), SIZE);
        let caps = &s.scene.layers[1].polylines[0];
        assert_eq!(crate::canvas::tests::segments(&caps.points).len(), 31);
        let y = s.y_axis.mapping.to_px(-20.0);
        assert!(
            caps.points
                .iter()
                .filter(|p| p[1].is_finite())
                .all(|p| (p[1] - y).abs() < 1e-3)
        );
    }

    #[test]
    fn banners_sit_above_the_plot() {
        let g = third_octaves();
        let (f, e) = (column_frequencies(&g), column_edges(&g));
        let level = vec![-40.0f32; f.len()];
        let view = ViewState {
            cursor_hz: Some(1000.0),
            ..ViewState::default()
        };
        let build = |status: &Status| {
            spectrum_scene(
                &[rta_trace(&f, &e, &level, LevelScale::Dbfs)],
                status,
                &view,
                &Theme::dark(),
                SIZE,
            )
        };
        let calm = build(&Status::default());
        assert_eq!(calm.strip.h, 0.0);
        // The legend row sits between the strip and the plot.
        assert_eq!(calm.legend_rect.y, 4.0);
        assert_eq!(calm.plot.y, calm.legend_rect.bottom() - 4.0 + MARGINS.top);
        let s = build(&crate::banner::tests::everything());
        assert_eq!(s.banners.len(), crate::banner::MAX_BANNERS);
        assert_eq!(s.legend_rect.y, s.strip.bottom() + 4.0);
        assert_eq!(s.plot.y, s.legend_rect.bottom() - 4.0 + MARGINS.top);
        assert_eq!(s.plot.bottom(), calm.plot.bottom());
        assert!(s.cursor.is_some());
        crate::canvas::tests::assert_banners_clear(&s.scene, &s.banners, &[s.plot, s.legend_rect]);
    }

    /// Several live spectra and captures are told apart by a legend above the plot: name,
    /// colour and tags in full while they fit, names alone when not, then the names that fit
    /// and `+N more`. It never reaches into the plot or past the pane.
    #[test]
    fn legend_names_every_curve_and_fits_small_panes() {
        let g = third_octaves();
        let (f, e) = (column_frequencies(&g), column_edges(&g));
        let level = vec![-40.0f32; f.len()];
        let names = ["Main L", "Main R", "Sub array", "Front fill", "Delay tower"];
        let mut traces: Vec<SpectrumTrace<'_>> = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
                t.key = TraceKey::Live(MeasId(i as u32 + 1));
                t.name = (*n).to_string();
                t.color = Color::rgb(0.1 * i as f32, 0.5, 0.5);
                t
            })
            .collect();
        traces[1].freshness = Some(Freshness::Stopped { age_s: 3.0 });
        traces[2].offset_db = -6.0;
        traces[3].selected = true;
        let build = |w: f32, h: f32| {
            spectrum_scene(
                &traces,
                &Status::default(),
                &ViewState::default(),
                &Theme::dark(),
                Viewport {
                    width: w,
                    height: h,
                },
            )
        };
        let texts = |s: &SpectrumScene| s.legend_shown.clone();
        let wide = build(1200.0, 500.0);
        assert_eq!(
            texts(&wide),
            [
                "Main L",
                "Main R · stopped",
                "Sub array · offset \u{2212}6.0 dB",
                "Front fill",
                "Delay tower"
            ]
        );
        assert_eq!(wide.legend.len(), 5);
        assert_eq!(wide.legend[1].tags, ["stopped"]);
        let selected: Vec<bool> = wide.legend.iter().map(|e| e.selected).collect();
        assert_eq!(selected, [false, false, false, true, false]);
        assert_eq!(wide.legend_rect.h, LEGEND_ROW);
        // Narrow: rows wrap, then the tags go, then `+N more`.
        let narrow = build(420.0, 500.0);
        assert!(
            narrow.legend_rect.h > LEGEND_ROW,
            "{:?}",
            narrow.legend_rect
        );
        let short = build(300.0, 120.0);
        assert_eq!(texts(&short).first().map(String::as_str), Some("Main L"));
        assert!(
            texts(&short).last().is_some_and(|t| t.ends_with(" more")),
            "{:?}",
            texts(&short)
        );
        assert!(!texts(&short).iter().any(|t| t.contains('·')));
        let tiny = build(160.0, 120.0);
        assert!(tiny.legend_rect.h <= LEGEND_ROW);
        for (w, h) in [
            (1200.0, 500.0),
            (420.0, 500.0),
            (300.0, 200.0),
            (220.0, 160.0),
            (160.0, 120.0),
        ] {
            let s = build(w, h);
            let boxes: Vec<Rect> = s
                .scene
                .layers
                .iter()
                .flat_map(|l| &l.labels)
                .filter(|l| s.legend_shown.contains(&l.text))
                .map(crate::canvas::tests::label_box)
                .collect();
            assert_eq!(boxes.len(), s.legend_shown.len(), "{w}x{h}");
            for b in &boxes {
                assert!(
                    !crate::canvas::tests::intersects(*b, s.plot),
                    "{w}x{h}: {b:?} in the plot"
                );
                assert!(b.right() <= w + 0.5, "{w}x{h}: {b:?} past the pane");
            }
            for (i, a) in boxes.iter().enumerate() {
                for b in &boxes[i + 1..] {
                    assert!(
                        !crate::canvas::tests::intersects(*a, *b),
                        "{w}x{h}: {a:?} {b:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn line_thinning_keeps_peaks() {
        // 32768 linear bins up to 24 kHz on a ~740 px log axis: thousands of bins per
        // pixel at the top. Two single-bin tones and one sub-pixel invalid bin.
        let n = 32_768;
        let f: Vec<f64> = (0..n).map(|i| i as f64 * 24_000.0 / n as f64).collect();
        let e: Vec<(f64, f64)> = f.iter().map(|x| (*x, *x)).collect();
        let mut level = vec![-90.0f32; n];
        let (k1, k2) = (13_653, 25_000); // 10.0 kHz, 18.3 kHz
        level[k1] = -12.0;
        level[k2] = -20.0;
        let mut validity = vec![ValidityMask::NONE; n];
        validity[k1 + 1] = ValidityMask::SETTLING;
        let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
        t.quantity = Quantity::Tone;
        t.validity = Some(&validity);
        t.peak = Some(&level);
        let view = ViewState {
            spectrum: SpectrumView {
                style: SpectrumStyle::Line,
                level: Range::new(-100.0, 0.0),
                peak_hold: true,
                ..SpectrumView::default()
            },
            ..ViewState::default()
        };
        let s = spectrum_scene(&[t], &Status::default(), &view, &Theme::dark(), SIZE);
        let (xm, ym) = (s.x_axis.mapping, s.y_axis.mapping);
        for line in &s.scene.layers[1].polylines {
            // At most one point per pixel column, plus the off-plot neighbours.
            assert!(
                line.points.len() <= s.plot.w as usize + 3,
                "{}",
                line.points.len()
            );
            assert_eq!(crate::canvas::tests::segments(&line.points).len(), 1);
            for (k, v) in [(k1, -12.0), (k2, -20.0)] {
                let want = [xm.to_px(f[k]), ym.to_px(v)];
                assert!(line.points.contains(&want), "peak at {} Hz missing", f[k]);
            }
            // Everything else stays on the floor.
            let floor = ym.to_px(-90.0);
            let high = line.points.iter().filter(|p| p[1] < floor - 1e-3).count();
            assert_eq!(high, 2);
        }
    }

    #[test]
    fn tone_traces_ignore_bar_style() {
        // 4096-point FFT at 48 kHz: a 1 kHz tone in one bin, ~0.5 px wide on this axis.
        let n = 2049;
        let f: Vec<f64> = (0..n).map(|i| i as f64 * 48_000.0 / 4096.0).collect();
        let e: Vec<(f64, f64)> = f.iter().map(|x| (x - 5.86, x + 5.86)).collect();
        let k = 85; // 996 Hz
        let mut level = vec![-80.0f32; n];
        level[k] = -30.0;
        let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
        t.quantity = Quantity::Tone;
        let bars = ViewState::default();
        assert_eq!(bars.spectrum.style, SpectrumStyle::Bars);
        let s = spectrum_scene(&[t], &Status::default(), &bars, &Theme::dark(), SIZE);
        assert!(s.scene.layers[1].rects.is_empty());
        let line = &s.scene.layers[1].polylines[0];
        let want = [s.x_axis.mapping.to_px(f[k]), s.y_axis.mapping.to_px(-30.0)];
        assert!(line.points.contains(&want));
    }

    #[test]
    fn max_per_pixel_groups_by_pixel() {
        let xs = [0.1, 0.5, 0.9, 1.2, 2.0, 2.5, 3.7];
        let ys = [5.0, 3.0, 4.0, f32::NAN, f32::NAN, 7.0, 1.0];
        let (x, y) = max_per_pixel(&xs, &ys);
        assert_eq!(x, [0.5, 1.2, 2.5, 3.7]);
        assert_eq!(y[0], 3.0);
        assert!(y[1].is_nan());
        assert_eq!(&y[2..], [7.0, 1.0]);
        // Sparse input passes through unchanged.
        let (x, y) = max_per_pixel(&[1.0, 5.0], &[2.0, 3.0]);
        assert_eq!((x, y), (vec![1.0, 5.0], vec![2.0, 3.0]));
    }
}
