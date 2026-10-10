//! The Settings view: the whole window below the top bar (which keeps showing what drives
//! the speakers), the pages in a sidebar, the shown page with whose its settings are, and
//! the page's keys. Every text comes from [`crate::settings`], the hosted dialogs' modules or
//! `ac2_scene::rig`.

use eframe::egui::{self, RichText};

use crate::app::App;
use crate::settings::{
    ConnLine, DisplayRow, Page, RecordingRow, Settings, conn_line_text, display_rows, link_lines,
    recording_rows,
};
use crate::state::{ConnState, Msg, Overlay, SettingsMsg};
use crate::theme::Chrome;

/// Draws the view from `top` (the top bar's bottom edge) down.
pub(super) fn settings(app: &mut App, ctx: &egui::Context, ch: &Chrome, top: f32) {
    let Overlay::Settings(s) = &app.state.overlay else {
        return;
    };
    let s: Settings = (**s).clone();
    let screen = ctx.content_rect();
    let rect = egui::Rect::from_min_max(egui::pos2(screen.min.x, top), screen.max);
    let mut msg: Option<Msg> = None;
    egui::Area::new(egui::Id::new("ac2-settings"))
        .order(egui::Order::Middle)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.painter().rect_filled(rect, 0.0, ch.panel);
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());
            let side_w = 220.0;
            let side = egui::Rect::from_min_size(
                rect.min + egui::vec2(12.0, 12.0),
                egui::vec2(side_w, rect.height() - 24.0),
            );
            let body = egui::Rect::from_min_max(
                egui::pos2(side.max.x + 20.0, rect.min.y + 12.0),
                rect.max - egui::vec2(16.0, 12.0),
            );
            ui.painter().line_segment(
                [
                    egui::pos2(side.max.x + 10.0, rect.min.y + 8.0),
                    egui::pos2(side.max.x + 10.0, rect.max.y - 8.0),
                ],
                egui::Stroke::new(1.0, ch.border),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(side), |ui| {
                sidebar(ui, &s, ch, &mut msg);
            });
            ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                page(app, ui, &s, ch, body, &mut msg);
            });
        });
    if s.page == Page::Calibration {
        super::cal::cal_dialogs(app, ctx, ch, &s.cal);
    }
    if s.page == Page::Leq
        && let Some(t) = s.leq.as_ref().and_then(|d| d.transfer.as_ref())
    {
        super::band_transfer::dialog(app, ctx, ch, t);
    }
    if let Some(m) = msg {
        app.dispatch(m);
    }
}

fn sidebar(ui: &mut egui::Ui, s: &Settings, ch: &Chrome, msg: &mut Option<Msg>) {
    ui.label(RichText::new("Settings").strong().size(20.0).color(ch.text));
    ui.add_space(10.0);
    for (i, p) in Page::ALL.iter().enumerate() {
        let on = s.page == *p;
        let t = RichText::new(p.title())
            .size(15.0)
            .color(if on { ch.focus } else { ch.text });
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Alt+{}", i + 1))
                    .small()
                    .monospace()
                    .color(ch.dim),
            );
            if ui
                .add(egui::Button::selectable(on, t).min_size(egui::vec2(150.0, 26.0)))
                .clicked()
            {
                *msg = Some(Msg::Settings(SettingsMsg::Page(*p)));
            }
        });
        ui.add_space(2.0);
    }
    ui.add_space(14.0);
    ui.label(
        RichText::new("Ctrl+PgUp / Ctrl+PgDn: pages\nEsc closes · Shift+Esc stops the stimulus")
            .small()
            .color(ch.dim),
    );
    ui.add_space(8.0);
    if ui.button("Close (Esc)").clicked() {
        *msg = Some(Msg::Settings(SettingsMsg::Close));
    }
}

fn page(
    app: &App,
    ui: &mut egui::Ui,
    s: &Settings,
    ch: &Chrome,
    body: egui::Rect,
    msg: &mut Option<Msg>,
) {
    // Wide pages (the calibrations' grid) scroll sideways; the header and the keys wrap
    // within the page.
    ui.set_max_width(body.width());
    ui.label(
        RichText::new(s.page.title())
            .strong()
            .size(20.0)
            .color(ch.text),
    );
    ui.label(
        RichText::new(format!("Whose: {}", s.page.whose()))
            .small()
            .color(ch.dim),
    );
    ui.add_space(8.0);
    let footer_h = 44.0;
    egui::ScrollArea::both()
        .id_salt(("ac2-settings-page", s.page.index()))
        .max_height((body.height() - 70.0 - footer_h).max(120.0))
        .auto_shrink([false, false])
        .show(ui, |ui| match s.page {
            Page::Io => io_page(app, ui, s, ch, msg),
            Page::Audio => {
                let mut m = None;
                super::session::audio_page(ui, &s.session, ch, &mut m);
                if let Some(m) = m {
                    *msg = Some(Msg::Session(m));
                }
            }
            Page::Calibration => super::cal::cal_page(app, ui, &s.cal, ch),
            Page::Leq => match &s.leq {
                Some(d) => {
                    if let Some(m) = super::leq::leq_page(ui, d, ch) {
                        *msg = Some(Msg::Leq(m));
                    }
                }
                None => {
                    ui.label(
                        RichText::new(
                            "No SPL meter: make one first (Ctrl+K → New SPL meter…); its Leq \
                             windows and limits are set here.",
                        )
                        .color(ch.dim),
                    );
                }
            },
            Page::Recording => recording_page(app, ui, s, ch),
            Page::Display => display_page(app, ui, s, ch, msg),
            Page::Connection => connection_page(app, ui, s, ch, msg),
        });
    ui.separator();
    ui.label(RichText::new(s.page.keys()).small().color(ch.dim));
}

/// A section title with whose its settings are.
fn section(ui: &mut egui::Ui, title: &str, whose: &str, ch: &Chrome) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong().size(16.0).color(ch.text));
        ui.label(RichText::new(format!("· {whose}")).small().color(ch.dim));
    });
    ui.add_space(4.0);
}

fn io_page(app: &App, ui: &mut egui::Ui, s: &Settings, ch: &Chrome, msg: &mut Option<Msg>) {
    let mut m = None;
    super::session::channels_page(app, ui, &s.session, ch, &mut m);
    if let Some(m) = m {
        *msg = Some(Msg::Session(m));
    }
    section(ui, "System max level", crate::settings::THE_RIG, ch);
    let g = app.state.daemon().map(|d| d.generator.clone());
    let focused = s.on_ceiling;
    ui.horizontal(|ui| {
        let marker = if focused { "▸" } else { " " };
        ui.label(RichText::new(marker).color(ch.focus).monospace());
        let now = g
            .as_ref()
            .map_or_else(|| "not connected".to_owned(), ac2_scene::rig::ceiling_line);
        let shown = if focused && s.ceiling.confirm.is_none() {
            if s.ceiling.text.is_empty() {
                format!("{now}   new level: ▏")
            } else {
                format!("{now}   new level: {}▏ dBFS", s.ceiling.text)
            }
        } else {
            now
        };
        let t = RichText::new(shown)
            .monospace()
            .color(if focused { ch.focus } else { ch.text });
        if ui.add(egui::Button::selectable(focused, t)).clicked() {
            *msg = Some(Msg::Settings(SettingsMsg::Ceiling));
        }
    });
    if let Some(change) = g.as_ref().and_then(ac2_scene::rig::ceiling_change) {
        ui.label(RichText::new(change).small().color(ch.dim));
    }
    ui.label(
        RichText::new(ac2_scene::rig::LOWER_STOPS)
            .small()
            .color(ch.dim),
    );
    if let Some(c) = &s.ceiling.confirm {
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, ch.armed))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.label(RichText::new(ac2_scene::rig::raise_prompt(c.from, c.to)).color(ch.armed));
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{}▏", c.typed))
                            .monospace()
                            .size(15.0)
                            .color(ch.text),
                    );
                    if ui.button("Raise").clicked() {
                        *msg = Some(Msg::Settings(SettingsMsg::CeilingApply));
                    }
                });
                ui.label(
                    RichText::new("Esc keeps the level as it is")
                        .small()
                        .color(ch.dim),
                );
            });
    }
    if let Some(e) = &s.ceiling.error {
        ui.label(RichText::new(e).color(ch.fault));
    }
}

/// A line's title in a fixed-width, left-aligned column.
fn title_cell(ui: &mut egui::Ui, title: &str, focused: bool, ch: &Chrome) {
    ui.allocate_ui_with_layout(
        egui::vec2(190.0, 20.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(190.0);
            ui.label(
                RichText::new(title)
                    .strong()
                    .color(if focused { ch.focus } else { ch.text }),
            );
        },
    );
}

fn recording_page(app: &App, ui: &mut egui::Ui, s: &Settings, ch: &Chrome) {
    let limit = app
        .state
        .prefs
        .record_limit_min
        .unwrap_or((crate::state::RECORD_MAX_S / 60.0) as u32);
    let server = s.connection.server.as_ref().and_then(|r| r.as_ref().ok());
    for (row, value) in recording_rows(limit, server) {
        let focused = s.recording.focus == row;
        let (title, whose) = match row {
            RecordingRow::Limit => ("Record limit", crate::settings::THIS_APP),
            RecordingRow::Folder => ("Folder", crate::settings::THE_RIG),
        };
        ui.horizontal(|ui| {
            let marker = if focused { "▸" } else { " " };
            ui.label(RichText::new(marker).color(ch.focus).monospace());
            title_cell(ui, title, focused, ch);
            let shown = if focused && row == RecordingRow::Limit && !s.recording.text.is_empty() {
                format!("{}▏ min", s.recording.text)
            } else {
                value
            };
            ui.label(RichText::new(shown).color(ch.text));
            ui.label(RichText::new(format!("· {whose}")).small().color(ch.dim));
        });
    }
    if let Some(e) = &s.recording.error {
        ui.label(RichText::new(e).color(ch.fault));
    }
}

fn display_page(app: &App, ui: &mut egui::Ui, s: &Settings, ch: &Chrome, msg: &mut Option<Msg>) {
    let st = &app.state;
    let rows = display_rows(
        st.theme,
        crate::settings::DisplaySwitches {
            key_hints: st.prefs.key_hints,
            warning_toasts: st.prefs.warning_toasts,
            resolution_marker: st.view.unresolved,
        },
        st.prefs.spl_hold_ms,
        st.view.spectrum.spectrograph.span_s,
        crate::settings::PaneViews {
            spectrum: st.kind_modes(crate::state::PaneKind::Spectrum).spectrum,
            sweep: st.kind_modes(crate::state::PaneKind::Distortion).sweep,
            leq: st.view.spl.layout,
            distortion_unit: st.view.distortion.unit,
            coherence: st.view.tf.coherence_placement,
        },
    );
    for (row, value) in rows {
        let focused = s.display == row;
        ui.horizontal(|ui| {
            let marker = if focused { "▸" } else { " " };
            ui.label(RichText::new(marker).color(ch.focus).monospace());
            title_cell(ui, row.title(), focused, ch);
            if row == DisplayRow::LevelAxes {
                ui.label(RichText::new(value).color(ch.text));
                if ui.small_button("Reset").clicked() {
                    *msg = Some(Msg::Settings(SettingsMsg::Display(row, 0)));
                }
            } else {
                if ui.small_button("‹").clicked() {
                    *msg = Some(Msg::Settings(SettingsMsg::Display(row, -1)));
                }
                ui.label(RichText::new(value).color(ch.text));
                if ui.small_button("›").clicked() {
                    *msg = Some(Msg::Settings(SettingsMsg::Display(row, 1)));
                }
            }
            ui.label(
                RichText::new(format!("· {}", crate::settings::THIS_APP))
                    .small()
                    .color(ch.dim),
            );
        });
    }
}

fn connection_page(app: &App, ui: &mut egui::Ui, s: &Settings, ch: &Chrome, msg: &mut Option<Msg>) {
    let st = &app.state;
    let (target, state, server) = match &st.conn {
        ConnState::Connecting { target } => (target.as_str(), "connecting…", None),
        ConnState::Connected { target, server, .. } => {
            (target.as_str(), "connected", Some(server.as_str()))
        }
        ConnState::Failed { target, error } => (target.as_str(), error.as_str(), None),
    };
    let me = st.my_client_id().map(|c| c.0.clone());
    section(ui, "This app", crate::settings::THIS_APP, ch);
    for l in link_lines(target, state, server, me.as_deref(), st.client_key.as_ref()) {
        ui.label(RichText::new(l).color(ch.text));
    }
    let p = &s.connection;
    let info = p.server.as_ref().and_then(|r| r.as_ref().ok());
    let lines = p.lines();
    let focus = p.focus.min(lines.len().saturating_sub(1));
    let mut row = |ui: &mut egui::Ui, i: usize, text: String, color| {
        let focused = i == focus;
        ui.horizontal(|ui| {
            let marker = if focused { "▸" } else { " " };
            ui.label(RichText::new(marker).color(ch.focus).monospace());
            let t = RichText::new(text).color(if focused { ch.focus } else { color });
            if ui.add(egui::Button::selectable(focused, t)).clicked() {
                *msg = Some(Msg::Settings(SettingsMsg::Connection(i)));
            }
        });
    };
    ui.add_space(4.0);
    for (i, l) in lines.iter().enumerate() {
        if matches!(l, ConnLine::Reconnect | ConnLine::ConnectOther) {
            row(ui, i, conn_line_text(l, info), ch.text);
        }
    }
    section(ui, "Server", crate::settings::THE_RIG, ch);
    match &p.server {
        None => {
            ui.label(RichText::new("asking the daemon…").color(ch.dim));
        }
        Some(Err(e)) => {
            ui.label(RichText::new(format!("cannot ask the daemon: {e}")).color(ch.fault));
        }
        Some(Ok(info)) => {
            for l in ac2_scene::rig::server_lines(info) {
                ui.label(RichText::new(l).color(ch.text));
            }
            if let ac2_proto::model::ServerMode::Network { refused, .. } = &info.mode {
                ui.add_space(6.0);
                ui.label(RichText::new("Authorized clients").strong().color(ch.text));
                for (i, l) in lines.iter().enumerate() {
                    if let ConnLine::Authorized(_) = l {
                        let pending = matches!(l, ConnLine::Authorized(n)
                            if p.confirm.as_deref() == Some(n.as_str()));
                        row(
                            ui,
                            i,
                            conn_line_text(l, Some(info)),
                            if pending { ch.fault } else { ch.text },
                        );
                    }
                }
                ui.add_space(6.0);
                ui.label(RichText::new("Refused keys").strong().color(ch.text));
                if refused.is_empty() {
                    ui.label(RichText::new("none since the daemon started").color(ch.dim));
                }
                let now = crate::scenes::daemon_wall(st, super::now().wall);
                let mut k = 0;
                for (i, l) in lines.iter().enumerate() {
                    if let ConnLine::Refused(..) = l {
                        if let Some(r) = refused.get(k) {
                            row(
                                ui,
                                i,
                                ac2_scene::rig::refused_row(
                                    r,
                                    now,
                                    ac2_scene::time::ClockOffset(0),
                                ),
                                ch.armed,
                            );
                        }
                        k += 1;
                    }
                }
                for (i, l) in lines.iter().enumerate() {
                    if let ConnLine::AddKey = l {
                        row(ui, i, conn_line_text(l, Some(info)), ch.text);
                    }
                }
            }
        }
    }
    if let (Some(label), Some((_, text))) = (p.edit_label(), &p.edit) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).color(ch.text));
            ui.label(
                RichText::new(format!("{text}▏"))
                    .monospace()
                    .color(ch.focus),
            );
        });
    }
    for (text, color) in [
        (p.notice.as_deref(), ch.armed),
        (p.error.as_deref(), ch.fault),
    ] {
        if let Some(t) = text {
            ui.label(RichText::new(t).color(color));
        }
    }
}
