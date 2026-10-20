//! The spectrum pane: its scale, its traces and banner, and the spectrograph under it.

use std::sync::Arc;

use ac2_client::TopicFrame;
use ac2_proto::FrameData;
use ac2_proto::model::{Compared, LevelScale, Measurement, TraceKind};
use ac2_proto::units::MeasId;
use ac2_scene::banner::Status;
use ac2_scene::grid::{GridColumns, columns};
use ac2_scene::primitives::Viewport;
use ac2_scene::spectrograph::{
    SpectrographHistory, SpectrographInput, SpectrographScene, spectrograph_scene,
};
use ac2_scene::spectrum::{Quantity, SpectrumScene, SpectrumTrace, spectrum_scene};
use ac2_scene::theme::Theme;
use ac2_scene::time::Freshness;
use ac2_scene::trace::TraceKey;

use crate::state::{AppState, PaneId, PaneKind};

use super::{Now, frame, freshness, math_status, pane_order, shown_math, status};

/// The scale spectrum pane `pane` shows its curves in: dB SPL when every curve it draws
/// (its group's, live and stored, and those compared) is calibrated, else dBFS. Picks which
/// of the pane's two level ranges applies.
pub fn spectrum_scale(st: &AppState, pane: PaneId) -> LevelScale {
    let kind = PaneKind::Spectrum;
    let shown = st.pane_meas(pane);
    let mut scales = Vec::new();
    for m in st.measurements() {
        if !(st.live_on_pane(kind, m, shown) || st.live_compared_on(kind, m, shown)) {
            continue;
        }
        let Some(stream) = crate::state::spectrum_stream(m) else {
            continue;
        };
        match frame(st, m.id, stream).map(|tf| &tf.frame.data) {
            Some(FrameData::Spec(f)) => scales.push(f.meta.scale),
            Some(FrameData::Rta(f)) => scales.push(f.meta.scale),
            _ => {}
        }
    }
    for (t, _) in st.traces.values() {
        if !(st.on_pane(kind, &t.meta, shown) || st.trace_compared_on(kind, &t.meta, shown)) {
            continue;
        }
        match t.meta.kind {
            TraceKind::Spectrum { scale, .. } | TraceKind::Rta { scale } => scales.push(scale),
            _ => {}
        }
    }
    if !scales.is_empty() && scales.iter().all(|s| *s == LevelScale::DbSpl) {
        LevelScale::DbSpl
    } else {
        LevelScale::Dbfs
    }
}

/// The spectrum / RTA view.
pub fn spectrum(
    st: &AppState,
    pane: PaneId,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> SpectrumScene {
    with_spectrum(st, pane, theme, now, |traces, status, view| {
        spectrum_scene(traces, status, view, theme, size)
    })
}

/// The spectrum pane with the spectrograph of the pane's measurement under it.
pub fn spectrograph(
    st: &AppState,
    pane: PaneId,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> SpectrographScene {
    let shown = st.pane_meas(pane).and_then(|m| {
        if st.meas_hidden(m) {
            return None;
        }
        let stream = crate::state::spectrum_stream(m)?;
        Some((m, stream))
    });
    let display = shown.map(|(m, _)| st.display_of(m)).unwrap_or_default();
    let empty = SpectrographHistory::new(display.spectrograph_span_s);
    let input = shown.map(|(m, stream)| {
        let history = st
            .spectrographs
            .get(&m.id)
            .filter(|h| h.span_s() == display.spectrograph_span_s)
            .unwrap_or(&empty);
        let scale = history.scale().unwrap_or_else(|| spectrum_scale(st, pane));
        SpectrographInput {
            history,
            name: m.config.name.clone(),
            range: st.view_for(pane).spectrum.range(scale),
            offset_db: st.meas_offset(m.id),
            freshness: frame(st, m.id, stream).map(|tf| freshness(st, tf)),
            colours: display.spectrograph_colours,
        }
    });
    with_spectrum(st, pane, theme, now, |traces, status, view| {
        spectrograph_scene(traces, status, input.as_ref(), view, theme, size)
    })
}

/// Calls `f` with the spectrum pane's traces, its banner status and the view on the level
/// range of the scale its curves are in. The pane draws its measurement's group — its live
/// curve, the math channels made on it, its shown stored spectra and band sets — and the
/// curves compared into that measurement (C), tagged so; never another measurement's
/// otherwise, nor an imported trace. Its own group comes first (live, then stored), then
/// the compared curves in compare order: the order they stack in, top first. Each draws
/// with its own measurement's style and peak hold (a stored one with its owner's).
pub(crate) fn with_spectrum<R>(
    st: &AppState,
    pane: PaneId,
    theme: &Theme,
    now: Now,
    f: impl FnOnce(&[SpectrumTrace<'_>], &Status, &ac2_scene::ViewState) -> R,
) -> R {
    let grids = st.data.as_ref().map(|d| &d.grids);
    struct Col<'a> {
        meas: &'a Measurement,
        tf: &'a TopicFrame,
        cols: Arc<GridColumns>,
    }
    let colours = st.curve_colours(theme);
    let kind = PaneKind::Spectrum;
    let shown = st.pane_meas(pane);
    let mut cols = Vec::new();
    for (_, m) in pane_order(st, pane) {
        if !(st.live_on_pane(kind, m, shown) || st.live_compared_on(kind, m, shown)) {
            continue;
        }
        let Some(stream) = crate::state::spectrum_stream(m) else {
            continue;
        };
        let Some(tf) = frame(st, m.id, stream) else {
            continue;
        };
        let Some(g) = tf
            .frame
            .stamp
            .grid_id
            .and_then(|g| grids.and_then(|gs| gs.get(&g)))
        else {
            continue;
        };
        cols.push(Col {
            meas: m,
            tf,
            cols: columns(g),
        });
    }
    let notes: Vec<Option<String>> = cols
        .iter()
        .map(|c| {
            let applied = match &c.tf.frame.data {
                FrameData::Spec(f) => f.meta.mic_curve,
                FrameData::Rta(f) => f.meta.mic_curve,
                _ => false,
            };
            crate::state::meas_input(&c.meas.config.kind).and_then(|i| st.curve_note(i, applied))
        })
        .collect();
    // Where each live curve's glide to its newest frame has got to: fresh values only, so
    // nothing moves on screen that is not being measured.
    let drawn: Vec<Option<Vec<f32>>> = cols
        .iter()
        .map(|c| {
            if !st.prefs.spec_ease || !matches!(freshness(st, c.tf), Freshness::Fresh { .. }) {
                return None;
            }
            st.spec_glide
                .get(&c.tf.topic)?
                .drawn(&c.tf.frame, now.instant)
        })
        .collect();
    let tracked = shown
        .map(|m| m.id)
        .filter(|id| cols.iter().any(|c| c.meas.id == *id))
        .or_else(|| cols.first().map(|c| c.meas.id));
    let mut traces = Vec::new();
    for ((c, note), drawn) in cols.iter().zip(&notes).zip(&drawn) {
        let name = c.meas.config.name.clone();
        let color = colours.meas(c.meas.id);
        let meta = ac2_scene::spectrum::LiveMeta {
            captured: c.tf.frame.stamp.capture_wall_ns,
            curve: note.as_deref(),
            freqs: &c.cols.freqs,
            edges: &c.cols.edges,
            name,
            color,
            freshness: freshness(st, c.tf),
        };
        let mut t = match &c.tf.frame.data {
            FrameData::Spec(f) => SpectrumTrace::spectrum(f, meta),
            FrameData::Rta(f) => SpectrumTrace::rta(f, meta),
            _ => continue,
        };
        t.drawn = drawn.as_deref();
        let display = st.display_of(c.meas);
        t.style = display.style;
        if display.peak.holds() {
            t.peak = st.peaks.get(&c.meas.id).map(|p| p.2.values());
        }
        // One curve's peaks: the pane's own measurement's (the first live curve without
        // one). Five marks per curve on several curves crowd the plot, and a list of
        // fifteen rows is no longer read at a glance.
        if display.peak.tracks() && Some(c.meas.id) == tracked {
            t.tracked = st.peak_tracks.get(&c.meas.id).map(|p| p.1.peaks());
        }
        // Its own offset, compared into another measurement's pane too.
        t.offset_db = st.meas_offset(c.meas.id);
        // A math channel's caption is its expression and what it means.
        if let Some(m) = math_status(st, c.meas, &c.tf.frame.data) {
            t.caption = m.tag();
        }
        if t.quantity.is_narrow() {
            t.bin_hz = c.cols.bin_hz;
        }
        t.compared = st.live_compared_on(kind, c.meas, shown);
        if t.compared {
            t.layer = st.compare_layer(shown, Compared::Meas { meas: c.meas.id });
        }
        traces.push(t);
    }
    // Stored spectra / RTA bands, drawn under the live ones' axis rules.
    struct Stored<'a> {
        data: &'a ac2_proto::model::TraceData,
        scale: LevelScale,
        quantity: Quantity,
        cols: Arc<GridColumns>,
    }
    let stored: Vec<Stored<'_>> = st
        .traces
        .values()
        .filter(|(t, _)| {
            st.on_pane(kind, &t.meta, shown) || st.trace_compared_on(kind, &t.meta, shown)
        })
        .filter_map(|(t, g)| {
            let (scale, q) = match t.meta.kind {
                TraceKind::Spectrum { scale, level } => (
                    scale,
                    // Served smoothed at its setting (`ac2_traces::smooth`).
                    Quantity::narrow(level, t.meta.edit.smoothing.map(|s| s.fraction)),
                ),
                TraceKind::Rta { scale } => (scale, Quantity::Band),
                _ => return None,
            };
            Some(Stored {
                data: t.as_ref(),
                scale,
                quantity: q,
                cols: columns(g),
            })
        })
        .collect();
    for Stored {
        data,
        scale,
        quantity,
        cols,
    } in &stored
    {
        let compared = st.trace_compared_on(kind, &data.meta, shown);
        traces.push(SpectrumTrace {
            key: TraceKey::Stored(data.meta.id),
            name: data.meta.edit.name.clone(),
            color: colours.trace(data.meta.id),
            freqs: &cols.freqs,
            edges: &cols.edges,
            level: &data.mag_db,
            drawn: None,
            validity: None,
            peak: None,
            tracked: None,
            scale: *scale,
            quantity: *quantity,
            bin_hz: match quantity {
                Quantity::Band => None,
                _ => cols.bin_hz,
            },
            caption: match ac2_scene::trace::curve_note(
                data.meta.mic.as_ref(),
                data.meta.mic_curve.as_deref(),
            ) {
                Some(n) => format!("stored · {n}"),
                None => "stored".into(),
            },
            freshness: None,
            offset_db: data.meta.edit.offset.0,
            selected: st.selected_trace == Some(data.meta.id),
            compared,
            bands: None,
            style: st.trace_display(&data.meta).style,
            layer: if compared {
                st.compare_layer(
                    shown,
                    Compared::Trace {
                        trace: data.meta.id,
                    },
                )
            } else {
                0
            },
        });
    }
    // Stable: the own group keeps live before stored.
    traces.sort_by_key(|t| t.layer);
    let drawn_frames: Vec<&TopicFrame> = cols.iter().map(|c| c.tf).collect();
    let mut status = status(st, &drawn_frames, None, now);
    let frames: Vec<(MeasId, &TopicFrame)> = cols.iter().map(|c| (c.meas.id, c.tf)).collect();
    status.math = shown_math(st, shown, &frames);
    // The pane draws on the level range of the scale its curves are in.
    let mut view = st.view_for(pane);
    view.spectrum.level = view.spectrum.range(spectrum_scale(st, pane));
    f(&traces, &status, &view)
}
