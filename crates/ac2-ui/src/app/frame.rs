//! The frame: `eframe::App` for [`App`] (input, the panes, the dialogs, repaint pacing)
//! and the `--smoke` device-list check run from it.

use super::*;

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

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
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
        if self.startup.first_frame.is_some() {
            self.notify_frame(Instant::now());
        }
        if self.smoke {
            self.smoke_step(&ctx);
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
        let daemon_health = self.state.mirror.as_ref().and_then(|m| m.health.as_ref());
        let remote_host = daemon_health
            .is_some_and(|h| !self.link_embedded && h.host != self.health.local_host());
        let xruns = daemon_health.and_then(|h| h.audio).map(|a| a.xruns);
        if self.health_measured {
            self.health.frame(now, remote_host);
        }
        self.health.daemon_xruns(xruns, now);
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
        // The window's close button: with unsaved changes on an embedded daemon the
        // question comes first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.state.close_requested() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
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
            if self.state.rasterizer == ac2_plot::Rasterizer::Software {
                theme::drop_shadows(&ctx);
            }
            self.applied_theme = Some(self.state.theme);
        }
        view::draw(self, ui, &t);
        // After the panes, so the export is the scene this pass drew.
        if let Some(req) = self.state.pane_export.take() {
            let result = self.export_pane(frame.wgpu_render_state(), ctx.pixels_per_point(), &req);
            let (what, result) = match result {
                Ok(done) => (done, Ok(())),
                Err(e) => ("export pane".to_string(), Err(e)),
            };
            self.dispatch(Msg::Conn(Box::new(crate::conn::ConnEvent::Reply {
                what,
                result,
            })));
        }
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
        if let Some(t) = self.health.xrun_warning_until() {
            let now = Instant::now();
            if t > now {
                // The xrun count leaves its warning colour then.
                ctx.request_repaint_after(t - now);
            }
        }
        if self.state.animating()
            || self.state.bars_easing(Instant::now())
            || self.state.spectrum_gliding(Instant::now())
            || self.startup.first_frame.is_none()
        {
            if self.frame_interval.is_zero() {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(self.frame_interval);
            }
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
            .list
            .iter()
            .map(|t| t.until_s)
            .min_by(f64::total_cmp)
        {
            let wait = (next - self.state.now_s).max(0.0);
            ctx.request_repaint_after(Duration::from_secs_f64(wait));
        }
        // An idle, healthy app still draws often enough to keep the watchdog fed.
        if let Some(idle) = self.notify.max_idle() {
            ctx.request_repaint_after(idle);
        }
    }
}

/// How long `--smoke` waits for the device list: the link's first connect, the daemon's
/// start and a driver that takes seconds to load all fit with room to spare.
const SMOKE_LIMIT: Duration = Duration::from_secs(60);

impl App {
    /// `--smoke`: prints every device of the daemon's list once it arrives and quits, or
    /// gives up after [`SMOKE_LIMIT`]. The last line says which.
    fn smoke_step(&mut self, ctx: &egui::Context) {
        let done = if let Some(backends) = &self.state.devices {
            for b in backends {
                for d in &b.devices {
                    println!(
                        "smoke: device {:?} {:?}: {} in, {} out",
                        b.kind,
                        d.id.0,
                        d.input.as_ref().map_or(0, |i| i.max_channels),
                        d.output.as_ref().map_or(0, |o| o.max_channels)
                    );
                }
            }
            println!("smoke: ok");
            true
        } else if self.started.elapsed() > SMOKE_LIMIT {
            println!(
                "smoke: failed: no device list within {} s",
                SMOKE_LIMIT.as_secs()
            );
            true
        } else {
            // Nothing else may ask for a frame while the link connects.
            ctx.request_repaint_after(Duration::from_millis(100));
            false
        };
        if done {
            self.smoke = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
