//! The top bar, the SPL meter and settings.

use super::*;

mod settings;

/// The health meter after the stream line: the daemon's host CPU, its callback load and
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
            stream_load: Some(0.12),
            stream_peak: Some(0.48),
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
            .any(|t| t == "CPU 23 % · DSP 14 % · 0 xruns · 14 Mbit/s · 30 fps"),
        "{bar:?}"
    );
    snapshot(&mut h, "top_bar_health");
}

/// The bar's line between its fixed ends. In a wide window it fits and stands still. In a
/// narrow one it scrolls leftwards with time, clipped at its room's edges and never over
/// the rig drop-down or the items at the right; it holds while the pointer rests on it;
/// the rig drop-down stays where it is and opens on a click. Pictured at its start and
/// part way along.
#[test]
fn top_bar_line_scrolls_when_narrow() {
    use std::sync::atomic::Ordering;

    use ac2_proto::frame::{AudioHealth, DaemonHealth};
    use ac2_ui::marquee::{GAP_PX, Marquee, SPEED_PX_S};
    use egui::accesskit::Role;
    use egui_kittest::kittest::{By, NodeT};
    if !have_gpu("top_bar_line_scrolls_when_narrow") {
        return;
    }
    const WIDE: f32 = 2400.0;
    const NARROW: f32 = 700.0;
    let rig = Rig::start();
    let mut h = harness(options());
    step_until(&mut h, "live frames", live);
    rig.set_health(DaemonHealth {
        host: "pupu".into(),
        cpu: Some(0.23),
        audio: Some(AudioHealth {
            load: Some(0.14),
            peak: Some(0.31),
            stream_load: Some(0.12),
            stream_peak: Some(0.48),
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
    let find = |l: &[(String, egui::Rect)], what: &str| {
        l.iter().find(|(t, _)| t.contains(what)).map(|(_, r)| *r)
    };
    let rig_name = h.state().state.rig_name(h.state().state.active());

    // Wide: the whole line stands in its room, and stays.
    h.set_size(egui::vec2(WIDE, 800.0));
    for _ in 0..4 {
        h.step();
    }
    let wide = bar(&h);
    assert_eq!(h.state().marquee.offset(), 0.0, "{wide:?}");
    let place = find(&wide, "local").unwrap_or_else(|| panic!("no place in {wide:?}"));
    for _ in 0..4 {
        h.step();
    }
    assert_eq!(h.state().marquee.offset(), 0.0);
    assert_eq!(find(&bar(&h), "local"), Some(place));
    assert!(find(&wide, "frames").is_some(), "{wide:?}");
    assert!(find(&wide, "CPU 23 %").is_some(), "{wide:?}");

    // Narrow: it moves with time.
    h.set_size(egui::vec2(NARROW, 800.0));
    h.step();
    let check = |labels: &[(String, egui::Rect)]| {
        let rig = find(labels, &rig_name).unwrap_or_else(|| panic!("no rig in {labels:?}"));
        for (t, r) in labels {
            assert!(
                r.min.x >= 0.0 && r.max.x <= NARROW,
                "{t:?} {r:?} outside the window"
            );
            // Nothing of the line under the drop-down at the left.
            assert!(
                *r == rig || r.min.x >= rig.max.x,
                "{t:?} {r:?} over the rig drop-down {rig:?}"
            );
        }
        rig
    };
    let rig_at = check(&bar(&h));
    let o0 = h.state().marquee.offset();
    for _ in 0..3 {
        h.step();
    }
    let o1 = h.state().marquee.offset();
    assert!(o1 > o0 + 1.0, "{o0} → {o1}");
    assert_eq!(check(&bar(&h)), rig_at, "the rig drop-down moved");

    // The pointer on the line holds it; away, it goes on.
    let on_line = egui::pos2(rig_at.max.x + 120.0, rig_at.center().y);
    h.event(egui::Event::PointerMoved(on_line));
    h.step();
    h.step();
    let held = h.state().marquee.offset();
    for _ in 0..4 {
        h.step();
    }
    assert_eq!(h.state().marquee.offset(), held);
    h.event(egui::Event::PointerMoved(egui::pos2(300.0, 500.0)));
    h.step();
    h.step();
    for _ in 0..3 {
        h.step();
    }
    assert!(h.state().marquee.offset() != held);

    let pin_at = |t: f64| move |a: &mut App| a.marquee = Marquee::at(t);
    snapshot_when(&mut h, "top_bar_marquee_start", pin_at(0.0), |_| true);
    let start = bar(&h);
    check(&start);
    let place_start = find(&start, "local").unwrap_or_else(|| panic!("no place in {start:?}"));
    assert!((place_start.min.x - rig_at.max.x) < 60.0, "{place_start:?}");
    snapshot_when(&mut h, "top_bar_marquee_mid", pin_at(8.0), |_| true);
    let mid = bar(&h);
    check(&mid);
    // 240 px on: the line's start has left its room at the left.
    assert!(8.0 * SPEED_PX_S > f64::from(place_start.width()));
    assert!(
        find(&mid, "local").is_none_or(|r| r.min.x > place_start.max.x + GAP_PX),
        "{mid:?}"
    );

    // The drop-down under the rig's name, wherever the line is.
    h.get_by_label(&rig_name).click();
    h.step();
    h.step();
    assert!(
        matches!(h.state().state.overlay, ac2_ui::state::Overlay::RigMenu(_)),
        "{:?}",
        h.state().state.overlay
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
