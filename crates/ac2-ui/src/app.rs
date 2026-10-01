//! The eframe app: wires egui input to the reducer, the reducer's requests to the link, and
//! the state to the views. Drawing lives in [`crate::view`]; state changes in
//! [`crate::state`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_plot::Scene;
use ac2_scene::theme::{Theme, ThemeName};
use eframe::egui::{self, Event, Key};

use crate::conn::{Conn, Target};
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
    /// Shown once as an error toast (bad `keys.toml`, embedded daemon unavailable…).
    pub notices: Vec<String>,
    /// Process start, for the startup measurement.
    pub started: Instant,
    /// Print the first-frame time and quit.
    pub bench_startup: bool,
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

/// Cached pane scene: rebuilt when the state generation, size or theme changes.
pub(crate) struct CachedScene {
    pub generation: u64,
    pub size: egui::Vec2,
    pub theme: ThemeName,
    pub scene: Arc<Scene>,
    /// The frequency axis mapping of the scene (TF / spectrum), for mouse navigation.
    pub x_axis: Option<ac2_scene::axis::Mapping>,
}

pub struct App {
    pub state: AppState,
    pub keymap: Keymap,
    pub(crate) keymap_path: Option<std::path::PathBuf>,
    conn: Option<Conn>,
    started: Instant,
    last_tick: Option<Instant>,
    pub startup: StartupTiming,
    bench_startup: bool,
    applied_theme: Option<ThemeName>,
    pub(crate) generation: u64,
    pub(crate) scenes: HashMap<PaneKind, CachedScene>,
    pub(crate) plots: bool,
    passes: u64,
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
        let conn = opts.target.and_then(|t| {
            let wake = Arc::new(move || ctx.request_repaint());
            Conn::start(t, wake).ok()
        });
        let mut state = AppState::new(opts.theme, describe);
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
            conn,
            started: opts.started,
            last_tick: None,
            startup: StartupTiming::default(),
            bench_startup: opts.bench_startup,
            applied_theme: None,
            generation: 0,
            scenes: HashMap::new(),
            plots,
            passes: 0,
        }
    }

    /// Feeds one message through the reducer and forwards its requests.
    pub fn dispatch(&mut self, msg: Msg) {
        let is_tick = matches!(msg, Msg::Tick { .. });
        let animating = self.state.animating();
        let reqs = self.state.update(msg, &self.keymap);
        if !is_tick || animating {
            self.generation += 1;
        }
        if let Some(c) = &self.conn {
            for r in reqs {
                c.send(r);
            }
        }
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
        let text_overlay = matches!(self.state.overlay, Overlay::Palette(_) | Overlay::Prompt(_));
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
                    let overlay = !matches!(self.state.overlay, Overlay::None | Overlay::Help);
                    if overlay && key == Key::Backspace {
                        self.dispatch(Msg::Backspace);
                        continue;
                    }
                    // Auto-repeat only for navigation; never for stimulus or toggles, so a
                    // held key cannot ramp the level or flicker a mode.
                    let nav = matches!(key, Key::ArrowLeft | Key::ArrowRight)
                        || (overlay && matches!(key, Key::ArrowUp | Key::ArrowDown));
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

/// Chrome text in the plots' bundled font (Inter), so panels and plots share one typeface and
/// arrows, minus signs and γ² render the same everywhere; egui's fonts stay as fallback.
fn install_fonts(ctx: &egui::Context) {
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
        self.input(&ctx);
        if self.state.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        let t = self.theme();
        if self.applied_theme != Some(self.state.theme) {
            theme::apply(&ctx, &t);
            self.applied_theme = Some(self.state.theme);
        }
        view::draw(self, ui, &t);

        if self.state.animating() || self.startup.first_frame.is_none() {
            ctx.request_repaint();
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
