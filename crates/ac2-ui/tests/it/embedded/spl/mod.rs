//! The SPL meter, Leq windows and limits, the SPL log and its history.

mod limits;
mod log;
mod rig;

use super::harness::{Driver, NAME, R, measure_from_empty, scene_texts};
use ac2_proto::FrameData;
use ac2_proto::model::MeasKind;
use ac2_proto::topic::{Stream, Topic};
use ac2_scene::primitives::Color;
use ac2_scene::theme::Theme;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::Keymap;
use ac2_ui::state::{AppState, Msg, Overlay};
use std::time::{Duration, Instant};

pub(super) fn spl_log(s: &AppState) -> Option<&ac2_proto::model::SplLog> {
    s.daemon()?.spl_logs.first()
}

pub(super) fn judgement(s: &AppState, i: usize) -> Option<ac2_proto::model::LeqJudgement> {
    spl_log(s)?.windows.get(i).map(|w| w.judgement)
}

/// The tiles the SPL pane would show now, from the newest `leq` frame.
pub(super) fn tiles(s: &AppState) -> Vec<ac2_scene::leq::LeqTile> {
    let Some(m) = s
        .measurements()
        .into_iter()
        .find(|m| matches!(m.config.kind, MeasKind::Spl { .. }))
    else {
        return Vec::new();
    };
    let MeasKind::Spl { config } = &m.config.kind else {
        return Vec::new();
    };
    let topic = Topic::Data {
        meas: m.id,
        stream: Stream::Leq,
    };
    match s
        .rig
        .data
        .as_ref()
        .and_then(|d| d.latest.get(&topic))
        .map(|f| &f.frame.data)
    {
        Some(FrameData::Leq(f)) => ac2_scene::leq::leq_tiles(&config.leq, f),
        _ => Vec::new(),
    }
}

/// The columns the SPL pane draws now, as the app builds them (the pane at 1280 × 720):
/// each window's name, background and bar colour, shortest window first.
pub(super) fn columns(s: &AppState) -> Vec<(String, Color, Color)> {
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos() as u64),
        ),
    };
    let size = ac2_scene::primitives::Viewport {
        width: 1280.0,
        height: 720.0,
    };
    ac2_ui::scenes::leq(
        s,
        crate::common::pane(s, ac2_ui::state::PaneKind::Spl),
        &Theme::dark(),
        size,
        now,
    )
    .and_then(|x| x.columns)
    .map(|k| {
        k.columns
            .into_iter()
            .map(|c| (c.name, c.background, c.bar_color))
            .collect()
    })
    .unwrap_or_default()
}

/// The SPL meter's weightings from the keys, from an empty daemon: an SPL meter from the
/// palette, F (Fast → Slow) and Z (A → C) in its pane; the meter reads `LCS` on the next
/// frames, and its number takes a new reading no more often than once a second (Slow's
/// display period), however often frames arrive.
#[test]
fn spl_weightings_from_the_keys_and_a_readable_number() -> R {
    use ac2_proto::model::{TimeWeighting, Weighting};
    use ac2_ui::state::PaneKind;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let ep = daemon.client_config(NAME);
    let mut d = Driver::connect(ep.clone(), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    d.key("Ctrl+K");
    d.send(Msg::Text("new spl".into()));
    d.key("Enter");
    d.until(
        "the SPL dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Spl),
    )?;
    d.key("Enter");
    d.until("the SPL meter running", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spl { .. }) && m.running)
    })?;
    d.key("Alt+4");
    assert_eq!(d.st.layout.focus_kind(), PaneKind::Spl);
    let config = |s: &AppState| {
        s.measurements()
            .into_iter()
            .find_map(|m| match &m.config.kind {
                MeasKind::Spl { config } => Some((m.id, config.clone())),
                _ => None,
            })
    };
    d.key("Shift+F");
    d.until("Slow", |s| {
        config(s).is_some_and(|(_, c)| c.time_weighting == TimeWeighting::Slow)
    })?;
    // Z steps A → C → Z → A from wherever the dialog's meter starts.
    for _ in 0..3 {
        let w = config(&d.st).map(|(_, c)| c.weighting);
        if w == Some(Weighting::C) {
            break;
        }
        d.key("Z");
        d.until("the next weighting", |s| {
            config(s).map(|(_, c)| c.weighting) != w
        })?;
    }
    d.until("C and Slow", |s| {
        config(s).is_some_and(|(_, c)| {
            c.weighting == Weighting::C && c.time_weighting == TimeWeighting::Slow
        })
    })?;
    assert_eq!(
        d.st.kind_modes(ac2_ui::state::PaneKind::Spl).spl,
        ac2_scene::view::SplMode::MeterLeq,
        "the view stays"
    );
    let (id, cfg) = config(&d.st).ok_or("meter")?;
    assert_eq!(cfg.leq, ac2_proto::model::LeqConfig::default_windows());
    let size = ac2_scene::primitives::Viewport {
        width: 1280.0,
        height: 720.0,
    };
    let now = || ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    // The statistics under one heading: since when the meter runs, and its reset key.
    d.until("the meter labelled LCS, its statistics headed once", |s| {
        ac2_ui::scenes::spl(
            s,
            crate::common::pane(s, ac2_ui::state::PaneKind::Spl),
            &Keymap::default(),
            &Theme::dark(),
            size,
            now(),
        )
        .is_some_and(|x| {
            let t = scene_texts(&x.scene);
            t.iter().any(|l| l.starts_with("LCS · "))
                && t.iter()
                    .filter(|l| l.starts_with("meter since") && l.ends_with("· R resets"))
                    .count()
                    == 1
                && t.iter().any(|l| l.starts_with("LCeq "))
        })
    })?;
    // Readings for 3.5 s: each new one at least a display period after the last.
    let mut at = Vec::new();
    let end = Instant::now() + Duration::from_millis(3500);
    let mut frames = std::collections::BTreeSet::new();
    while Instant::now() < end {
        d.pump();
        if let Some(h) = d.st.rig.spl_hold.get(&id)
            && h.frame.meta.time_weighting == TimeWeighting::Slow
            && at.last() != Some(&h.at_ns)
        {
            at.push(h.at_ns);
        }
        if let Some(f) = d.st.rig.data.as_ref().and_then(|x| {
            x.latest.get(&Topic::Data {
                meas: id,
                stream: Stream::Spl,
            })
        }) {
            frames.insert(f.frame.stamp.seq);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        frames.len() > 2 * at.len(),
        "{} frames, {} readings",
        frames.len(),
        at.len()
    );
    assert!(at.len() >= 3, "{at:?}");
    for w in at.windows(2) {
        assert!(
            w[1] - w[0] >= 1_000_000_000,
            "readings {} ms apart",
            (w[1] - w[0]) / 1_000_000
        );
    }
    drop(d);
    drop(daemon);
    Ok(())
}

/// A new SPL meter's pane, from an empty daemon: the meter's number over its Leq windows by
/// default (one caption for both), in the split layout and, W twice, full screen; G steps
/// meter → Leq windows → meter + Leq.
#[test]
fn spl_pane_shows_meter_and_leq_from_an_empty_daemon() -> R {
    use ac2_scene::meter_leq::MeterForm;
    use ac2_scene::primitives::Viewport;
    use ac2_scene::view::SplMode;
    use ac2_ui::state::PaneKind;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let ep = daemon.client_config(NAME);
    let mut d = Driver::connect(ep.clone(), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    d.key("Ctrl+K");
    d.send(Msg::Text("new spl".into()));
    d.key("Enter");
    d.until(
        "the SPL dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Spl),
    )?;
    d.key("Enter");
    d.until("the SPL meter running", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spl { .. }) && m.running)
    })?;
    d.key("Alt+4");
    assert_eq!(d.st.layout.focus_kind(), PaneKind::Spl);
    assert_eq!(
        d.st.kind_modes(ac2_ui::state::PaneKind::Spl).spl,
        SplMode::MeterLeq,
        "the default view"
    );
    let now = || ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let keys = Keymap::default();
    let both = |s: &AppState, width: f32, height: f32| {
        let size = Viewport { width, height };
        ac2_ui::scenes::meter_leq(
            s,
            crate::common::pane(s, ac2_ui::state::PaneKind::Spl),
            &keys,
            &Theme::dark(),
            size,
            now(),
        )
        .filter(|x| {
            let t = scene_texts(&x.leq.scene);
            x.meter.form == MeterForm::Block
                && x.leq.columns.as_ref().is_some_and(|k| k.columns.len() == 5)
                && t.iter().filter(|l| l.starts_with("LAF · ")).count() == 1
                // The caption names the meter once; on the stage, without the history,
                // there is no caption.
                && t.iter().filter(|l| l.starts_with("SPL 1 · ")).count()
                    == usize::from(!s.stage_view() || s.view.spl.layout.history)
                && !t.iter().any(|l| l.starts_with("meter since"))
        })
    };
    // A pane of the split layout: the number over the windows.
    d.until("the meter over its windows", |s| {
        both(s, 640.0, 420.0).is_some()
    })?;
    // W twice: full screen, the same two parts.
    d.key("W");
    d.key("W");
    assert!(d.st.stage_view(), "full screen");
    let stage = both(&d.st, 1920.0, 1080.0).ok_or("both parts full screen")?;
    let under = 1080.0 - stage.leq.caption.bottom();
    assert!(
        stage.meter.region.h > 0.25 * under && stage.meter.region.h < 0.4 * under,
        "{:?} of {under}",
        stage.meter.region
    );
    // G: the meter alone, the windows alone, both again.
    d.key("G");
    assert_eq!(
        d.st.kind_modes(ac2_ui::state::PaneKind::Spl).spl,
        SplMode::Meter
    );
    d.key("G");
    assert_eq!(
        d.st.kind_modes(ac2_ui::state::PaneKind::Spl).spl,
        SplMode::Leq
    );
    d.key("G");
    assert_eq!(
        d.st.kind_modes(ac2_ui::state::PaneKind::Spl).spl,
        SplMode::MeterLeq
    );
    drop(d);
    drop(daemon);
    Ok(())
}
