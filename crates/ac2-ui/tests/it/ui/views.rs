//! The top bar, the SPL meter and settings.

use super::*;

mod settings;

/// The health meter after the session line: the daemon's host CPU, its callback load and
/// xruns, the data rate and the window's frame rate, from figures the test sets (the rig's
/// keepalives and this window's own, fed at chosen instants).
#[test]
fn top_bar_health_meter() {
    use std::sync::atomic::Ordering;

    use ac2_proto::frame::{AudioHealth, DaemonHealth};
    use egui::accesskit::Role;
    use egui_kittest::kittest::{By, NodeT};
    if !have_gpu("top_bar_health_meter") {
        return;
    }
    let rig = Rig::start();
    let mut h = harness(options());
    step_until(&mut h, "live frames", live);
    rig.set_health(DaemonHealth {
        host: "pupu".into(),
        cpu: Some(0.23),
        audio: Some(AudioHealth {
            load: Some(0.14),
            peak: Some(0.31),
            session_load: Some(0.12),
            session_peak: Some(0.48),
            xruns: 0,
        }),
    });
    step_until(&mut h, "the daemon's figures", |a| {
        a.state
            .rig
            .mirror
            .as_ref()
            .and_then(|m| m.health.as_ref())
            .is_some_and(|h| h.cpu.is_some())
    });
    {
        let app = h.state_mut();
        // A counter of the test's own: the link keeps adding to the one it was given.
        app.health.received = std::sync::Arc::default();
        let t0 = Instant::now() + Duration::from_secs(10);
        app.health.data_arrived();
        app.health.frame(t0, false);
        app.health.received.fetch_add(1_800_000, Ordering::Relaxed);
        for k in 1..=30u64 {
            app.health.data_arrived();
            app.health
                .frame(t0 + Duration::from_secs(1) * k as u32 / 30, false);
        }
    }
    for _ in 0..3 {
        h.step();
    }
    let bar: Vec<String> = h
        .query_all(By::new().predicate(|n| n.role() == Role::Label))
        .filter(|n| n.rect().max.y <= 34.0)
        .map(|n| {
            let a = n.accesskit_node();
            a.label().or_else(|| a.value()).unwrap_or_default()
        })
        .collect();
    assert!(
        bar.iter()
            .any(|t| t == "· CPU remote 23 % · DSP 14 % · 0 xruns · 14 Mbit/s · 30 fps"),
        "{bar:?}"
    );
    snapshot(&mut h, "top_bar_health");
}

/// A narrow window: the session line and the health meter take the room between the bar's
/// two groups and scroll through it, clipped at its edges, never over the stimulus items.
/// A long daemon host name stays out of the bar (it is in the tooltip). Pictured in the
/// start pause and part way along.
#[test]
fn top_bar_marquee_scrolls_when_narrow() {
    use std::sync::atomic::Ordering;

    use ac2_proto::frame::{AudioHealth, DaemonHealth};
    use ac2_ui::marquee::{Marquee, PAUSE_S};
    use egui::accesskit::Role;
    use egui_kittest::kittest::{By, NodeT};
    if !have_gpu("top_bar_marquee_scrolls_when_narrow") {
        return;
    }
    const HOST: &str = "pupu-stage-left-amp-rack-02.venue.example";
    const WIDTH: f32 = 700.0;
    let rig = Rig::start();
    let mut h = harness(options());
    step_until(&mut h, "live frames", live);
    rig.set_health(DaemonHealth {
        host: HOST.into(),
        cpu: Some(0.23),
        audio: Some(AudioHealth {
            load: Some(0.14),
            peak: Some(0.31),
            session_load: Some(0.12),
            session_peak: Some(0.48),
            xruns: 0,
        }),
    });
    step_until(&mut h, "the daemon's figures", |a| {
        a.state
            .rig
            .mirror
            .as_ref()
            .and_then(|m| m.health.as_ref())
            .is_some_and(|h| h.host == HOST)
    });
    {
        let app = h.state_mut();
        app.health.received = std::sync::Arc::default();
        let t0 = Instant::now() + Duration::from_secs(10);
        app.health.data_arrived();
        app.health.frame(t0, false);
        app.health.received.fetch_add(1_800_000, Ordering::Relaxed);
        for k in 1..=30u64 {
            app.health.data_arrived();
            app.health
                .frame(t0 + Duration::from_secs(1) * k as u32 / 30, false);
        }
    }
    h.set_size(egui::vec2(WIDTH, 800.0));
    let bar = |h: &Harness<'_, App>| -> Vec<(String, egui::Rect)> {
        h.query_all(By::new().predicate(|n| n.role() == Role::Label))
            .map(|n| {
                let a = n.accesskit_node();
                (
                    a.label().or_else(|| a.value()).unwrap_or_default(),
                    n.rect(),
                )
            })
            .filter(|(_, r)| r.max.y <= 34.0)
            .collect()
    };
    let session = |l: &[(String, egui::Rect)]| {
        l.iter()
            .find(|(t, _)| t.contains("frames"))
            .map(|(_, r)| *r)
    };
    let health = |l: &[(String, egui::Rect)]| {
        l.iter()
            .find(|(t, _)| t.contains("CPU remote"))
            .map(|(_, r)| *r)
    };
    let check = |labels: &[(String, egui::Rect)]| {
        assert!(
            labels.iter().all(|(t, _)| !t.contains(HOST)),
            "host name in the bar: {labels:?}"
        );
        for (i, (a, ra)) in labels.iter().enumerate() {
            assert!(
                ra.min.x >= 0.0 && ra.max.x <= WIDTH,
                "{a:?} {ra:?} outside the window"
            );
            for (b, rb) in &labels[i + 1..] {
                let o = ra.intersect(*rb);
                assert!(
                    o.width() <= 0.5 || o.height() <= 0.5,
                    "{a:?} {ra:?} overlaps {b:?} {rb:?}"
                );
            }
        }
        // The stimulus state keeps its place right of the segment.
        let stim = labels
            .iter()
            .find(|(t, _)| t.contains("no level") || t.contains("dBFS"))
            .unwrap_or_else(|| panic!("no stimulus level in {labels:?}"))
            .1;
        for r in [session(labels), health(labels)].into_iter().flatten() {
            assert!(r.max.x < stim.min.x, "{r:?} reaches {stim:?}: {labels:?}");
        }
    };

    let pin_at = |t: f64| move |a: &mut App| a.marquee = Marquee::at(t);
    snapshot_when(&mut h, "top_bar_marquee_start", pin_at(0.5), |_| true);
    let start = bar(&h);
    check(&start);
    let session_start = session(&start).unwrap_or_else(|| panic!("no session in {start:?}"));

    snapshot_when(&mut h, "top_bar_marquee_mid", pin_at(PAUSE_S + 3.0), |_| {
        true
    });
    let mid = bar(&h);
    check(&mid);
    // Scrolled left by about 100 px: the session line shows less of itself (or is gone),
    // the health meter more, both inside the segment's room.
    let session_mid = session(&mid);
    assert!(
        session_mid.is_none_or(|r| r.width() < session_start.width() - 50.0),
        "{session_start:?} → {session_mid:?}"
    );
    assert!(
        session_mid.is_none_or(|r| r.min.x >= session_start.min.x - 0.5),
        "{session_mid:?} left of the segment"
    );
    let health_mid = health(&mid).unwrap_or_else(|| panic!("no health in {mid:?}"));
    assert!(
        health(&start).is_none_or(|r| health_mid.width() > r.width()),
        "{:?} → {health_mid:?}",
        health(&start)
    );
}

/// The SPL meter maximised (W): the held level large and centred with `LAF · dBFS` under
/// it, the level bar, the statistics and the footer; W again, full screen: the stage view,
/// the meter alone on the screen, without the calibration footer.
#[test]
fn spl_meter_big_and_stage() {
    if !have_gpu("spl_meter_big_and_stage") {
        return;
    }
    let _rig = Rig::start();
    let mut h = harness(options());
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::ALT, Key::Num4);
    // G: from meter + Leq (the default) to the meter alone.
    h.key_press(Key::G);
    h.key_press(Key::W);
    step_until(&mut h, "the meter maximised", |a| {
        a.state.kind_modes(ac2_ui::state::PaneKind::Spl).spl == ac2_scene::view::SplMode::Meter
            && a.state.layout.maximized
            && a.state.layout.focus_kind() == PaneKind::Spl
            && a.state.rig.folded.spl_hold.contains_key(&MeasId(4))
    });
    // The group's toast expires on the wall clock: a slower rasterizer reaches the stage
    // view after it has gone, so neither picture shows it.
    h.state_mut().state.toasts.list.clear();
    snapshot(&mut h, "spl_meter_big");
    h.key_press(Key::W);
    step_until(&mut h, "the stage view", |a| a.state.stage_view());
    h.state_mut().state.toasts.list.clear();
    snapshot(&mut h, "spl_meter_stage");
}
