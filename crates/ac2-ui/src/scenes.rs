//! Assembles `ac2-scene` inputs from the mirrored state, the latest frames and the
//! operator's display choices, and calls the builders. Bookkeeping only: which frames go to
//! which pane, names, colours, freshness. Every displayed number is made by `ac2-scene`.

use std::sync::Arc;
use std::time::Instant;

use ac2_client::TopicFrame;
use ac2_proto::FrameData;
use ac2_proto::frame::ProtectionFlags;
use ac2_proto::model::{LevelScale, MeasKind, Measurement, Polarity, TraceKind};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{MeasId, Seconds, WallNs};
use ac2_scene::average::AverageStatus;
use ac2_scene::banner::{Status, no_delay_estimate};
use ac2_scene::distortion::{DistortionScene, SweepView, distortion_scene, sweep_ir_scene};
use ac2_scene::format;
use ac2_scene::grid::{GridColumns, column_frequencies, columns};
use ac2_scene::ir::{IrScene, ir_scene};
use ac2_scene::leq::{LeqScene, LeqView, leq_scene, leq_tiles};
use ac2_scene::meter_leq::{MeterLeqScene, meter_leq_scene};
use ac2_scene::primitives::{Scene, Viewport};
use ac2_scene::spectrograph::{
    SpectrographHistory, SpectrographInput, SpectrographScene, spectrograph_scene,
};
use ac2_scene::spectrum::{Quantity, SpectrumScene, SpectrumTrace, spectrum_scene};
use ac2_scene::spl::{SplReadout, SplScene, cal_text, spl_readout, spl_scene};
use ac2_scene::tf::{TfScene, transfer_scene};
use ac2_scene::theme::Theme;
use ac2_scene::time::{ClockOffset, Freshness};
use ac2_scene::trace::{TfTrace, TimeBase, TraceKey};
use ac2_scene::view::SplMode;

use crate::state::{AppState, PaneKind};

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
/// within the stream's threshold — 3 s for Leq, 1 s otherwise — or the daemon not
/// responding), or stopped when its measurement no longer runs (no frame is due).
pub fn freshness(st: &AppState, tf: &TopicFrame) -> Freshness {
    let since = tf.since_new.as_secs_f64();
    let age = tf.age.unwrap_or(since);
    if stopped(st, tf) {
        return Freshness::Stopped {
            age_s: age.max(since),
        };
    }
    // The client's flag already applies each stream's own threshold (a once-a-second Leq frame
    // is 3 s, the rest 1 s); judging the age again here with the general 1 s would dim a Leq
    // view for a moment whenever a frame arrived a little after its second.
    if tf.stale {
        Freshness::Stale {
            age_s: age.max(since),
        }
    } else {
        Freshness::Fresh { age_s: age }
    }
}

/// The frame's measurement is listed and not running: its last frame is its final result.
fn stopped(st: &AppState, tf: &TopicFrame) -> bool {
    let Topic::Data { meas, .. } = tf.topic else {
        return false;
    };
    st.daemon()
        .and_then(|s| s.measurements.iter().find(|m| m.id == meas))
        .is_some_and(|m| !m.running)
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
    // A stopped measurement raises no fault: nothing is due from it, and its last frame's
    // protection flags describe a signal that is no longer measured.
    let live: Vec<&TopicFrame> = shown.iter().copied().filter(|f| !stopped(st, f)).collect();
    let protection = live.iter().fold(ProtectionFlags::NONE, |a, f| {
        a.with(f.frame.stamp.protection)
    });
    let frame_age_s = live
        .iter()
        .map(|f| freshness(st, f).age_s())
        .min_by(f64::total_cmp);
    Status {
        daemon_silence_s,
        protection,
        frame_age_s,
        timing: st.mirror.as_ref().and_then(|m| m.timing),
        clock_drift_ppm: st
            .daemon()
            .and_then(|d| d.timing.drift)
            .filter(|d| d.warning)
            .map(|d| d.ppm),
        no_delay_estimate: tf_meas.and_then(no_delay_estimate),
        average: None,
    }
}

fn is_tf(m: &Measurement) -> bool {
    m.config.kind.publishes_tf()
}

/// What a spatial average's frame averaged, members named as the measurement list names
/// them; `None` for any other frame.
fn average_status(st: &AppState, frame: &FrameData) -> Option<AverageStatus> {
    let FrameData::Tf(f) = frame else {
        return None;
    };
    let a = f.meta.average.as_ref()?;
    let ms = st.measurements();
    Some(AverageStatus::new(a, |id| {
        ms.iter()
            .find(|m| m.id == id)
            .map_or_else(|| format!("measurement {id}"), |m| m.config.name.clone())
    }))
}

/// The TF measurement the IR pane and the delay banner follow: the one the transfer pane
/// shows.
pub fn focus_tf(st: &AppState) -> Option<&Measurement> {
    st.pane_meas(PaneKind::Transfer)
}

/// Measurements in list order with the one pane `p` shows first (its legend row and
/// caption lead).
fn pane_order(st: &AppState, p: PaneKind) -> Vec<(usize, &Measurement)> {
    let shown = st.pane_meas(p).map(|m| m.id);
    let mut v: Vec<(usize, &Measurement)> = st.measurements().into_iter().enumerate().collect();
    v.sort_by_key(|(_, m)| Some(m.id) != shown);
    v
}

struct LiveTf<'a> {
    meas: &'a Measurement,
    tf: &'a TopicFrame,
    cols: Arc<GridColumns>,
    color: usize,
}

/// The transfer view: every transfer measurement with a frame, plus visible stored traces.
pub fn transfer(st: &AppState, theme: &Theme, size: Viewport, now: Now) -> TfScene {
    let grids = st.data.as_ref().map(|d| &d.grids);
    let mut live = Vec::new();
    for (i, m) in pane_order(st, PaneKind::Transfer) {
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
            cols: columns(grid),
            color: i,
        });
    }
    let mut stored: Vec<(&Arc<ac2_proto::model::TraceData>, Arc<GridColumns>)> = st
        .traces
        .values()
        .filter(|(t, _)| crate::state::on_transfer_pane(&t.meta))
        .map(|(t, g)| (t, columns(g)))
        .collect();
    stored.sort_by_key(|(t, _)| (t.meta.edit.order, t.meta.id));

    let mut traces: Vec<TfTrace<'_>> = Vec::new();
    for l in &live {
        let FrameData::Tf(f) = &l.tf.frame.data else {
            continue;
        };
        let mut t = TfTrace::live(
            f,
            &l.tf.frame.stamp,
            &l.cols.freqs,
            l.meas.config.name.clone(),
            theme.trace_color(l.color),
            freshness(st, l.tf),
        );
        let e = st.edit(l.meas.id);
        t.offset_db = e.offset_db;
        t.polarity = if e.inverted {
            Polarity::Inverted
        } else {
            Polarity::Normal
        };
        t.nudge = Seconds(e.nudge_s);
        t.note = average_status(st, &l.tf.frame.data).map(|a| a.tag());
        traces.push(t);
    }
    for (data, cols) in &stored {
        let mut t = TfTrace::stored(data, &cols.freqs);
        t.selected = st.selected_trace == Some(data.meta.id);
        // A capture from an earlier epoch is not in this epoch's time base (decision 8a).
        if let (Some(epoch), Some(cur)) = (
            data.meta.source.shared_epoch(),
            st.mirror.as_ref().and_then(|m| m.session_epoch),
        ) && epoch != cur
        {
            t.time_base = TimeBase::Independent;
        }
        traces.push(t);
    }
    let shown: Vec<&TopicFrame> = live.iter().map(|l| l.tf).collect();
    let focus = focus_tf(st);
    let mut status = status(st, &shown, focus, now);
    // The shown average's positions, unless it is stopped (its last frame is history).
    status.average = focus.and_then(|m| {
        let l = live
            .iter()
            .find(|l| l.meas.id == m.id && !stopped(st, l.tf))?;
        Some((m.config.name.clone(), average_status(st, &l.tf.frame.data)?))
    });
    transfer_scene(&traces, &st.tf_display, &status, &st.view, theme, size)
}

/// The spectrum / RTA view.
/// The scale the spectrum pane shows its curves in: dB SPL when every curve it draws (live
/// and stored) is calibrated, else dBFS. Picks which of the pane's two level ranges applies.
pub fn spectrum_scale(st: &AppState) -> LevelScale {
    let mut scales = Vec::new();
    for (_, m) in pane_order(st, PaneKind::Spectrum) {
        let stream = match m.config.kind {
            MeasKind::Spectrum { .. } => Stream::Spec,
            MeasKind::Rta { .. } => Stream::Rta,
            _ => continue,
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
            TraceKind::Spectrum { scale } | TraceKind::Rta { scale } => scales.push(scale),
            _ => {}
        }
    }
    if !scales.is_empty() && scales.iter().all(|s| *s == LevelScale::DbSpl) {
        LevelScale::DbSpl
    } else {
        LevelScale::Dbfs
    }
}

pub fn spectrum(st: &AppState, theme: &Theme, size: Viewport, now: Now) -> SpectrumScene {
    with_spectrum(st, theme, now, |traces, status, view| {
        spectrum_scene(traces, status, view, theme, size)
    })
}

/// The spectrum pane with the spectrograph of the pane's measurement under it.
pub fn spectrograph(st: &AppState, theme: &Theme, size: Viewport, now: Now) -> SpectrographScene {
    let shown = st.pane_meas(PaneKind::Spectrum).and_then(|m| {
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
            range: st.view.spectrum.range(scale),
            offset_db: st.edit(m.id).offset_db,
            freshness: frame(st, m.id, stream).map(|tf| freshness(st, tf)),
        }
    });
    with_spectrum(st, theme, now, |traces, status, view| {
        spectrograph_scene(traces, status, input.as_ref(), view, theme, size)
    })
}

/// Calls `f` with the spectrum pane's traces (live, then stored), its banner status and
/// the view on the level range of the scale its curves are in.
fn with_spectrum<R>(
    st: &AppState,
    theme: &Theme,
    now: Now,
    f: impl FnOnce(&[SpectrumTrace<'_>], &Status, &ac2_scene::ViewState) -> R,
) -> R {
    let grids = st.data.as_ref().map(|d| &d.grids);
    struct Col<'a> {
        meas: &'a Measurement,
        tf: &'a TopicFrame,
        cols: Arc<GridColumns>,
        color: usize,
    }
    let mut cols = Vec::new();
    for (i, m) in pane_order(st, PaneKind::Spectrum) {
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
            cols: columns(g),
            color: i,
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
    let mut traces = Vec::new();
    for (c, note) in cols.iter().zip(&notes) {
        let name = c.meas.config.name.clone();
        let color = theme.trace_color(c.color);
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
        if st.view.spectrum.peak_hold {
            t.peak = st.peaks.get(&c.meas.id).map(|p| p.2.values());
        }
        t.offset_db = st.edit(c.meas.id).offset_db;
        if matches!(t.quantity, Quantity::Tone | Quantity::SmoothedTone(_)) {
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
                TraceKind::Spectrum { scale } => (
                    scale,
                    // Served smoothed at its setting (`ac2_traces::smooth`).
                    Quantity::tone(t.meta.edit.smoothing.map(|s| s.fraction)),
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
        let c = data.meta.edit.color;
        traces.push(SpectrumTrace {
            key: TraceKey::Stored(data.meta.id),
            name: data.meta.edit.name.clone(),
            color: ac2_scene::primitives::Color::from_rgba8([c.r, c.g, c.b, 255]),
            freqs: &cols.freqs,
            edges: &cols.edges,
            level: &data.mag_db,
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
        });
    }
    let shown: Vec<&TopicFrame> = cols.iter().map(|c| c.tf).collect();
    let status = status(st, &shown, None, now);
    // The pane draws on the level range of the scale its curves are in.
    let mut view = st.view;
    view.spectrum.level = st.view.spectrum.range(spectrum_scale(st));
    f(&traces, &status, &view)
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
        Some(freshness(st, tf)),
        &status,
        &st.view,
        theme,
        size,
    ))
}

/// What the sweep pane draws.
#[derive(Debug)]
pub enum SweepPane {
    Distortion(Box<DistortionScene>),
    Ir(Box<IrScene>),
}

impl SweepPane {
    pub fn scene(self) -> ac2_scene::Scene {
        match self {
            SweepPane::Distortion(s) => s.scene,
            SweepPane::Ir(s) => s.scene,
        }
    }

    /// The frequency axis, for navigation (the IR view has a time axis).
    pub fn x_axis(&self) -> Option<ac2_scene::axis::Mapping> {
        match self {
            SweepPane::Distortion(s) => Some(s.x_axis.mapping),
            SweepPane::Ir(_) => None,
        }
    }

    /// The distortion axis in dB, for zooming about the pointer (the percent axis is a log
    /// of the same ratios: no dB value under the pointer).
    pub fn y_level(
        &self,
        unit: ac2_scene::view::DistortionUnit,
    ) -> Option<ac2_scene::axis::Mapping> {
        match self {
            SweepPane::Distortion(s) if unit == ac2_scene::view::DistortionUnit::Db => {
                Some(s.y_axis.mapping)
            }
            _ => None,
        }
    }
}

/// The sweep pane: the shown sweep trace's distortion, or its impulse response.
pub fn sweep(st: &AppState, theme: &Theme, size: Viewport, now: Now) -> SweepPane {
    let status = status(st, &[], None, now);
    let shown = st.shown_sweep();
    if st.view.distortion.show_ir
        && let Some((d, _)) = shown
    {
        let c = d.meta.edit.color;
        let color = ac2_scene::primitives::Color::from_rgba8([c.r, c.g, c.b, 255]);
        if let Some(s) = sweep_ir_scene(d, color, &status, &st.view, theme, size) {
            return SweepPane::Ir(Box::new(s));
        }
    }
    let freqs = shown
        .map(|(_, g)| column_frequencies(g))
        .unwrap_or_default();
    let view = shown.map(|(d, _)| SweepView {
        data: d,
        freqs: &freqs,
    });
    SweepPane::Distortion(Box::new(distortion_scene(
        view, &status, &st.view, theme, size,
    )))
}

/// Calibration text of an SPL meter's input: the daemon's verdict with the input's mic.
fn spl_cal(
    st: &AppState,
    input: u16,
    cal: ac2_proto::model::CalStatus,
    mic_curve: bool,
    now: Now,
) -> String {
    let offset = ClockOffset(
        st.mirror
            .as_ref()
            .and_then(|v| v.clock_offset_ns)
            .map_or(0, |o| {
                o.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
            }),
    );
    // The daemon decided which calibration applies (verified / other mic or input) and
    // whether the mic curve ran; the readout names the input's mic next to it, and which
    // curve (or why none).
    let cal = cal_text(cal, false, now.wall, offset);
    let cal = match st.curve_note(input, mic_curve) {
        Some(n) => format!("{cal} · {n}"),
        None => cal,
    };
    match st
        .daemon()
        .and_then(|d| d.inputs.iter().find(|i| i.channel == input))
        .and_then(|i| i.mic.as_deref())
    {
        Some(mic) => format!("{mic} · {cal}"),
        None => cal,
    }
}

/// How wall times become local times of day (the SPL log's start).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LocalZone {
    /// The computer's time zone, with the offset in force at each instant.
    #[default]
    System,
    /// A fixed offset from UTC (tests that compare pictures).
    Fixed { offset_s: i32 },
}

impl LocalZone {
    /// The UTC offset (s) in force at wall time `t`.
    pub fn offset_s(self, t: ac2_proto::units::WallNs) -> i32 {
        match self {
            LocalZone::System => {
                use chrono::{Local, Offset, TimeZone};
                let ns = i64::try_from(t.0).unwrap_or(i64::MAX);
                Local.timestamp_nanos(ns).offset().fix().local_minus_utc()
            }
            LocalZone::Fixed { offset_s } => offset_s,
        }
    }
}

/// Whether the daemon has an SPL meter.
pub fn has_spl(st: &AppState) -> bool {
    st.measurements()
        .iter()
        .any(|m| matches!(m.config.kind, MeasKind::Spl { .. }))
}

/// The SPL meters in the order the pane picks them: the one it shows first.
fn spl_meters(st: &AppState) -> impl Iterator<Item = &Measurement> {
    pane_order(st, PaneKind::Spl)
        .into_iter()
        .map(|(_, m)| m)
        .filter(|m| matches!(m.config.kind, MeasKind::Spl { .. }))
}

/// What the Leq view shows of meter `m`, with its `leq` frame.
fn leq_view<'a>(st: &'a AppState, m: &'a Measurement, now: Now) -> Option<LeqView<'a>> {
    let tf = frame(st, m.id, Stream::Leq)?;
    let FrameData::Leq(f) = &tf.frame.data else {
        return None;
    };
    let MeasKind::Spl { config } = &m.config.kind else {
        return None;
    };
    // The frame describes the windows of the configuration it was made under.
    let cfg = &config.leq;
    let fresh = freshness(st, tf);
    Some(LeqView {
        meter: m.config.name.clone(),
        cal: spl_cal(st, config.input, f.meta.cal, f.meta.mic_curve, now),
        cfg,
        tiles: leq_tiles(cfg, f),
        history: st.leq_history.get(&m.id).map(|(_, h)| h),
        stale: match fresh {
            Freshness::Stale { age_s } => Some(format!("STALE {}", format::age(age_s))),
            Freshness::Stopped { .. } => Some("STOPPED".into()),
            Freshness::Fresh { .. } => None,
        },
        scale: f.meta.scale,
        layout: st.view.spl.layout,
        run: f
            .meta
            .run
            .map(|r| ac2_scene::leq::run_text(&r, cfg, |t| st.local_zone.offset_s(t))),
    })
}

/// The meter readout of meter `m` (the held reading, [`ac2_scene::spl::SplHold`]), with its
/// newest `spl` frame. `keymap` names the key that resets the meter's statistics.
fn spl_readout_of<'a>(
    st: &'a AppState,
    m: &Measurement,
    keymap: &crate::keys::Keymap,
    now: Now,
) -> Option<(SplReadout, &'a TopicFrame)> {
    let tf = frame(st, m.id, Stream::Spl)?;
    let FrameData::Spl(f) = &tf.frame.data else {
        return None;
    };
    let MeasKind::Spl { config } = &m.config.kind else {
        return None;
    };
    st.daemon()?;
    // The number shows the held reading; the bar and the freshness follow the newest frame.
    let held = st.spl_hold.get(&m.id).map_or(f, |h| &h.frame);
    let cal = spl_cal(st, config.input, held.meta.cal, held.meta.mic_curve, now);
    // The statistics run from the newest frame's capture less its interval: the same instant
    // in every frame until the meter is reset.
    let newest = tf.frame.stamp.capture_wall_ns;
    let start_ns = newest
        .0
        .saturating_sub((f.meta.duration.0.max(0.0) * 1e9).round() as u64);
    let reset = keymap
        .first_chord(
            crate::keys::CommandId::ResetAverage,
            crate::keys::Scope::Spl,
        )
        .map(|c| c.label());
    let since = ac2_scene::spl::meter_since(
        ac2_proto::units::WallNs(start_ns),
        newest,
        |t| st.local_zone.offset_s(t),
        reset.as_deref(),
    );
    let r = spl_readout(held, f.meta.level, cal, Some(freshness(st, tf)), since);
    Some((r, tf))
}

/// The Leq windows of the SPL meter the pane shows (else the first one with a `leq` frame).
pub fn leq(st: &AppState, theme: &Theme, size: Viewport, now: Now) -> Option<LeqScene> {
    let v = spl_meters(st).find_map(|m| leq_view(st, m, now))?;
    let status = status(st, &[], None, now);
    Some(leq_scene(&v, &status, theme, size))
}

/// The SPL measurement the pane shows (else the first one with a frame). `keymap` names the
/// key that resets the meter's statistics.
pub fn spl(
    st: &AppState,
    keymap: &crate::keys::Keymap,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> Option<SplScene> {
    let (r, tf) = spl_meters(st).find_map(|m| spl_readout_of(st, m, keymap, now))?;
    let status = status(st, &[tf], None, now);
    Some(spl_scene(&r, &status, theme, size))
}

/// The meter + Leq view of the SPL meter the pane shows (else the first one with both
/// frames): its held number over its Leq windows.
pub fn meter_leq(
    st: &AppState,
    keymap: &crate::keys::Keymap,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> Option<MeterLeqScene> {
    let (r, tf, v) = spl_meters(st).find_map(|m| {
        let (r, tf) = spl_readout_of(st, m, keymap, now)?;
        Some((r, tf, leq_view(st, m, now)?))
    })?;
    let status = status(st, &[tf], None, now);
    Some(meter_leq_scene(&r, &v, &status, theme, size))
}

/// The SPL pane's picture in the chosen view ([`SplMode`]). The meter + Leq view shows
/// whichever part has frames until both have (the windows' first second).
pub fn spl_pane(
    st: &AppState,
    keymap: &crate::keys::Keymap,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> Option<Scene> {
    match st.view.spl.mode {
        SplMode::Meter => spl(st, keymap, theme, size, now).map(|s| s.scene),
        SplMode::Leq => leq(st, theme, size, now).map(|s| s.scene),
        SplMode::MeterLeq => meter_leq(st, keymap, theme, size, now)
            .map(|s| s.leq.scene)
            .or_else(|| spl(st, keymap, theme, size, now).map(|s| s.scene))
            .or_else(|| leq(st, theme, size, now).map(|s| s.scene)),
    }
}
