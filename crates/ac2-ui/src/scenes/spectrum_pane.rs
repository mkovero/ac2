//! The spectrum pane: its scale, its traces and banner, and the spectrograph under it.

use std::sync::Arc;

use ac2_client::TopicFrame;
use ac2_proto::FrameData;
use ac2_proto::model::{LevelScale, Measurement, TraceKind};
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

use crate::state::{AppState, PaneId};

use super::{Now, frame, freshness, math_status, pane_order, shown_math, status};

/// The spectrum / RTA view.
/// The scale the spectrum pane shows its curves in: dB SPL when every curve it draws (live
/// and stored) is calibrated, else dBFS. Picks which of the pane's two level ranges applies.
pub fn spectrum_scale(st: &AppState) -> LevelScale {
    let mut scales = Vec::new();
    for m in st.measurements() {
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
        if !t.meta.edit.visible {
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
    let empty = SpectrographHistory::new(st.view.spectrum.spectrograph.span_s);
    let input = shown.map(|(m, stream)| {
        let history = st.spectrographs.get(&m.id).unwrap_or(&empty);
        let scale = history.scale().unwrap_or_else(|| spectrum_scale(st));
        SpectrographInput {
            history,
            name: m.config.name.clone(),
            range: st.view_for(pane).spectrum.range(scale),
            offset_db: st.edit(m.id).offset_db,
            freshness: frame(st, m.id, stream).map(|tf| freshness(st, tf)),
        }
    });
    with_spectrum(st, pane, theme, now, |traces, status, view| {
        spectrograph_scene(traces, status, input.as_ref(), view, theme, size)
    })
}

/// Calls `f` with the spectrum pane's traces (live, then stored), its banner status and
/// the view on the level range of the scale its curves are in.
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
    let mut cols = Vec::new();
    for (_, m) in pane_order(st, pane) {
        if st.meas_hidden(m) {
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
    let mut traces = Vec::new();
    for ((c, note), drawn) in cols.iter().zip(&notes).zip(&drawn) {
        let name = c.meas.config.name.clone();
        let color = colours.meas(c.meas.id);
        let mut t = match &c.tf.frame.data {
            FrameData::Spec(f) => SpectrumTrace::spectrum(
                f,
                c.tf.frame.stamp.capture_wall_ns,
                note.as_deref(),
                &c.cols.freqs,
                &c.cols.edges,
                name,
                color,
                freshness(st, c.tf),
            ),
            FrameData::Rta(f) => SpectrumTrace::rta(
                f,
                c.tf.frame.stamp.capture_wall_ns,
                note.as_deref(),
                &c.cols.freqs,
                &c.cols.edges,
                name,
                color,
                freshness(st, c.tf),
            ),
            _ => continue,
        };
        t.drawn = drawn.as_deref();
        if st.view.spectrum.peak_hold {
            t.peak = st.peaks.get(&c.meas.id).map(|p| p.2.values());
        }
        t.offset_db = st.edit(c.meas.id).offset_db;
        // A math channel's caption is its expression and what it means.
        if let Some(m) = math_status(st, c.meas, &c.tf.frame.data) {
            t.caption = m.tag();
        }
        if t.quantity.is_narrow() {
            t.bin_hz = c.cols.bin_hz;
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
        .filter(|(t, _)| t.meta.edit.visible)
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
            bands: None,
        });
    }
    let shown: Vec<&TopicFrame> = cols.iter().map(|c| c.tf).collect();
    let mut status = status(st, &shown, None, now);
    let frames: Vec<(MeasId, &TopicFrame)> = cols.iter().map(|c| (c.meas.id, c.tf)).collect();
    status.math = shown_math(st, st.pane_meas(pane), &frames);
    // The pane draws on the level range of the scale its curves are in.
    let mut view = st.view_for(pane);
    view.spectrum.level = view.spectrum.range(spectrum_scale(st));
    f(&traces, &status, &view)
}
