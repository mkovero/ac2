//! Headless UI snapshots against the fake daemon, rendered through egui-wgpu with the plots
//! in paint callbacks, exactly as on screen.
//!
//! Needs a wgpu adapter: locally `AC2_GPU_FALLBACK=1 WGPU_BACKEND=vulkan` (lavapipe). Without
//! one the tests print SKIP and pass, unless `AC2_REQUIRE_GPU=1`. References in
//! `tests/snapshots/` are blessed on lavapipe: `UPDATE_SNAPSHOTS=1 cargo test -p ac2-ui`.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ac2_client::ClientConfig;
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::MeasId;
use ac2_scene::theme::ThemeName;
use ac2_ui::conn::Target;
use ac2_ui::keys::Keymap;
use ac2_ui::state::{ConnState, Overlay, PaneKind};
use ac2_ui::{App, AppOptions};
use eframe::egui::{self, Event, Key, Modifiers};
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, SnapshotOptions};

const SIZE: egui::Vec2 = egui::vec2(1280.0, 800.0);

/// `true` when a wgpu adapter exists; otherwise prints why and skips (or fails under
/// `AC2_REQUIRE_GPU=1`).
fn have_gpu(test: &str) -> bool {
    match ac2_plot::Gpu::new() {
        Ok(g) => {
            eprintln!("{test}: adapter {}", g.describe());
            true
        }
        Err(e) if std::env::var("AC2_REQUIRE_GPU").is_ok_and(|v| v == "1") => {
            panic!("{test}: {e} (AC2_REQUIRE_GPU=1)")
        }
        Err(e) => {
            eprintln!("SKIP {test}: {e}");
            false
        }
    }
}

fn options(rig: Option<&common::Rig>) -> AppOptions {
    options_at(rig.map(|r| r.fake.endpoints()))
}

fn options_at(endpoints: Option<ac2_client::Endpoints>) -> AppOptions {
    AppOptions {
        target: endpoints.map(|ep| Target {
            config: ClientConfig::new(ep, "ac2-ui test"),
            describe: "fake daemon".into(),
        }),
        theme: ThemeName::Dark,
        keymap: Keymap::default(),
        keymap_path: Some("~/.config/ac2/keys.toml".into()),
        prefs: ac2_ui::prefs::UiPrefs::default(),
        prefs_path: None,
        notices: vec![],
        started: Instant::now(),
        bench_startup: false,
        open_session_dialog: false,
    }
}

fn harness(opts: AppOptions) -> Harness<'static, App> {
    // Screenshots are compared on every OS; macOS would otherwise label keys with glyphs.
    ac2_ui::keys::set_label_style(ac2_ui::keys::LabelStyle::Pc);
    Harness::builder()
        .with_size(SIZE)
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(move |cc| App::new(cc, opts))
}

/// Steps the UI (in real time, the link runs on its own thread) until `cond` holds.
fn step_until(h: &mut Harness<'_, App>, what: &str, cond: impl Fn(&App) -> bool) {
    let t0 = Instant::now();
    while !cond(h.state()) {
        assert!(
            t0.elapsed() < Duration::from_secs(15),
            "timed out waiting for {what}"
        );
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    // A few more passes so layout settles and the plots are prepared.
    for _ in 0..3 {
        h.step();
    }
}

fn live(app: &App) -> bool {
    let st = &app.state;
    let synced = st.mirror.as_ref().is_some_and(|m| m.synced());
    let have = |m: u32, s: Stream| {
        st.data.as_ref().is_some_and(|d| {
            d.latest
                .get(&Topic::Data {
                    meas: MeasId(m),
                    stream: s,
                })
                .is_some_and(|f| {
                    f.frame
                        .stamp
                        .grid_id
                        .is_none_or(|g| d.grids.contains_key(&g))
                })
        })
    };
    matches!(st.conn, ConnState::Connected { .. })
        && synced
        && st.measurements().len() == 4
        && have(1, Stream::Tf)
        && have(2, Stream::Tf)
        && have(1, Stream::Ir)
        && have(3, Stream::Spec)
        && have(4, Stream::Spl)
}

fn snapshot_options() -> SnapshotOptions {
    // lavapipe is bit-exact run to run; the thresholds only absorb other rasterizers'
    // edge pixels (Metal differs in 1–2 pixels of 1280×800).
    SnapshotOptions::new()
        .threshold(1.0)
        .max_failed_pixels(egui_kittest::OsThreshold::new(0).macos(16))
}

#[test]
fn transfer_view_two_traces_and_banner() {
    if !have_gpu("transfer_view_two_traces_and_banner") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    {
        let st = &h.state().state;
        assert_eq!(st.selected, Some(MeasId(1)));
        let s = ac2_ui::scenes::transfer(
            st,
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport {
                width: 1000.0,
                height: 450.0,
            },
            ac2_ui::scenes::Now {
                instant: Instant::now(),
                wall: ac2_proto::units::WallNs(0),
            },
        );
        let legend: Vec<&str> = s.legend.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(legend, ["Main L", "Delay tower"]);
        let banners: Vec<&str> = s.banners.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(banners, ["NO DELAY ESTIMATE"]);
    }
    h.snapshot_options("transfer_two_traces_banner", &snapshot_options());
}

#[test]
fn help_overlay() {
    if !have_gpu("help_overlay") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press(Key::Slash);
    step_until(&mut h, "help", |a| a.state.overlay == Overlay::Help);
    h.snapshot_options("help_overlay", &snapshot_options());
    // `/` closes it again.
    h.key_press(Key::Slash);
    step_until(&mut h, "help closed", |a| a.state.overlay == Overlay::None);
}

#[test]
fn command_palette() {
    if !have_gpu("command_palette") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    step_until(&mut h, "palette", |a| {
        matches!(a.state.overlay, Overlay::Palette(_))
    });
    h.event(Event::Text("delay".into()));
    step_until(
        &mut h,
        "query",
        |a| matches!(&a.state.overlay, Overlay::Palette(p) if p.query == "delay"),
    );
    h.snapshot_options("command_palette", &snapshot_options());
    // Enter runs the highlighted command against the fake daemon.
    h.key_press(Key::Enter);
    step_until(&mut h, "palette closed", |a| {
        a.state.overlay == Overlay::None
    });
}

/// X on an ambiguous finding: the candidate list over the transfer pane (decision 1c), the
/// banner saying tracking waits for the pick, and a key inserting a candidate.
#[test]
fn ambiguous_delay_candidate_list() {
    if !have_gpu("ambiguous_delay_candidate_list") {
        return;
    }
    let rig = common::Rig::start();
    rig.fake.lock().finding = ac2_client::fake::FakeFinding::Ambiguous;
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::ALT, Key::Num1);
    h.key_press(Key::X);
    step_until(&mut h, "candidate list", |a| {
        matches!(a.state.overlay, Overlay::DelayPick(_))
            && a.state
                .meas(MeasId(1))
                .is_some_and(|m| m.delay.as_ref().is_some_and(|d| d.awaiting_pick))
    });
    assert_eq!(
        rig.fake.lock().last_find,
        Some((ac2_proto::model::FinderBand::Auto, None))
    );
    // Toasts expire on the wall clock; the snapshot shows the list and the plots.
    h.state_mut().state.toasts.clear();
    h.step();
    h.snapshot_options("delay_pick_candidates", &snapshot_options());
    // 2 inserts the second candidate (12.7 ms) and resolves the finding.
    h.key_press(Key::Num2);
    step_until(&mut h, "inserted", |a| {
        a.state.overlay == Overlay::None
            && a.state.meas(MeasId(1)).is_some_and(|m| {
                m.delay
                    .as_ref()
                    .is_some_and(|d| !d.awaiting_pick && (d.applied.0 - 0.0127).abs() < 1e-9)
            })
    });
}

#[test]
fn themes() {
    if !have_gpu("themes") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press(Key::T);
    step_until(&mut h, "light", |a| a.state.theme == ThemeName::Light);
    h.snapshot_options("theme_light", &snapshot_options());
    h.key_press(Key::T);
    step_until(&mut h, "high contrast", |a| {
        a.state.theme == ThemeName::HighContrast
    });
    h.snapshot_options("theme_high_contrast", &snapshot_options());
}

#[test]
fn stimulus_flow_against_the_fake_daemon() {
    if !have_gpu("stimulus_flow_against_the_fake_daemon") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    // Space without a level: the prompt opens; nothing reaches the daemon.
    h.key_press(Key::Space);
    step_until(&mut h, "level prompt", |a| {
        matches!(a.state.overlay, Overlay::Prompt(_))
    });
    assert_eq!(rig.fake.executions("gen.acquire"), 0);
    h.event(Event::Text("-24".into()));
    h.key_press(Key::Enter);
    step_until(&mut h, "level", |a| a.state.stimulus.level.is_some());
    h.key_press(Key::Space);
    step_until(&mut h, "armed", |a| {
        a.state.daemon().is_some_and(|s| s.generator.armed)
    });
    assert!(!h.state().state.daemon().is_some_and(|s| s.generator.firing));
    h.key_press(Key::Enter);
    step_until(&mut h, "firing", |a| {
        a.state.daemon().is_some_and(|s| s.generator.firing)
    });
    h.key_press(Key::ArrowDown);
    step_until(&mut h, "level −25", |a| {
        a.state.daemon().is_some_and(|s| {
            s.generator
                .settings
                .as_ref()
                .is_some_and(|g| g.level.0 == -25.0)
        })
    });
    h.key_press(Key::Escape);
    step_until(&mut h, "stopped", |a| {
        a.state.daemon().is_some_and(|s| {
            !s.generator.firing && !s.generator.armed && s.generator.owner.is_none()
        })
    });
    assert!(rig.fake.executions("gen.stop") >= 1);
}

#[test]
fn startup_first_frame() {
    if !have_gpu("startup_first_frame") {
        return;
    }
    // Device creation is part of startup, as in the real app; the daemon link is not
    // (the first frame never waits for it).
    let t0 = Instant::now();
    let mut h = harness(options(None));
    h.step();
    let img = h.render().expect("render");
    let first = t0.elapsed();
    assert_eq!(img.width(), SIZE.x as u32);
    println!(
        "startup: first frame rendered headless in {:.1} ms (target < 300 ms)",
        first.as_secs_f64() * 1e3
    );
    // Software rasterizers in CI are slower than the target hardware; this bound catches a
    // blocking call on the startup path, `--bench-startup` measures the real window.
    assert!(first < Duration::from_secs(2), "{first:?}");
}

/// Two captures in slots 1 and 2 and an imported target curve, drawn with the live traces:
/// captures share the epoch's time base (Δt to the reference), the target is independent.
#[test]
fn stored_traces_and_target() {
    if !have_gpu("stored_traces_and_target") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    // Ctrl+1 captures Main L (selected) into slot 1; N, Ctrl+2 Delay tower into slot 2.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    step_until(&mut h, "slot 1", |a| a.state.slots()[0].is_some());
    h.key_press(Key::N);
    step_until(&mut h, "delay tower", |a| {
        a.state.selected == Some(MeasId(2))
    });
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    step_until(&mut h, "slot 2", |a| a.state.slots()[1].is_some());
    // Z: a target curve from a file.
    h.key_press(Key::Z);
    step_until(&mut h, "target prompt", |a| {
        matches!(a.state.overlay, Overlay::Prompt(_))
    });
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ac2-traces/tests/fixtures/house_curve.txt");
    h.event(Event::Text(path.to_string_lossy().into_owned()));
    h.key_press(Key::Enter);
    step_until(&mut h, "three stored traces with data", |a| {
        a.state.traces.len() == 3
    });
    // 2 hides slot 2, 2 again shows it.
    h.key_press(Key::Num2);
    step_until(&mut h, "slot 2 hidden", |a| {
        a.state.slots()[1].is_some_and(|t| !t.edit.visible)
    });
    h.key_press(Key::Num2);
    step_until(&mut h, "slot 2 shown", |a| {
        a.state.slots()[1].is_some_and(|t| t.edit.visible)
            && a.state.traces.values().all(|(d, _)| d.meta.edit.visible)
    });
    {
        let st = &h.state().state;
        let s = ac2_ui::scenes::transfer(
            st,
            &ac2_scene::theme::Theme::dark(),
            ac2_scene::primitives::Viewport {
                width: 1000.0,
                height: 450.0,
            },
            ac2_ui::scenes::Now {
                instant: Instant::now(),
                wall: ac2_proto::units::WallNs(0),
            },
        );
        let legend: Vec<&str> = s.legend.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(legend.len(), 5, "{legend:?}");
        assert!(legend.contains(&"house_curve · indep."), "{legend:?}");
        assert!(
            legend.iter().any(|l| l.starts_with("Main L S1 · Δt")),
            "{legend:?}"
        );
    }
    // Toasts expire on the wall clock; the snapshot shows the plots only.
    h.state_mut().state.toasts.clear();
    h.step();
    h.snapshot_options("transfer_stored_traces", &snapshot_options());
}

/// The transfer pane's title chip names the measurement it shows; a click opens the list of
/// transfer measurements and a pick switches the pane (and the selection) to it.
#[test]
fn pane_measurement_chip_and_list() {
    if !have_gpu("pane_measurement_chip_and_list") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    // The transfer pane's chip (the IR pane's names the same measurement, drawn after it).
    h.get_all_by_label("Main L")
        .next()
        .expect("transfer pane chip")
        .click();
    step_until(
        &mut h,
        "list",
        |a| matches!(a.state.overlay, Overlay::PaneMenu(m) if m.pane == PaneKind::Transfer),
    );
    h.state_mut().state.toasts.clear();
    h.step();
    h.snapshot_options("pane_measurement_list", &snapshot_options());
    h.get_by_label("TF  Delay tower").click();
    step_until(&mut h, "delay tower shown", |a| {
        a.state.overlay == Overlay::None
            && a.state.selected == Some(MeasId(2))
            && a.state.pane_meas(PaneKind::Transfer).map(|m| m.id) == Some(MeasId(2))
    });
    // The pane now leads with it: first legend row, IR of it.
    let st = &h.state().state;
    let s = ac2_ui::scenes::transfer(
        st,
        &ac2_scene::theme::Theme::dark(),
        ac2_scene::primitives::Viewport {
            width: 1000.0,
            height: 450.0,
        },
        ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: ac2_proto::units::WallNs(0),
        },
    );
    assert_eq!(s.legend[0].name, "Delay tower");
    assert_eq!(ac2_ui::scenes::focus_tf(st).map(|m| m.id), Some(MeasId(2)));
}

/// A slot selected in the list takes the smoothing keys: K re-smooths the stored capture
/// (the daemon serves it at the new setting) while the live curves keep theirs.
#[test]
fn slot_resmoothed() {
    if !have_gpu("slot_resmoothed") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    step_until(&mut h, "slot 1 with data", |a| {
        a.state.slots()[0].is_some() && a.state.traces.len() == 1
    });
    let id = h.state().state.slots()[0].map(|t| t.id).expect("slot 1");
    h.get_by_label("1  Main L S1 · 1/6 oct").click();
    step_until(&mut h, "slot selected", |a| {
        a.state.selected_trace == Some(id)
    });
    let before = h.state().state.traces[&id].0.mag_db.clone();
    h.key_press(Key::K);
    step_until(&mut h, "re-smoothed data", |a| {
        a.state.traces.get(&id).is_some_and(|(d, _)| {
            d.meta
                .edit
                .smoothing
                .is_some_and(|s| s.fraction == ac2_proto::model::SmoothingFraction::Third)
        })
    });
    let st = &h.state().state;
    assert_ne!(st.traces[&id].0.mag_db, before, "served at the new setting");
    assert_eq!(
        st.smoothing_caption(PaneKind::Transfer).as_deref(),
        Some("slot 1 (Main L S1): smoothing 1/3 oct")
    );
    // The live measurement was not touched.
    assert!(st.meas(MeasId(1)).is_some_and(|m| matches!(
        &m.config.kind,
        ac2_proto::model::MeasKind::Transfer { config }
            if config.smoothing.is_some_and(|s| s.fraction == ac2_proto::model::SmoothingFraction::Sixth)
    )));
    h.state_mut().state.toasts.clear();
    h.step();
    h.snapshot_options("slot_resmoothed", &snapshot_options());
}

/// Smoothing on phase and spectrum: the rig publishes scattered curves smoothed as the
/// daemon does. The transfer scene draws the smoothed phase (wrapped, unwrapped and group
/// delay all follow from it), and the spectrum axis says the level is smoothed.
#[test]
fn smoothed_phase_and_spectrum() {
    if !have_gpu("smoothed_phase_and_spectrum") {
        return;
    }
    let rig = common::Rig::start_smoothed();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    {
        let st = &h.state().state;
        let now = ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: ac2_proto::units::WallNs(0),
        };
        let size = ac2_scene::primitives::Viewport {
            width: 1000.0,
            height: 450.0,
        };
        let theme = ac2_scene::theme::Theme::dark();
        let s = ac2_ui::scenes::transfer(st, &theme, size, now);
        let raw = common::raw_tf_frame(1, 0.0, 0.0, 2000.0);
        let want = common::smoothed_tf_frame(&raw);
        let t = &s.traces[0];
        let diff = |a: f64, b: f64| {
            let d = (a - b).rem_euclid(360.0);
            d.min(360.0 - d)
        };
        let mut moved: f64 = 0.0;
        for (i, p) in t.phase_wrapped_deg.iter().enumerate() {
            assert!(
                diff(*p, f64::from(want.phase[i])) < 1e-3,
                "column {i}: drawn {p} vs smoothed {}",
                want.phase[i]
            );
            moved = moved.max(diff(*p, f64::from(raw.phase[i])));
        }
        assert!(moved > 5.0, "the drawn phase is the raw one");
        // Unwrapped phase steps (and so group delay) follow the smoothed phase.
        for i in 1..want.phase.len() {
            let step = t.phase_unwrapped_deg[i] - t.phase_unwrapped_deg[i - 1];
            let want_step = f64::from(want.phase[i] - want.phase[i - 1]);
            assert!(diff(step, want_step) < 1e-3, "column {i}");
        }
        assert_eq!(s.legend[0].text, "Main L · ref · 1/6 oct");

        let sp = ac2_ui::scenes::spectrum(st, &theme, size, now);
        assert_eq!(sp.unit, "dBFS (tone, 1/6 oct smoothed)");
        assert_eq!(sp.caption, "Hann window");
    }
    h.snapshot_options("smoothed_phase_and_spectrum", &snapshot_options());
}

/// A daemon with no audio session (a fresh local daemon, an embedded one on real audio):
/// the transfer pane says how to open one.
#[test]
fn empty_session_hint() {
    if !have_gpu("empty_session_hint") {
        return;
    }
    let fake = ac2_client::fake::FakeDaemon::start(ac2_client::fake::FakeOptions::default())
        .expect("fake daemon");
    let mut h = harness(options_at(Some(fake.endpoints())));
    step_until(&mut h, "synced, no session", |a| {
        a.state.mirror.as_ref().is_some_and(|m| m.synced())
            && a.state.empty_hint(&a.keymap).is_some()
    });
    assert_eq!(
        h.state().state.empty_hint(&h.state().keymap).as_deref(),
        Some("No audio session — press Shift+O (or Ctrl+K → Open audio session)")
    );
    h.state_mut().state.toasts.clear();
    h.step();
    h.snapshot_options("empty_session_hint", &snapshot_options());
}

/// Publishes steady input meters on the fake daemon: the preview of `fake:loop` (in 1 the
/// loopback at −12 dBFS, in 2 the room at −31, in 3–4 silent) and, once a session is open,
/// its session meters.
struct Meters {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Meters {
    fn start(fake: Arc<ac2_client::fake::FakeDaemon>) -> Self {
        use ac2_proto::frame::{
            ClipFlags, LevelsMeta, PreviewLevelsFrame, PreviewLevelsMeta, SessionLevelsFrame,
        };
        use ac2_proto::model::{BackendKind, DeviceId};
        use ac2_proto::{Frame, FrameData};
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let peak = [-1.5f32, -19.0, f32::NEG_INFINITY, f32::NEG_INFINITY];
        let rms = [-12.0f32, -31.0, f32::NEG_INFINITY, f32::NEG_INFINITY];
        let thread = std::thread::spawn(move || {
            let mut seq = 1;
            while !s.load(Ordering::Acquire) {
                {
                    let mut st = fake.lock();
                    let preview = FrameData::PreviewLevels(PreviewLevelsFrame {
                        meta: PreviewLevelsMeta {
                            backend: BackendKind::Fake,
                            device: DeviceId("fake:loop".into()),
                            channels: vec![0, 1, 2, 3],
                        },
                        peak: peak.to_vec(),
                        rms: rms.to_vec(),
                        clip: vec![ClipFlags::NONE; 4],
                    });
                    let frame = Frame {
                        stamp: st.stamp(seq, None),
                        data: preview,
                    };
                    st.publish(&frame);
                    if let Some(o) = st.state.session.open.clone() {
                        let ch = o.config.input_channels;
                        let pick = |v: &[f32]| ch.iter().map(|c| v[usize::from(*c)]).collect();
                        let frame = Frame {
                            stamp: st.stamp(seq, None),
                            data: FrameData::SessionLevels(SessionLevelsFrame {
                                meta: LevelsMeta {
                                    channels: ch.clone(),
                                },
                                peak: pick(&peak),
                                rms: pick(&rms),
                                clip: vec![ClipFlags::NONE; ch.len()],
                            }),
                        };
                        st.publish(&frame);
                    }
                }
                seq += 1;
                std::thread::sleep(Duration::from_millis(40));
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Meters {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn session_dialog_of(a: &App) -> Option<&ac2_ui::session_dialog::SessionDialog> {
    match &a.state.overlay {
        Overlay::Session(d) => Some(d),
        _ => None,
    }
}

/// Shift+O: the session dialog lists the backends and the rig's channels by name with their
/// live meters and roles; the mic is named inline, the loopback detected after a typed
/// level, Enter opens and one more key creates the transfer measurement.
#[test]
fn session_dialog() {
    if !have_gpu("session_dialog") {
        return;
    }
    let fake = Arc::new(
        ac2_client::fake::FakeDaemon::start(ac2_client::fake::FakeOptions::default())
            .expect("fake daemon"),
    );
    let _meters = Meters::start(Arc::clone(&fake));
    let mut h = harness(options_at(Some(fake.endpoints())));
    step_until(&mut h, "synced", |a| {
        a.state.mirror.as_ref().is_some_and(|m| m.synced())
    });
    h.key_press_modifiers(Modifiers::SHIFT, Key::O);
    step_until(&mut h, "dialog with devices and meters", |a| {
        session_dialog_of(a).is_some_and(|d| d.device_info().is_some())
            && a.state.input_meters().len() == 4
    });
    assert!(fake.lock().preview.is_some(), "the device is previewed");
    // ↓↓↓ to input 2, N names its mic.
    for _ in 0..3 {
        h.key_press(Key::ArrowDown);
    }
    h.key_press(Key::N);
    h.event(Event::Text("n".into()));
    h.event(Event::Text("M30 FOH".into()));
    h.key_press(Key::Enter);
    step_until(&mut h, "mic named", |a| {
        session_dialog_of(a).is_some_and(|d| d.inputs[1].mic == "M30 FOH" && d.edit.is_none())
    });
    h.state_mut().state.toasts.clear();
    h.step();
    h.snapshot_options("session_dialog", &snapshot_options());

    // D, a typed level, Enter: the fake answers in 1, which becomes the Reference.
    h.key_press(Key::D);
    h.event(Event::Text("d".into()));
    h.event(Event::Text("-30".into()));
    step_until(&mut h, "level typed", |a| {
        session_dialog_of(a)
            .and_then(|d| d.detect.as_ref())
            .is_some_and(|p| p.level == "-30")
    });
    h.state_mut().state.toasts.clear();
    h.step();
    h.snapshot_options("session_dialog_detect_confirm", &snapshot_options());
    h.key_press(Key::Enter);
    step_until(&mut h, "detected", |a| {
        session_dialog_of(a).is_some_and(|d| {
            matches!(
                d.detect.as_ref().map(|p| &p.phase),
                Some(ac2_ui::session_dialog::DetectPhase::Done(_))
            )
        })
    });
    assert_eq!(fake.executions("session.detect_loopback"), 1);
    step_until(&mut h, "lease released", |a| {
        a.state
            .daemon()
            .is_some_and(|s| s.generator.owner.is_none())
    });
    h.state_mut().state.toasts.clear();
    h.step();
    h.snapshot_options("session_dialog_detected", &snapshot_options());

    h.key_press(Key::Enter);
    step_until(&mut h, "session open, offer shown", |a| {
        a.state.open_session().is_some() && matches!(a.state.overlay, Overlay::Offer(_))
    });
    assert_eq!(fake.executions("session.open"), 1);
    assert_eq!(fake.executions("session.inputs"), 1);
    assert!(
        fake.lock().preview.is_none(),
        "the session replaced the preview"
    );
    h.key_press(Key::Enter);
    step_until(&mut h, "measurement created", |a| {
        a.state
            .measurements()
            .iter()
            .any(|m| m.config.name == "Reference → M30 FOH")
    });

    // The new-measurement dialog picks inputs by name, with the session's meters.
    h.state_mut().dispatch(ac2_ui::state::Msg::Command(
        ac2_ui::keys::CommandId::NewTransfer,
    ));
    step_until(&mut h, "transfer dialog with meters", |a| {
        matches!(a.state.overlay, Overlay::Form(_)) && a.state.input_meters().len() == 2
    });
    h.state_mut().state.toasts.clear();
    h.step();
    h.snapshot_options("transfer_dialog", &snapshot_options());
    h.key_press(Key::Escape);
    step_until(&mut h, "dialog closed", |a| {
        a.state.overlay == Overlay::None
    });

    // Shift+S: the sweep dialog, inputs and outputs by name; a typed level arms it, Enter
    // plays it and the result opens the distortion pane.
    h.key_press_modifiers(Modifiers::SHIFT, Key::S);
    h.event(Event::Text("S".into()));
    step_until(
        &mut h,
        "sweep dialog",
        |a| matches!(&a.state.overlay, Overlay::Form(f) if f.kind == ac2_ui::forms::FormKind::Sweep),
    );
    for _ in 0..3 {
        h.key_press(Key::ArrowDown);
    }
    h.event(Event::Text("-30".into()));
    step_until(&mut h, "level typed", |a| {
        matches!(&a.state.overlay, Overlay::Form(f)
            if f.text(ac2_ui::forms::FieldId::Level) == "-30")
    });
    h.state_mut().state.toasts.clear();
    h.step();
    h.snapshot_options("sweep_dialog", &snapshot_options());
    h.key_press(Key::Enter);
    step_until(&mut h, "armed with the sweep", |a| {
        a.state.daemon().is_some_and(|s| {
            s.generator.armed
                && s.generator
                    .settings
                    .as_ref()
                    .is_some_and(|g| matches!(g.signal, ac2_proto::model::Signal::Ess { .. }))
        })
    });
    h.key_press(Key::Enter);
    step_until(&mut h, "sweep stored and shown", |a| {
        a.state.layout.focus == PaneKind::Distortion && a.state.shown_sweep().is_some()
    });
    assert_eq!(fake.executions("ir.capture"), 1);
    // The focused pane alone, for the picture.
    h.key_press(Key::W);
    step_until(&mut h, "maximized", |a| a.state.layout.maximized);
    h.state_mut().state.toasts.clear();
    h.step();
    h.snapshot_options("sweep_distortion", &snapshot_options());
    h.key_press(Key::U);
    step_until(&mut h, "percent", |a| {
        a.state.view.distortion.unit == ac2_scene::view::DistortionUnit::Percent
    });
    h.key_press(Key::H);
    step_until(&mut h, "sweep IR", |a| a.state.view.distortion.show_ir);
    // G: the log view, where the harmonics' impulses read at their level.
    h.key_press(Key::G);
    step_until(&mut h, "log IR", |a| {
        a.state.view.ir.mode == ac2_scene::view::IrMode::Log
    });
    h.state_mut().state.toasts.clear();
    h.step();
    h.snapshot_options("sweep_ir", &snapshot_options());
    h.key_press(Key::Escape);
    step_until(&mut h, "stopped and released", |a| {
        a.state
            .daemon()
            .is_some_and(|s| s.generator.owner.is_none() && !s.generator.armed)
    });
}
