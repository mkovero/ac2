//! The SPL pane: the meter readout, its Leq windows, the band meter and the local zone of
//! the SPL log.

use ac2_client::TopicFrame;
use ac2_proto::frame::BandLeqFrame;
use ac2_proto::model::{MeasKind, Measurement};
use ac2_proto::topic::Stream;
use ac2_proto::units::MeasId;
use ac2_proto::{Frame, FrameData};
use ac2_scene::band_leq::{BandLeqScene, BandLeqView, band_leq_scene, band_leq_text};
use ac2_scene::format;
use ac2_scene::leq::{LeqScene, LeqView, leq_scene, leq_tiles};
use ac2_scene::meter_leq::{MeterLeqScene, meter_leq_scene};
use ac2_scene::primitives::{Scene, Viewport};
use ac2_scene::spl::{SplReadout, SplScene, cal_text, spl_readout, spl_scene};
use ac2_scene::theme::Theme;
use ac2_scene::time::{ClockOffset, Freshness};
use ac2_scene::view::SplMode;

use crate::state::{AppState, PaneId};

use super::{Now, frame, freshness, status};

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

/// Whether any SPL meter has a band meter.
pub fn has_band_meter(st: &AppState) -> bool {
    st.measurements()
        .iter()
        .any(|m| matches!(&m.config.kind, MeasKind::Spl { config } if config.bands.is_some()))
}

/// The frame `tf` replaced and the seconds since `tf` arrived, while its bars still ease
/// from that one: the operator lets them, and the values are fresh (stale or stopped values
/// are drawn as they are, so nothing moves on screen that is not being measured).
fn easing_from<'a>(st: &'a AppState, tf: &TopicFrame, now: Now) -> Option<(&'a Frame, f64)> {
    if !st.prefs.bar_ease || !matches!(freshness(st, tf), Freshness::Fresh { .. }) {
        return None;
    }
    let since = now
        .instant
        .saturating_duration_since(tf.received)
        .as_secs_f64();
    let from = st.bar_from.get(&tf.topic)?;
    ac2_scene::bar_ease::easing(since).then_some((from.as_ref(), since))
}

/// What the band view of pane `pane` shows of meter `m`, with its `band_leq` frame.
fn band_view<'a>(
    st: &'a AppState,
    pane: PaneId,
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
    let chrome = st.pane_modes(pane).chrome;
    let mut text = band_leq_text(bands, f, chrome);
    if let Some((from, since)) = easing_from(st, tf, now)
        && let FrameData::BandLeq(p) = &from.data
    {
        let levels = |f: &BandLeqFrame| f.leq.iter().map(|&l| f64::from(l)).collect::<Vec<_>>();
        let drawn = ac2_scene::bar_ease::eased(&levels(p), &levels(f), since);
        for b in text.rows.iter_mut().flat_map(|r| &mut r.bars) {
            b.drawn_db = drawn.get(b.window).copied();
        }
    }
    Some((
        BandLeqView {
            meter: m.config.name.clone(),
            cal: spl_cal(st, config.input, f.meta.cal, f.meta.mic_curve, now),
            text,
            stale,
            chrome,
        },
        tf,
    ))
}

/// The band meter of the SPL meter the pane shows.
pub fn band_leq(
    st: &AppState,
    pane: PaneId,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> Option<BandLeqScene> {
    let (v, tf) = band_view(st, pane, pane_meter(st, pane)?, now)?;
    let status = status(st, &[tf], None, now);
    Some(band_leq_scene(&v, &status, theme, size))
}

/// The SPL meter pane `pane` shows, unless hidden. Only its own: a meter is a number, not
/// a curve to overlay, so two meters are compared side by side, each on a pane of its own;
/// one standing in for another would be read as the wrong place's level.
fn pane_meter(st: &AppState, pane: PaneId) -> Option<&Measurement> {
    st.pane_meas(pane)
        .filter(|m| matches!(m.config.kind, MeasKind::Spl { .. }) && !st.meas_hidden(m))
}

/// What SPL pane `pane` says while it draws nothing: no meter, or why its own meter has
/// nothing to show in the pane's view.
pub fn spl_placeholder(st: &AppState, pane: PaneId, keymap: &crate::keys::Keymap) -> String {
    use ac2_scene::missing::{Missing, missing_text};
    let Some(m) = st
        .pane_meas(pane)
        .filter(|m| matches!(m.config.kind, MeasKind::Spl { .. }))
    else {
        return "no SPL meter".to_owned();
    };
    let mode = st.pane_modes(pane).spl;
    let has = |stream| frame(st, m.id, stream).is_some();
    let bands = matches!(&m.config.kind, MeasKind::Spl { config } if config.bands.is_some());
    let a_second = Some("they show once the meter has measured a second");
    let why = if st.meas_hidden(m) {
        Missing::Hidden
    } else if mode == SplMode::Bands && !bands {
        Missing::NoBandMeter
    } else if !m.running {
        Missing::Stopped
    } else {
        match mode {
            SplMode::Leq => Missing::NotYet {
                what: "Leq windows",
                after: a_second,
            },
            SplMode::Bands => Missing::NotYet {
                what: "band frame",
                after: Some("the bands show once the meter has measured a second"),
            },
            SplMode::Meter | SplMode::MeterLeq if !has(Stream::Spl) => Missing::NotYet {
                what: "SPL frame",
                after: None,
            },
            SplMode::Meter | SplMode::MeterLeq => Missing::NotYet {
                what: "Leq windows",
                after: a_second,
            },
        }
    };
    let (command, scope) = if why == Missing::Hidden {
        (
            crate::keys::CommandId::ToggleSelected,
            crate::keys::Scope::Global,
        )
    } else {
        (crate::keys::CommandId::StartStop, crate::keys::Scope::Spl)
    };
    let key = keymap
        .first_chord(command, scope)
        .map_or_else(|| "the palette".to_owned(), |c| c.label());
    missing_text(&m.config.name, why, &key)
}

/// What the Leq view shows of meter `m`, with its `leq` frame.
fn leq_view<'a>(
    st: &'a AppState,
    pane: PaneId,
    m: &'a Measurement,
    keymap: Option<&crate::keys::Keymap>,
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
    let mut tiles = leq_tiles(cfg, f);
    if let Some((from, since)) = easing_from(st, tf, now)
        && let FrameData::Leq(p) = &from.data
    {
        let from: Vec<f64> = leq_tiles(cfg, p).iter().map(|t| t.bar_db()).collect();
        let to: Vec<f64> = tiles.iter().map(|t| t.bar_db()).collect();
        let drawn = ac2_scene::bar_ease::eased(&from, &to, since);
        for (t, d) in tiles.iter_mut().zip(drawn) {
            t.drawn_db = Some(d);
        }
    }
    Some(LeqView {
        meter: m.config.name.clone(),
        cal: spl_cal(st, config.input, f.meta.cal, f.meta.mic_curve, now),
        cfg,
        tiles,
        history: st.leq.history.get(&m.id).map(|(_, h)| h),
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
        strip: st.strip_view(pane, m.id, live_key(keymap)),
    })
}

/// The key that brings a history strip back to live, named.
fn live_key(keymap: Option<&crate::keys::Keymap>) -> Option<String> {
    keymap?
        .first_chord(
            crate::keys::CommandId::ResetView,
            crate::keys::Scope::Global,
        )
        .map(|c| c.label())
}

/// The SPL meter whose Leq windows pane `pane` shows: the one it shows, once it has a
/// `leq` frame.
pub fn leq_meter(st: &AppState, pane: PaneId) -> Option<MeasId> {
    pane_meter(st, pane)
        .filter(|m| frame(st, m.id, Stream::Leq).is_some())
        .map(|m| m.id)
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

/// The Leq windows of the SPL meter the pane shows.
pub fn leq(
    st: &AppState,
    pane: PaneId,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> Option<LeqScene> {
    leq_keyed(st, pane, None, theme, size, now)
}

/// [`leq`], with `keymap` naming the key back to live on a strip looking back.
pub fn leq_keyed(
    st: &AppState,
    pane: PaneId,
    keymap: Option<&crate::keys::Keymap>,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> Option<LeqScene> {
    let v = leq_view(st, pane, pane_meter(st, pane)?, keymap, now)?;
    let status = status(st, &[], None, now);
    Some(leq_scene(&v, &status, theme, size))
}

/// The SPL measurement the pane shows. `keymap` names the key that resets the meter's
/// statistics.
pub fn spl(
    st: &AppState,
    pane: PaneId,
    keymap: &crate::keys::Keymap,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> Option<SplScene> {
    let (r, tf) = spl_readout_of(st, pane_meter(st, pane)?, keymap, now)?;
    let status = status(st, &[tf], None, now);
    Some(spl_scene(&r, st.stage_view(), &status, theme, size))
}

/// The meter + Leq view of the SPL meter the pane shows, once it has both frames: its held
/// number over its Leq windows.
pub fn meter_leq(
    st: &AppState,
    pane: PaneId,
    keymap: &crate::keys::Keymap,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> Option<MeterLeqScene> {
    let m = pane_meter(st, pane)?;
    let (r, tf) = spl_readout_of(st, m, keymap, now)?;
    let v = leq_view(st, pane, m, Some(keymap), now)?;
    let status = status(st, &[tf], None, now);
    Some(meter_leq_scene(&r, &v, &status, theme, size))
}

/// Where an SPL pane's Leq history strip is drawn and the time it spans (daemon wall
/// clock, s, left edge to right): what the mouse on it moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StripPlace {
    pub plot: ac2_scene::primitives::Rect,
    pub from: f64,
    pub to: f64,
}

fn strip_place(s: &LeqScene) -> Option<StripPlace> {
    s.history.as_ref().map(|h| StripPlace {
        plot: h.plot,
        from: h.from,
        to: h.to,
    })
}

/// The SPL pane's picture in the chosen view ([`SplMode`]), with its history strip's place
/// when one is drawn. The meter + Leq view shows whichever part has frames until both have
/// (the windows' first second).
pub fn spl_pane(
    st: &AppState,
    pane: PaneId,
    keymap: &crate::keys::Keymap,
    theme: &Theme,
    size: Viewport,
    now: Now,
) -> Option<(Scene, Option<StripPlace>)> {
    let leq = |s: LeqScene| {
        let place = strip_place(&s);
        (s.scene, place)
    };
    match st.pane_modes(pane).spl {
        SplMode::Meter => spl(st, pane, keymap, theme, size, now).map(|s| (s.scene, None)),
        SplMode::Leq => leq_keyed(st, pane, Some(keymap), theme, size, now).map(leq),
        SplMode::Bands => band_leq(st, pane, theme, size, now).map(|s| (s.scene, None)),
        SplMode::MeterLeq => meter_leq(st, pane, keymap, theme, size, now)
            .map(|s| leq(s.leq))
            .or_else(|| spl(st, pane, keymap, theme, size, now).map(|s| (s.scene, None)))
            .or_else(|| leq_keyed(st, pane, Some(keymap), theme, size, now).map(leq)),
    }
}
