//! Assembles `ac2-scene` inputs from the mirrored state, the latest frames and the
//! operator's display choices, and calls the builders. Bookkeeping only: which frames go to
//! which pane, names, colours, freshness. Every displayed number is made by `ac2-scene`.

use std::sync::Arc;
use std::time::Instant;

use ac2_client::TopicFrame;
use ac2_proto::FrameData;
use ac2_proto::frame::ProtectionFlags;
use ac2_proto::model::{MeasKind, Measurement, PhaseBasis, Polarity};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{MeasId, WallNs};
use ac2_scene::banner::{Status, no_delay_estimate};
use ac2_scene::distortion::{DistortionScene, SweepView, distortion_scene, sweep_ir_scene};
use ac2_scene::grid::{GridColumns, columns};
use ac2_scene::ir::{IrScene, ir_scene};
use ac2_scene::math::MathStatus;
use ac2_scene::primitives::Viewport;
use ac2_scene::tf::{TfScene, transfer_scene};
use ac2_scene::theme::Theme;
use ac2_scene::time::{ClockOffset, Freshness};
use ac2_scene::trace::{TfTrace, TimeBase};

use crate::state::{AppState, PaneId, PaneKind};

mod alignment;
mod programme;
mod sim;
mod spectrum_pane;
mod spl_meters;
mod walk;
pub use alignment::{alignment, is_alignment, pane_alignment};
pub use programme::programme;
#[cfg(test)]
pub(crate) use spectrum_pane::with_spectrum;
pub use spectrum_pane::{spectrograph, spectrum, spectrum_jump, spectrum_scale};
pub use spl_meters::{
    LocalZone, StripPlace, band_leq, has_band_meter, leq, leq_keyed, leq_meter, meter_leq,
    refresh_spl_scales, spl, spl_pane, spl_placeholder,
};
pub use walk::walk;

/// Inputs that come from the clock, passed in so the assembly stays testable.
#[derive(Clone, Copy, Debug)]
pub struct Now {
    pub instant: Instant,
    pub wall: WallNs,
}

/// The frame of `meas`'s `stream`, if received.
pub fn frame(st: &AppState, meas: MeasId, stream: Stream) -> Option<&TopicFrame> {
    st.rig
        .data
        .as_ref()?
        .latest
        .get(&Topic::Data { meas, stream })
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
    let daemon_silence_s = st
        .rig
        .mirror
        .as_ref()
        .and_then(|m| m.last_ka)
        .map_or(0.0, |t| {
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
        timing: st.rig.mirror.as_ref().and_then(|m| m.timing),
        clock_drift_ppm: st
            .daemon()
            .and_then(|d| d.timing.drift)
            .filter(|d| d.warning)
            .map(|d| d.ppm),
        audio_stopped: audio_stopped(st, now.wall),
        no_delay_estimate: tf_meas.and_then(no_delay_estimate),
        math: None,
        log_not_saved: log_not_saved(st, now.wall),
        file_damaged: st
            .daemon()
            .and_then(|d| ac2_scene::autosave::damage_text(&d.rig.damaged_files)),
        drive: st.drive(),
        advice: None,
    }
}

/// The LOG NOT SAVED banner's text while the daemon cannot write an SPL meter's log.
pub fn log_not_saved(
    st: &AppState,
    client_now: WallNs,
) -> Option<ac2_scene::autosave::LogFailureText> {
    let d = st.daemon()?;
    ac2_scene::autosave::log_failure_text(
        &d.autosave.log_failures,
        |m| {
            d.measurements
                .iter()
                .find(|x| x.id == m)
                .map(|x| x.config.name.clone())
        },
        client_now,
        clock_offset(st),
    )
}

/// The daemon's offset estimate as the scene takes it.
fn clock_offset(st: &AppState) -> ClockOffset {
    ClockOffset(
        st.rig
            .mirror
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
/// A prediction's legend tag: what its curve already contains of the set
/// (`assumes none of it in`, `change since rev 4`, `EQ 1 in · no change`).
fn eq_prediction_note(st: &AppState, m: &Measurement) -> Option<String> {
    let MeasKind::Math { config } = &m.config.kind else {
        return None;
    };
    let (meas, set, curve) = ac2_scene::eq::prediction(&config.expr)?;
    let set = st.meas(meas)?.eq.iter().find(|s| s.id == set)?;
    let basis = ac2_scene::eq::curve_basis(set, curve, |id| {
        st.rig.traces.get(&id).map(|(t, _)| &t.meta)
    });
    Some(ac2_scene::eq::basis_tag(&set.body.name, basis))
}

/// An EQ set's filter curve's tags (`bypassed`, `in console (rev 4)`); `None` for any other
/// trace.
fn eq_curve_tags(st: &AppState, meta: &ac2_proto::model::TraceMeta) -> Option<String> {
    let ac2_proto::model::TraceSource::Eq { meas, set, .. } = meta.source else {
        return None;
    };
    let set = st.meas(meas)?.eq.iter().find(|s| s.id == set)?;
    let tags = ac2_scene::eq::set_tags(set);
    (!tags.is_empty()).then(|| tags.join(" · "))
}

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
        MathStatus::new(config, state, |o| st.operand_name(o))
            .with_arrival(
                config,
                state,
                |o| st.operand_delay(o),
                st.view.temperature_c,
            )
            .with_unsettled(st.unsettled_operands(&config.expr.operands())),
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
/// so. Other groups wait until theirs is the pane's measurement; Imported until moved under
/// a measurement.
pub fn transfer(st: &AppState, pane: PaneId, theme: &Theme, size: Viewport, now: Now) -> TfScene {
    with_transfer(st, pane, theme, |traces, live| {
        let shown: Vec<&TopicFrame> = live.iter().map(|l| l.tf).collect();
        let focus = pane_tf(st, pane);
        let mut status = status(st, &shown, focus, now);
        // The shown math channel's operands.
        let frames: Vec<(MeasId, &TopicFrame)> = live.iter().map(|l| (l.meas.id, l.tf)).collect();
        status.math = shown_math(st, focus, &frames);
        // Headroom advice is about the measurement the pane shows, from live levels; a stored
        // trace or math channel has no stimulus to change.
        status.advice = focus
            .filter(|m| matches!(m.config.kind, MeasKind::Transfer { .. }))
            .and_then(|m| st.advice_of(m.id));
        // Under the stack's top pane, which says it for the column. Faults (clipping,
        // STALE, audio stopped) stay in every pane: each must be seen wherever one looks.
        if st.stacked_under(pane) {
            status.no_delay_estimate = None;
            status.advice = None;
        }
        transfer_scene(
            traces,
            &st.rig.tf_display,
            &status,
            &st.view_for(pane),
            theme,
            size,
        )
    })
}

/// Where the cursor jumps in transfer pane `pane` ([`ac2_scene::tf::jump`]): along the
/// curve it draws as it draws it.
pub fn transfer_jump(
    st: &AppState,
    pane: PaneId,
    shown: ac2_scene::axis::Range,
    kind: ac2_scene::cursor_jump::Landmark,
    step: ac2_scene::cursor_jump::Step,
) -> Option<f64> {
    let theme = st.scene_theme();
    with_transfer(st, pane, &theme, |traces, _| {
        ac2_scene::tf::jump(
            traces,
            &st.rig.tf_display,
            &st.view_for(pane),
            shown,
            kind,
            step,
        )
    })
}

/// Calls `f` with the transfer pane's curves in legend order and the live frames they
/// come from.
fn with_transfer<R>(
    st: &AppState,
    pane: PaneId,
    theme: &Theme,
    f: impl FnOnce(&[TfTrace<'_>], &[LiveTf<'_>]) -> R,
) -> R {
    let grids = st.rig.data.as_ref().map(|d| &d.grids);
    let colours = st.curve_colours(theme);
    let pane_shows = st.pane_meas(pane);
    let mut live = Vec::new();
    for (_, m) in pane_order(st, pane) {
        // The pane's group (its measurement and the math channels made on it), and what is
        // compared. A hidden measurement keeps its colour: showing it again brings back the
        // same curve.
        if !(st.live_on_pane(PaneKind::Transfer, m, pane_shows)
            || st.live_compared_on(PaneKind::Transfer, m, pane_shows))
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
        .rig
        .traces
        .values()
        .filter(|(t, _)| {
            st.on_pane(PaneKind::Transfer, &t.meta, pane_shows)
                || st.trace_compared_on(PaneKind::Transfer, &t.meta, pane_shows)
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
        t.offset_db = st.meas_offset(l.meas.id);
        t.polarity = if e.inverted {
            Polarity::Inverted
        } else {
            Polarity::Normal
        };
        t.note = eq_prediction_note(st, l.meas)
            .or_else(|| math_status(st, l.meas, &l.tf.frame.data).map(|a| a.tag()));
        t.compared = st.live_compared_on(PaneKind::Transfer, l.meas, pane_shows);
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
        t.selected = st.rig.selected_trace == Some(data.meta.id);
        if let Some(tags) = eq_curve_tags(st, &data.meta) {
            t.note = Some(tags);
        }
        t.compared = st.trace_compared_on(PaneKind::Transfer, &data.meta, pane_shows);
        // A capture from an earlier epoch is not in this epoch's time base (decision 8a).
        if let (Some(epoch), Some(cur)) = (
            data.meta.source.shared_epoch(),
            st.rig.mirror.as_ref().and_then(|m| m.session_epoch),
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
    f(&traces, &live)
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
            &ac2_scene::ir::IrCtx {
                status: &status,
                view: &view,
                axes: &view.ir.axes,
                style: ac2_scene::view::Style {
                    chrome: view.chrome,
                    theme,
                },
                size,
            },
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
            &ac2_scene::ir::IrCtx {
                status: &status,
                view: &view,
                axes: &view.ir.axes,
                style: ac2_scene::view::Style {
                    chrome: view.chrome,
                    theme,
                },
                size,
            },
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
        ac2_scene::view::Style {
            chrome: view.chrome,
            theme,
        },
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

/// The sweep pane: the distortion, impulse response or room parameters of the run of its
/// sweep measurement it shows ([`AppState::pane_sweep`]); the distortion view draws the
/// runs compared (C) over it. Never another measurement's run otherwise.
pub fn sweep(st: &AppState, pane: PaneId, theme: &Theme, size: Viewport, now: Now) -> SweepPane {
    let status = status(st, &[], None, now);
    let shown = st.pane_sweep(pane);
    let pv = st.view_for(pane);
    if st.pane_modes(pane).sweep == ac2_scene::view::SweepMode::Room {
        use ac2_scene::room::RoomShown;
        let walk = st.pane_walk_room(pane);
        let (room, name) = match walk {
            Some(w) => (w.room.as_ref().map(RoomShown::Walk), Some(w.name.as_str())),
            None => (
                shown
                    .and_then(|(d, _)| d.sweep.as_ref()?.room.as_ref())
                    .map(RoomShown::Run),
                shown.map(|(d, _)| d.meta.edit.name.as_str()),
            ),
        };
        let simulated = pv
            .sim_shown()
            .then(|| sim::room(st, walk, shown.map(|(d, _)| &d.meta)))
            .flatten();
        return SweepPane::Room(Box::new(ac2_scene::room::room_scene_with_sim(
            room,
            name,
            simulated,
            &st.new_sweep_key(),
            &status,
            theme,
            size,
        )));
    }
    if st.pane_modes(pane).sweep.ir().is_some()
        && let Some((d, _)) = shown
    {
        let color = st.curve_colours(theme).trace(d.meta.id);
        if let Some(mut s) = sweep_ir_scene(d, color, &status, &pv, theme, size) {
            if pv.sim_shown() {
                sim::reflections(st, &d.meta, &mut s, theme);
            }
            return SweepPane::Ir(Box::new(s));
        }
    }
    let freqs = shown.map(|(_, g)| g.centres()).unwrap_or_default();
    let colours = st.curve_colours(theme);
    let view = shown.map(|(d, _)| SweepView {
        data: d,
        freqs: &freqs,
        color: colours.trace(d.meta.id),
    });
    let others = st.compared_sweeps(pane);
    let other_freqs: Vec<Vec<f64>> = others.iter().map(|(_, g)| g.centres()).collect();
    let compared: Vec<SweepView<'_>> = others
        .iter()
        .zip(&other_freqs)
        .map(|((d, _), freqs)| SweepView {
            data: d,
            freqs,
            color: colours.trace(d.meta.id),
        })
        .collect();
    SweepPane::Distortion(Box::new(distortion_scene(
        view, &compared, &status, &pv, theme, size,
    )))
}
