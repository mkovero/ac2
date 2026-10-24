//! Headless UI snapshots against the fake daemon, rendered through egui-wgpu with the plots
//! in paint callbacks, exactly as on screen.
//!
//! Needs a wgpu adapter: locally `AC2_GPU_FALLBACK=1 WGPU_BACKEND=vulkan` (lavapipe). Without
//! one the tests print SKIP and pass, unless `AC2_REQUIRE_GPU=1`. References in
//! `tests/snapshots/` are blessed on lavapipe: `UPDATE_SNAPSHOTS=1 cargo test -p ac2-ui`.

use crate::common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ac2_client::ClientConfig;
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::MeasId;
use ac2_scene::theme::ThemeName;
use ac2_ui::conn::Target;
use ac2_ui::keys::Keymap;
use ac2_ui::state::{ConnState, Overlay, PaneKind, Severity};
use ac2_ui::{App, AppOptions};
use eframe::egui::{self, Event, Key, Modifiers};
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, SnapshotOptions};

mod band;
mod drag;
mod eq;
mod export;
mod leq;
mod overlays;
mod session;
mod traces;
mod transfer;
mod views;

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
        prefs: common::grid_ui_prefs(),
        prefs_path: None,
        notices: vec![],
        started: Instant::now(),
        bench_startup: false,
        smoke: false,
        open_session_dialog: false,
        embedded_autosave: None,
        client_key: None,
        connect: None,
        rasterizer: Some(ac2_plot::Rasterizer::Gpu),
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
    // The build id names the commit: pinned, or every commit would change the pictures.
    h.state_mut().state.build_id = "0.0.0+test".into();
    // The window's own frame rate and data rate move run to run: a test that shows them
    // feeds them.
    h.state_mut().health_measured = false;
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
            let toasts: Vec<&str> = a
                .state
                .toasts
                .list
                .iter()
                .map(|t| t.text.as_str())
                .collect();
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

/// The topics [`healthy`] finds stale, for a failure to name.
fn stale_topics(app: &App) -> Vec<String> {
    let st = &app.state;
    st.data.as_ref().map_or_else(Vec::new, |d| {
        d.latest
            .frames
            .iter()
            .filter(|(_, f)| ac2_ui::scenes::freshness(st, f).is_stale())
            .map(|(k, _)| k.to_string())
            .collect()
    })
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
    settled_snapshot(h, name, pin, drawn, &snapshot_options());
}

/// [`snapshot`] of a plot dense with thin curves, compared with [`dense_snapshot_options`].
fn dense_snapshot(h: &mut Harness<'_, App>, name: &str) {
    settled_snapshot(h, name, |_| {}, |_| true, &dense_snapshot_options());
}

fn settled_snapshot(
    h: &mut Harness<'_, App>,
    name: &str,
    pin: impl Fn(&mut App),
    drawn: impl Fn(&App) -> bool,
    options: &SnapshotOptions,
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
        assert!(
            t0.elapsed() < WAIT,
            "{name}: the link never settled; stale: {:?}",
            stale_topics(h.state())
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    h.snapshot_options(name, options);
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

/// [`snapshot_options`] for a plot of many thin anti-aliased curves: every curve crossing
/// adds edge pixels where another rasterizer's coverage differs, so Metal and WARP differ
/// in proportionally more pixels than on a plot of a few traces. Linux stays bit-exact.
fn dense_snapshot_options() -> SnapshotOptions {
    SnapshotOptions::new()
        .threshold(1.0)
        .max_failed_pixels(egui_kittest::OsThreshold::new(0).macos(64).windows(64))
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
            ClipFlags, PreviewLevelsFrame, PreviewLevelsMeta, SessionLevelsFrame, SessionLevelsMeta,
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
                            inputs: Vec::new(),
                            pairs: Vec::new(),
                            ballistic: Default::default(),
                            pair_levels: Vec::new(),
                        },
                        peak: peak.to_vec(),
                        rms: rms.to_vec(),
                        clip: vec![ClipFlags::NONE; 4],
                        ppm: peak.to_vec(),
                        true_peak: peak.to_vec(),
                        max: peak.to_vec(),
                        overs: vec![0; 4],
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
                                meta: SessionLevelsMeta {
                                    channels: ch.clone(),
                                    ballistic: Default::default(),
                                    pair_levels: Vec::new(),
                                },
                                peak: pick(&peak),
                                rms: pick(&rms),
                                clip: vec![ClipFlags::NONE; ch.len()],
                                ppm: pick(&peak),
                                true_peak: pick(&peak),
                                max: pick(&peak),
                                overs: vec![0; ch.len()],
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
    a.state.overlay.settings().map(|s| &s.session)
}

/// Whether the New measurement chooser is open (as a fresh session opens it).
fn new_meas_chooser(a: &App) -> bool {
    matches!(&a.state.overlay, Overlay::Choose(c)
        if c.purpose == ac2_ui::state::ChoicePurpose::NewMeasurement)
}
/// The pictures' local time: UTC+2, so the show starts at 19:02 on any machine.
const SHOW_ZONE: ac2_ui::scenes::LocalZone = ac2_ui::scenes::LocalZone::Fixed { offset_s: 7200 };

/// Steps the UI a few frames, with real time passing (the wheel's travel is smoothed over
/// frames).
fn settle(h: &mut Harness<'_, App>, frames: usize) {
    for _ in 0..frames {
        h.step();
        std::thread::sleep(Duration::from_millis(16));
    }
}

/// A primary-button drag from `from` to `to`, in a few moves.
fn drag(h: &mut Harness<'_, App>, from: egui::Pos2, to: egui::Pos2) {
    let button = |pos, pressed| Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.event(Event::PointerMoved(from));
    settle(h, 2);
    h.event(button(from, true));
    settle(h, 2);
    for k in 1..=6 {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 6.0)));
        settle(h, 1);
    }
    h.event(button(to, false));
    settle(h, 3);
}

/// Two clicks at `at` in quick succession.
fn double_click(h: &mut Harness<'_, App>, at: egui::Pos2) {
    let button = |pressed| Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.event(Event::PointerMoved(at));
    settle(h, 2);
    // The harness gives each queued event a frame of a quarter second, longer than egui's
    // 0.3 s between the clicks of a double click: both clicks go into one frame's input.
    for _ in 0..2 {
        h.input_mut().events.push(button(true));
        h.input_mut().events.push(button(false));
    }
    settle(h, 3);
}

/// Runs a palette command by typing `what`.
fn palette(h: &mut Harness<'_, App>, what: &str) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.event(Event::Text(what.into()));
    step_until(h, "palette typed", |a| {
        matches!(&a.state.overlay, Overlay::Palette(_))
    });
    h.key_press(Key::Enter);
    step_until(h, "palette closed", |a| a.state.overlay == Overlay::None);
}

/// Tab until the list's row of measurement `id` is selected: the stored traces listed
/// before it are rows Tab stops on.
fn tab_to(h: &mut Harness<'_, App>, id: MeasId) {
    for _ in 0..8 {
        h.key_press(Key::Tab);
        settle(h, 2);
        let s = &h.state().state;
        if s.selected == Some(id) && s.selected_trace.is_none() {
            return;
        }
    }
    panic!("Tab never reached {id:?}");
}
