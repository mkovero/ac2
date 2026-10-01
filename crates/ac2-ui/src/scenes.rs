//! Assembles `ac2-scene` inputs from the mirrored state, the latest frames and the
//! operator's display choices, and calls the builders. Bookkeeping only: which frames go to
//! which pane, names, colours, freshness. Every displayed number is made by `ac2-scene`.

use std::time::Instant;

use ac2_client::TopicFrame;
use ac2_proto::FrameData;
use ac2_proto::frame::ProtectionFlags;
use ac2_proto::model::{LevelScale, MeasKind, Measurement, Polarity, TraceSource};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{MeasId, Seconds, WallNs};
use ac2_scene::banner::{Status, no_delay_estimate};
use ac2_scene::grid::{column_edges, column_frequencies};
use ac2_scene::ir::{IrScene, ir_scene};
use ac2_scene::primitives::Viewport;
use ac2_scene::spectrum::{SpectrumScene, SpectrumTrace, spectrum_scene};
use ac2_scene::spl::{SplScene, cal_text, spl_readout, spl_scene};
use ac2_scene::tf::{TfScene, transfer_scene};
use ac2_scene::theme::Theme;
use ac2_scene::time::{ClockOffset, Freshness};
use ac2_scene::trace::{TfTrace, TimeBase};

use crate::state::AppState;

/// Inputs that come from the clock, passed in so the assembly stays testable.
#[derive(Clone, Copy, Debug)]
pub struct Now {
    pub instant: Instant,
    pub wall: WallNs,
}

/// The frame of `meas`'s `stream`, if received.
pub fn frame(st: &AppState, meas: MeasId, stream: Stream) -> Option<&TopicFrame> {
    st.data.as_ref()?.latest.get(&Topic::Data { meas, stream })
}

/// Freshness of a received frame: its age, or STALE when the client says so (no new frame
/// for 1 s, or the daemon not responding).
pub fn freshness(tf: &TopicFrame) -> Freshness {
    let since = tf.since_new.as_secs_f64();
    let age = tf.age.unwrap_or(since);
    if tf.stale {
        Freshness::Stale {
            age_s: age.max(since),
        }
    } else {
        Freshness::from_age(age)
    }
}

/// Banner inputs for a pane showing `shown` frames.
pub fn status(
    st: &AppState,
    shown: &[&TopicFrame],
    tf_meas: Option<&Measurement>,
    now: Now,
) -> Status {
    let daemon_silence_s = st.mirror.as_ref().and_then(|m| m.last_ka).map_or(0.0, |t| {
        now.instant.saturating_duration_since(t).as_secs_f64()
    });
    let protection = shown.iter().fold(ProtectionFlags::NONE, |a, f| {
        a.with(f.frame.stamp.protection)
    });
    let frame_age_s = shown
        .iter()
        .map(|f| freshness(f).age_s())
        .min_by(f64::total_cmp);
    Status {
        daemon_silence_s,
        protection,
        frame_age_s,
        timing: st.mirror.as_ref().and_then(|m| m.timing),
        no_delay_estimate: tf_meas.is_some_and(no_delay_estimate),
    }
}

fn is_tf(m: &Measurement) -> bool {
    matches!(m.config.kind, MeasKind::Transfer { .. })
}

/// The TF measurement the IR pane and the delay banner follow: the selected one if it is a
/// transfer measurement, else the first.
pub fn focus_tf(st: &AppState) -> Option<&Measurement> {
    st.selected_meas()
        .filter(|m| is_tf(m))
        .or_else(|| st.measurements().into_iter().find(|m| is_tf(m)))
}

struct LiveTf<'a> {
    meas: &'a Measurement,
    tf: &'a TopicFrame,
    freqs: Vec<f64>,
    color: usize,
}

/// The transfer view: every transfer measurement with a frame, plus visible stored traces.
pub fn transfer(st: &AppState, theme: &Theme, size: Viewport, now: Now) -> TfScene {
    let grids = st.data.as_ref().map(|d| &d.grids);
    let mut live = Vec::new();
    for (i, m) in st.measurements().into_iter().enumerate() {
        if !is_tf(m) {
            continue;
        }
        let Some(tf) = frame(st, m.id, Stream::Tf) else {
            continue;
        };
        let Some(grid) = tf
            .frame
            .stamp
            .grid_id
            .and_then(|g| grids.and_then(|gs| gs.get(&g)))
        else {
            continue;
        };
        live.push(LiveTf {
            meas: m,
            tf,
            freqs: column_frequencies(grid),
            color: i,
        });
    }
    let stored: Vec<(&ac2_proto::model::TraceData, Vec<f64>)> = st
        .traces
        .values()
        .filter(|(t, _)| t.meta.edit.visible)
        .map(|(t, g)| (t.as_ref(), column_frequencies(g)))
        .collect();

    let mut traces: Vec<TfTrace<'_>> = Vec::new();
    for l in &live {
        let FrameData::Tf(f) = &l.tf.frame.data else {
            continue;
        };
        let mut t = TfTrace::live(
            f,
            &l.tf.frame.stamp,
            &l.freqs,
            l.meas.config.name.clone(),
            theme.trace_color(l.color),
            freshness(l.tf),
        );
        let e = st.edit(l.meas.id);
        t.offset_db = e.offset_db;
        t.polarity = if e.inverted {
            Polarity::Inverted
        } else {
            Polarity::Normal
        };
        t.nudge = Seconds(e.nudge_s);
        traces.push(t);
    }
    for (data, freqs) in &stored {
        let mut t = TfTrace::stored(data, freqs);
        // A capture from an earlier epoch is not in this epoch's time base (decision 8a).
        if let (TraceSource::Captured { epoch, .. }, Some(cur)) = (
            &data.meta.source,
            st.mirror.as_ref().and_then(|m| m.session_epoch),
        ) && *epoch != cur
        {
            t.time_base = TimeBase::Independent;
        }
        traces.push(t);
    }
    let shown: Vec<&TopicFrame> = live.iter().map(|l| l.tf).collect();
    let status = status(st, &shown, focus_tf(st), now);
    transfer_scene(&traces, &status, &st.view, theme, size)
}

/// The spectrum / RTA view.
pub fn spectrum(st: &AppState, theme: &Theme, size: Viewport, now: Now) -> SpectrumScene {
    let grids = st.data.as_ref().map(|d| &d.grids);
    struct Col<'a> {
        meas: &'a Measurement,
        tf: &'a TopicFrame,
        freqs: Vec<f64>,
        edges: Vec<(f64, f64)>,
        color: usize,
    }
    let mut cols = Vec::new();
    for (i, m) in st.measurements().into_iter().enumerate() {
        let stream = match m.config.kind {
            MeasKind::Spectrum { .. } => Stream::Spec,
            MeasKind::Rta { .. } => Stream::Rta,
            _ => continue,
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
            freqs: column_frequencies(g),
            edges: column_edges(g),
            color: i,
        });
    }
    let mut traces = Vec::new();
    for c in &cols {
        let name = c.meas.config.name.clone();
        let color = theme.trace_color(c.color);
        let mut t = match &c.tf.frame.data {
            FrameData::Spec(f) => {
                SpectrumTrace::spectrum(f, &c.freqs, &c.edges, name, color, freshness(c.tf))
            }
            FrameData::Rta(f) => {
                SpectrumTrace::rta(f, &c.freqs, &c.edges, name, color, freshness(c.tf))
            }
            _ => continue,
        };
        if st.view.spectrum.peak_hold {
            t.peak = st.peaks.get(&c.meas.id).map(|p| p.2.values());
        }
        traces.push(t);
    }
    let shown: Vec<&TopicFrame> = cols.iter().map(|c| c.tf).collect();
    let status = status(st, &shown, None, now);
    spectrum_scene(&traces, &status, &st.view, theme, size)
}

/// The IR of the focused transfer measurement; `None` without one or without its IR frame.
pub fn ir(st: &AppState, theme: &Theme, size: Viewport, now: Now) -> Option<IrScene> {
    let m = focus_tf(st)?;
    let tf = frame(st, m.id, Stream::Ir)?;
    let FrameData::Ir(f) = &tf.frame.data else {
        return None;
    };
    let i = st
        .measurements()
        .iter()
        .position(|x| x.id == m.id)
        .unwrap_or(0);
    let status = status(st, &[tf], Some(m), now);
    Some(ir_scene(
        f,
        theme.trace_color(i),
        Some(freshness(tf)),
        &status,
        &st.view,
        theme,
        size,
    ))
}

/// The first SPL measurement with a frame.
pub fn spl(st: &AppState, theme: &Theme, size: Viewport, now: Now) -> Option<SplScene> {
    let (m, tf) = st.measurements().into_iter().find_map(|m| {
        matches!(m.config.kind, MeasKind::Spl { .. })
            .then(|| frame(st, m.id, Stream::Spl).map(|f| (m, f)))
            .flatten()
    })?;
    let FrameData::Spl(f) = &tf.frame.data else {
        return None;
    };
    let MeasKind::Spl { config } = &m.config.kind else {
        return None;
    };
    let daemon = st.daemon()?;
    let offset = ClockOffset(
        st.mirror
            .as_ref()
            .and_then(|v| v.clock_offset_ns)
            .map_or(0, |o| {
                o.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
            }),
    );
    // The daemon applies the calibration of this device + input; the mic name is not in the
    // meter config, so the entry for the input is shown with its own key, and a different
    // device or channel reads as "cal from other mic / input".
    let device = daemon.session.open.as_ref().map(|o| o.input_device.clone());
    let entry = daemon
        .calibrations
        .iter()
        .find(|e| Some(&e.key.device) == device.as_ref() && e.key.channel == config.input);
    let cal = match (f.meta.scale, entry) {
        (LevelScale::DbSpl, Some(e)) => cal_text(f.meta.scale, &e.key, Some(e), now.wall, offset),
        (scale, _) => {
            let key = ac2_proto::model::CalKey {
                device: device.unwrap_or_else(|| ac2_proto::model::DeviceId(String::new())),
                channel: config.input,
                mic: String::new(),
            };
            cal_text(scale, &key, None, now.wall, offset)
        }
    };
    let r = spl_readout(f, cal, Some(freshness(tf)));
    let status = status(st, &[tf], None, now);
    Some(spl_scene(&r, &status, theme, size))
}
