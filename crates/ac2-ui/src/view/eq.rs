//! The EQ editor ([`crate::eq_editor::view`] texts): a card at the bottom right, so the
//! transfer pane it edits stays in sight. Keys only; nothing here computes a value.

use eframe::egui::{self, RichText};

use super::overlays::card;
use crate::app::App;
use crate::state::Overlay;
use crate::theme::Chrome;

const WIDTH: f32 = 560.0;

fn cells(ui: &mut egui::Ui, cells: &[(String, bool)], ch: &Chrome) {
    ui.horizontal_wrapped(|ui| {
        for (t, focused) in cells {
            let r = RichText::new(t).size(14.0);
            if *focused {
                ui.label(r.color(ch.text).background_color(ch.focus));
            } else {
                ui.label(r.color(ch.dim));
            }
            ui.add_space(8.0);
        }
    });
}

pub(super) fn eq(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    let Overlay::Eq(e) = &app.state.overlay else {
        return;
    };
    let st = &app.state;
    let Some(m) = st.meas(e.meas) else {
        return;
    };
    let v = crate::eq_editor::view(e, m, &st.trace_list(), &st.measurements());
    egui::Area::new(egui::Id::new("ac2-eq"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(WIDTH);
                ui.label(RichText::new(&v.title).strong().size(16.0));
                cells(ui, &v.header, ch);
                ui.separator();
                if v.sections.is_empty() {
                    ui.label(RichText::new("no sections · Insert adds one").color(ch.dim));
                }
                for ((t, selected), fields) in &v.sections {
                    let r = RichText::new(t).size(14.0);
                    ui.label(if *selected {
                        r.strong().color(ch.text)
                    } else {
                        r.color(ch.text)
                    });
                    if let Some(f) = fields {
                        ui.indent("fields", |ui| cells(ui, f, ch));
                    }
                }
                ui.separator();
                for (t, selected) in &v.curves {
                    let r = RichText::new(t).size(13.0);
                    ui.label(if *selected {
                        r.color(ch.text).background_color(ch.focus)
                    } else {
                        r.color(ch.dim)
                    });
                }
                ui.add_space(4.0);
                let b = RichText::new(&v.bottom).size(12.0);
                ui.label(if v.refused {
                    b.color(ch.fault)
                } else {
                    b.color(ch.dim)
                });
            });
        });
}
