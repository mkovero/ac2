//! Headless UI snapshots of recorded daemon state (`crate::rig`), rendered through egui-wgpu
//! with the plots in paint callbacks, exactly as on screen. What the app does with a daemon
//! (commands, replies, dialogs that wait on one) is tested against a real `ac2d` in `ac2-e2e`.
//!
//! Needs a wgpu adapter: locally `AC2_GPU_FALLBACK=1 WGPU_BACKEND=vulkan` (lavapipe). Without
//! one the tests print SKIP and pass, unless `AC2_REQUIRE_GPU=1`. References in
//! `tests/snapshots/` are blessed on lavapipe: `UPDATE_SNAPSHOTS=1 cargo test -p ac2-ui`.

use crate::common;
use crate::rig::Rig;

use std::time::{Duration, Instant};

use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::MeasId;
use ac2_scene::theme::ThemeName;
use ac2_ui::keys::Keymap;
use ac2_ui::state::{ConnState, Overlay, PaneKind, Severity};
use ac2_ui::{App, AppOptions};
use eframe::egui::{self, Event, Key, Modifiers};
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, SnapshotOptions};

mod drag;
mod eq;
mod overlays;
mod traces;
mod transfer;
mod views;
mod walk;

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

/// The app with no link of its own: a test's [`Rig`] feeds it (`crate::rig`).
fn options() -> AppOptions {
    AppOptions {
        target: None,
        target_addr: None,
        theme: ThemeName::Dark,
        keymap: Keymap::default(),
        keymap_path: Some("~/.config/ac2/keys.toml".into()),
        prefs: {
            let mut p = common::grid_ui_prefs();
            p.layout.panes = p.layout.panes.map(common::rig_picks);
            p
        },
        prefs_path: None,
        notices: vec![],
        started: Instant::now(),
        bench_startup: false,
        smoke: false,
        open_session_dialog: false,
        start: None,
        at_exit: None,
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
    // The 3D view's tier is the adapter's pick (a software rasteriser bare, Metal full), and
    // Settings › Display names it: pinned to the software one the references were taken on.
    h.state_mut().state.auto_tier = Some(ac2_scene::primitives::SkylineTier::Bare);
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
    // with a stream open, a screenshot waits for it.
    let named = |a: &App| a.state.open_stream().is_none() || a.state.rig.devices.is_some();
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
                a.state.rig.conn, a.state.overlay, a.state.rig.stimulus.phase
            );
        }
        step(h);
        std::thread::sleep(Duration::from_millis(10));
    }
    // A few more passes so layout settles and the plots are prepared.
    for _ in 0..3 {
        step(h);
    }
}

/// One pass, after the recorded rigs alive on this thread fed the app what a link would
/// have delivered since the last one ([`crate::rig::pump`]).
fn step(h: &mut Harness<'_, App>) {
    crate::rig::pump(h.state_mut());
    h.step();
}

/// The link is healthy as of `now`: responding, and every frame shown fresh. These are
/// wall-clock indications (STALE, "not responding", the dimmed curves) that a picture
/// would otherwise catch in whichever state a stalled thread left them.
fn healthy(app: &App, now: Instant) -> bool {
    let st = &app.state;
    let link = match &st.rig.conn {
        ConnState::Connected { .. } => st.rig.mirror.as_ref().is_some_and(|m| m.responding(now)),
        _ => true,
    };
    let fresh = st.rig.data.as_ref().is_none_or(|d| {
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
    st.rig.data.as_ref().map_or_else(Vec::new, |d| {
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
        step(h);
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
    let synced = st.rig.mirror.as_ref().is_some_and(|m| m.synced());
    let have = |m: u32, s: Stream| {
        st.rig.data.as_ref().is_some_and(|d| {
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
    matches!(st.rig.conn, ConnState::Connected { .. })
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

/// Steps the UI a few frames, with real time passing (the wheel's travel is smoothed over
/// frames).
fn settle(h: &mut Harness<'_, App>, frames: usize) {
    for _ in 0..frames {
        step(h);
        std::thread::sleep(Duration::from_millis(16));
    }
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

/// Measurement `id`'s row dropped in the middle of the pane of `kind` focused last, as the
/// mouse would: what a pane shows is what was dropped on it, never the selection.
fn drop_on_kind(h: &mut Harness<'_, App>, kind: PaneKind, id: MeasId) {
    use ac2_scene::meas_list::TreeKey;
    use ac2_ui::state::{DragItem, DragMsg, DropTarget, Msg};
    let a = h.state_mut();
    let pane = common::pane(&a.state, kind);
    let rig = a.state.active();
    let key = TreeKey::Meas(id);
    a.dispatch(Msg::RowDrag(DragMsg::Start(DragItem::Row { rig, key })));
    a.dispatch(Msg::RowDrag(DragMsg::Over(Some(DropTarget {
        pane,
        edge: None,
    }))));
    a.dispatch(Msg::RowDrag(DragMsg::Drop));
    settle(h, 2);
}
