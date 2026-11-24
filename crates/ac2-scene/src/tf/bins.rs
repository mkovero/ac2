//! A transfer's own analysis bins, drawn where the view resolves them.
//!
//! A transfer at the `bins` resolution sends, beside its 1/96-octave grid, each analysis
//! stage's own bins (`bins/<meas>/<stage>`). Where a stage's bins are finer than the grid
//! (above the curve's unresolved ranges, outside the blends where two stages hand over) and
//! the view puts at least a pixel between neighbouring bins, the curve is drawn through
//! those bins instead of the grid: zooming in reveals detail the grid averages away. On a
//! log axis the spacing of bins `Δf` apart is `px_per_octave · Δf / (f · ln 2)`, smallest at
//! the top of the range, so that is where it is judged.
//!
//! Only values the daemon sent are drawn: each bin with the display edits of its curve
//! (offset, polarity, phase reference), as the grid columns get them.

use std::ops::Range as Cols;

use ac2_proto::frame::{BinLayout, BinsFrame, FrameStamp, TfMeta};
use ac2_proto::model::{BinRange, Resolution};

use crate::axis::Range;
use crate::format;
use crate::trace::{DisplayTrace, TfTrace, coherence_alpha, wrap_deg};
use crate::view::{CoherenceStyle, PhaseView, ViewState};

/// Bins are drawn once neighbours are this many logical pixels apart: closer, they draw no
/// more than the grid already does.
pub const DRAW_PX: f64 = 1.0;
/// A stage is received from this spacing, short of [`DRAW_PX`], so a zoom reaching it finds
/// the stage already there.
pub const WANT_PX: f64 = 0.8;
/// A stage is received while the view widened by this much each side meets it, so a pan
/// toward it finds it there.
pub const MARGIN_OCT: f64 = 1.0 / 3.0;
/// The daemon's publish refresh: a stage frame of a live curve may trail the shown `tf` by
/// its hop plus this.
const REFRESH_S: f64 = 0.25;

/// One stage's newest frame, as received.
#[derive(Clone, Copy, Debug)]
pub struct StageFrame<'a> {
    pub stamp: &'a FrameStamp,
    pub frame: &'a BinsFrame,
}

/// A live curve's bins: the layout its `tf` frame states and the stage frames held.
#[derive(Clone, Debug)]
pub struct BinSource<'a> {
    pub layout: &'a BinLayout,
    /// The shown `tf` frame's meta and stamp: a stage frame is drawn only beside the `tf`
    /// of its own configuration, delay and time.
    pub tf_meta: &'a TfMeta,
    pub tf_stamp: &'a FrameStamp,
    /// Per stage (the layout's order), its newest frame held.
    pub stages: Vec<Option<StageFrame<'a>>>,
    /// The measurement runs: its stage frames follow the `tf` within a hop. Stopped, both
    /// carry the final stamp.
    pub running: bool,
    /// Its measurement's peak hold is on.
    pub holds: bool,
}

/// Why a curve at bins is drawn on the grid however far the view is zoomed: a setting that
/// works on grid columns only. Its smoothing is a 1/n-octave average of grid columns; the
/// unwrapped phase and group delay are derived along the grid, and a pane reading its phase
/// there reads every part on the same column; a peak hold folds grid columns; peaks are
/// picked on grid columns. In that order of precedence: the curve's own setting first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Forced {
    Smoothed,
    UnwrappedPhase,
    GroupDelay,
    PeakHold,
    Peaks,
}

impl Forced {
    /// What a legend says after `bins`: `(grid: peak hold)`.
    pub fn text(self) -> &'static str {
        match self {
            Forced::Smoothed => "(grid: smoothed)",
            Forced::UnwrappedPhase => "(grid: unwrapped phase)",
            Forced::GroupDelay => "(grid: group delay)",
            Forced::PeakHold => "(grid: peak hold)",
            Forced::Peaks => "(grid: peaks)",
        }
    }
}

/// Whether a pane in `phase` view draws the grid everywhere for its phase view.
fn phase_forces(phase: PhaseView) -> Option<Forced> {
    match phase {
        PhaseView::Wrapped => None,
        PhaseView::Unwrapped { .. } => Some(Forced::UnwrappedPhase),
        PhaseView::GroupDelay { .. } => Some(Forced::GroupDelay),
    }
}

/// Whether a pane with `traces` in `phase` view draws the grid everywhere, and why: its phase
/// view first, then a peak hold, then peaks.
pub fn forced(traces: &[TfTrace<'_>], phase: PhaseView) -> Option<Forced> {
    if let Some(f) = phase_forces(phase) {
        Some(f)
    } else if traces
        .iter()
        .any(|t| t.hold.is_some() || t.bins.as_ref().is_some_and(|b| b.holds))
    {
        Some(Forced::PeakHold)
    } else if traces.iter().any(|t| t.tracks) {
        Some(Forced::Peaks)
    } else {
        None
    }
}

/// Why curve `t` in a pane forced by `pane` ([`forced`]) is drawn on the grid at any zoom:
/// its own smoothing before what the pane forces.
pub fn kept_on_grid(t: &TfTrace<'_>, pane: Option<Forced>) -> Option<Forced> {
    if t.smoothing.is_some() {
        Some(Forced::Smoothed)
    } else {
        pane
    }
}

/// The bins of stage `b` finer than the grid: its first and last bin outside every
/// unresolved range and every blend; `None` when it has none, or when the curve states no
/// unresolved ranges (where its columns are coarser than a stage's bins is then unknown,
/// so no bin can be said to be finer).
pub fn fine_bins(
    layout: &BinLayout,
    unresolved: Option<&[BinRange]>,
    b: usize,
) -> Option<(u32, u32)> {
    let unresolved = unresolved?;
    let s = layout.stages.get(b)?;
    let dx = s.bin_hz.0;
    if dx.is_nan() || dx <= 0.0 || s.bins == 0 {
        return None;
    }
    let coarse = |k: u32| {
        let f = f64::from(k) * dx;
        unresolved.iter().any(|r| f >= r.lo.0 && f <= r.hi.0)
            || layout.blends.iter().any(|r| f >= r.lo.0 && f <= r.hi.0)
    };
    let ks = s.first_bin..s.first_bin + s.bins;
    let lo = ks.clone().find(|k| !coarse(*k))?;
    let hi = ks.rev().find(|k| !coarse(*k))?;
    Some((lo, hi))
}

/// Logical pixels per octave of a plot `plot_px` wide showing `view`.
pub fn px_per_octave(view: Range, plot_px: f64) -> f64 {
    let oct = (view.hi / view.lo).log2();
    if oct.is_finite() && oct > 0.0 {
        plot_px / oct
    } else {
        0.0
    }
}

/// Spacing on screen of bins `bin_hz` apart at `hz`, logical px.
pub fn spacing_px(ppo: f64, bin_hz: f64, hz: f64) -> f64 {
    ppo * bin_hz / (hz * std::f64::consts::LN_2)
}

/// Stage `b`'s bin spacing at the view's top clamped into its fine range, when the view
/// widened by `margin_oct` each side meets that range.
fn stage_spacing(
    layout: &BinLayout,
    unresolved: Option<&[BinRange]>,
    b: usize,
    view: Range,
    plot_px: f64,
    margin_oct: f64,
) -> Option<f64> {
    let (lo, hi) = fine_bins(layout, unresolved, b)?;
    let dx = layout.stages[b].bin_hz.0;
    let (f_lo, f_hi) = (f64::from(lo) * dx, f64::from(hi) * dx);
    let k = 2f64.powf(margin_oct);
    if view.hi * k < f_lo || view.lo / k > f_hi {
        return None;
    }
    Some(spacing_px(
        px_per_octave(view, plot_px),
        dx,
        view.hi.clamp(f_lo, f_hi),
    ))
}

/// The stages a pane `plot_px` wide showing `view` receives: within a third of an octave of
/// the view and at least [`WANT_PX`] apart. A function of its inputs alone, so the same
/// view and width always want the same stages.
pub fn wanted(
    layout: &BinLayout,
    unresolved: Option<&[BinRange]>,
    view: Range,
    plot_px: f64,
) -> Vec<u8> {
    (0..layout.stages.len())
        .filter(|&b| {
            stage_spacing(layout, unresolved, b, view, plot_px, MARGIN_OCT)
                .is_some_and(|s| s >= WANT_PX)
        })
        .filter_map(|b| u8::try_from(b).ok())
        .collect()
}

/// Whether stage `b`'s frame can stand beside the shown `tf`: same configuration, delay
/// and nudge, an average to show, and of the same time (within a hop and the publish
/// refresh either way while running; the same stamp once stopped).
pub fn fresh<'s, 'a>(src: &'s BinSource<'a>, b: usize) -> Option<&'s StageFrame<'a>> {
    let s = src.stages.get(b)?.as_ref()?;
    let m = &s.frame.meta;
    let same = s.stamp.config_rev == src.tf_stamp.config_rev
        && s.stamp.session_epoch == src.tf_stamp.session_epoch
        && m.delay == src.tf_meta.delay
        && m.nudged == src.tf_meta.nudged
        && m.blocks > 0
        && usize::from(m.stage) == b;
    if !same {
        return None;
    }
    let (tf_at, at) = (src.tf_stamp.audio_sample.0, s.stamp.audio_sample.0);
    let timely = if src.running {
        // A stage frame and the `tf` of one emit share a stamp, but a stage is also sent
        // alone between `tf` refreshes, and the `tf` shown may be the older of the two:
        // either may lead by up to a hop of that stage plus the refresh. Further apart they
        // are averages of different times.
        let full_rate = m.rate.0 * f64::from(m.factor);
        let apart = (i128::from(tf_at) - i128::from(at)).unsigned_abs();
        (apart as f64) <= m.hop.0 as f64 + REFRESH_S * full_rate
    } else {
        tf_at == at
    };
    timely.then_some(s)
}

/// Bins drawn on one curve, as columns of its drawn trace.
#[derive(Clone, Debug, PartialEq)]
pub struct BinSpan {
    pub stage: u8,
    pub bin_hz: f64,
    /// Columns of the drawn trace holding this stage's bins.
    pub cols: Cols<usize>,
}

/// Curves as drawn with their bins in place of the grid where those are drawn, and each
/// curve's spans (aligned with the traces).
#[derive(Clone, Debug, PartialEq)]
pub struct Spliced {
    pub traces: Vec<DisplayTrace>,
    pub spans: Vec<Vec<BinSpan>>,
}

impl Spliced {
    /// The span holding column `col` of curve `i`.
    pub fn span_at(&self, i: usize, col: usize) -> Option<&BinSpan> {
        self.spans.get(i)?.iter().find(|s| s.cols.contains(&col))
    }

    /// Whether curve `i` draws a stage next to blend `j` (between stages `j` and `j + 1`).
    fn beside_blend(&self, i: usize, j: usize) -> bool {
        self.spans
            .get(i)
            .into_iter()
            .flatten()
            .any(|s| usize::from(s.stage) == j || usize::from(s.stage) == j + 1)
    }
}

/// One stage's bins to draw on a curve: its frame and the bins `lo..=hi`.
struct Run<'f> {
    stage: u8,
    frame: &'f BinsFrame,
    lo: u32,
    hi: u32,
}

/// The runs of bins curve `t` draws in `view` on a plot `plot_px` wide: of each fresh
/// stage at least [`DRAW_PX`] apart, its fine bins in the view and the first beyond each
/// edge. Empty for a curve [`kept_on_grid`] by its own smoothing (its bins are not
/// smoothed); the pane's reasons are `splice`'s.
fn runs<'f>(t: &'f TfTrace<'_>, view: Range, plot_px: f64) -> Vec<Run<'f>> {
    let Some(src) = t.bins.as_ref().filter(|_| kept_on_grid(t, None).is_none()) else {
        return Vec::new();
    };
    let unresolved = src.tf_meta.unresolved.as_deref();
    let mut out: Vec<Run<'f>> = (0..src.layout.stages.len())
        .filter_map(|b| {
            let s = stage_spacing(src.layout, unresolved, b, view, plot_px, 0.0)?;
            if s < DRAW_PX {
                return None;
            }
            let f = fresh(src, b)?;
            let (lo, hi) = fine_bins(src.layout, unresolved, b)?;
            let dx = f.frame.meta.bin_hz.0;
            let below = (view.lo / dx).floor() as u32;
            let above = (view.hi / dx).ceil() as u32;
            let (lo, hi) = (lo.max(below), hi.min(above));
            let first = f.frame.meta.first_bin;
            let n = f.frame.mag.len() as u32;
            let (lo, hi) = (lo.max(first), hi.min((first + n).saturating_sub(1)));
            (lo <= hi).then_some(Run {
                stage: f.frame.meta.stage,
                frame: f.frame,
                lo,
                hi,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        let at = |r: &Run<'_>| f64::from(r.lo) * r.frame.meta.bin_hz.0;
        at(a).total_cmp(&at(b))
    });
    out
}

/// The curves as a pane `plot_px` wide draws them in `view`; `None` when none draws a bin
/// (every curve as `shown`): under [`forced`] (a phase view not wrapped, a peak hold,
/// peaks), so that every part of the pane, its cursor, mark and jumps read the one grid it
/// draws.
pub fn splice(
    traces: &[TfTrace<'_>],
    shown: &[DisplayTrace],
    view: &ViewState,
    plot_px: f64,
) -> Option<Spliced> {
    if forced(traces, view.tf.phase).is_some() {
        return None;
    }
    let range = view.freq.range();
    let all: Vec<Vec<Run<'_>>> = traces.iter().map(|t| runs(t, range, plot_px)).collect();
    if all.iter().all(Vec::is_empty) {
        return None;
    }
    let (traces, spans) = shown
        .iter()
        .zip(all)
        .map(|(d, runs)| {
            if runs.is_empty() {
                (d.clone(), Vec::new())
            } else {
                with_bins(d, &runs, &view.tf.coherence)
            }
        })
        .unzip();
    Some(Spliced { traces, spans })
}

/// `d` with `runs` in place of the grid columns they cover; the columns either side stay,
/// so the line runs from grid to bins without a gap.
fn with_bins(
    d: &DisplayTrace,
    runs: &[Run<'_>],
    style: &CoherenceStyle,
) -> (DisplayTrace, Vec<BinSpan>) {
    let mut out = DisplayTrace {
        key: d.key,
        name: d.name.clone(),
        color: d.color,
        relation: d.relation,
        shift_s: d.shift_s,
        offset_db: d.offset_db,
        inverted: d.inverted,
        freqs: Vec::new(),
        hold_db: None,
        magnitude_db: Vec::new(),
        phase_wrapped_deg: Vec::new(),
        phase_unwrapped_deg: Vec::new(),
        group_delay_s: Vec::new(),
        coherence: Vec::new(),
        alpha: Vec::new(),
        freshness: d.freshness,
        smoothing: d.smoothing,
        note: d.note.clone(),
        dashed: d.dashed,
        joined: Vec::new(),
    };
    let joined = !d.joined.is_empty();
    let grid = |out: &mut DisplayTrace, i: usize| {
        out.freqs.push(d.freqs[i]);
        out.magnitude_db.push(d.magnitude_db[i]);
        out.phase_wrapped_deg.push(d.phase_wrapped_deg[i]);
        out.phase_unwrapped_deg.push(d.phase_unwrapped_deg[i]);
        out.group_delay_s.push(d.group_delay_s[i]);
        out.coherence.push(d.coherence[i]);
        out.alpha.push(d.alpha[i]);
        if joined {
            out.joined.push(d.joined.get(i).copied().flatten());
        }
    };
    let flip = if d.inverted { 180.0 } else { 0.0 };
    let mut spans = Vec::new();
    let mut i = 0;
    for r in runs {
        let dx = r.frame.meta.bin_hz.0;
        let (f_a, f_b) = (f64::from(r.lo) * dx, f64::from(r.hi) * dx);
        while i < d.freqs.len() && d.freqs[i] < f_a {
            grid(&mut out, i);
            i += 1;
        }
        while i < d.freqs.len() && d.freqs[i] <= f_b {
            i += 1;
        }
        let start = out.freqs.len();
        for k in r.lo..=r.hi {
            let e = (k - r.frame.meta.first_bin) as usize;
            let f = f64::from(k) * dx;
            let at = |v: &[f32]| v.get(e).map_or(f64::NAN, |x| f64::from(*x));
            let mag = at(&r.frame.mag);
            let valid = mag.is_finite();
            let g2 = if valid { at(&r.frame.coh) } else { f64::NAN };
            let blanked = style
                .blank_below
                .is_some_and(|th| g2.is_finite() && g2 < f64::from(th));
            let shown = valid && !blanked;
            let ph = at(&r.frame.phase) + flip;
            out.freqs.push(f);
            out.magnitude_db
                .push(if shown { mag + d.offset_db } else { f64::NAN });
            out.phase_wrapped_deg.push(if shown && ph.is_finite() {
                wrap_deg(ph - 360.0 * f * d.shift_s)
            } else {
                f64::NAN
            });
            // The unwrapped phase and group delay are the grid's only: those views draw it.
            out.phase_unwrapped_deg.push(f64::NAN);
            out.group_delay_s.push(f64::NAN);
            out.coherence.push(g2);
            out.alpha.push(coherence_alpha(g2, style));
            if joined {
                out.joined.push(None);
            }
        }
        spans.push(BinSpan {
            stage: r.stage,
            bin_hz: dx,
            cols: start..out.freqs.len(),
        });
    }
    while i < d.freqs.len() {
        grid(&mut out, i);
        i += 1;
    }
    (out, spans)
}

/// Whether a pane in `phase` view draws and reads bins at all: the unwrapped phase and
/// group delay exist on the grid only, and a pane whose phase reads the grid draws its
/// magnitude and coherence there too, so one column is under the cursor in every part.
pub fn reads_bins(phase: PhaseView) -> bool {
    phase_forces(phase).is_none()
}

/// What the cursor's frequency line says of the column it reads on curve `i` (column `col`
/// of `drawn`): `12 Hz bin` on a drawn bin, `1/96 oct` on a resolved grid column of a
/// curve that has bins; `None` elsewhere (an unresolved column says its bin width through
/// its mark).
pub fn form_note(
    traces: &[TfTrace<'_>],
    spliced: Option<&Spliced>,
    i: usize,
    col: usize,
    hz: f64,
) -> Option<String> {
    if let Some(s) = spliced.and_then(|s| s.span_at(i, col)) {
        return Some(format!("{} bin", format::bin_width(s.bin_hz)));
    }
    let t = traces.get(i)?;
    let src = t.bins.as_ref()?;
    let unresolved = src
        .tf_meta
        .unresolved
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|r| hz >= r.lo.0 && hz <= r.hi.0);
    (!unresolved).then(|| format::resolution(Resolution::NinetySixth))
}

/// The curve whose unresolved ranges a pane marks: the selected shown curve that states
/// them, else the first of the pane's own (not compared) that does; an index into `traces`.
pub fn marked(traces: &[TfTrace<'_>], shown: &[DisplayTrace]) -> Option<usize> {
    let known = || {
        traces
            .iter()
            .enumerate()
            .filter(|(_, t)| t.unresolved.is_some() && shown.iter().any(|d| d.key == t.key))
    };
    known()
        .find(|(_, t)| t.selected)
        .or_else(|| known().find(|(_, t)| !t.compared))
        .map(|(i, _)| i)
}

/// The blends next to a stage drawn on the curve the unresolved marks are of (`marked`,
/// an index into `traces`), as marks that say the grid is drawn there: its own layout and
/// its own drawn stages, since curves of other ladders or rates blend elsewhere. Never
/// shaded (each lies inside an unresolved mark already).
pub fn blend_marks(
    traces: &[TfTrace<'_>],
    spliced: &Spliced,
    marked: Option<usize>,
) -> Vec<crate::unresolved::RangeMark> {
    const NOTE: &str = "grid";
    let Some((i, layout)) = marked.and_then(|i| Some((i, traces.get(i)?.bins.as_ref()?.layout)))
    else {
        return Vec::new();
    };
    layout
        .blends
        .iter()
        .enumerate()
        .filter(|(j, _)| spliced.beside_blend(i, *j))
        .map(|(_, r)| crate::unresolved::RangeMark {
            lo_hz: r.lo.0,
            hi_hz: r.hi.0,
            from_bottom: false,
            text: format!(
                "{}–{}: {NOTE}",
                format::freq_rough(r.lo.0),
                format::freq_rough(r.hi.0)
            ),
            note: NOTE.to_owned(),
        })
        .collect()
}

/// What the legend of a curve with bins (a transfer at `bins`, a math over such) adds about
/// them in this pane: `bins` while one of its stages is drawn (with `range band on grid
/// parts` when its range band shows), `bins (grid: smoothed)` and the like while a setting
/// keeps it on the grid ([`kept_on_grid`] of the pane's [`forced`]); nothing when only the
/// zoom does. Per pane, since what forces the grid is.
pub fn legend_tags(
    t: &TfTrace<'_>,
    spans: &[BinSpan],
    forced: Option<Forced>,
    band_shown: bool,
) -> Vec<String> {
    if t.bins.is_none() {
        return Vec::new();
    }
    match kept_on_grid(t, forced) {
        Some(f) => vec![format!("bins {}", f.text())],
        None if spans.is_empty() => Vec::new(),
        None if band_shown && t.range.is_some_and(|r| r.band.is_some()) => {
            vec!["bins".into(), "range band on grid parts".into()]
        }
        None => vec!["bins".into()],
    }
}

/// `edges` (column-aligned with `freqs`) left out where `spans` of the drawn trace `drawn`
/// cover: an average's range band is per grid column, so it is not drawn under bins.
pub fn band_outside(
    freqs: &[f64],
    edges: &mut [(f64, f64)],
    drawn: &DisplayTrace,
    spans: &[BinSpan],
) {
    for s in spans {
        let (Some(lo), Some(hi)) = (
            drawn.freqs.get(s.cols.start),
            s.cols.end.checked_sub(1).and_then(|e| drawn.freqs.get(e)),
        ) else {
            continue;
        };
        for (f, e) in freqs.iter().zip(edges.iter_mut()) {
            if f >= lo && f <= hi {
                *e = (f64::NAN, f64::NAN);
            }
        }
    }
}
