//! The eframe app: wires egui input to the reducer, the reducer's requests to the link, and
//! the state to the views. Drawing lives in [`crate::view`]; state changes in
//! [`crate::state`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_plot::Scene;
use ac2_scene::theme::{Theme, ThemeName};
use eframe::egui::{self, Event, Key};

use ac2_proto::topic::{Stream, Topic};

use crate::conn::{Conn, DataSnapshot, Target};
use crate::connect::{Choice, ConnectDialog};
use crate::embedded::{Embedded, EmbeddedBackend, start_embedded};
use crate::keys::{Chord, Keymap};
use crate::state::{AppState, Msg, Overlay, PaneKind};
use crate::{theme, view};

/// How the app was started.
pub struct AppOptions {
    /// `None`: no daemon link (UI tests of chrome only).
    pub target: Option<Target>,
    pub theme: ThemeName,
    pub keymap: Keymap,
    /// Where the keymap came from, for the help overlay.
    pub keymap_path: Option<std::path::PathBuf>,
    /// Preferences read at start.
    pub prefs: crate::prefs::UiPrefs,
    /// Where changed preferences are saved; `None` keeps them in memory (tests).
    pub prefs_path: Option<std::path::PathBuf>,
    /// Shown once as an error toast (bad `keys.toml`, embedded daemon unavailable…).
    pub notices: Vec<String>,
    /// Process start, for the startup measurement.
    pub started: Instant,
    /// Print the first-frame time and quit.
    pub bench_startup: bool,
    /// Open the session dialog once connected if the daemon has no audio session (an
    /// embedded daemon on real audio starts without one).
    pub open_session_dialog: bool,
    /// This client's key, shown on the Connection page of Settings.
    pub client_key: Option<crate::settings::ClientKey>,
    /// What the connect dialog needs, so Settings can open it again; `None`: it cannot.
    pub connect: Option<ConnectSetup>,
}

/// What the connect dialog is made from (Settings › Connection opens it while running).
#[derive(Clone, Debug)]
pub struct ConnectSetup {
    pub key_dir: std::path::PathBuf,
    pub client_name: String,
    pub backends: Vec<EmbeddedBackend>,
    /// Browse the network for rigs.
    pub discovery: bool,
}

impl ConnectSetup {
    /// A dialog as the app starts with one.
    pub fn dialog(&self) -> ConnectDialog {
        let mdns = ac2_discovery::Options::default();
        ConnectDialog::new(
            ac2_client::KeyDir::new(self.key_dir.clone()),
            self.client_name.clone(),
            self.backends.clone(),
            crate::connect::local_daemon_running(),
            self.discovery.then_some(&mdns),
        )
    }
}

impl std::fmt::Debug for AppOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppOptions")
            .field("theme", &self.theme)
            .field("bench_startup", &self.bench_startup)
            .finish_non_exhaustive()
    }
}

/// Startup timing: process start → first `ui` pass, and → the pass after the first frame was
/// presented.
#[derive(Clone, Copy, Debug, Default)]
pub struct StartupTiming {
    pub first_ui: Option<Duration>,
    pub first_frame: Option<Duration>,
}

/// Cached pane scene: rebuilt when its pane's generation ([`App::pane_generation`]), size or
/// theme changes.
pub(crate) struct CachedScene {
    pub generation: u64,
    pub size: egui::Vec2,
    pub theme: ThemeName,
    pub scene: Arc<Scene>,
    /// The frequency axis mapping of the scene (TF / spectrum), for mouse navigation.
    pub x_axis: Option<ac2_scene::axis::Mapping>,
    /// The level axis mapping in dB (transfer magnitude, spectrum, distortion in dB), for
    /// zooming about the pointer.
    pub y_level: Option<ac2_scene::axis::Mapping>,
    /// The spectrograph's time axis, when the spectrum pane shows it.
    pub time_axis: Option<ac2_scene::axis::Mapping>,
    /// An IR picture's time and value axes, and which picture it is.
    pub ir_axes: Option<(
        ac2_scene::view::IrPane,
        ac2_scene::axis::Mapping,
        ac2_scene::axis::Mapping,
    )>,
    /// Where on the scene the level axis's unit is drawn and what it means, for a tooltip.
    pub unit_tip: Option<(egui::Rect, String)>,
    /// Where the scene put its legend, for moving, resizing and scrolling it.
    pub legend: Option<ac2_scene::legend::LegendBox>,
}

/// A drag on the legend: where on the plate (or its grip) the pointer took it, so the
/// plate keeps that point under the pointer.
#[derive(Clone, Copy, Debug)]
pub(crate) enum LegendDrag {
    /// From its top-left corner.
    Move { grab: egui::Vec2 },
    /// From its bottom-right corner.
    Resize { grab: egui::Vec2 },
}

pub struct App {
    pub state: AppState,
    pub keymap: Keymap,
    pub(crate) keymap_path: Option<std::path::PathBuf>,
    prefs_path: Option<std::path::PathBuf>,
    conn: Option<Conn>,
    started: Instant,
    last_tick: Option<Instant>,
    /// When `ui.toml` was last written ([`PREFS_SAVE_INTERVAL`]).
    prefs_saved: Option<Instant>,
    pub startup: StartupTiming,
    bench_startup: bool,
    applied_theme: Option<ThemeName>,
    /// The full-screen state last sent to the window.
    applied_fullscreen: bool,
    /// Counts state changes; each pane remembers the count of the last change it shows.
    generation: u64,
    pane_generations: HashMap<PaneKind, u64>,
    /// The time-driven texts the panes were last built with ([`ClockTexts`]).
    clock: ClockTexts,
    pub(crate) scenes: HashMap<PaneKind, CachedScene>,
    pub(crate) legend_drag: Option<LegendDrag>,
    /// Where the transfer legend's plate and grip were last drawn, on screen.
    pub(crate) legend_screen: Option<(egui::Rect, egui::Rect)>,
    /// Wheel travel over the legend not yet a whole row.
    pub(crate) legend_wheel: f32,
    pub(crate) plots: bool,
    passes: u64,
    /// The connect dialog, while open: the app has no link until the operator picks one.
    connect: Option<ConnectDialog>,
    /// A daemon hosted in this process, chosen in the connect dialog. Declared after `conn`
    /// so the link (and the stimulus it may hold) goes first.
    embedded: Option<Embedded>,
    /// The window's size and position as last seen in a normal (not full-screen, not
    /// maximised) state: saved with the preferences on exit.
    window: Option<crate::prefs::WindowPrefs>,
    /// Whether the restored window size was checked against the screen.
    window_checked: bool,
    /// What the connect dialog is made from.
    connect_setup: Option<ConnectSetup>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("passes", &self.passes)
            .finish_non_exhaustive()
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, opts: AppOptions) -> Self {
        let plots = match &cc.wgpu_render_state {
            Some(rs) => {
                crate::plot::install(rs);
                true
            }
            None => false,
        };
        install_fonts(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let describe = opts
            .target
            .as_ref()
            .map_or_else(|| "no daemon".to_string(), |t| t.describe.clone());
        let conn = opts
            .target
            .and_then(|t| Conn::start(t, link_wake(ctx)).ok());
        let mut state = AppState::new(opts.theme, describe);
        state.set_prefs(opts.prefs);
        state.open_session_when_empty = opts.open_session_dialog;
        state.client_key = opts.client_key;
        for n in opts.notices {
            state.update(
                Msg::Conn(Box::new(crate::conn::ConnEvent::Reply {
                    what: "startup".into(),
                    result: Err(n),
                })),
                &opts.keymap,
            );
        }
        Self {
            state,
            keymap: opts.keymap,
            keymap_path: opts.keymap_path,
            prefs_path: opts.prefs_path,
            conn,
            started: opts.started,
            last_tick: None,
            prefs_saved: None,
            startup: StartupTiming::default(),
            bench_startup: opts.bench_startup,
            applied_theme: None,
            applied_fullscreen: false,
            generation: 0,
            pane_generations: HashMap::new(),
            clock: ClockTexts::default(),
            scenes: HashMap::new(),
            legend_drag: None,
            legend_screen: None,
            legend_wheel: 0.0,
            plots,
            passes: 0,
            connect: None,
            embedded: None,
            window: None,
            window_checked: false,
            connect_setup: opts.connect,
        }
    }

    /// Follows the window's geometry, and once, makes a restored window fit its screen
    /// (the screen it was saved on may be gone or smaller now).
    fn follow_window(&mut self, ctx: &egui::Context) {
        let (inner, outer, fullscreen, maximized, monitor) = ctx.input(|i| {
            let v = i.viewport();
            (
                v.inner_rect,
                v.outer_rect,
                v.fullscreen,
                v.maximized,
                v.monitor_size,
            )
        });
        if !self.window_checked
            && let (Some(m), Some(r)) = (monitor, inner)
            && m.x > 0.0
            && m.y > 0.0
        {
            self.window_checked = true;
            if r.width() > m.x || r.height() > m.y {
                let size = egui::vec2(
                    (m.x * 0.9).max(crate::prefs::WindowPrefs::MIN.0 as f32),
                    (m.y * 0.9).max(crate::prefs::WindowPrefs::MIN.1 as f32),
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                    m.x * 0.05,
                    m.y * 0.05,
                )));
            }
        }
        if fullscreen == Some(true) || maximized == Some(true) {
            return;
        }
        if let Some(r) = inner {
            let (w, h) = (r.width().round(), r.height().round());
            let min = crate::prefs::WindowPrefs::MIN;
            if w >= min.0 as f32 && h >= min.1 as f32 {
                self.window = Some(crate::prefs::WindowPrefs {
                    width: w as u32,
                    height: h as u32,
                    pos: outer.map(|o| (o.min.x.round() as i32, o.min.y.round() as i32)),
                });
            }
        }
    }

    /// Shows the connect dialog; the choice replaces the current link.
    pub fn open_connect(&mut self, dialog: ConnectDialog) {
        self.connect = Some(dialog);
    }

    /// Whether the connect dialog is open.
    pub fn connect_open(&self) -> bool {
        self.connect.is_some()
    }

    fn connect_to(&mut self, ctx: &egui::Context, choice: Choice) {
        let Some(dialog) = self.connect.as_mut() else {
            return;
        };
        let target = match choice {
            Choice::Target(t) => *t,
            Choice::Embedded(b) => match start_embedded(b) {
                Ok(e) => {
                    let t = Target {
                        config: e.client_config(dialog.client_name()),
                        describe: e.describe(),
                    };
                    self.conn = None;
                    self.embedded = Some(e);
                    // Real audio: the operator picks the interface and channels. The
                    // simulated rig is already measuring.
                    self.state.open_session_when_empty = b != EmbeddedBackend::Fake;
                    if b == EmbeddedBackend::Fake {
                        self.dispatch(Msg::Conn(Box::new(crate::conn::ConnEvent::Reply {
                            what: "simulated rig: session open, \"demo\" measuring · L types a \
                                   level, Space arms, Enter fires"
                                .into(),
                            result: Ok(()),
                        })));
                    }
                    t
                }
                Err(e) => {
                    // Never a silent switch to another daemon: the operator chooses again.
                    dialog.error = Some(format!("embedded daemon: {e}"));
                    return;
                }
            },
        };
        self.conn = None;
        match Conn::start(target, link_wake(ctx.clone())) {
            Ok(c) => {
                self.conn = Some(c);
                self.connect = None;
            }
            Err(e) => dialog_error(&mut self.connect, format!("link thread: {e}")),
        }
    }

    /// Marks the state as changed, so cached pane scenes are rebuilt on the next pass. Only
    /// needed after editing `state` directly instead of through [`App::dispatch`].
    pub fn state_edited(&mut self) {
        self.touch(&PaneKind::ALL);
    }

    /// The count of the last state change pane `p` shows.
    pub(crate) fn pane_generation(&self, p: PaneKind) -> u64 {
        self.pane_generations.get(&p).copied().unwrap_or(0)
    }

    /// Where the transfer legend's plate and its resize grip were last drawn, on screen
    /// (what a test points the mouse at).
    pub fn legend_rects(&self) -> Option<(egui::Rect, egui::Rect)> {
        self.legend_screen
    }

    /// Marks `panes` as changed: their scenes are rebuilt on the next pass.
    fn touch(&mut self, panes: &[PaneKind]) {
        if panes.is_empty() {
            return;
        }
        self.generation += 1;
        for p in panes {
            self.pane_generations.insert(*p, self.generation);
        }
    }

    /// Feeds one message through the reducer and forwards its requests.
    pub fn dispatch(&mut self, msg: Msg) {
        let what = Touches::of(&msg);
        let animating = self.state.animating();
        let data = self.state.data.clone();
        let mirror = self.state.mirror.clone();
        let mut reqs = self.state.update(msg, &self.keymap);
        reqs.extend(self.state.sync_link());
        let panes = match what {
            // The frequency axis moves while navigation settles; the last step settles it.
            Touches::Tick if animating || self.state.animating() => {
                vec![PaneKind::Transfer, PaneKind::Spectrum, PaneKind::Distortion]
            }
            Touches::Tick | Touches::Toasts => Vec::new(),
            Touches::Data => data_touches(data.as_deref(), self.state.data.as_deref()),
            Touches::Mirror => match (&mirror, &self.state.mirror) {
                (Some(a), Some(b)) if !crate::conn::mirror_differs(a, b) => Vec::new(),
                _ => PaneKind::ALL.to_vec(),
            },
            Touches::Spl => vec![PaneKind::Spl],
            Touches::Legend => vec![PaneKind::Transfer],
            Touches::All => PaneKind::ALL.to_vec(),
        };
        self.touch(&panes);
        if let Some(c) = &self.conn {
            for r in reqs {
                c.send(r);
            }
        }
        self.save_prefs(Instant::now());
    }

    /// Writes `ui.toml` when a preference changed, at most once per
    /// [`PREFS_SAVE_INTERVAL`]; `Some(wait)` while a write waits.
    fn save_prefs(&mut self, now: Instant) -> Option<Duration> {
        if !self.state.prefs_dirty {
            return None;
        }
        if let Some(t) = self.prefs_saved
            && now.duration_since(t) < PREFS_SAVE_INTERVAL
        {
            return Some(PREFS_SAVE_INTERVAL - now.duration_since(t));
        }
        self.state.prefs_dirty = false;
        self.prefs_saved = Some(now);
        // A few hundred bytes, written only when the operator changes a preference.
        if let Some(path) = &self.prefs_path
            && let Err(e) = self.state.prefs.save(path)
        {
            self.dispatch(Msg::Conn(Box::new(crate::conn::ConnEvent::Reply {
                what: "preferences".into(),
                result: Err(e),
            })));
        }
        None
    }

    fn pump_link(&mut self) {
        let events = self.conn.as_ref().map(Conn::drain).unwrap_or_default();
        for e in events {
            self.dispatch(Msg::Conn(Box::new(e)));
        }
    }

    /// Keyboard and text events → reducer. Handled events are removed so egui widgets never
    /// also act on them (Space must not click a focused button, Tab must not move focus).
    fn input(&mut self, ctx: &egui::Context) {
        let text_overlay = matches!(
            self.state.overlay,
            Overlay::Palette(_) | Overlay::Prompt(_) | Overlay::Form(_) | Overlay::Settings(_)
        );
        let events = ctx.input_mut(|i| {
            let (mine, rest): (Vec<Event>, Vec<Event>) =
                std::mem::take(&mut i.events).into_iter().partition(|e| {
                    matches!(e, Event::Key { .. }) || (matches!(e, Event::Text(_)) && text_overlay)
                });
            i.events = rest;
            mine
        });
        for e in events {
            match e {
                Event::Key {
                    key,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } => {
                    let open = self.state.overlay != Overlay::None;
                    let overlay = open && !self.state.overlay.is_reading();
                    if overlay && key == Key::Backspace {
                        // Held, it keeps erasing typed text; where Backspace is Delete (the
                        // delete confirmation), one held press must not open and answer it.
                        if !repeat || text_overlay {
                            self.dispatch(Msg::Backspace);
                        }
                        continue;
                    }
                    // Auto-repeat only for navigation; never for stimulus or toggles, so a
                    // held key cannot ramp the level or flicker a mode. In an open window
                    // ↑/↓ and the page keys only move or scroll it (the window owns them).
                    // Ctrl+↑/↓ move the level axis (never the stimulus level, which is ↑/↓
                    // without Ctrl).
                    let nav = matches!(key, Key::ArrowLeft | Key::ArrowRight)
                        || (open && matches!(key, Key::PageUp | Key::PageDown))
                        || (matches!(key, Key::ArrowUp | Key::ArrowDown)
                            && (open || (modifiers.command && !modifiers.alt)));
                    if repeat && !nav {
                        continue;
                    }
                    self.dispatch(Msg::Key(Chord::from_event(key, modifiers)));
                }
                Event::Text(t) => self.dispatch(Msg::Text(t)),
                _ => {}
            }
        }
    }

    pub fn theme(&self) -> Theme {
        Theme::by_name(self.state.theme)
    }
}

/// Which panes a message can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Touches {
    /// The frame clock: only navigation in motion.
    Tick,
    /// New frames: the panes drawing the topics whose picture changed.
    Data,
    /// A mirror view: every pane, unless only keepalive time moved.
    Mirror,
    /// The SPL meter's Leq history.
    Spl,
    /// The toasts' hover and dismissal: no pane.
    Toasts,
    /// The mouse on the legend: the transfer pane only, rebuilt on every step of a drag.
    Legend,
    /// Keys, commands, replies: anything may change.
    All,
}

impl Touches {
    fn of(msg: &Msg) -> Self {
        use crate::conn::ConnEvent;
        match msg {
            Msg::Tick { .. } => Self::Tick,
            Msg::ToastsHeld(_) | Msg::DismissToast(_) => Self::Toasts,
            Msg::Legend(_) => Self::Legend,
            Msg::Conn(e) => match **e {
                ConnEvent::Data(_) => Self::Data,
                ConnEvent::Mirror(_) => Self::Mirror,
                ConnEvent::LeqBackfill { .. } => Self::Spl,
                _ => Self::All,
            },
            _ => Self::All,
        }
    }
}

/// The panes that draw `topic`. Input meters are drawn by the chrome, outside any pane.
fn topic_panes(topic: &Topic) -> &'static [PaneKind] {
    match topic {
        Topic::Data { stream, .. } => match stream {
            Stream::Tf => &[PaneKind::Transfer],
            Stream::Ir => &[PaneKind::Ir],
            Stream::Spec | Stream::Rta => &[PaneKind::Spectrum],
            Stream::Spl | Stream::Leq | Stream::BandLeq => &[PaneKind::Spl],
            Stream::Levels => &[],
        },
        _ => &[],
    }
}

/// The panes whose picture differs between snapshots `old` and `new`: a topic coming or
/// going, new content, a STALE flip; every pane when the daemon's liveness flips.
fn data_touches(old: Option<&DataSnapshot>, new: Option<&DataSnapshot>) -> Vec<PaneKind> {
    let (Some(old), Some(new)) = (old, new) else {
        return PaneKind::ALL.to_vec();
    };
    if old.latest.responding != new.latest.responding {
        return PaneKind::ALL.to_vec();
    }
    let mut out: Vec<PaneKind> = Vec::new();
    let mut add = |t: &Topic| {
        for p in topic_panes(t) {
            if !out.contains(p) {
                out.push(*p);
            }
        }
    };
    for (k, n) in &new.latest.frames {
        let changed = old.latest.frames.get(k).is_none_or(|o| {
            o.stale != n.stale
                || (o.frame.stamp.seq != n.frame.stamp.seq
                    && !crate::conn::same_picture(&o.frame, &n.frame))
        });
        if changed {
            add(&n.topic);
        }
    }
    for (k, o) in &old.latest.frames {
        if !new.latest.frames.contains_key(k) {
            add(&o.topic);
        }
    }
    out
}

/// The panes' texts that change with time alone: the age of each STALE frame and how long
/// the daemon has been silent. Panes are rebuilt when these change, and the UI wakes when
/// they will.
#[derive(Clone, Debug, Default, PartialEq)]
struct ClockTexts {
    texts: Vec<String>,
}

impl ClockTexts {
    /// The texts as of `now`, with how long until one of them changes.
    fn at(st: &AppState, now: Instant) -> (Self, Option<Duration>) {
        use ac2_scene::format::{age, age_changes_in, age_step};
        let mut texts = Vec::new();
        let mut changes: Vec<(f64, f64)> = Vec::new();
        // The AUDIO STOPPED countdown ticks by the second.
        if let Some(stopped) = st.daemon().and_then(|d| d.session.stopped.as_ref()) {
            let wall = crate::scenes::daemon_wall(st, crate::view::now().wall);
            let in_s = ac2_scene::audio::changes_in(stopped, wall);
            texts.push(format!("{}", wall.0 / 1_000_000_000));
            changes.push((in_s, 1.0));
        }
        let mut count = |a: f64, texts: &mut Vec<String>| {
            texts.push(age(a));
            changes.push((age_changes_in(a), age_step(a)));
        };
        if let Some(d) = &st.data {
            for f in d.latest.frames.values() {
                let fresh = crate::scenes::freshness(st, f);
                // Only a STALE tag counts its age; an audio-stopped curve says so without one.
                if matches!(fresh, ac2_scene::time::Freshness::Stale { .. })
                    && matches!(f.topic, Topic::Data { .. })
                {
                    count(fresh.age_s(), &mut texts);
                }
            }
        }
        // A calibration's age in the SPL readout counts in minutes: ten-second steps keep
        // it within a sixth of its last digit.
        if st.layout.visible().contains(&PaneKind::Spl) {
            texts.push(format!(
                "{}",
                (st.now_s / SLOW_REFRESH.as_secs_f64()).floor()
            ));
        }
        if let Some(t) = st.mirror.as_ref().and_then(|m| m.last_ka) {
            let silence = now.saturating_duration_since(t).as_secs_f64();
            if silence > ac2_scene::time::DAEMON_SILENT_AFTER_S {
                count(silence, &mut texts);
            }
        }
        // Frames stopped at different moments (a once-a-second Leq frame, say) count their
        // ages out of step: one pass takes every change due within half a step of the
        // first, so each text is at most half its last digit late. Just past the last of
        // them, so the pass that wakes reads the new texts.
        let (first, step) = changes
            .iter()
            .copied()
            .fold((f64::INFINITY, 0.0), |a, c| if c.0 < a.0 { c } else { a });
        let last = changes
            .iter()
            .map(|c| c.0)
            .filter(|c| *c <= first + step / 2.0)
            .fold(first, f64::max);
        let wait = last
            .is_finite()
            .then(|| Duration::from_secs_f64(last) + Duration::from_millis(2));
        (Self { texts }, wait)
    }
}

/// Repaint at least this often while connected, for the coarse clock texts outside the
/// panes' banners ("saved 3 min ago", a calibration's age).
const SLOW_REFRESH: Duration = Duration::from_secs(10);

/// Shortest time between two writes of `ui.toml`: a wheel or drag on a level axis changes
/// it on every event, and each write is synced to disk.
const PREFS_SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// Wakes the UI for what the link reports: one pass. egui answers a plain
/// `request_repaint` with two passes (for responses that land a frame late), which on a
/// software rasteriser doubles the cost of every data frame; a delayed request is painted
/// once, and a nanosecond is no delay at all.
fn link_wake(ctx: egui::Context) -> crate::conn::Wake {
    Arc::new(move || ctx.request_repaint_after(Duration::from_nanos(1)))
}

fn dialog_error(d: &mut Option<ConnectDialog>, msg: String) {
    if let Some(d) = d {
        d.error = Some(msg);
    }
}

/// Chrome text in the plots' bundled font (Inter), so panels and plots share one typeface and
/// arrows, minus signs and γ² render the same everywhere; egui's fonts stay as fallback.
pub(crate) fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(egui::FontData::from_static(ac2_plot::FONT_DATA)),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".into());
    ctx.set_fonts(fonts);
}

impl eframe::App for App {
    /// The window's geometry goes into the preferences on the way out (the layout already
    /// did, whenever it changed).
    fn on_exit(&mut self) {
        if self.window.is_some() && self.window != self.state.prefs.window {
            self.state.prefs.window = self.window;
            self.state.prefs_dirty = true;
        }
        // Also a change still waiting for its write.
        if std::mem::take(&mut self.state.prefs_dirty)
            && let Some(path) = &self.prefs_path
            && let Err(e) = self.state.prefs.save(path)
        {
            eprintln!("ac2-ui: {e}");
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.passes += 1;
        if self.startup.first_ui.is_none() {
            self.startup.first_ui = Some(self.started.elapsed());
        } else if self.startup.first_frame.is_none() {
            self.startup.first_frame = Some(self.started.elapsed());
            if self.bench_startup {
                let t = self.startup;
                println!(
                    "startup: first ui pass {:.1} ms, first frame presented {:.1} ms (target < 300 ms)",
                    t.first_ui.unwrap_or_default().as_secs_f64() * 1e3,
                    t.first_frame.unwrap_or_default().as_secs_f64() * 1e3
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        let now = Instant::now();
        let dt = self
            .last_tick
            .map_or(0.0, |t| now.duration_since(t).as_secs_f64())
            .min(0.1);
        self.last_tick = Some(now);
        self.dispatch(Msg::Tick {
            now_s: now.duration_since(self.started).as_secs_f64(),
            dt_s: dt,
        });
        self.pump_link();
        // Ages keep counting between the link's snapshots.
        let now = Instant::now();
        if let Some(d) = &self.state.data
            && d.drained < now
        {
            self.state.data = Some(Arc::new(d.aged(now)));
        }
        let (clock, clock_wait) = ClockTexts::at(&self.state, now);
        if clock != self.clock {
            self.clock = clock;
            self.state_edited();
        }
        // The dialog owns the keyboard while open (text entry); there is no link to drive.
        if self.connect.is_none() {
            self.input(&ctx);
            // The keys reach the reducer, which keeps each dialog's focus itself; egui's own
            // keyboard focus would only wander: Tab and the arrows move it from widget to
            // widget across the window (an open dialog's buttons to the sidebar's chips)
            // whatever is open, and highlight what no key acts on. egui reads those keys at
            // the start of the pass, before the reducer takes them; here, before any widget
            // is laid out, the move is cancelled and no widget keeps the focus.
            ctx.memory_mut(|m| {
                m.move_focus(egui::FocusDirection::None);
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
        }
        if self.state.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if std::mem::take(&mut self.state.want_connect_dialog) {
            match &self.connect_setup {
                Some(c) => self.connect = Some(c.dialog()),
                None => self.dispatch(Msg::Conn(Box::new(crate::conn::ConnEvent::Reply {
                    what: "connect dialog".into(),
                    result: Err("not available in this window".into()),
                }))),
            }
        }
        self.follow_window(&ctx);
        if self.applied_fullscreen != self.state.fullscreen {
            self.applied_fullscreen = self.state.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.state.fullscreen));
        }

        let t = self.theme();
        if self.applied_theme != Some(self.state.theme) {
            theme::apply(&ctx, &t);
            self.applied_theme = Some(self.state.theme);
        }
        view::draw(self, ui, &t);
        if let Some(d) = self.connect.as_mut()
            && let Some(choice) = d.show(&ctx, &theme::chrome(&t))
        {
            self.connect_to(&ctx, choice);
        }

        if let Some(w) = clock_wait {
            // egui wakes a predicted frame time early; the texts must have changed by then.
            let early = Duration::from_secs_f32(ctx.input(|i| i.predicted_dt).max(0.0));
            ctx.request_repaint_after(w + early);
        }
        if self.conn.is_some() {
            ctx.request_repaint_after(SLOW_REFRESH);
        }
        if let Some(wait) = self.save_prefs(Instant::now()) {
            ctx.request_repaint_after(wait);
        }
        if self.state.animating() || self.startup.first_frame.is_none() {
            ctx.request_repaint();
        } else if self.state.overlay.session().is_some()
            || self
                .state
                .overlay
                .settings()
                .is_some_and(|s| s.page == crate::settings::Page::Connection)
        {
            // The device preview is renewed, and the server info asked again, from the frame
            // tick, also when no frame arrives to wake the UI.
            ctx.request_repaint_after(Duration::from_millis(500));
        } else if self.state.operation().is_some() {
            // The progress bar and time left move between the daemon's step reports.
            ctx.request_repaint_after(Duration::from_millis(200));
        } else if let Some(next) = self
            .state
            .toasts
            .iter()
            .map(|t| t.until_s)
            .min_by(f64::total_cmp)
        {
            let wait = (next - self.state.now_s).max(0.0);
            ctx.request_repaint_after(Duration::from_secs_f64(wait));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{ClipFlags, LevelsMeta, SessionLevelsFrame};
    use ac2_proto::units::MeasId;
    use ac2_proto::{Frame, FrameData};

    use super::*;

    fn frame(topic: Topic, seq: u64, peak: f32, stale: bool) -> TopicFrame {
        let mut stamp = ac2_proto::samples::stamp(None);
        stamp.seq = seq;
        TopicFrame {
            topic,
            frame: Arc::new(Frame {
                stamp,
                data: FrameData::SessionLevels(SessionLevelsFrame {
                    meta: LevelsMeta { channels: vec![0] },
                    peak: vec![peak],
                    rms: vec![-30.0],
                    clip: vec![ClipFlags::NONE],
                }),
            }),
            received: Instant::now(),
            since_new: Duration::ZERO,
            age: Some(0.0),
            stale,
        }
    }

    fn snap(frames: &[TopicFrame]) -> DataSnapshot {
        DataSnapshot {
            latest: Latest {
                frames: frames
                    .iter()
                    .map(|f| (f.topic.to_string().into(), f.clone()))
                    .collect(),
                ..Latest::default()
            },
            grids: BTreeMap::new(),
            drained: Instant::now(),
        }
    }

    fn data(meas: u32, stream: Stream) -> Topic {
        Topic::Data {
            meas: MeasId(meas),
            stream,
        }
    }

    /// Only the panes drawing a changed topic are rebuilt: new content, a STALE flip, a
    /// topic gone; a new `seq` with the same content, and the input meters, rebuild none.
    #[test]
    fn new_frames_touch_the_panes_that_draw_them() {
        let tf = data(1, Stream::Tf);
        let spl = data(4, Stream::Spl);
        let old = snap(&[
            frame(tf, 1, -10.0, false),
            frame(spl, 1, -10.0, false),
            frame(Topic::SessionLevels, 1, -10.0, false),
        ]);
        let same = snap(&[
            frame(tf, 2, -10.0, false),
            frame(spl, 2, -10.0, false),
            frame(Topic::SessionLevels, 2, -11.0, false),
        ]);
        assert!(data_touches(Some(&old), Some(&same)).is_empty());
        let tf_new = snap(&[
            frame(tf, 2, -12.0, false),
            frame(spl, 1, -10.0, false),
            frame(Topic::SessionLevels, 1, -10.0, false),
        ]);
        assert_eq!(
            data_touches(Some(&old), Some(&tf_new)),
            vec![PaneKind::Transfer]
        );
        let spl_stale = snap(&[frame(tf, 1, -10.0, false), frame(spl, 1, -10.0, true)]);
        assert_eq!(
            data_touches(Some(&old), Some(&spl_stale)),
            vec![PaneKind::Spl]
        );
        assert_eq!(data_touches(None, Some(&old)), PaneKind::ALL.to_vec());
    }
}
