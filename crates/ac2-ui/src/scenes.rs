//! Assembles `ac2-scene` inputs from the mirrored state, the latest frames and the
//! operator's display choices, and calls the builders. Bookkeeping only: which frames go to
//! which pane, names, colours, freshness. Every displayed number is made by `ac2-scene`.

use std::sync::Arc;
use std::time::Instant;

use ac2_client::TopicFrame;
use ac2_proto::FrameData;
use ac2_proto::frame::ProtectionFlags;
use ac2_proto::model::{LevelScale, MeasKind, Measurement, PhaseBasis, Polarity, TraceKind};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{MeasId, WallNs};
use ac2_scene::band_leq::{BandLeqScene, BandLeqView, band_leq_scene, band_leq_text};
use ac2_scene::banner::{Status, no_delay_estimate};
use ac2_scene::distortion::{DistortionScene, SweepView, distortion_scene, sweep_ir_scene};
use ac2_scene::format;
use ac2_scene::grid::{GridColumns, column_frequencies, columns};
use ac2_scene::ir::{IrScene, ir_scene};
use ac2_scene::leq::{LeqScene, LeqView, leq_scene, leq_tiles};
use ac2_scene::math::MathStatus;
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

use crate::state::{AppState, PaneId, PaneKind};

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
/// within the stream's threshold — 3 s for the once-a-second Leq and band streams, 1 s
/// otherwise — or the daemon not responding), or stopped when its measurement no longer runs (no frame is due).
pub fn freshness(st: &AppState, tf: &TopicFrame) -> Freshness {
    let since = tf.since_new.as_secs_f64();
    let age = tf.age.unwrap_or(since);
    if stopped(st, tf) {
        return Freshness::Stopped {
            age_s: age.max(since),
        };
    }
    // Late because the audio stopped: the AUDIO STOPPED banner explains it.
    if tf.stale && st.daemon().is_some_and(|s| s.session.stopped.is_some()) {
        return Freshness::AudioStopped {
            age_s: age.max(since),
        };
    }
    // The client's flag already applies each stream's own threshold (a once-a-second frame
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
    // The newest frame and its own stream's threshold: a once-a-second band or Leq frame is
    // not late at 1.5 s, and judging it on the general 1 s would flash STALE between frames.
    let newest = live
        .iter()
        .map(|f| {
            let after = ac2_client::data::stale_after(&f.topic).as_secs_f64();
            (freshness(st, f).age_s(), after)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let frame_age_s = newest.map(|n| n.0);
    Status {
        daemon_silence_s,
        protection,
        frame_age_s,
        stale_after_s: newest.map(|n| n.1),
        timing: st.mirror.as_ref().and_then(|m| m.timing),
        clock_drift_ppm: st
            .daemon()
            .and_then(|d| d.timing.drift)
            .filter(|d| d.warning)
            .map(|d| d.ppm),
        audio_stopped: audio_stopped(st, now.wall),
        no_delay_estimate: tf_meas.and_then(no_delay_estimate),
        math: None,
        drive: st.drive(),
    }
}

/// The daemon's offset estimate as the scene takes it.
fn clock_offset(st: &AppState) -> ClockOffset {
    ClockOffset(
        st.mirror
            .as_ref()
            .and_then(|v| v.clock_offset_ns)
            .map_or(0, |o| {
                o.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
            }),
    )
}

/// What the AUDIO STOPPED banner and the top bar say while the open session's audio is
/// stopped and the daemon reopens it; `None` while it runs.
pub fn audio_stopped(
    st: &AppState,
    client_now: WallNs,
) -> Option<ac2_scene::audio::AudioStoppedText> {
    let stopped = st.daemon()?.session.stopped.as_ref()?;
    Some(ac2_scene::audio::audio_stopped_text(
        stopped,
        daemon_wall(st, client_now),
        |t| st.local_zone.offset_s(t),
    ))
}

/// `client_now` on the daemon's clock.
pub fn daemon_wall(st: &AppState, client_now: WallNs) -> WallNs {
    let t = ac2_scene::time::daemon_now(client_now, clock_offset(st));
    WallNs(t.clamp(0, i128::from(u64::MAX)) as u64)
}

/// What a math channel's frame combined, operands named as the lists name them; `None`
/// for any other frame.
fn math_status(st: &AppState, m: &Measurement, frame: &FrameData) -> Option<MathStatus> {
    let MeasKind::Math { config } = &m.config.kind else {
        return None;
    };
    let state = match frame {
        FrameData::Tf(f) => f.meta.math.as_deref(),
        FrameData::Spec(f) => f.meta.math.as_deref(),
        FrameData::Rta(f) => f.meta.math.as_deref(),
        _ => None,
    }?;
    Some(
        MathStatus::new(config, state, |o| st.operand_name(o)).with_arrival(
            config,
            state,
            |o| st.operand_delay(o),
            st.view.temperature_c,
        ),
    )
}

/// The banner status of the math channel `focus` shows, unless it is stopped (its last
/// frame is history).
fn shown_math(
    st: &AppState,
    focus: Option<&Measurement>,
    frames: &[(MeasId, &TopicFrame)],
) -> Option<(String, MathStatus)> {
    let m = focus?;
    let (_, f) = frames
        .iter()
        .find(|(id, f)| *id == m.id && !stopped(st, f))?;
    Some((m.config.name.clone(), math_status(st, m, &f.frame.data)?))
}

/// The TF measurement the IR keys and the delay banner follow: the one the transfer pane
/// worked in last shows; while that is a sweep (its runs drawn, no live curve), the live
/// one another transfer pane shows.
pub fn focus_tf(st: &AppState) -> Option<&Measurement> {
    let live = |m: &&Measurement| m.config.kind.publishes_tf();
    st.kind_meas(PaneKind::Transfer).filter(live).or_else(|| {
        st.layout
            .panes()
            .into_iter()
            .filter(|id| st.layout.kind(*id) == PaneKind::Transfer)
            .find_map(|id| st.pane_meas(id).filter(live))
    })
}

/// The measurement with a live transfer curve transfer pane `pane` shows (`None` while it
/// shows a sweep's runs).
fn pane_tf(st: &AppState, pane: PaneId) -> Option<&Measurement> {
    st.pane_meas(pane).filter(|m| m.config.kind.publishes_tf())
}

/// The colour of measurement `id`'s live curve (or math result) in every pane: its colour
/// family's ([`ac2_scene::families`]), so it keeps its colour whichever measurement a pane
/// leads with.
pub fn meas_color(st: &AppState, theme: &Theme, id: MeasId) -> ac2_scene::primitives::Color {
    st.curve_colours(theme).meas(id)
}

/// Measurements in list order with the one pane `p` shows first (its legend row and
/// caption lead).
fn pane_order(st: &AppState, p: PaneId) -> Vec<(usize, &Measurement)> {
    let shown = st.pane_meas(p).map(|m| m.id);
    let mut v: Vec<(usize, &Measurement)> = st.measurements().into_iter().enumerate().collect();
    v.sort_by_key(|(_, m)| Some(m.id) != shown);
    v
}

struct LiveTf<'a> {
    meas: &'a Measurement,
    tf: &'a TopicFrame,
    cols: Arc<GridColumns>,
}

/// The transfer view: the group of the measurement the pane shows — its live curve, the
/// math channels made on it, its shown stored traces — and the curves compared (C), tagged
/// so. Other groups (Imported too) wait until theirs is the pane's measurement.
pub fn transfer(st: &AppState, pane: PaneId, theme: &Theme, size: Viewport, now: Now) -> TfScene {
    let grids = st.data.as_ref().map(|d| &d.grids);
    let colours = st.curve_colours(theme);
    let pane_shows = st.pane_meas(pane);
    let mut live = Vec::new();
    for (_, m) in pane_order(st, pane) {
        // The pane's group (its measurement and the math channels made on it), and what is
        // compared. A hidden measurement keeps its colour: showing it again brings back the
        // same curve.
        if !(st.live_on_transfer_pane(m, pane_shows) || st.live_compared_on_transfer(m, pane_shows))
        {
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
        });
    }
    let mut stored: Vec<(&Arc<ac2_proto::model::TraceData>, Arc<GridColumns>)> = st
        .traces
        .values()
        .filter(|(t, _)| {
            st.on_transfer_pane(&t.meta, pane_shows)
                || st.trace_compared_on_transfer(&t.meta, pane_shows)
        })
        .map(|(t, g)| (t, columns(g)))
        .collect();
    stored.sort_by_key(|(t, _)| (t.meta.edit.order, t.meta.id));

    // The legend groups a measurement's curves: its live curve, its stored traces, the
    // math channels made on it, group by group in the tree's order.
    let ms = st.measurements();
    // The group of the measurement the pane shows leads, as its curve does.
    let groups = ac2_scene::meas_list::group_order(&ms);
    let lead = pane_tf(st, pane).map(|m| match &m.config.kind {
        ac2_proto::model::MeasKind::Math { config } => config.owner,
        _ => ac2_proto::model::TraceOwner::Meas { meas: m.id },
    });
    let rank = |g: ac2_proto::model::TraceOwner| {
        if Some(g) == lead {
            0
        } else {
            1 + groups.iter().position(|x| *x == g).unwrap_or(groups.len())
        }
    };
    let mut ranks: Vec<usize> = Vec::new();
    let mut traces: Vec<TfTrace<'_>> = Vec::new();
    for l in &live {
        ranks.push(rank(match &l.meas.config.kind {
            ac2_proto::model::MeasKind::Math { config } => config.owner,
            _ => ac2_proto::model::TraceOwner::Meas { meas: l.meas.id },
        }));
        let FrameData::Tf(f) = &l.tf.frame.data else {
            continue;
        };
        let mut t = TfTrace::live(
            f,
            &l.tf.frame.stamp,
            &l.cols.freqs,
            l.meas.config.name.clone(),
            colours.meas(l.meas.id),
            freshness(st, l.tf),
        );
        let e = st.edit(l.meas.id);
        t.offset_db = e.offset_db;
        t.polarity = if e.inverted {
            Polarity::Inverted
        } else {
            Polarity::Normal
        };
        t.note = math_status(st, l.meas, &l.tf.frame.data).map(|a| a.tag());
        t.compared = st.live_compared_on_transfer(l.meas, pane_shows);
        // A ratio or cascade of operands without a shared time base has each operand's
        // own alignment in its phase, not a time base of this session.
        if f.meta
            .math
            .as_ref()
            .is_some_and(|m| m.phase == PhaseBasis::OwnAlignments)
        {
            t.time_base = TimeBase::Independent;
        }
        traces.push(t);
    }
    for (data, cols) in &stored {
        ranks.push(rank(ac2_scene::meas_list::group_of(&data.meta, &ms)));
        let mut t = TfTrace::stored(data, &cols.freqs, colours.trace(data.meta.id));
        t.selected = st.selected_trace == Some(data.meta.id);
        t.compared = st.trace_compared_on_transfer(&data.meta, pane_shows);
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
    let mut order: Vec<usize> = (0..traces.len()).collect();
    order.sort_by_key(|i| (ranks.get(*i).copied().unwrap_or(usize::MAX), *i));
    let mut slots: Vec<Option<TfTrace<'_>>> = traces.into_iter().map(Some).collect();
    let traces: Vec<TfTrace<'_>> = order.iter().filter_map(|i| slots[*i].take()).collect();
    let shown: Vec<&TopicFrame> = live.iter().map(|l| l.tf).collect();
    let focus = pane_tf(st, pane);
    let mut status = status(st, &shown, focus, now);
    // The shown math channel's operands.
    let frames: Vec<(MeasId, &TopicFrame)> = live.iter().map(|l| (l.meas.id, l.tf)).collect();
    status.math = shown_math(st, focus, &frames);
    transfer_scene(
        &traces,
        &st.tf_display,
        &status,
        &st.view_for(pane),
        theme,
        size,
    )
}

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
            range: st.view.spectrum.range(scale),
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
    let mut traces = Vec::new();
    for (c, note) in cols.iter().zip(&notes) {
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
        if st.view.spectrum.peak_hold {
            t.peak = st.peaks.get(&c.meas.id).map(|p| p.2.values());
        }
        t.offset_db = st.edit(c.meas.id).offset_db;
        // A math channel's caption is its expression and what it means.
        if let Some(m) = math_status(st, c.meas, &c.tf.frame.data) {
            t.caption = m.tag();
        }
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
        traces.push(SpectrumTrace {
            key: TraceKey::Stored(data.meta.id),
            name: data.meta.edit.name.clone(),
            color: colours.trace(data.meta.id),
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
    let mut status = status(st, &shown, None, now);
    let frames: Vec<(MeasId, &TopicFrame)> = cols.iter().map(|c| (c.meas.id, c.tf)).collect();
    status.math = shown_math(st, st.pane_meas(pane), &frames);
    // The pane draws on the level range of the scale its curves are in.
    let mut view = st.view_for(pane);
    view.spectrum.level = st.view.spectrum.range(spectrum_scale(st));
    f(&traces, &status, &view)
}

/// The IR of the focused transfer measurement, under the banners of its transfer stream
/// (protection flags, stopped, audio stopped, daemon silence), tagged as its transfer curve
/// is; without its IR frame, an empty plot that says why. `None` without a transfer
/// measurement. While the selected trace is a stored transfer trace with an IR that the
/// pane's transfer group draws ([`AppState::stored_ir`]), that IR instead, named as its
/// legend row is.
pub fn ir(
    st: &AppState,
    pane: PaneId,
    keymap: &crate::keys::Keymap,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> Option<IrScene> {
    use ac2_scene::ir::{IrMissing, missing_scene, missing_text};
    let view = st.view_for(pane);
    if let Some(d) = st.stored_ir(st.pane_meas(pane)) {
        // A stored trace is no live stream: none of the measurement's banners apply to it.
        let status = status(st, &[], None, now);
        return ac2_scene::ir::stored_ir_scene(
            d,
            st.curve_colours(theme).trace(d.meta.id),
            &status,
            &view,
            &view.ir.axes,
            view.chrome,
            theme,
            size,
        );
    }
    let m = st.pane_meas(pane)?;
    let tf = frame(st, m.id, Stream::Tf);
    let ir = frame(st, m.id, Stream::Ir);
    let shown: Vec<&TopicFrame> = [tf, ir].into_iter().flatten().collect();
    let status = status(st, &shown, Some(m), now);
    // A stopped measurement's last IR describes a system no longer measured: like its
    // response it is not drawn as live; the empty plot says it is stopped.
    if let Some(ir) = ir.filter(|_| !st.meas_hidden(m) && m.running)
        && let FrameData::Ir(f) = &ir.frame.data
    {
        return Some(ir_scene(
            f,
            meas_color(st, theme, m.id),
            Some(freshness(st, ir)),
            &status,
            &view,
            &view.ir.axes,
            view.chrome,
            theme,
            size,
        ));
    }
    let hidden = st.meas_hidden(m);
    let why = if hidden {
        IrMissing::Hidden
    } else if !m.running {
        IrMissing::Stopped
    } else if status.audio_stopped.is_some() {
        IrMissing::AudioStopped
    } else if status.protection.contains(ProtectionFlags::NO_REFERENCE) {
        IrMissing::NoReference(status.drive.clone())
    } else if status.protection.contains(ProtectionFlags::NO_SIGNAL) {
        IrMissing::NoSignal
    } else {
        IrMissing::NotYet
    };
    let (command, scope) = if hidden {
        (
            crate::keys::CommandId::ToggleSelected,
            crate::keys::Scope::Global,
        )
    } else {
        (crate::keys::CommandId::StartStop, crate::keys::Scope::Ir)
    };
    let start = keymap
        .first_chord(command, scope)
        .map_or_else(|| "the palette".to_owned(), |c| c.label());
    Some(missing_scene(
        missing_text(&m.config.name, why, &start),
        &status,
        &view.ir.axes,
        view.chrome,
        theme,
        size,
    ))
}

/// What the sweep pane draws.
#[derive(Debug)]
pub enum SweepPane {
    Distortion(Box<DistortionScene>),
    Ir(Box<IrScene>),
    Room(Box<ac2_scene::room::RoomScene>),
}

impl SweepPane {
    pub fn scene(self) -> ac2_scene::Scene {
        match self {
            SweepPane::Distortion(s) => s.scene,
            SweepPane::Ir(s) => s.scene,
            SweepPane::Room(s) => s.scene,
        }
    }

    /// The frequency axis, for navigation (the IR view has a time axis).
    pub fn x_axis(&self) -> Option<ac2_scene::axis::Mapping> {
        match self {
            SweepPane::Distortion(s) => Some(s.x_axis.mapping),
            SweepPane::Ir(_) | SweepPane::Room(_) => None,
        }
    }

    /// The distortion axis, for zooming about the pointer: linear in dB, or the percent
    /// of the fundamental on a log scale over the same ratios.
    pub fn y_level(&self) -> Option<ac2_scene::axis::Mapping> {
        match self {
            SweepPane::Distortion(s) => Some(s.y_axis.mapping),
            _ => None,
        }
    }

    /// The IR view's time and value axes, for navigation.
    pub fn ir_axes(&self) -> Option<(ac2_scene::axis::Mapping, ac2_scene::axis::Mapping)> {
        match self {
            SweepPane::Ir(s) => Some((s.x_axis.mapping, s.y_axis.mapping)),
            _ => None,
        }
    }
}

/// The sweep pane: the shown sweep trace's distortion, its impulse response or its room
/// parameters.
pub fn sweep(st: &AppState, pane: PaneId, theme: &Theme, size: Viewport, now: Now) -> SweepPane {
    let status = status(st, &[], None, now);
    let shown = st.shown_sweep();
    let pv = st.view_for(pane);
    if st.pane_modes(pane).sweep == ac2_scene::view::SweepMode::Room {
        let room = shown.and_then(|(d, _)| d.sweep.as_ref()?.room.as_ref());
        let name = shown.map(|(d, _)| d.meta.edit.name.as_str());
        return SweepPane::Room(Box::new(ac2_scene::room::room_scene(
            room, name, &status, theme, size,
        )));
    }
    if st.pane_modes(pane).sweep.ir().is_some()
        && let Some((d, _)) = shown
    {
        let color = st.curve_colours(theme).trace(d.meta.id);
        if let Some(s) = sweep_ir_scene(d, color, &status, &pv, theme, size) {
            return SweepPane::Ir(Box::new(s));
        }
    }
    let freqs = shown
        .map(|(_, g)| column_frequencies(g))
        .unwrap_or_default();
    let colours = st.curve_colours(theme);
    let view = shown.map(|(d, _)| SweepView {
        data: d,
        freqs: &freqs,
        color: colours.trace(d.meta.id),
    });
    SweepPane::Distortion(Box::new(distortion_scene(view, &status, &pv, theme, size)))
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

/// Whether any SPL meter has a band meter.
pub fn has_band_meter(st: &AppState) -> bool {
    st.measurements()
        .iter()
        .any(|m| matches!(&m.config.kind, MeasKind::Spl { config } if config.bands.is_some()))
}

/// What the band view shows of meter `m`, with its `band_leq` frame.
fn band_view<'a>(
    st: &'a AppState,
    m: &Measurement,
    now: Now,
) -> Option<(BandLeqView, &'a TopicFrame)> {
    let tf = frame(st, m.id, Stream::BandLeq)?;
    let FrameData::BandLeq(f) = &tf.frame.data else {
        return None;
    };
    let MeasKind::Spl { config } = &m.config.kind else {
        return None;
    };
    let bands = config.bands.as_deref()?;
    let stale = match freshness(st, tf) {
        Freshness::Stale { age_s } => Some(format!("STALE {}", format::age(age_s))),
        Freshness::Stopped { .. } => Some("STOPPED".into()),
        Freshness::AudioStopped { .. } => Some("AUDIO STOPPED".into()),
        Freshness::Fresh { .. } => None,
    };
    Some((
        BandLeqView {
            meter: m.config.name.clone(),
            cal: spl_cal(st, config.input, f.meta.cal, f.meta.mic_curve, now),
            text: band_leq_text(bands, f),
            stale,
        },
        tf,
    ))
}

/// The band meter of the SPL meter the pane shows (else the first one with a band frame).
pub fn band_leq(
    st: &AppState,
    pane: PaneId,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> Option<BandLeqScene> {
    let (v, tf) = spl_meters(st, pane).find_map(|m| band_view(st, m, now))?;
    let status = status(st, &[tf], None, now);
    Some(band_leq_scene(&v, &status, theme, size))
}

/// The SPL meters in the order the pane picks them: the one it shows first.
fn spl_meters(st: &AppState, pane: PaneId) -> impl Iterator<Item = &Measurement> {
    pane_order(st, pane)
        .into_iter()
        .map(|(_, m)| m)
        .filter(|m| matches!(m.config.kind, MeasKind::Spl { .. }))
}

/// What the Leq view shows of meter `m`, with its `leq` frame.
fn leq_view<'a>(
    st: &'a AppState,
    pane: PaneId,
    m: &'a Measurement,
    now: Now,
) -> Option<LeqView<'a>> {
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
            Freshness::AudioStopped { .. } => Some("AUDIO STOPPED".into()),
            Freshness::Fresh { .. } => None,
        },
        scale: f.meta.scale,
        layout: st.view.spl.layout,
        run: f
            .meta
            .run
            .map(|r| ac2_scene::leq::run_text(&r, cfg, |t| st.local_zone.offset_s(t))),
        stage: st.stage_view(),
        chrome: st.pane_modes(pane).chrome,
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
pub fn leq(
    st: &AppState,
    pane: PaneId,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> Option<LeqScene> {
    let v = spl_meters(st, pane).find_map(|m| leq_view(st, pane, m, now))?;
    let status = status(st, &[], None, now);
    Some(leq_scene(&v, &status, theme, size))
}

/// The SPL measurement the pane shows (else the first one with a frame). `keymap` names the
/// key that resets the meter's statistics.
pub fn spl(
    st: &AppState,
    pane: PaneId,
    keymap: &crate::keys::Keymap,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> Option<SplScene> {
    let (r, tf) = spl_meters(st, pane).find_map(|m| spl_readout_of(st, m, keymap, now))?;
    let status = status(st, &[tf], None, now);
    Some(spl_scene(&r, st.stage_view(), &status, theme, size))
}

/// The meter + Leq view of the SPL meter the pane shows (else the first one with both
/// frames): its held number over its Leq windows.
pub fn meter_leq(
    st: &AppState,
    pane: PaneId,
    keymap: &crate::keys::Keymap,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> Option<MeterLeqScene> {
    let (r, tf, v) = spl_meters(st, pane).find_map(|m| {
        let (r, tf) = spl_readout_of(st, m, keymap, now)?;
        Some((r, tf, leq_view(st, pane, m, now)?))
    })?;
    let status = status(st, &[tf], None, now);
    Some(meter_leq_scene(&r, &v, &status, theme, size))
}

/// The SPL pane's picture in the chosen view ([`SplMode`]). The meter + Leq view shows
/// whichever part has frames until both have (the windows' first second).
pub fn spl_pane(
    st: &AppState,
    pane: PaneId,
    keymap: &crate::keys::Keymap,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> Option<Scene> {
    match st.pane_modes(pane).spl {
        SplMode::Meter => spl(st, pane, keymap, theme, size, now).map(|s| s.scene),
        SplMode::Leq => leq(st, pane, theme, size, now).map(|s| s.scene),
        SplMode::Bands => band_leq(st, pane, theme, size, now).map(|s| s.scene),
        SplMode::MeterLeq => meter_leq(st, pane, keymap, theme, size, now)
            .map(|s| s.leq.scene)
            .or_else(|| spl(st, pane, keymap, theme, size, now).map(|s| s.scene))
            .or_else(|| leq(st, pane, theme, size, now).map(|s| s.scene)),
    }
}
