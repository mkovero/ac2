//! The Sessions dialog (texts from `ac2_scene::sessions`, rows from
//! [`crate::sessions`]): rows tall enough for a finger on the kiosk's touch screen.

use eframe::egui::{self, RichText};

use super::overlays::{backdrop, card};
use crate::app::App;
use crate::sessions::{Page, SessionsDialog};
use crate::state::{Msg, Overlay, SessionsMsg};
use crate::theme::Chrome;

/// Row height: a fingertip on a 7" touch screen.
const ROW_H: f32 = 44.0;
const WIDTH: f32 = 620.0;
/// Every page's buttons are this size, so they line up page to page and take a finger.
const BUTTON: [f32; 2] = [110.0, 40.0];

/// The page's buttons (`ac2_scene::sessions::page_buttons`) in one row: the index clicked.
fn buttons(ui: &mut egui::Ui, labels: &[&str]) -> Option<usize> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        for (i, l) in labels.iter().enumerate() {
            if ui
                .add_sized(BUTTON, egui::Button::new(RichText::new(*l).size(15.0)))
                .clicked()
            {
                clicked = Some(i);
            }
        }
    });
    clicked
}

fn row(ui: &mut egui::Ui, selected: bool, title: &str, detail: &str, ch: &Chrome) -> bool {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        title,
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(16.0),
            color: ch.text,
            ..Default::default()
        },
    );
    if !detail.is_empty() {
        job.append(
            &format!("\n{detail}"),
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(12.0),
                color: ch.dim,
                ..Default::default()
            },
        );
    }
    ui.add_sized(
        [ui.available_width(), ROW_H],
        // The text before a growing atom starts at the left edge: every title in the
        // column lines up whatever the length of the detail under it.
        egui::Button::selectable(selected, (job, egui::Atom::grow())),
    )
    .clicked()
}

pub(super) fn sessions(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    let Overlay::Sessions(d) = &app.state.overlay else {
        return;
    };
    let d: SessionsDialog = d.as_ref().clone();
    let now = super::now().wall;
    let w = app.state.working();
    let label = ac2_scene::sessions::working_label(&w, now, app.state.clock_offset());
    let rows = app.state.sessions_rows(now);
    backdrop(ctx);
    let mut msg = None;
    egui::Area::new(egui::Id::new("ac2-sessions"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 50.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(WIDTH);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Sessions").strong().size(18.0));
                    ui.label(
                        RichText::new(&label.text)
                            .size(15.0)
                            .color(if label.unsaved { ch.warn } else { ch.dim }),
                    )
                    .on_hover_text(&label.detail);
                });
                ui.add_space(6.0);
                if let Some(b) = &d.busy {
                    ui.label(RichText::new(b).color(ch.armed));
                    ui.add_space(4.0);
                }
                let max_h = (ctx.content_rect().height() - 200.0).max(ROW_H * 3.0);
                match &d.page {
                    Page::Main => {
                        egui::ScrollArea::vertical()
                            .max_height(max_h)
                            .show(ui, |ui| {
                                let actions = rows
                                    .iter()
                                    .take_while(|r| {
                                        !matches!(r.action, crate::sessions::Action::Recent(_))
                                    })
                                    .count();
                                for (i, r) in rows.iter().enumerate() {
                                    if i == actions {
                                        ui.add_space(6.0);
                                        ui.label(RichText::new("Recent").strong().color(ch.dim));
                                    }
                                    if row(ui, i == d.selected, &r.title, &r.detail, ch) {
                                        msg = Some(SessionsMsg::Row(i));
                                    }
                                }
                                if d.daemon.is_none() {
                                    ui.label(RichText::new("asking the daemon…").color(ch.dim));
                                }
                                if let Some(e) = &d.list_error {
                                    ui.label(RichText::new(e).color(ch.warn));
                                }
                            });
                    }
                    Page::Browse(b) => {
                        match buttons(ui, ac2_scene::sessions::page_buttons(d.page.scene())) {
                            Some(0) => msg = Some(SessionsMsg::Places),
                            Some(_) => msg = Some(SessionsMsg::Back),
                            None => {}
                        }
                        ui.label(
                            RichText::new(b.dir.as_ref().map_or_else(
                                || "Home and removable media".to_owned(),
                                |p| p.display().to_string(),
                            ))
                            .color(ch.dim),
                        );
                        if let Some(e) = &b.error {
                            ui.label(RichText::new(e).color(ch.warn));
                        }
                        egui::ScrollArea::vertical()
                            .max_height(max_h)
                            .show(ui, |ui| {
                                let mut i = 0;
                                if let Some(dir) = b.dir.as_ref().filter(|_| b.here_row()) {
                                    if row(
                                        ui,
                                        b.selected == 0,
                                        "Export here",
                                        &dir.display().to_string(),
                                        ch,
                                    ) {
                                        msg = Some(SessionsMsg::Browse(0));
                                    }
                                    i = 1;
                                }
                                for r in &b.rows {
                                    if row(ui, b.selected == i, &r.label(), "", ch) {
                                        msg = Some(SessionsMsg::Browse(i));
                                    }
                                    i += 1;
                                }
                                if b.rows.is_empty() {
                                    ui.label(
                                        RichText::new("nothing here: no folders or sessions")
                                            .color(ch.dim),
                                    );
                                }
                            });
                    }
                    Page::Name(n) => {
                        ui.label(RichText::new(n.title()).size(15.0));
                        ui.label(RichText::new(format!("{}▏", n.text)).size(18.0).monospace());
                        if let Some(e) = &n.error {
                            ui.label(RichText::new(e).color(ch.warn));
                        }
                        ui.add_space(6.0);
                        match buttons(ui, ac2_scene::sessions::page_buttons(d.page.scene())) {
                            Some(0) => msg = Some(SessionsMsg::Submit),
                            Some(_) => msg = Some(SessionsMsg::Back),
                            None => {}
                        }
                    }
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(ac2_scene::sessions::page_hint(d.page.scene()))
                        .small()
                        .color(ch.dim),
                );
            });
        });
    if let Some(m) = msg {
        app.dispatch(Msg::Sessions(m));
    }
}
