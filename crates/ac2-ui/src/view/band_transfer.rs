//! The band transfer step over the Leq settings: what to do next, the place's name, the
//! three spans with
//! their times, coverage and band averages, and the transfer it stored. Every text comes
//! from `ac2_scene::band_transfer` and `ac2_scene::band_leq`.

use eframe::egui::{self, RichText};

use ac2_scene::band_transfer::{self as bt, SpanRole, SpanState};

use crate::app::App;
use crate::leq_dialog::TransferStep;
use crate::theme::Chrome;

use super::overlays::card;

pub(super) fn dialog(app: &App, ctx: &egui::Context, ch: &Chrome, t: &TransferStep) {
    let st = &app.state;
    egui::Area::new(egui::Id::new("ac2-band-transfer"))
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(760.0);
                ui.label(
                    RichText::new(format!("Band transfer — {}", t.meter))
                        .strong()
                        .size(16.0),
                );
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(t.next_step()).color(ch.text)).wrap());
                ui.add_space(6.0);
                let place = t.place_name();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(if t.on_place { "▸" } else { " " })
                            .color(ch.focus)
                            .monospace(),
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(130.0, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(130.0);
                            ui.label(RichText::new("Limits are for").color(if t.on_place {
                                ch.focus
                            } else {
                                ch.text
                            }));
                        },
                    );
                    let shown = if t.on_place {
                        format!("{}▏", t.place)
                    } else {
                        place.to_owned()
                    };
                    ui.label(RichText::new(shown).monospace().color(ch.text));
                    ui.label(
                        RichText::new("the place's name: ↑ here and type")
                            .small()
                            .color(ch.dim),
                    );
                });
                ui.add_space(2.0);
                for r in SpanRole::ALL {
                    let s = t.span(r);
                    let focused = !t.on_place && t.focus == r;
                    let now = st.meter_now(s.meas).unwrap_or(match s.state {
                        SpanState::Marking { from } | SpanState::Marked { from, .. } => from,
                        SpanState::Unmarked => ac2_proto::units::WallNs(0),
                    });
                    let at = match s.state {
                        SpanState::Marking { from } | SpanState::Marked { from, .. } => from,
                        SpanState::Unmarked => now,
                    };
                    let offset = st.local_zone.offset_s(at);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(if focused { "▸" } else { " " })
                                .color(ch.focus)
                                .monospace(),
                        );
                        ui.allocate_ui_with_layout(
                            egui::vec2(130.0, 20.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_min_width(130.0);
                                ui.label(RichText::new(r.title(place)).color(if focused {
                                    ch.focus
                                } else {
                                    ch.text
                                }));
                            },
                        );
                        ui.label(RichText::new(&s.meter).color(ch.text));
                        ui.label(
                            RichText::new(bt::span_time_text(s.state, now, offset))
                                .monospace()
                                .color(ch.text),
                        );
                    });
                    let note = |ui: &mut egui::Ui, text: String, color| {
                        ui.horizontal(|ui| {
                            ui.add_space(150.0);
                            ui.add(
                                egui::Label::new(RichText::new(text).small().color(color)).wrap(),
                            );
                        });
                    };
                    note(ui, r.what(place), ch.dim);
                    if let (SpanState::Marked { from, until }, Some(a)) = (s.state, &s.average) {
                        note(ui, bt::coverage_text(a, from, until), ch.dim);
                        if let Some(l) = bt::averages_text(a, &t.shown) {
                            note(ui, l, ch.text);
                        }
                    }
                    if let Some(e) = &s.error {
                        note(ui, e.clone(), ch.fault);
                    }
                    ui.add_space(2.0);
                }
                let result = t.result_lines();
                if !result.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(&result[0]).strong().color(ch.text));
                    ui.add(
                        egui::Label::new(
                            RichText::new(result[1..].join(" · "))
                                .small()
                                .color(ch.text),
                        )
                        .wrap(),
                    );
                }
                for (text, color) in [
                    (t.storing.then_some("storing the band transfer…"), ch.dim),
                    (t.error.as_deref(), ch.fault),
                ] {
                    if let Some(text) = text {
                        ui.label(RichText::new(text).color(color));
                    }
                }
                ui.add_space(4.0);
                ui.label(RichText::new(bt::KEYS).small().color(ch.dim));
            });
        });
}
