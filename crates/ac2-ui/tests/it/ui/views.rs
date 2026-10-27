//! The top bar, key hints, the SPL meter, the spectrograph and settings.

use super::*;

mod settings;

/// A set of sweeps running while the transfer pane is maximised: the progress strip shows
/// which sweep of how many, the bar and the time left, and the sidebar marks the sweep's
/// inputs; the strip's Stop button stops the stimulus. The strip is drawn over the panes:
/// the sweep starting moves no pane (its title row stays where it was).
#[test]
fn sweep_progress_strip() {
    use ac2_proto::event::Change;
    use ac2_proto::model::{EssSpec, SweepRun, SweepStatus};
    use ac2_proto::units::{ClientId, Dbfs, Hz, Seconds, SweepId, WallNs};
    if !have_gpu("sweep_progress_strip") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    // The pane's measurement chip, in its title row (the tree has rows of that name too).
    let title = |h: &Harness<'_, App>| {
        use egui_kittest::kittest::By;
        h.query_all(By::new().label("Main L"))
            .map(|n| n.rect())
            .filter(|r| r.min.x > 230.0)
            .fold(egui::Rect::NOTHING, |a, r| a.union(r))
    };
    let before = title(&h);
    assert!(before.is_positive(), "no measurement chip in the pane");
    {
        let mut s = rig.fake.lock();
        let mut g = s.state.generator.clone();
        g.armed = true;
        g.firing = true;
        s.commit(Change::Generator(g));
        s.commit(Change::Sweep(SweepRun {
            id: SweepId(1),
            meas: ac2_proto::units::MeasId(1),
            owner: ClientId("other".into()),
            name: "Sweep 1".into(),
            reference_input: 0,
            measurement_input: 1,
            outputs: vec![0, 1],
            level: Dbfs(-30.0),
            sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(30.0)),
            sweep_duration: Seconds(30.0),
            post_roll: Seconds(1.0),
            repeats: 2,
            gate: None,
            lf_harmonics: ac2_proto::model::LfHarmonics::Standard,
            resolution: ac2_proto::model::Resolution::FortyEighth,
            status: SweepStatus::Playing { repeat: 1 },
            started_at: WallNs(0),
        }));
    }
    step_until(&mut h, "the progress strip", |a| {
        a.state.operation().is_some()
    });
    let p = h.state().state.operation().expect("progress");
    assert_eq!(p.step, "sweep 1 of 2");
    h.step();
    assert_eq!(title(&h), before, "the strip moved the panes");
    h.key_press(Key::W);
    step_until(&mut h, "maximized", |a| a.state.layout.maximized);
    h.state_mut().state.toasts.list.clear();
    // The bar at the start of the step, for a picture that does not depend on timing.
    let start = {
        let st = &h.state().state;
        let run = st.daemon().and_then(|s| s.sweep.clone()).expect("run");
        ac2_scene::progress::sweep(&run, 0.0).expect("progress")
    };
    snapshot_when(
        &mut h,
        "sweep_progress",
        |a| {
            let now = a.state.now_s;
            if let Some(seen) = &mut a.state.rig.sweep.step_seen {
                seen.2 = now;
            }
        },
        |a| {
            a.state.operation().is_some_and(|p| {
                p.remaining == start.remaining && (p.fraction - start.fraction).abs() < 2e-3
            })
        },
    );
    let stops = rig.fake.executions("gen.stop");
    h.get_by_label_contains("Stop (").click();
    step_until(&mut h, "stop sent", |_| {
        rig.fake.executions("gen.stop") > stops
    });
}

/// The top bar never overlaps itself: with the generator armed by another client (badge,
/// level, "held by", hint all shown) and a session open, at every width from wide to
/// narrow every label in the bar keeps its own space inside the window; lower-priority
/// texts shorten or go first, the state badge always stays.
#[test]
fn top_bar_never_overlaps() {
    use ac2_proto::event::Change;
    use egui::accesskit::Role;
    use egui_kittest::kittest::{By, NodeT};
    if !have_gpu("top_bar_never_overlaps") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    {
        let mut s = rig.fake.lock();
        let mut g = s.state.generator.clone();
        g.owner = Some(ac2_proto::units::ClientId("FOH laptop".into()));
        g.armed = true;
        s.commit(Change::Generator(g));
    }
    step_until(&mut h, "armed by the other client", |a| {
        a.state.daemon().is_some_and(|s| s.generator.armed)
    });
    for width in [1600.0, 1290.0, 1100.0, 900.0, 760.0, 640.0] {
        h.set_size(egui::vec2(width, 800.0));
        for _ in 0..3 {
            h.step();
        }
        let labels: Vec<(String, egui::Rect)> = h
            .query_all(By::new().predicate(|n| n.role() == Role::Label))
            .map(|n| {
                let a = n.accesskit_node();
                (
                    a.label().or_else(|| a.value()).unwrap_or_default(),
                    n.rect(),
                )
            })
            .filter(|(_, r)| r.max.y <= 34.0)
            .collect();
        assert!(
            labels.iter().any(|(t, _)| t == "ARMED"),
            "{width} px: no badge in {labels:?}"
        );
        let has = |s: &str| labels.iter().any(|(t, _)| t.contains(s));
        if width >= 1600.0 {
            // Wide: everything in full.
            for s in ["held by FOH laptop", "commands", "frames", "fake daemon"] {
                assert!(has(s), "{width} px: no {s:?} in {labels:?}");
            }
        }
        if width <= 640.0 {
            // Narrow: the key help and the hint have given way to the stimulus state.
            assert!(!has("commands"), "{width} px: {labels:?}");
            assert!(has("dBFS") || has("no level"), "{width} px: {labels:?}");
        }
        for (i, (a, ra)) in labels.iter().enumerate() {
            assert!(
                ra.min.x >= 0.0 && ra.max.x <= width,
                "{width} px: {a:?} {ra:?} outside the window"
            );
            for (b, rb) in &labels[i + 1..] {
                let o = ra.intersect(*rb);
                assert!(
                    o.width() <= 0.5 || o.height() <= 0.5,
                    "{width} px: {a:?} {ra:?} overlaps {b:?} {rb:?}"
                );
            }
        }
    }
}

/// The health meter after the session line: the daemon's host CPU, its callback load and
/// xruns, the data rate and the window's frame rate, from figures the test sets (the fake
/// daemon's keepalives and this window's own, fed at chosen instants).
#[test]
fn top_bar_health_meter() {
    use std::sync::atomic::Ordering;

    use ac2_proto::frame::{AudioHealth, DaemonHealth};
    use egui::accesskit::Role;
    use egui_kittest::kittest::{By, NodeT};
    if !have_gpu("top_bar_health_meter") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    rig.fake.lock().health = DaemonHealth {
        host: "pupu".into(),
        cpu: Some(0.23),
        audio: Some(AudioHealth {
            load: Some(0.14),
            peak: Some(0.31),
            session_load: Some(0.12),
            session_peak: Some(0.48),
            xruns: 0,
        }),
    };
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
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    rig.fake.lock().health = DaemonHealth {
        host: HOST.into(),
        cpu: Some(0.23),
        audio: Some(AudioHealth {
            load: Some(0.14),
            peak: Some(0.31),
            session_load: Some(0.12),
            session_peak: Some(0.48),
            xruns: 0,
        }),
    };
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

/// The key hints on a crowded grid: the sweep pane shown beside the others (four narrow
/// panes under the transfer pane at 1280 px), the SPL meter focused with its hint line cut
/// to what fits, its footer naming the mic and its curve without running into the readouts,
/// and the transfer title naming the selected stored trace, shortened. Then the SPL title's
/// tooltip: the pane's hints in full.
#[test]
fn key_hints() {
    use ac2_proto::model::{CurveChoice, InputKind, InputSetup, InputSetups, Mic, MicCurveRef};
    use ac2_proto::units::{Hz, WallNs};
    use ac2_proto::{Change, Patch};
    if !have_gpu("key_hints") {
        return;
    }
    let rig = common::Rig::start();
    {
        let curve = |label: &str| MicCurveRef {
            label: label.into(),
            file_name: format!("34804_{label}.txt"),
            content_hash: "0123456789abcdef".into(),
            points: 100,
            f_lo: Hz(50.0),
            f_hi: Hz(20_000.0),
            imported_at: WallNs(0),
            stated_sensitivity: None,
        };
        let mut f = rig.fake.lock();
        f.commit(Change::Mic(Patch::Set(Mic {
            name: "MM1 34804".into(),
            curves: vec![curve("0°"), curve("90°")],
        })));
        f.commit(Change::Inputs(InputSetups {
            inputs: vec![InputSetup {
                channel: 1,
                kind: InputKind::Mic {
                    mic: Some("MM1 34804".into()),
                    curve: CurveChoice::Curve {
                        label: "90°".into(),
                    },
                },
                pair: None,
            }],
            pairs: Vec::new(),
        }));
    }
    let mut opts = options(Some(&rig));
    opts.prefs.layout.panes = Some(common::grid_with_sweep_prefs());
    let mut h = harness(opts);
    step_until(&mut h, "live frames with the mic", |a| {
        live(a) && a.state.daemon().is_some_and(|s| !s.mics.is_empty())
    });
    // A capture of the transfer measurement.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    step_until(&mut h, "slot 1", |a| a.state.slots()[0].is_some());
    // The sweep pane focused (Alt+5), then the SPL meter.
    h.key_press_modifiers(Modifiers::ALT, Key::Num5);
    h.key_press_modifiers(Modifiers::ALT, Key::Num4);
    step_until(&mut h, "five panes, SPL focused", |a| {
        crate::common::visible(&a.state).len() == 5 && a.state.layout.focus_kind() == PaneKind::Spl
    });
    // The capture selected (the focus stays): the transfer title names it.
    h.key_press(Key::V);
    step_until(&mut h, "slot 1 selected", |a| {
        a.state.selected_trace_meta().is_some() && a.state.layout.focus_kind() == PaneKind::Spl
    });
    let names = |a: &App| {
        a.state
            .pane_caption_variants(crate::common::pane(&a.state, PaneKind::Transfer))
    };
    assert!(
        names(h.state())
            .last()
            .is_some_and(|n| n.starts_with("slot 1 (")),
        "{:?}",
        names(h.state())
    );
    let style = ac2_ui::keys::LabelStyle::Pc;
    assert!(
        h.state()
            .state
            .key_hint_line(
                &h.state().keymap,
                common::pane(&h.state().state, PaneKind::Spl),
                style
            )
            .is_some()
    );
    h.event(Event::PointerGone);
    h.state_mut().state.toasts.list.clear();
    h.step();
    snapshot(&mut h, "key_hints");
    // The SPL hint line's tooltip (the title has none: it would cover the plot's top).
    h.get_by_label("SPL keys").hover();
    for _ in 0..6 {
        h.step();
    }
    snapshot(&mut h, "key_hints_tooltip");
}

/// The SPL meter maximised (W): the held level large and centred with `LAF · dBFS` under
/// it, the level bar, the statistics and the footer; W again, full screen: the stage view,
/// the meter alone on the screen, without the calibration footer.
#[test]
fn spl_meter_big_and_stage() {
    if !have_gpu("spl_meter_big_and_stage") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::ALT, Key::Num4);
    // G: from meter + Leq (the default) to the meter alone.
    h.key_press(Key::G);
    h.key_press(Key::W);
    step_until(&mut h, "the meter maximised", |a| {
        a.state.kind_modes(ac2_ui::state::PaneKind::Spl).spl == ac2_scene::view::SplMode::Meter
            && a.state.layout.maximized
            && a.state.layout.focus_kind() == PaneKind::Spl
            && a.state.rig.spl_hold.contains_key(&MeasId(4))
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

/// 25 s of a spectrum with a tone sweeping from 100 Hz to 12.8 kHz over a sloped floor and a
/// steady 1 kHz tone, with 1.5 s without frames (STALE) in the middle, at a fixed time: a
/// picture that does not depend on when the frames arrived.
fn sweep_history(def: &ac2_proto::GridDef) -> ac2_scene::spectrograph::SpectrographHistory {
    use ac2_scene::spectrograph::{SpectrographFrame, SpectrographHistory};
    let edges = def.edges();
    let freqs = def.centres();
    let mut h = SpectrographHistory::new(30);
    let t0 = common::SPL_SINCE.0;
    for i in 0..750u64 {
        let t = i as f64 / 30.0;
        if (12.0..13.5).contains(&t) {
            h.mark_break();
            continue;
        }
        let sweep = 100.0 * 2f64.powf(7.0 * t / 25.0);
        let level: Vec<f32> = freqs
            .iter()
            .map(|&f| {
                let f = f.max(5.0);
                let floor = -70.0 - 3.0 * (f / 1000.0).log2();
                let tone = if (f - 1000.0).abs() < 6.0 {
                    -32.0
                } else {
                    -200.0
                };
                let swept = if (f / sweep).log2().abs() < 1.0 / 24.0 {
                    -20.0
                } else {
                    -200.0
                };
                floor.max(tone).max(swept) as f32
            })
            .collect();
        h.push(&SpectrographFrame {
            seq: i + 1,
            at: ac2_proto::units::WallNs(t0 + i * 1_000_000_000 / 30),
            grid: def.id(),
            edges: &edges,
            scale: ac2_proto::model::LevelScale::Dbfs,
            spec_level: None,
            level: &level,
            validity: None,
        });
    }
    h
}

/// The spectrum pane maximised with the spectrograph under it (G): the stopped spectrum on
/// top, a sweep and a steady tone over 25 s below on the same frequency axis with a gap
/// where frames stopped, the colour bar on the pane's level range, and the cursor's
/// frequency, time and level.
#[test]
fn spectrograph() {
    if !have_gpu("spectrograph") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::ALT, Key::Num2);
    h.key_press(Key::W);
    h.key_press(Key::G);
    // Stopped: the rig's frames keep coming but are not folded in, so the picture is the
    // pinned history alone.
    h.key_press(Key::S);
    step_until(
        &mut h,
        "the spectrum stopped, maximised, with its spectrograph",
        |a| {
            let st = &a.state;
            st.layout.maximized
                && st.layout.focus_kind() == PaneKind::Spectrum
                && st.kind_modes(ac2_ui::state::PaneKind::Spectrum).spectrum
                    == ac2_scene::view::SpectrumMode::Split
                && st.meas(MeasId(3)).is_some_and(|m| !m.running)
        },
    );
    let def = {
        let st = &h.state().state;
        let d = st.rig.data.as_ref().expect("data");
        let f = d
            .latest
            .get(&Topic::Data {
                meas: MeasId(3),
                stream: Stream::Spec,
            })
            .expect("spectrum frame");
        d.grids
            .get(&f.frame.stamp.grid_id.expect("grid"))
            .expect("grid definition")
            .clone()
    };
    let pinned = sweep_history(&def);
    let same = |a: &App| {
        a.state.rig.spectrographs.get(&MeasId(3)).is_some_and(|x| {
            x.ring()
                .iter()
                .zip(pinned.ring())
                .all(|(p, q)| match (p, q) {
                    (Some(p), Some(q)) => Arc::ptr_eq(p, q),
                    (None, None) => true,
                    _ => false,
                })
        })
    };
    let pin = |a: &mut App| {
        let st = &mut a.state;
        st.rig.spectrographs.insert(MeasId(3), pinned.clone());
        st.view.cursor_hz = Some(1000.0);
        st.view.spectrum.spectrograph.cursor_s = Some(8.0);
        st.toasts.list.clear();
    };
    {
        pin(h.state_mut());
        let st = &h.state().state;
        let now = ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: ac2_proto::units::WallNs(0),
        };
        let theme = ac2_scene::theme::Theme::dark();
        let size = ac2_scene::primitives::Viewport {
            width: 1000.0,
            height: 600.0,
        };
        let s = ac2_ui::scenes::spectrograph(
            st,
            crate::common::pane(st, ac2_ui::state::PaneKind::Spectrum),
            &theme,
            size,
            now,
        );
        assert_eq!(s.caption, "Mic 1 FFT · last 30 s · dBFS (tone) · stopped");
        assert_eq!(
            s.cursor.expect("cursor").text,
            "1.00 kHz · 8.0 s ago · \u{2212}32.0 dBFS (tone)"
        );
    }
    snapshot_when(&mut h, "spectrograph", pin, same);

    // G again: the spectrograph alone, maximised, its history kept; the caption carries the
    // spectrum's window.
    h.key_press(Key::G);
    step_until(&mut h, "the spectrograph alone", |a| {
        a.state
            .kind_modes(ac2_ui::state::PaneKind::Spectrum)
            .spectrum
            == ac2_scene::view::SpectrumMode::Spectrograph
    });
    {
        pin(h.state_mut());
        let st = &h.state().state;
        let now = ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: ac2_proto::units::WallNs(0),
        };
        let theme = ac2_scene::theme::Theme::dark();
        let size = ac2_scene::primitives::Viewport {
            width: 1000.0,
            height: 600.0,
        };
        let s = ac2_ui::scenes::spectrograph(
            st,
            crate::common::pane(st, ac2_ui::state::PaneKind::Spectrum),
            &theme,
            size,
            now,
        );
        assert!(s.spectrum.is_none());
        assert_eq!(
            s.caption,
            "Mic 1 FFT · last 30 s · dBFS (tone) · stopped · Hann window"
        );
    }
    snapshot_when(&mut h, "spectrograph_alone", pin, same);
}
