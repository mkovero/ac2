//! Spectrograph: the level of a spectrum / RTA measurement over frequency and time, as a
//! colour field under the spectrum, frequency on the spectrum's own axis (it follows zoom
//! and pan) and the newest frame at the top.
//!
//! The history is display state kept by the UI between frames, like [`PeakHold`]: the
//! frames' columns resampled once, when they arrive, onto an even log-frequency grid of
//! [`ROWS_PER_OCTAVE`], and placed on a time grid of [`SLOTS`] slots over the history
//! length by their capture time. A frame stands for the time since the previous one, so a
//! long FFT that updates a few times a second fills its slots, and so does a run of frames
//! the link left out because they drew the same. Time without frames (the stream went
//! STALE, the measurement stopped) is a gap, never the last spectrum smeared across it.
//!
//! The renderer keeps the history's columns on the GPU and uploads only the slots a new
//! frame wrote ([`crate::primitives::Heatmap`]); zoom, pan and the level range change the
//! drawing, not the texture.
//!
//! [`PeakHold`]: crate::spectrum::PeakHold

use std::sync::Arc;

use ac2_proto::GridId;
use ac2_proto::frame::ValidityMask;
use ac2_proto::model::LevelScale;
use ac2_proto::units::WallNs;

use crate::axis::{self, Axis, Range, TickKind};
use crate::banner::Status;
use crate::canvas::{self, Canvas, MARGINS, PANE_GAP, anchor, label};
use crate::format;
use crate::primitives::{
    FillRect, HAlign, Heatmap, HeatmapAxes, HeatmapColumn, HeatmapId, Polyline, Rect, Scene,
    VAlign, Viewport,
};
use crate::spectrum::{SpectrumScene, SpectrumTrace, offset_note, scale_unit, spectrum_scene_in};
use crate::theme::Theme;
use crate::time::Freshness;
use crate::view::{FREQ_LIMIT_HI, FREQ_LIMIT_LO, ViewState};

/// Time slots in the history, whatever its length: 30 s at 33 ms, about the spectrum's
/// fastest update, and well inside the 4096 texels a Pi 4 class GPU allows per texture side.
pub const SLOTS: usize = 900;
/// Frequency rows per octave: the spectrum's own display columns per octave, so the
/// spectrograph resolves what the curve above it shows.
pub const ROWS_PER_OCTAVE: f64 = 96.0;
/// At most this many frequency rows (about 21 octaves at [`ROWS_PER_OCTAVE`]).
pub const MAX_ROWS: usize = 2048;

/// A grid's columns resampled onto rows evenly spaced in log frequency: each row is the
/// highest level among the columns that overlap it, so a tone in one narrow column keeps
/// its level, and a wide column (a third-octave band, a low single-bin column) fills every
/// row it spans.
#[derive(Clone, Debug, PartialEq)]
pub struct RowMap {
    lo_hz: f64,
    hi_hz: f64,
    /// Columns `[first, end)` overlapping each row.
    cols: Vec<(u32, u32)>,
}

impl RowMap {
    /// The rows for columns with band `edges` (ascending), from the lowest positive edge to
    /// the highest; `None` when that is no span (no column above 0 Hz).
    pub fn new(edges: &[(f64, f64)]) -> Option<Self> {
        let lo = edges
            .iter()
            .flat_map(|e| [e.0, e.1])
            .find(|f| *f > 0.0)?
            .max(FREQ_LIMIT_LO);
        let hi = edges.last()?.1.min(FREQ_LIMIT_HI);
        if hi.is_nan() || hi <= lo {
            return None;
        }
        let rows = ((hi / lo).log2() * ROWS_PER_OCTAVE)
            .ceil()
            .clamp(1.0, MAX_ROWS as f64) as usize;
        let step = (hi / lo).ln() / rows as f64;
        let cols = (0..rows)
            .map(|r| {
                let a = lo * (step * r as f64).exp();
                let b = lo * (step * (r + 1) as f64).exp();
                let first = edges.partition_point(|e| e.1 <= a);
                let end = edges.partition_point(|e| e.0 < b).max(first);
                (first as u32, end as u32)
            })
            .collect();
        Some(Self {
            lo_hz: lo,
            hi_hz: hi,
            cols,
        })
    }

    pub fn rows(&self) -> usize {
        self.cols.len()
    }

    /// Lower edge of the lowest row, Hz.
    pub fn lo_hz(&self) -> f64 {
        self.lo_hz
    }

    /// Upper edge of the highest row, Hz.
    pub fn hi_hz(&self) -> f64 {
        self.hi_hz
    }

    /// The row `hz` falls in; `None` outside the rows.
    pub fn row_of(&self, hz: f64) -> Option<usize> {
        if !(hz >= self.lo_hz && hz < self.hi_hz) {
            return None;
        }
        let t = (hz / self.lo_hz).ln() / (self.hi_hz / self.lo_hz).ln();
        Some(((t * self.rows() as f64) as usize).min(self.rows() - 1))
    }

    /// One frame's `level` per column (NaN, or a validity flag set, is no value) as rows;
    /// a row without a value is NaN.
    pub fn resample(&self, level: &[f32], validity: Option<&[ValidityMask]>) -> HeatmapColumn {
        let ok =
            |i: usize| validity.is_none_or(|v| v.get(i).is_some_and(|m| *m == ValidityMask::NONE));
        self.cols
            .iter()
            .map(|&(a, b)| {
                (a as usize..(b as usize).min(level.len()))
                    .filter(|&i| ok(i) && level[i].is_finite())
                    .map(|i| level[i])
                    .fold(f32::NAN, f32::max)
            })
            .collect()
    }
}

/// One spectrum / RTA frame as the history takes it.
#[derive(Clone, Copy, Debug)]
pub struct SpectrographFrame<'a> {
    pub seq: u64,
    /// Capture time on the daemon's clock: frames are placed by when they were measured,
    /// not by when the link delivered them.
    pub at: WallNs,
    pub grid: GridId,
    /// Band edges of the grid's columns ([`crate::grid::column_edges`]).
    pub edges: &'a [(f64, f64)],
    pub scale: LevelScale,
    pub level: &'a [f32],
    pub validity: Option<&'a [ValidityMask]>,
}

/// The spectrograph history of one measurement: a ring of [`SLOTS`] time slots of
/// resampled frames, newest at `newest`.
#[derive(Clone, Debug)]
pub struct SpectrographHistory {
    span_s: u32,
    slot_ns: u64,
    grid: Option<GridId>,
    scale: Option<LevelScale>,
    map: Option<Arc<RowMap>>,
    ring: Vec<Option<HeatmapColumn>>,
    /// Absolute slot (capture time / slot length) of the newest frame.
    newest: Option<u64>,
    last_seq: Option<u64>,
    /// The stream had a break (stale, stopped) since the newest frame.
    broken: bool,
}

impl SpectrographHistory {
    /// An empty history `span_s` seconds long.
    pub fn new(span_s: u32) -> Self {
        let span_s = span_s.max(1);
        Self {
            span_s,
            slot_ns: u64::from(span_s) * 1_000_000_000 / SLOTS as u64,
            grid: None,
            scale: None,
            map: None,
            ring: vec![None; SLOTS],
            newest: None,
            last_seq: None,
            broken: false,
        }
    }

    pub fn span_s(&self) -> u32 {
        self.span_s
    }

    /// Length of one time slot, seconds.
    pub fn slot_s(&self) -> f64 {
        self.slot_ns as f64 / 1e9
    }

    /// The level scale of the frames held.
    pub fn scale(&self) -> Option<LevelScale> {
        self.scale
    }

    pub fn rows(&self) -> Option<&RowMap> {
        self.map.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.newest.is_none()
    }

    /// The ring of slots by ring position.
    pub fn ring(&self) -> &[Option<HeatmapColumn>] {
        &self.ring
    }

    /// Ring position of the oldest slot shown (the bottom of the picture).
    pub fn scroll(&self) -> u32 {
        self.newest.map_or(0, |n| ((n + 1) % SLOTS as u64) as u32)
    }

    /// Forgets every frame (the grid and scale are taken again from the next one).
    pub fn clear(&mut self) {
        self.ring.iter_mut().for_each(|c| *c = None);
        self.newest = None;
        self.broken = false;
    }

    /// The stream stopped delivering (STALE, measurement stopped): the time until the next
    /// frame is a gap.
    pub fn mark_break(&mut self) {
        self.broken = true;
    }

    /// Takes frame `f`; whether the picture changed. A frame seen already (same `seq`) is
    /// ignored. A new grid or level scale starts over: rows of another grid, or levels in
    /// another unit, cannot share a colour field.
    pub fn push(&mut self, f: &SpectrographFrame<'_>) -> bool {
        if self.last_seq == Some(f.seq) {
            return false;
        }
        self.last_seq = Some(f.seq);
        if self.grid != Some(f.grid) || self.scale != Some(f.scale) || self.map.is_none() {
            self.clear();
            self.grid = Some(f.grid);
            self.scale = Some(f.scale);
            self.map = RowMap::new(f.edges).map(Arc::new);
        }
        let Some(map) = &self.map else {
            return false;
        };
        let col = map.resample(f.level, f.validity);
        let slot = f.at.0 / self.slot_ns;
        let n = SLOTS as u64;
        let at = |a: u64| (a % n) as usize;
        match self.newest {
            // Two frames in one slot: the slot shows the higher level of each row, as a
            // pixel over several slots does.
            Some(p) if slot == p => {
                let merged = match &self.ring[at(p)] {
                    Some(old) => old.iter().zip(col.iter()).map(|(a, b)| a.max(*b)).collect(),
                    None => col,
                };
                self.ring[at(p)] = Some(merged);
            }
            Some(p) if slot > p && slot - p < n => {
                // Without a break, the slots since the previous frame held what it showed
                // (the link leaves out frames that draw the same); after one, nothing.
                let fill = if self.broken {
                    None
                } else {
                    self.ring[at(p)].clone()
                };
                for a in p + 1..slot {
                    self.ring[at(a)] = fill.clone();
                }
                self.ring[at(slot)] = Some(col);
            }
            // First frame, more than a history after the last, or the clock went back.
            _ => {
                self.ring.iter_mut().for_each(|c| *c = None);
                self.ring[at(slot)] = Some(col);
            }
        }
        self.newest = Some(slot);
        self.broken = false;
        true
    }

    /// The level at `hz`, `before_s` seconds before the newest frame; `None` outside the
    /// history or where there is no value.
    pub fn value_at(&self, hz: f64, before_s: f64) -> Option<f32> {
        let newest = self.newest?;
        if before_s.is_nan() || before_s < 0.0 {
            return None;
        }
        let back = (before_s / self.slot_s()).floor() as u64;
        if back >= SLOTS as u64 || back > newest {
            return None;
        }
        let row = self.rows()?.row_of(hz)?;
        let col = self.ring[((newest - back) % SLOTS as u64) as usize].as_ref()?;
        col.get(row).copied().filter(|v| v.is_finite())
    }
}

/// What the spectrograph shows: the history of the pane's measurement and how to read it.
#[derive(Clone, Debug)]
pub struct SpectrographInput<'a> {
    pub history: &'a SpectrographHistory,
    pub name: String,
    /// The level range the colours span (the pane's level axis for the history's scale).
    pub range: Range,
    /// The measurement's display offset, dB, added to its levels as on its curve.
    pub offset_db: f64,
    pub freshness: Option<Freshness>,
}

/// Cursor values in the spectrograph.
#[derive(Clone, Debug, PartialEq)]
pub struct SpectrographCursor {
    pub freq_hz: f64,
    pub before_s: f64,
    /// `1.00 kHz · 4.2 s ago · −23.5 dBFS`.
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpectrographScene {
    /// The whole picture: the spectrum on top, the spectrograph under it.
    pub scene: Scene,
    /// The spectrum part as built; its `scene` is empty (its layers are in `scene`).
    pub spectrum: SpectrumScene,
    /// The spectrograph's plot rectangle.
    pub plot: Rect,
    /// Frequency, as the spectrum's (same pixels).
    pub x_axis: Axis,
    /// Seconds before the newest frame, 0 at the top.
    pub time_axis: Axis,
    /// The colour bar and its level axis.
    pub bar: Rect,
    pub bar_axis: Axis,
    /// Above the plot: `Main L · last 30 s · dBFS`, ` · stopped` when it is.
    pub caption: String,
    pub cursor: Option<SpectrographCursor>,
    /// Shown in the plot instead of a picture (nothing measured yet).
    pub message: Option<String>,
}

/// Share of the pane height the spectrum keeps above the spectrograph.
pub const SPECTRUM_SHARE: f32 = 0.4;
const BAR_GAP: f32 = 8.0;
const BAR_W: f32 = 10.0;
/// Right of the plots: the colour bar and its labels.
pub const RIGHT_MARGIN: f32 = BAR_GAP + BAR_W + 4.0 + 34.0;
/// Above the spectrograph plot: its caption and cursor readout.
const CAPTION_H: f32 = 16.0;
/// Entries of the colour bar: the colormap's own resolution.
const BAR_STEPS: usize = 256;

const HISTORY_ID: HeatmapId = HeatmapId(1);
const BAR_ID: HeatmapId = HeatmapId(2);

/// The spectrum pane with the spectrograph under it: the spectrum (`traces`, as
/// [`crate::spectrum::spectrum_scene`] draws them) in the top [`SPECTRUM_SHARE`], the
/// spectrograph of `sg` below on the same frequency pixels, a colour bar with the level
/// range right of it. `None` (no spectrum or RTA measurement) draws an empty plot that
/// says so.
pub fn spectrograph_scene(
    traces: &[SpectrumTrace<'_>],
    status: &Status,
    sg: Option<&SpectrographInput<'_>>,
    view: &ViewState,
    theme: &Theme,
    size: Viewport,
) -> SpectrographScene {
    let top_h = ((size.height - PANE_GAP) * SPECTRUM_SHARE).floor().max(1.0);
    let mut spectrum = spectrum_scene_in(
        traces,
        status,
        view,
        theme,
        Viewport {
            width: size.width,
            height: top_h,
        },
        RIGHT_MARGIN,
    );
    let mut layers = std::mem::take(&mut spectrum.scene.layers);
    let mut c = Canvas::default();
    let top = top_h + PANE_GAP;
    c.base.rects.push(FillRect {
        rect: Rect::new(0.0, top_h, size.width, (size.height - top_h).max(0.0)),
        color: theme.background,
        clip: None,
    });
    let plot = Rect::new(
        MARGINS.left,
        top + CAPTION_H,
        (size.width - MARGINS.left - RIGHT_MARGIN).max(1.0),
        (size.height - top - CAPTION_H - MARGINS.bottom).max(1.0),
    );
    let x_axis = axis::freq_axis(view.freq.range(), plot.x, plot.right());
    let span = sg.map_or(view.spectrum.spectrograph.span_s, |s| s.history.span_s());
    let mut time_axis = axis::linear_axis(
        Range::new(0.0, f64::from(span)),
        plot.y,
        plot.bottom(),
        "s ago",
    );
    for t in &mut time_axis.ticks {
        if let Some(l) = &mut t.label {
            l.push_str(" s");
        }
    }
    // Only the major lines over the colours: the minor ones would hatch them.
    let major = |a: &Axis| Axis {
        ticks: a
            .ticks
            .iter()
            .filter(|t| t.kind == TickKind::Major)
            .cloned()
            .collect(),
        ..a.clone()
    };
    canvas::pane_frame(
        &mut c,
        plot,
        &major(&x_axis),
        &major(&time_axis),
        true,
        "",
        theme,
    );

    let range = sg.map_or(view.spectrum.level, |s| s.range);
    let bar = Rect::new(plot.right() + BAR_GAP, plot.y, BAR_W, plot.h);
    let unit = sg
        .and_then(|s| s.history.scale())
        .map_or(String::new(), |s| scale_unit(s).to_string());
    let bar_axis = axis::linear_axis(range, bar.bottom(), bar.y, &unit);
    if range.is_valid() {
        let values: HeatmapColumn = (0..BAR_STEPS)
            .map(|i| (range.lo + range.span() * (i as f64 + 0.5) / BAR_STEPS as f64) as f32)
            .collect();
        c.base.heatmaps.push(Heatmap {
            id: BAR_ID,
            rect: bar,
            clip: None,
            columns: 1,
            rows: BAR_STEPS as u32,
            axes: HeatmapAxes::TimeAcross,
            scroll: 0,
            range: [range.lo as f32, range.hi as f32],
            colormap: theme.colormap,
            opacity: 1.0,
            data: vec![Some(values)],
        });
        for t in &bar_axis.ticks {
            if let Some(text) = &t.label {
                c.base.labels.push(label(
                    text.clone(),
                    [bar.right() + 4.0, t.pos],
                    anchor(HAlign::Left, VAlign::Center),
                    theme.small_font_size,
                    theme.axis_text,
                ));
            }
        }
    }

    let mut caption = String::new();
    let mut message = None;
    let mut cursor = None;
    let xm = x_axis.mapping;
    let tm = time_axis.mapping;
    match sg {
        None => message = Some("no spectrum or RTA measurement".to_string()),
        Some(s) => {
            // An offset measurement says so: its colours are of the offset level.
            let name = if s.offset_db != 0.0 {
                offset_note(&s.name, s.offset_db)
            } else {
                s.name.clone()
            };
            caption = format!("{name} · last {span} s");
            if let Some(scale) = s.history.scale() {
                caption.push_str(&format!(" · {}", scale_unit(scale)));
            }
            if s.freshness.is_some_and(|f| f.is_stopped()) {
                caption.push_str(" · stopped");
            }
            match s.history.rows() {
                Some(rows) if !s.history.is_empty() && range.is_valid() => {
                    let x0 = xm.to_px(rows.lo_hz());
                    let x1 = xm.to_px(rows.hi_hz());
                    let off = s.offset_db;
                    c.base.heatmaps.push(Heatmap {
                        id: HISTORY_ID,
                        rect: Rect::new(x0, plot.y, (x1 - x0).max(1.0), plot.h),
                        clip: Some(plot),
                        columns: SLOTS as u32,
                        rows: rows.rows() as u32,
                        axes: HeatmapAxes::TimeUp,
                        scroll: s.history.scroll(),
                        // The colours are those of the displayed (offset) level.
                        range: [(range.lo - off) as f32, (range.hi - off) as f32],
                        colormap: theme.colormap,
                        opacity: if s.freshness.is_some_and(|f| f.is_stale()) {
                            theme.stale_alpha
                        } else {
                            1.0
                        },
                        data: s.history.ring().to_vec(),
                    });
                }
                _ => message = Some(format!("{}: no frames yet", s.name)),
            }
            if let (Some(hz), Some(before)) = (view.cursor_hz, view.spectrum.spectrograph.cursor_s)
                && before <= f64::from(span)
            {
                let v = s
                    .history
                    .value_at(hz, before)
                    .map_or(f64::NAN, |v| f64::from(v) + s.offset_db);
                let level = match s.history.scale() {
                    Some(scale) if v.is_finite() => {
                        format!("{} {}", format::level(v), scale_unit(scale))
                    }
                    _ => format::level(f64::NAN),
                };
                cursor = Some(SpectrographCursor {
                    freq_hz: hz,
                    before_s: before,
                    text: format!(
                        "{} · {} ago · {level}",
                        format::freq_readout(hz),
                        format::age(before)
                    ),
                });
            }
        }
    }
    // The cursor readout wins the line above the plot when both do not fit.
    let room = cursor.as_ref().map_or(plot.w, |cur| {
        plot.w - canvas::text_width(&cur.text, theme.small_font_size) - 12.0
    });
    let shown_caption = if canvas::text_width(&caption, theme.small_font_size) <= room {
        caption.clone()
    } else {
        String::new()
    };
    c.base.labels.push(label(
        shown_caption,
        [plot.x, plot.y - 3.0],
        anchor(HAlign::Left, VAlign::Bottom),
        theme.small_font_size,
        theme.text_dim,
    ));
    if let Some(m) = &message {
        c.overlay.labels.push(label(
            m.clone(),
            [plot.x + plot.w / 2.0, plot.y + plot.h / 2.0],
            anchor(HAlign::Center, VAlign::Center),
            theme.small_font_size,
            theme.text_dim,
        ));
    }
    if let Some(hz) = view.cursor_hz {
        canvas::vline(&mut c.overlay, plot, xm.to_px(hz), theme.cursor);
    }
    if let Some(cur) = &cursor {
        let y = tm.to_px(cur.before_s);
        if y.is_finite() && y >= plot.y && y <= plot.bottom() {
            c.overlay.polylines.push(Polyline {
                points: vec![[plot.x, y], [plot.right(), y]],
                alpha: vec![],
                stroke: theme.cursor,
                clip: Some(plot),
            });
        }
        c.overlay.labels.push(label(
            cur.text.clone(),
            [plot.right(), plot.y - 3.0],
            anchor(HAlign::Right, VAlign::Bottom),
            theme.small_font_size,
            theme.text,
        ));
    }
    layers.extend(c.into_scene(size).layers.into_iter().take(3));
    SpectrographScene {
        scene: Scene {
            viewport: size,
            layers,
        },
        spectrum,
        plot,
        x_axis,
        time_axis,
        bar,
        bar_axis,
        caption,
        cursor,
        message,
    }
}

#[cfg(test)]
#[path = "spectrograph_tests.rs"]
mod tests;
