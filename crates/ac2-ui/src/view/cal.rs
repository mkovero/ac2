//! The calibrations view: inputs (mic, mic curve, sensitivity), the mic library, the
//! sensitivity calibrations. Every string comes from `ac2-scene` through
//! [`crate::cal_view::line_texts`].

use eframe::egui::{self, RichText};

use crate::app::App;
use crate::cal_view::{CalLine, CalView, line_texts};
use crate::state::Overlay;
use crate::theme::Chrome;

use super::overlays::{backdrop, card};

pub(super) fn calibrations(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    let Overlay::Calibrations(v) = &app.state.overlay else {
        return;
    };
    let v: CalView = (**v).clone();
    let Some(st) = app.state.daemon() else {
        return;
    };
    let now = super::now();
    let offset = ac2_scene::time::ClockOffset(
        app.state
            .mirror
            .as_ref()
            .and_then(|m| m.clock_offset_ns)
            .map_or(0, |o| {
                o.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
            }),
    );
    let rows = line_texts(st, now.wall, offset);
    let focus = v.focus.min(rows.len().saturating_sub(1));
    backdrop(ctx);
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("ac2-calibrations"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 40.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(1040.0);
                ui.label(
                    RichText::new("Calibrations and input setup")
                        .strong()
                        .size(16.0),
                );
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height((screen.height() - 260.0).max(160.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Grid::new("ac2-calibrations-lines")
                            .num_columns(4)
                            .spacing(egui::vec2(12.0, 4.0))
                            .min_col_width(0.0)
                            .show(ui, |ui| {
                                let mut section = "";
                                for (i, r) in rows.iter().enumerate() {
                                    let title = match r.line {
                                        CalLine::Input(_) => "Inputs",
                                        CalLine::Curve(_) => "Mic library",
                                        CalLine::Sensitivity(_) => "Sensitivity calibrations",
                                    };
                                    if title != section {
                                        section = title;
                                        ui.label("");
                                        ui.label(RichText::new(title).strong().color(ch.text));
                                        ui.end_row();
                                    }
                                    let focused = i == focus;
                                    let marker = if focused { "▸" } else { " " };
                                    ui.label(RichText::new(marker).color(ch.focus).monospace());
                                    let resp =
                                        ui.label(RichText::new(&r.title).color(if focused {
                                            ch.focus
                                        } else {
                                            ch.text
                                        }));
                                    ui.label(RichText::new(&r.detail).color(if r.warn {
                                        ch.armed
                                    } else {
                                        ch.text
                                    }));
                                    ui.label(RichText::new(&r.extra).small().color(ch.dim));
                                    ui.end_row();
                                    if focused {
                                        resp.scroll_to_me(None);
                                    }
                                }
                            });
                        if rows.is_empty() {
                            ui.label(
                                RichText::new(
                                    "Nothing stored and no session open: open a session \
                                     (Shift+O), name the mic on its input (N), import its \
                                     curve (I).",
                                )
                                .color(ch.dim),
                            );
                        }
                    });
                ui.separator();
                if let (Some(label), Some(e)) = (v.edit_label(), &v.edit) {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(label).color(ch.text));
                        ui.label(
                            RichText::new(format!("{}▏", e.text))
                                .monospace()
                                .color(ch.focus),
                        );
                    });
                }
                for (text, color) in [
                    (v.notice.as_deref(), ch.armed),
                    (v.error.as_deref(), ch.fault),
                ] {
                    if let Some(t) = text {
                        ui.label(RichText::new(t).color(color));
                    }
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "↑↓ move · ←→ mic curve of an input (off / 0° / 90° …) · N names the \
                         mic · I imports a curve file · R renames a curve · Delete deletes \
                         (twice) · Enter ends typing / closes · Esc closes",
                    )
                    .small()
                    .color(ch.dim),
                );
            });
        });
}
