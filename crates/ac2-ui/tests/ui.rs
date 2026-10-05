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
    let mut h = Harness::builder()
        .with_size(SIZE)
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(move |cc| App::new(cc, opts));
    // Local times in UTC on every machine.
    h.state_mut().state.local_zone = ac2_ui::scenes::LocalZone::Fixed { offset_s: 0 };
    h
}

/// How long a wait for the UI may take. Only a ceiling: every wait ends as soon as its
/// condition holds, and on a loaded machine (software rasteriser, every core busy) the
/// link thread and the rig's publisher can each fall seconds behind.
const WAIT: Duration = Duration::from_secs(60);

/// Steps the UI (in real time, the link runs on its own thread) until `cond` holds.
fn step_until(h: &mut Harness<'_, App>, what: &str, cond: impl Fn(&App) -> bool) {
    let t0 = Instant::now();
    // The sidebar's input labels take the device's channel names once the list arrives:
    // with a session open, a screenshot waits for it.
    let named = |a: &App| a.state.open_session().is_none() || a.state.devices.is_some();
    while !(cond(h.state()) && named(h.state())) {
        if t0.elapsed() > WAIT {
            let a = h.state();
            let toasts: Vec<&str> = a.state.toasts.iter().map(|t| t.text.as_str()).collect();
            panic!(
                "timed out waiting for {what}; conn {:?}; overlay {:?}; stimulus {:?}; toasts {toasts:?}",
                a.state.conn, a.state.overlay, a.state.stimulus.phase
            );
        }
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    // A few more passes so layout settles and the plots are prepared.
    for _ in 0..3 {
        h.step();
    }
}

/// The link is healthy as of `now`: responding, and every frame shown fresh. These are
/// wall-clock indications (STALE, "not responding", the dimmed curves) that a picture
/// would otherwise catch in whichever state a stalled thread left them.
fn healthy(app: &App, now: Instant) -> bool {
    let st = &app.state;
    let link = match &st.conn {
        ConnState::Connected { .. } => st.mirror.as_ref().is_some_and(|m| m.responding(now)),
        _ => true,
    };
    let fresh = st.data.as_ref().is_none_or(|d| {
        d.latest
            .frames
            .values()
            .all(|f| !ac2_ui::scenes::freshness(st, f).is_stale())
    });
    link && fresh
}

/// Compares the UI with its reference once a pass has been laid out while the link was
/// healthy. Health is checked after the pass, against a later clock, so the pass itself
/// was healthy too (ages only grow).
fn snapshot(h: &mut Harness<'_, App>, name: &str) {
    snapshot_when(h, name, |_| {}, |_| true);
}

/// [`snapshot`] of state that moves with the wall clock: `pin` before every pass, and the
/// pass is taken only when `drawn` says it shows the pinned state (a pass advances the
/// reducer's clock by however long the machine took since the previous one).
fn snapshot_when(
    h: &mut Harness<'_, App>,
    name: &str,
    pin: impl Fn(&mut App),
    drawn: impl Fn(&App) -> bool,
) {
    let t0 = Instant::now();
    loop {
        pin(h.state_mut());
        // A pin edits the state directly: cached pane scenes must see it too.
        h.state_mut().state_edited();
        h.step();
        if healthy(h.state(), Instant::now()) && drawn(h.state()) {
            break;
        }
        assert!(t0.elapsed() < WAIT, "{name}: the link never settled");
        std::thread::sleep(Duration::from_millis(10));
    }
    h.snapshot_options(name, &snapshot_options());
}

fn live(app: &App) -> bool {
    live_with(app, 4)
}

/// [`live`] with `count` measurements listed.
fn live_with(app: &App, count: usize) -> bool {
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
        && st.measurements().len() == count
        && have(1, Stream::Tf)
        && have(2, Stream::Tf)
        && have(1, Stream::Ir)
        && have(3, Stream::Spec)
        && have(4, Stream::Spl)
}

fn snapshot_options() -> SnapshotOptions {
    // lavapipe is bit-exact run to run; the thresholds only absorb other rasterizers'
    // edge pixels (Metal differs in 1–2 pixels of 1280×800, WARP in up to 4 at the ends of
    // hard-edged grid rectangles).
    SnapshotOptions::new()
        .threshold(1.0)
        .max_failed_pixels(egui_kittest::OsThreshold::new(0).macos(16).windows(16))
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
    snapshot(&mut h, "transfer_two_traces_banner");
}

/// The top bar while recording: `REC` with the audio's length and size in the record
/// colour, with a dropout counted.
#[test]
fn recording_indicator() {
    use ac2_proto::Change;
    use ac2_proto::model::{RecordingRun, RecordingStatus};
    use ac2_proto::units::{ClientId, SampleIndex, Seconds, SessionEpoch, WallNs};
    if !have_gpu("recording_indicator") {
        return;
    }
    let rig = common::Rig::start();
    rig.fake.lock().commit(Change::Recording(RecordingRun {
        name: "rec-2026-10-05T18-00-00".into(),
        path: "/home/op/.local/share/ac2/recordings/rec-2026-10-05T18-00-00.wav".into(),
        inputs: vec![0, 1],
        sample_rate_hz: 48_000,
        session_epoch: SessionEpoch(1),
        start_sample: SampleIndex(0),
        started_at: WallNs(0),
        started_by: ClientId("ac2-ui test".into()),
        frames: 48_000 * 83,
        bytes: 116 + 48_000 * 83 * 8,
        discontinuities: 1,
        max_duration: Seconds(3600.0),
        max_bytes: None,
        status: RecordingStatus::Recording,
    }));
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames and the recording", |a| {
        live(a) && a.state.recording_label().is_some()
    });
    assert_eq!(
        h.state().state.recording_label().map(|l| l.text),
        Some("REC 1:23 · 31.9 MB · 1 dropout".to_owned())
    );
    snapshot(&mut h, "recording_indicator");
}

#[test]
fn spatial_average_legend_and_banner() {
    if !have_gpu("spatial_average_legend_and_banner") {
        return;
    }
    let rig = common::Rig::start();
    rig.add_spatial_average();
    let mut h = harness(options(Some(&rig)));
    let has_average = |app: &App| {
        app.state.measurements().len() == 6
            && app.state.data.as_ref().is_some_and(|d| {
                d.latest
                    .get(&Topic::Data {
                        meas: MeasId(6),
                        stream: Stream::Tf,
                    })
                    .is_some()
            })
    };
    step_until(&mut h, "the average's frames", |a| {
        live_with(a, 6) && has_average(a)
    });
    // The transfer pane shows the average: its banner names the position left out.
    h.state_mut()
        .state
        .pane_meas
        .insert(PaneKind::Transfer, MeasId(6));
    h.state_mut().state.selected = Some(MeasId(6));
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
        assert!(
            legend
                .iter()
                .any(|l| l.starts_with("Audience · 2 of 3 positions · power avg")),
            "{legend:?}"
        );
        let banners: Vec<(&str, Option<&str>)> = s
            .banners
            .iter()
            .map(|b| (b.text.as_str(), b.detail.as_deref()))
            .collect();
        assert_eq!(
            banners,
            [(
                "AVERAGE · 2 OF 3 POSITIONS",
                Some("Audience: left out Seat 3: stopped")
            )]
        );
    }
    snapshot(&mut h, "transfer_spatial_average");
}

#[test]
fn help_overlay() {
    if !have_gpu("help_overlay") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press(Key::H);
    step_until(&mut h, "help", |a| a.state.overlay == Overlay::Help);
    snapshot(&mut h, "help_overlay");
    // H closes it again.
    h.key_press(Key::H);
    step_until(&mut h, "help closed", |a| a.state.overlay == Overlay::None);
}

/// The keys belong to the reducer, so egui never holds keyboard focus: Tab and the arrows in
/// an open dialog (or with none open) leave no widget focused — the sidebar's measurement
/// chip in particular — and the dialog stays open on its own focus.
#[test]
fn keyboard_focus_stays_in_the_open_dialog() {
    if !have_gpu("keyboard_focus_stays_in_the_open_dialog") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    let wander = |h: &mut Harness<'_, App>, what: &str| {
        for key in [
            Key::Tab,
            Key::ArrowDown,
            Key::ArrowRight,
            Key::ArrowLeft,
            Key::ArrowUp,
            Key::Tab,
            Key::ArrowDown,
        ] {
            h.key_press(key);
            h.step();
            h.step();
            assert_eq!(
                h.ctx.memory(|m| m.focused()),
                None,
                "{what}: egui focus after {key:?}"
            );
        }
    };
    wander(&mut h, "no dialog");
    type Open = (
        &'static str,
        fn(&mut Harness<'_, App>),
        fn(&Overlay) -> bool,
    );
    let dialogs: [Open; 4] = [
        (
            "Leq windows",
            |h| h.key_press_modifiers(Modifiers::SHIFT, Key::L),
            |o| matches!(o, Overlay::Leq(_)),
        ),
        (
            "palette",
            |h| h.key_press_modifiers(Modifiers::COMMAND, Key::K),
            |o| matches!(o, Overlay::Palette(_)),
        ),
        (
            "level prompt",
            |h| h.key_press(Key::Space),
            |o| matches!(o, Overlay::Prompt(_)),
        ),
        ("help", |h| h.key_press(Key::H), |o| *o == Overlay::Help),
    ];
    for (what, open, is_open) in dialogs {
        open(&mut h);
        step_until(&mut h, what, |a| is_open(&a.state.overlay));
        wander(&mut h, what);
        assert!(is_open(&h.state().state.overlay), "{what} stays open");
        h.key_press(Key::Escape);
        step_until(&mut h, "closed", |a| a.state.overlay == Overlay::None);
    }
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
    snapshot(&mut h, "command_palette");
    // Enter runs the highlighted command against the fake daemon.
    h.key_press(Key::Enter);
    step_until(&mut h, "palette closed", |a| {
        a.state.overlay == Overlay::None
    });
}

/// The calibrations view over a rig whose mic has two curves (0° chosen on its input), a
/// sensitivity calibration taken 3 h ago and the 90° curve also stored: what each input uses,
/// the library and the calibrations, with the sidebar naming the curve in use.
#[test]
fn calibrations_view() {
    use ac2_proto::model::{
        CalEntry, CalKey, CurveChoice, DeviceId, InputSetup, Mic, MicCurveRef, SplCal,
    };
    use ac2_proto::units::{Db, DbSpl, Dbfs, Hz, WallNs};
    use ac2_proto::{Change, Patch};
    if !have_gpu("calibrations_view") {
        return;
    }
    let rig = common::Rig::start();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let curve = |label: &str, file: &str| MicCurveRef {
        label: label.into(),
        file_name: file.into(),
        content_hash: "0123456789abcdef".into(),
        points: 100,
        f_lo: Hz(50.0),
        f_hi: Hz(20_000.0),
        imported_at: WallNs(now),
        stated_sensitivity: Some(15.0),
    };
    {
        let mut f = rig.fake.lock();
        f.commit(Change::Mic(Patch::Set(Mic {
            name: "MM1 34804".into(),
            curves: vec![
                curve("0°", "449350_34804_0Grad.txt"),
                curve("90°", "449350_34804_90Grad.txt"),
            ],
        })));
        f.commit(Change::Calibration(Patch::Set(CalEntry {
            key: CalKey {
                device: DeviceId("fake:loop".into()),
                channel: 1,
                mic: "MM1 34804".into(),
            },
            spl: SplCal {
                sensitivity: Db(130.5),
                method: ac2_proto::model::CalMethod::Acoustic {
                    calibrator_level: DbSpl(94.0),
                },
                freq: Hz(1000.0),
                measured: Dbfs(-36.5),
                calibrated_at: WallNs(now - 3 * 3_600_000_000_000 - 60_000_000_000),
            },
        })));
        f.commit(Change::Inputs(vec![InputSetup {
            channel: 1,
            mic: Some("MM1 34804".into()),
            curve: CurveChoice::Curve {
                label: "0°".into()
            },
        }]));
    }
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", |a| {
        live(a) && a.state.daemon().is_some_and(|s| !s.mics.is_empty())
    });
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    step_until(&mut h, "palette", |a| {
        matches!(a.state.overlay, Overlay::Palette(_))
    });
    h.event(Event::Text("calibrations".into()));
    h.key_press(Key::Enter);
    step_until(&mut h, "calibrations view", |a| {
        matches!(a.state.overlay, Overlay::Calibrations(_))
    });
    h.state_mut().state.toasts.clear();
    snapshot(&mut h, "calibrations_view");
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
    snapshot(&mut h, "delay_pick_candidates");
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
    snapshot(&mut h, "theme_light");
    h.key_press(Key::T);
    step_until(&mut h, "high contrast", |a| {
        a.state.theme == ThemeName::HighContrast
    });
    snapshot(&mut h, "theme_high_contrast");
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
    // The bound is on the app's own startup: from `App::new` (fonts, state, the link's
    // thread) through its first UI pass, which never waits for the daemon. GPU device
    // creation and the first render are timed and printed but not bounded: on a software
    // rasteriser they are dominated by shader compilation, whose wall time scales with
    // whatever else the machine is running (3–7 s at load 30 on 12 cores).
    let t0 = Instant::now();
    let mut h = Harness::builder()
        .with_size(SIZE)
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(move |cc| {
            let mut opts = options(None);
            opts.started = Instant::now();
            App::new(cc, opts)
        });
    h.step();
    let app = h.state().startup.first_ui.expect("first UI pass");
    let t_render = Instant::now();
    let img = h.render().expect("render");
    let render = t_render.elapsed();
    let total = t0.elapsed();
    assert_eq!(img.width(), SIZE.x as u32);
    println!(
        "startup: app {:.1} ms to its first UI pass; first frame rendered headless in {:.1} ms \
         (render {:.1} ms; target < 300 ms on the real window, `--bench-startup`)",
        app.as_secs_f64() * 1e3,
        total.as_secs_f64() * 1e3,
        render.as_secs_f64() * 1e3,
    );
    // Tens of milliseconds unloaded; a blocking call on the startup path (a connect, a
    // device or file wait) takes the bound out at once.
    assert!(app < Duration::from_secs(1), "{app:?}");
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
    snapshot(&mut h, "transfer_stored_traces");
}

/// Two captures, the second spread +3 dB with Alt+Shift+↑: its curve sits 3 dB above the
/// other's and its legend row says `+3.0 dB`, so the spread is never read as a level
/// difference.
#[test]
fn trace_offset_spread() {
    if !have_gpu("trace_offset_spread") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    step_until(&mut h, "slot 1", |a| a.state.slots()[0].is_some());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    step_until(&mut h, "both slots with data", |a| {
        a.state.slots()[1].is_some() && a.state.traces.len() == 2
    });
    let id = h.state().state.slots()[1].map(|t| t.id).expect("slot 2");
    h.key_press(Key::V);
    h.key_press(Key::V);
    step_until(&mut h, "slot 2 selected", |a| {
        a.state.selected_trace == Some(id)
    });
    h.key_press_modifiers(Modifiers::ALT | Modifiers::SHIFT, Key::ArrowUp);
    step_until(&mut h, "slot 2 spread", |a| {
        a.state
            .traces
            .get(&id)
            .is_some_and(|(d, _)| d.meta.edit.offset.0 == 3.0)
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
        assert!(
            legend
                .iter()
                .any(|l| l.starts_with("Main L S2") && l.contains("· +3.0 dB")),
            "{legend:?}"
        );
        assert!(
            legend
                .iter()
                .any(|l| l.starts_with("Main L S1") && !l.contains("dB")),
            "{legend:?}"
        );
    }
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "trace_offset_spread");
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
    snapshot(&mut h, "pane_measurement_list");
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
    h.get_by_label("Main L S1\ncapture · slot 1 · 1/6 oct")
        .click();
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
    snapshot(&mut h, "slot_resmoothed");
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
        assert_eq!(sp.unit, "dBFS per 11.7 Hz bin (tone, 1/6 oct smoothed)");
        assert_eq!(sp.caption, "Hann window");
    }
    snapshot(&mut h, "smoothed_phase_and_spectrum");
}

/// A daemon with no audio session (a fresh local daemon, an embedded one on real audio):
/// the transfer pane says how to open one.
#[test]
fn empty_session_hint() {
    if !have_gpu("empty_session_hint") {
        return;
    }
    let fake = ac2_client::fake::FakeDaemon::start(common::fake_options()).expect("fake daemon");
    let mut h = harness(options_at(Some(fake.endpoints())));
    step_until(&mut h, "synced, no session", |a| {
        a.state.mirror.as_ref().is_some_and(|m| m.synced())
            && a.state.empty_hint(&a.keymap).is_some()
    });
    assert_eq!(
        h.state()
            .state
            .empty_hint(&h.state().keymap)
            .map(|h| h.text)
            .as_deref(),
        Some("No audio session — press Shift+O (or Ctrl+K → Open audio session)")
    );
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "empty_session_hint");

    // A stored curve (an imported target) is drawn without a session: the hint moves to
    // the pane's title strip, out of the curve's way.
    h.key_press(Key::Z);
    step_until(&mut h, "target prompt", |a| {
        matches!(a.state.overlay, Overlay::Prompt(_))
    });
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ac2-traces/tests/fixtures/house_curve.txt");
    h.event(Event::Text(path.to_string_lossy().into_owned()));
    h.key_press(Key::Enter);
    step_until(&mut h, "the target drawn", |a| {
        a.state.transfer_shows_stored() && a.state.overlay == Overlay::None
    });
    assert_eq!(
        h.state()
            .state
            .empty_hint(&h.state().keymap)
            .map(|h| h.place),
        Some(ac2_ui::state::HintPlace::Title)
    );
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "empty_session_hint_over_traces");
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
    let fake =
        Arc::new(ac2_client::fake::FakeDaemon::start(common::fake_options()).expect("fake daemon"));
    let _meters = Meters::start(Arc::clone(&fake));
    let mut h = harness(options_at(Some(fake.endpoints())));
    step_until(&mut h, "synced", |a| {
        a.state.mirror.as_ref().is_some_and(|m| m.synced())
    });
    h.key_press_modifiers(Modifiers::SHIFT, Key::O);
    // This fake publishes preview frames unasked, and with a session open the meters are
    // subscribed before the dialog opens: the preview request is waited for on its own.
    step_until(
        &mut h,
        "dialog with devices, meters and the device previewed",
        |a| {
            session_dialog_of(a).is_some_and(|d| d.device_info().is_some())
                && a.state.input_meters().len() == 4
                && fake.lock().preview.is_some()
        },
    );
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
    snapshot(&mut h, "session_dialog");

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
    snapshot(&mut h, "session_dialog_detect_confirm");
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
    snapshot(&mut h, "session_dialog_detected");

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
    // Created, then started by a second command: the picture shows it running.
    step_until(&mut h, "measurement created and running", |a| {
        a.state
            .measurements()
            .iter()
            .any(|m| m.config.name == "Reference → M30 FOH" && m.running)
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
    snapshot(&mut h, "transfer_dialog");
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
    step_until(&mut h, "level typed, inputs metered", |a| {
        matches!(&a.state.overlay, Overlay::Form(f)
            if f.text(ac2_ui::forms::FieldId::Level) == "-30")
            && a.state.input_meters().len() == 2
    });
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "sweep_dialog");
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
    // The sweep left nothing armed: the stimulus is off (STIM OFF) and the lease was given
    // back.
    step_until(&mut h, "stimulus off and released", |a| {
        !a.state.stimulus_live()
            && a.state
                .daemon()
                .is_some_and(|s| s.generator.owner.is_none() && !s.generator.armed)
    });
    // In the grid, beside the other panes: caption and legend fit a small pane.
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "sweep_distortion_pane");
    // The focused pane alone, for the picture.
    h.key_press(Key::W);
    step_until(&mut h, "maximized", |a| a.state.layout.maximized);
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "sweep_distortion");
    // The title's dB | % toggle (U does the same): percent on a log axis.
    h.get_by_label("%").click();
    step_until(&mut h, "percent", |a| {
        a.state.view.distortion.unit == ac2_scene::view::DistortionUnit::Percent
    });
    // The pointer away again: no cursor or tooltip in the pictures.
    h.event(Event::PointerGone);
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "sweep_distortion_percent");
    h.key_press_modifiers(Modifiers::SHIFT, Key::I);
    step_until(&mut h, "sweep IR", |a| a.state.view.distortion.show_ir);
    // G: the log view, where the harmonics' impulses read at their level.
    h.key_press(Key::G);
    step_until(&mut h, "log IR", |a| {
        a.state.view.ir.mode == ac2_scene::view::IrMode::Log
    });
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "sweep_ir");
}

/// A set of sweeps running while the transfer pane is maximised: the progress strip shows
/// which sweep of how many, the bar and the time left, and the sidebar marks the sweep's
/// inputs; the strip's Stop button stops the stimulus.
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
    {
        let mut s = rig.fake.lock();
        let mut g = s.state.generator.clone();
        g.armed = true;
        g.firing = true;
        s.commit(Change::Generator(g));
        s.commit(Change::Sweep(SweepRun {
            id: SweepId(1),
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
            status: SweepStatus::Playing { repeat: 1 },
            started_at: WallNs(0),
        }));
    }
    step_until(&mut h, "the progress strip", |a| {
        a.state.operation().is_some()
    });
    let p = h.state().state.operation().expect("progress");
    assert_eq!(p.step, "sweep 1 of 2");
    h.key_press(Key::W);
    step_until(&mut h, "maximized", |a| a.state.layout.maximized);
    h.state_mut().state.toasts.clear();
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
            if let Some(seen) = &mut a.state.sweep.step_seen {
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

/// Publishes the given `leq` frame of an SPL meter on the fake daemon every 200 ms, with a
/// new `seq` and the fake's clock each time (fresh, as the daemon's once-a-second frames
/// are); the test swaps the frame.
struct LeqPublisher {
    frame: Arc<std::sync::Mutex<Option<ac2_proto::frame::LeqFrame>>>,
    /// The meter's own frame, published alongside once set.
    spl: Arc<std::sync::Mutex<Option<ac2_proto::frame::SplFrame>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LeqPublisher {
    fn start(fake: Arc<ac2_client::fake::FakeDaemon>) -> Self {
        use ac2_proto::{Frame, FrameData};
        let frame: Arc<std::sync::Mutex<Option<ac2_proto::frame::LeqFrame>>> =
            Arc::new(std::sync::Mutex::new(None));
        let spl: Arc<std::sync::Mutex<Option<ac2_proto::frame::SplFrame>>> =
            Arc::new(std::sync::Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (f, m, s) = (Arc::clone(&frame), Arc::clone(&spl), Arc::clone(&stop));
        let thread = std::thread::spawn(move || {
            let mut seq = 1;
            while !s.load(Ordering::Acquire) {
                let data = f.lock().ok().and_then(|g| g.clone());
                if let Some(data) = data {
                    let mut st = fake.lock();
                    let frame = Frame {
                        stamp: st.stamp(seq, None),
                        data: FrameData::Leq(data),
                    };
                    st.publish(&frame);
                    seq += 1;
                }
                let meter = m.lock().ok().and_then(|g| g.clone());
                if let Some(data) = meter {
                    let mut st = fake.lock();
                    let frame = Frame {
                        stamp: st.stamp(seq, None),
                        data: FrameData::Spl(data),
                    };
                    st.publish(&frame);
                    seq += 1;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        Self {
            frame,
            spl,
            stop,
            thread: Some(thread),
        }
    }

    fn set(&self, f: ac2_proto::frame::LeqFrame) {
        if let Ok(mut g) = self.frame.lock() {
            *g = Some(f);
        }
    }

    fn set_spl(&self, f: ac2_proto::frame::SplFrame) {
        if let Ok(mut g) = self.spl.lock() {
            *g = Some(f);
        }
    }
}

impl Drop for LeqPublisher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// 17:02 UTC on 3 October 2026.
const SHOW_START_S: u64 = 1_791_046_920;
/// The pictures' local time: UTC+2, so the show starts at 19:02 on any machine.
const SHOW_ZONE: ac2_ui::scenes::LocalZone = ac2_ui::scenes::LocalZone::Fixed { offset_s: 7200 };

/// One `leq` frame of meter `meas`: a value per window, over / near as given, a one-minute
/// horizon. Window order is the default one with DIN 15905-5 on 30 min and a typed limit on
/// 1 min: 1, 5, 10, 30, 60 min.
fn leq_frame(
    meas: MeasId,
    leq: [f32; 5],
    states: [Option<ac2_proto::frame::LeqFlags>; 5],
    allowed: [f32; 5],
    elapsed: [f32; 5],
    calibrated_at: u64,
) -> ac2_proto::frame::LeqFrame {
    use ac2_proto::frame::{LeqFlags, LeqFrame, LeqMeta, LeqRun};
    use ac2_proto::model::{CalStatus, LevelScale};
    use ac2_proto::units::{Seconds, WallNs};
    LeqFrame {
        meas,
        meta: LeqMeta {
            scale: LevelScale::DbSpl,
            cal: CalStatus::Verified {
                calibrated_at: WallNs(calibrated_at),
                basis: ac2_proto::model::CalBasis::Acoustic {
                    calibrator_level: ac2_proto::units::DbSpl(94.0),
                },
            },
            mic_curve: false,
            horizon: Seconds(60.0),
            logged: 2718,
            // The show so far: 45:30 from 19:02 (UTC+2, [`SHOW_ZONE`]), 12 s of it lost.
            run: Some(LeqRun {
                started_at: WallNs(SHOW_START_S * 1_000_000_000),
                until: WallNs((SHOW_START_S + 2730) * 1_000_000_000),
                measured: Seconds(2718.0),
                gaps: Seconds(12.0),
                trimmed: false,
                laeq: f64::from(leq[4]) + 0.4,
                lceq: f64::from(leq[4]) + 6.0,
                lzeq: f64::from(leq[4]) + 9.0,
            }),
        },
        leq: leq.to_vec(),
        elapsed: elapsed.to_vec(),
        measured: elapsed.to_vec(),
        allowed: allowed.to_vec(),
        recover: vec![f32::NAN; 5],
        // A filling window ends at its energy so far over its whole length if the rest is
        // silent.
        least: leq
            .iter()
            .zip(elapsed)
            .zip([60.0f32, 300.0, 600.0, 1800.0, 3600.0])
            .map(|((l, e), d)| l + 10.0 * (e / d).min(1.0).log10())
            .collect(),
        over_in: vec![f32::NAN; 5],
        flags: states
            .iter()
            .map(|s| match s {
                None => LeqFlags::NONE,
                Some(f) => LeqFlags::LIMIT.with(LeqFlags::JUDGED).with(*f),
            })
            .collect(),
    }
}

/// From an empty fake daemon, using the app: the session from its dialog, an SPL meter from
/// the palette, Shift+L for its Leq windows — a preset shown (DIN 15905-5: its LAeq 30 min
/// window alone) and left, typed limits on LAeq 1 min and 30 min — Enter sends them and the
/// SPL pane shows them as columns. The
/// daemon (here the test, through the fake) then reports the 1 min and 30 min windows over:
/// their columns turn red and the alarms toast; maximised, the columns fill the screen. B and
/// H switch to tiles with the history strip below. Then they recover, and back on columns,
/// F11 is the stage view.
#[test]
fn leq_tiles_from_an_empty_daemon() {
    use ac2_proto::frame::LeqFlags;
    use ac2_proto::model::{LeqAlarm, LeqAlarmKind, LeqJudgement, MeasKind, SplLog};
    use ac2_proto::units::{DbSpl, Seconds, WallNs};
    use ac2_proto::{Change, Patch};
    if !have_gpu("leq_tiles_from_an_empty_daemon") {
        return;
    }
    let fake =
        Arc::new(ac2_client::fake::FakeDaemon::start(common::fake_options()).expect("fake daemon"));
    let _meters = Meters::start(Arc::clone(&fake));
    let leq = LeqPublisher::start(Arc::clone(&fake));
    let mut h = harness(options_at(Some(fake.endpoints())));
    h.state_mut().state.local_zone = SHOW_ZONE;
    step_until(&mut h, "synced", |a| {
        a.state.mirror.as_ref().is_some_and(|m| m.synced())
    });

    // The session from its dialog.
    h.key_press_modifiers(Modifiers::SHIFT, Key::O);
    step_until(&mut h, "the session dialog with its devices", |a| {
        session_dialog_of(a).is_some_and(|d| d.device_info().is_some())
            && a.state.input_meters().len() == 4
    });
    h.key_press(Key::Enter);
    step_until(&mut h, "session open", |a| a.state.open_session().is_some());
    if matches!(h.state().state.overlay, Overlay::Offer(_)) {
        h.key_press(Key::N);
    }
    step_until(&mut h, "no dialog", |a| a.state.overlay == Overlay::None);

    // An SPL meter from the palette: Ctrl+K, "new spl", Enter, Enter.
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.event(Event::Text("new spl".into()));
    step_until(&mut h, "palette typed", |a| {
        matches!(&a.state.overlay, Overlay::Palette(_))
    });
    h.key_press(Key::Enter);
    step_until(
        &mut h,
        "the SPL dialog",
        |a| matches!(&a.state.overlay, Overlay::Form(f) if f.kind == ac2_ui::forms::FormKind::Spl),
    );
    h.key_press(Key::Enter);
    step_until(&mut h, "the meter, running, with its windows", |a| {
        a.state
            .measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spl { .. }) && m.running)
            && a.state.daemon().is_some_and(|s| s.spl_logs.len() == 1)
    });
    let meas = h
        .state()
        .state
        .measurements()
        .iter()
        .find(|m| matches!(m.config.kind, MeasKind::Spl { .. }))
        .map(|m| m.id)
        .expect("meter");

    // Shift+L: → the DIN preset, its one window; ← back to the meter's windows. ↓↓ the
    // 1 min window, Tab Tab its limit, 102; ↓↓↓ the 30 min one's, 99.
    h.key_press_modifiers(Modifiers::SHIFT, Key::L);
    step_until(&mut h, "the Leq dialog", |a| {
        matches!(a.state.overlay, Overlay::Leq(_))
    });
    h.key_press(Key::ArrowRight);
    step_until(
        &mut h,
        "the DIN preset alone",
        |a| matches!(&a.state.overlay, Overlay::Leq(d) if d.preset == Some(0) && d.rows.len() == 1),
    );
    h.key_press(Key::ArrowLeft);
    h.key_press(Key::ArrowDown);
    h.key_press(Key::ArrowDown);
    h.key_press(Key::Tab);
    h.key_press(Key::Tab);
    h.event(Event::Text("102".into()));
    for _ in 0..3 {
        h.key_press(Key::ArrowDown);
    }
    h.event(Event::Text("99".into()));
    step_until(&mut h, "the limits typed", |a| {
        matches!(&a.state.overlay, Overlay::Leq(d)
            if d.preset.is_none() && d.rows.len() == 5
                && d.rows[0].limit == "102" && d.rows[3].limit == "99")
    });
    h.event(Event::PointerGone);
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "leq_dialog");
    // ↑ ×5 to the preset row, → to the French preset for children (its two windows, the
    // longest name and source), then ← back to "none": the windows as typed.
    let typed = match &h.state().state.overlay {
        Overlay::Leq(d) => d.rows.clone(),
        _ => panic!("the Leq dialog"),
    };
    let to = ac2_proto::model::LeqPreset::ALL
        .iter()
        .position(|p| *p == ac2_proto::model::LeqPreset::FranceChildren)
        .expect("listed");
    for _ in 0..5 {
        h.key_press(Key::ArrowUp);
    }
    for _ in 0..=to {
        h.key_press(Key::ArrowRight);
    }
    step_until(
        &mut h,
        "two windows from one preset",
        |a| matches!(&a.state.overlay, Overlay::Leq(d) if d.preset == Some(to) && d.rows.len() == 2),
    );
    snapshot(&mut h, "leq_dialog_two_window_preset");
    for _ in 0..=to {
        h.key_press(Key::ArrowLeft);
    }
    step_until(
        &mut h,
        "back to the windows as typed",
        |a| matches!(&a.state.overlay, Overlay::Leq(d) if d.preset.is_none() && d.rows == typed),
    );
    // egui walks its own widget focus on arrow keys too; the app's keys never need it.
    h.ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
    h.key_press(Key::Enter);
    step_until(&mut h, "the windows set", |a| {
        a.state.overlay == Overlay::None
            && a.state.measurements().iter().any(|m| match &m.config.kind {
                MeasKind::Spl { config } => {
                    config.leq.windows[0].limit == Some(DbSpl(102.0))
                        && config.leq.windows[3].limit == Some(DbSpl(99.0))
                }
                _ => false,
            })
    });
    assert_eq!(fake.executions("meas.update"), 1);
    assert!(h.state().state.view.spl.mode.shows_leq());
    assert_eq!(h.state().state.layout.focus, PaneKind::Spl);
    assert_eq!(
        h.state().state.view.spl.layout,
        ac2_scene::view::LeqLayout::default()
    );

    // The daemon's view: calibrated 3 h ago, the 1 min and 30 min windows over.
    let cal_at = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64))
    .saturating_sub(3 * 3600 * 1_000_000_000 + 600_000_000_000);
    let full = [60.0, 300.0, 600.0, 1800.0, 2730.0];
    let over_frame = leq_frame(
        meas,
        [103.1, 99.6, 98.7, 99.4, 97.2],
        [Some(LeqFlags::OVER), None, None, Some(LeqFlags::OVER), None],
        [102.0, f32::NAN, f32::NAN, 85.3, f32::NAN],
        full,
        cal_at,
    );
    leq.set(over_frame.clone());
    let alarm = |at: u64, duration: f64, kind, leq: f64, limit: f64| LeqAlarm {
        at: WallNs(at),
        duration: Seconds(duration),
        weighting: ac2_proto::model::Weighting::A,
        kind,
        leq: DbSpl(leq),
        limit: DbSpl(limit),
    };
    let mut log = fake.lock().state.spl_logs[0].clone();
    log.windows[0].judgement = LeqJudgement::Over;
    log.windows[3].judgement = LeqJudgement::Over;
    log.alarms = vec![
        alarm(cal_at, 60.0, LeqAlarmKind::Over, 102.3, 102.0),
        alarm(cal_at, 1800.0, LeqAlarmKind::Over, 99.1, 99.0),
    ];
    fake.lock()
        .commit(Change::SplLog(Patch::Set(SplLog { ..log.clone() })));
    step_until(&mut h, "the alarms toasted", |a| {
        a.state
            .toasts
            .iter()
            .filter(|t| t.error && t.text.contains("over its limit"))
            .count()
            == 2
    });
    // The history the app would have gathered over the last 45 minutes, pinned so the
    // picture does not depend on when frames arrived.
    let history = {
        let st = &h.state().state;
        let m = st
            .measurements()
            .into_iter()
            .find(|m| m.id == meas)
            .cloned()
            .expect("meter");
        let MeasKind::Spl { config } = m.config.kind else {
            unreachable!()
        };
        let mut hist = ac2_scene::leq::LeqHistory::default();
        for k in 0..2700u32 {
            let t = f64::from(k);
            // A show building up: 92 dB rising to 101 at the end, a loud passage at 30 min.
            let base = 92.0 + 6.0 * (t / 2700.0) as f32;
            let peak = if (1700..2000).contains(&k) { 7.0 } else { 0.0 };
            let one = base + peak + 2.0 * ((t / 40.0).sin() as f32);
            let longer = |w: f32| base + peak * (60.0 / w).min(1.0) * 0.8;
            let vals = [
                one,
                longer(300.0),
                longer(600.0),
                longer(1800.0) + 0.6,
                base - 1.0,
            ];
            let flags = [
                if vals[0] > 102.0 {
                    LeqFlags::OVER
                } else {
                    LeqFlags::NONE
                },
                LeqFlags::NONE,
                LeqFlags::NONE,
                if vals[3] > 99.0 {
                    LeqFlags::OVER
                } else {
                    LeqFlags::NONE
                },
                LeqFlags::NONE,
            ];
            let f = leq_frame(meas, vals, flags.map(Some), [f32::NAN; 5], full, cal_at);
            hist.push(&config.leq, &f, t);
        }
        hist
    };
    // G twice: from meter + Leq (the default) past the meter alone to the windows alone.
    assert_eq!(
        h.state().state.view.spl.mode,
        ac2_scene::view::SplMode::MeterLeq
    );
    h.key_press(Key::G);
    h.key_press(Key::G);
    step_until(&mut h, "the windows alone", |a| {
        a.state.view.spl.mode == ac2_scene::view::SplMode::Leq
    });
    h.key_press(Key::W);
    step_until(&mut h, "maximised", |a| a.state.layout.maximized);
    let pin = |a: &mut ac2_ui::App| {
        a.state.toasts.clear();
        a.state.local_zone = SHOW_ZONE;
        a.state
            .leq_history
            .insert(meas, (u64::MAX, history.clone()));
    };
    let first_is = move |over: bool| {
        move |a: &ac2_ui::App| {
            a.state.data.as_ref().is_some_and(|d| {
                d.latest
                    .get(&Topic::Data {
                        meas,
                        stream: Stream::Leq,
                    })
                    .is_some_and(|f| match &f.frame.data {
                        ac2_proto::FrameData::Leq(l) => (l.leq[0] > 103.0) == over,
                        _ => false,
                    })
            })
        }
    };
    snapshot_when(&mut h, "leq_columns_over", pin, first_is(true));
    // B: tiles, Shift+B: the history strip under them.
    h.key_press(Key::B);
    h.key_press_modifiers(Modifiers::SHIFT, Key::B);
    step_until(&mut h, "tiles with history", |a| {
        a.state.view.spl.layout
            == ac2_scene::view::LeqLayout {
                style: ac2_scene::view::LeqStyle::Tiles,
                history: true,
            }
    });
    snapshot_when(&mut h, "leq_tiles_over", pin, first_is(true));

    // Back under: the 1 min window plain again, the 30 min one near its limit.
    leq.set(leq_frame(
        meas,
        [96.4, 97.9, 98.2, 98.6, 97.0],
        [Some(LeqFlags::NONE), None, None, Some(LeqFlags::NEAR), None],
        [102.0, f32::NAN, f32::NAN, 100.4, f32::NAN],
        full,
        cal_at,
    ));
    log.windows[0].judgement = LeqJudgement::Ok;
    log.windows[3].judgement = LeqJudgement::Near;
    log.alarms
        .push(alarm(cal_at, 60.0, LeqAlarmKind::Recovered, 101.9, 102.0));
    log.alarms
        .push(alarm(cal_at, 1800.0, LeqAlarmKind::Recovered, 98.9, 99.0));
    fake.lock().commit(Change::SplLog(Patch::Set(log)));
    step_until(&mut h, "the recoveries toasted", |a| {
        a.state
            .toasts
            .iter()
            .filter(|t| t.text.contains("back within its limit"))
            .count()
            == 2
    });
    snapshot_when(&mut h, "leq_tiles_recovered", pin, first_is(false));
    // Back to columns without the strip.
    h.key_press(Key::B);
    h.key_press_modifiers(Modifiers::SHIFT, Key::B);
    step_until(&mut h, "columns again", |a| {
        a.state.view.spl.layout == ac2_scene::view::LeqLayout::default()
    });
    snapshot_when(&mut h, "leq_columns_recovered", pin, first_is(false));

    // Shift+R asks before a new log, naming the run that ends; N keeps it.
    h.key_press_modifiers(Modifiers::SHIFT, Key::R);
    step_until(&mut h, "the new log confirmation", |a| {
        matches!(a.state.overlay, Overlay::NewLog(_))
    });
    snapshot_when(&mut h, "leq_new_log_confirm", pin, first_is(false));
    h.key_press(Key::N);
    step_until(&mut h, "no dialog", |a| a.state.overlay == Overlay::None);

    // F11 with the pane maximised: the stage view, the columns alone; over again.
    leq.set(over_frame);
    h.key_press(Key::F11);
    step_until(&mut h, "the stage view", |a| a.state.stage_view());
    snapshot_when(&mut h, "leq_columns_fullscreen", pin, first_is(true));

    // G: the meter's number over the windows, on the stage; F11 again, in the window.
    leq.set_spl(spl_frame_at(meas, 101.84, cal_at));
    h.key_press(Key::G);
    let held = move |a: &ac2_ui::App| {
        a.state.view.spl.mode == ac2_scene::view::SplMode::MeterLeq
            && a.state.spl_hold.contains_key(&meas)
            && first_is(true)(a)
    };
    snapshot_when(&mut h, "spl_meter_leq_stage", pin, held);
    h.key_press(Key::F11);
    step_until(&mut h, "maximised in the window", |a| {
        a.state.layout.maximized && !a.state.stage_view()
    });
    snapshot_when(&mut h, "spl_meter_leq", pin, held);
    drop(leq);
}

/// The meter's frame for the Leq test's meter: LAF `level`, calibrated at `calibrated_at`.
fn spl_frame_at(meas: MeasId, level: f64, calibrated_at: u64) -> ac2_proto::frame::SplFrame {
    use ac2_proto::frame::{SplFrame, SplMeta};
    use ac2_proto::model::{CalStatus, LevelScale, PeakWeighting, TimeWeighting, Weighting};
    use ac2_proto::units::{Seconds, WallNs};
    SplFrame {
        meas,
        meta: SplMeta {
            scale: LevelScale::DbSpl,
            weighting: Weighting::A,
            time_weighting: TimeWeighting::Fast,
            peak_weighting: PeakWeighting::C,
            level,
            lmax: level + 4.0,
            lmin: level - 20.0,
            leq: level - 1.5,
            lpeak: level + 15.0,
            duration: Seconds(2730.0),
            cal: CalStatus::Verified {
                calibrated_at: WallNs(calibrated_at),
                basis: ac2_proto::model::CalBasis::Acoustic {
                    calibrator_level: ac2_proto::units::DbSpl(94.0),
                },
            },
            mic_curve: false,
        },
    }
}

/// The Traces list: every stored trace by name with what it is, its slot, shown or hidden
/// (the dot, a click on it toggles) and the selected one highlighted (a click on the row).
#[test]
fn traces_list() {
    if !have_gpu("traces_list") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    step_until(&mut h, "slot 1", |a| a.state.slots()[0].is_some());
    h.key_press(Key::N);
    step_until(&mut h, "delay tower", |a| {
        a.state.selected == Some(MeasId(2))
    });
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    step_until(&mut h, "slot 2", |a| a.state.slots()[1].is_some());
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
    // The dot hides slot 2; a click on the target's row selects it.
    h.get_by_label("Hide Delay tower S2").click();
    step_until(&mut h, "slot 2 hidden", |a| {
        a.state.slots()[1].is_some_and(|t| !t.edit.visible)
    });
    h.get_by_label("house_curve\ntarget").click();
    step_until(&mut h, "target selected", |a| {
        a.state
            .selected_trace_meta()
            .is_some_and(|t| t.edit.name == "house_curve")
    });
    assert_eq!(
        h.state().state.pane_caption(PaneKind::Transfer).as_deref(),
        Some("house_curve")
    );
    // The hidden trace's dot now offers to show it.
    h.get_by_label("Show Delay tower S2");
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "traces_list");
}

/// The key hints on a crowded grid: the sweep pane shown beside the others (four narrow
/// panes under the transfer pane at 1280 px), the SPL meter focused with its hint line cut
/// to what fits, its footer naming the mic and its curve without running into the readouts,
/// and the transfer title naming the selected stored trace, shortened. Then the SPL title's
/// tooltip: the pane's hints in full.
#[test]
fn key_hints() {
    use ac2_proto::model::{CurveChoice, InputSetup, Mic, MicCurveRef};
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
        f.commit(Change::Inputs(vec![InputSetup {
            channel: 1,
            mic: Some("MM1 34804".into()),
            curve: CurveChoice::Curve {
                label: "90°".into(),
            },
        }]));
    }
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames with the mic", |a| {
        live(a) && a.state.daemon().is_some_and(|s| !s.mics.is_empty())
    });
    // A capture of the transfer measurement.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1);
    step_until(&mut h, "slot 1", |a| a.state.slots()[0].is_some());
    // The sweep pane shown (focused by Alt+5), then the SPL meter focused.
    h.key_press_modifiers(Modifiers::ALT, Key::Num5);
    h.key_press_modifiers(Modifiers::ALT, Key::Num4);
    step_until(&mut h, "five panes, SPL focused", |a| {
        a.state.layout.visible().len() == 5 && a.state.layout.focus == PaneKind::Spl
    });
    // The capture selected (the focus stays): the transfer title names it.
    h.key_press(Key::V);
    step_until(&mut h, "slot 1 selected", |a| {
        a.state.selected_trace_meta().is_some() && a.state.layout.focus == PaneKind::Spl
    });
    let names = |a: &App| a.state.pane_caption_variants(PaneKind::Transfer);
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
            .key_hint_line(&h.state().keymap, PaneKind::Spl, style)
            .is_some()
    );
    h.event(Event::PointerGone);
    h.state_mut().state.toasts.clear();
    h.step();
    snapshot(&mut h, "key_hints");
    // The SPL title's tooltip.
    h.get_by_label("4  SPL").hover();
    for _ in 0..6 {
        h.step();
    }
    snapshot(&mut h, "key_hints_tooltip");
}

/// The SPL meter maximised (W): the held level large and centred with `LAF · dBFS` under
/// it, the level bar, the statistics and the footer; W again, full screen: the stage view,
/// the meter alone on the screen.
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
        a.state.view.spl.mode == ac2_scene::view::SplMode::Meter
            && a.state.layout.maximized
            && a.state.layout.focus == PaneKind::Spl
            && a.state.spl_hold.contains_key(&MeasId(4))
    });
    snapshot(&mut h, "spl_meter_big");
    h.key_press(Key::W);
    step_until(&mut h, "the stage view", |a| a.state.stage_view());
    snapshot(&mut h, "spl_meter_stage");
}

/// 25 s of a spectrum with a tone sweeping from 100 Hz to 12.8 kHz over a sloped floor and a
/// steady 1 kHz tone, with 1.5 s without frames (STALE) in the middle, at a fixed time: a
/// picture that does not depend on when the frames arrived.
fn sweep_history(def: &ac2_proto::GridDef) -> ac2_scene::spectrograph::SpectrographHistory {
    use ac2_scene::spectrograph::{SpectrographFrame, SpectrographHistory};
    let edges = ac2_scene::grid::column_edges(def);
    let freqs = ac2_scene::grid::column_frequencies(def);
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
                && st.layout.focus == PaneKind::Spectrum
                && st.view.spectrum.spectrograph.shown
                && st.meas(MeasId(3)).is_some_and(|m| !m.running)
        },
    );
    let def = {
        let st = &h.state().state;
        let d = st.data.as_ref().expect("data");
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
        a.state.spectrographs.get(&MeasId(3)).is_some_and(|x| {
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
        st.spectrographs.insert(MeasId(3), pinned.clone());
        st.view.cursor_hz = Some(1000.0);
        st.view.spectrum.spectrograph.cursor_s = Some(8.0);
        st.toasts.clear();
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
        let s = ac2_ui::scenes::spectrograph(st, &theme, size, now);
        assert_eq!(s.caption, "Mic 1 FFT · last 30 s · dBFS · stopped");
        assert_eq!(
            s.cursor.expect("cursor").text,
            "1.00 kHz · 8.0 s ago · \u{2212}32.0 dBFS"
        );
    }
    snapshot_when(&mut h, "spectrograph", pin, same);
}
