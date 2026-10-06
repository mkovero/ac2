//! The calibrations view: inputs (mic, mic curve, sensitivity), the mic library, the
//! sensitivity calibrations. Every string comes from `ac2-scene` through
//! [`crate::cal_view::line_texts`].

use eframe::egui::{self, RichText};

use crate::acoustic_dialog::{AcousticDialog, Field as AcousticField};
use crate::app::App;
use crate::cal_view::{CalLine, CalView, line_texts};
use crate::electrical_dialog::{ElectricalDialog, Field};
use crate::theme::Chrome;

use super::overlays::card;

/// The Calibration page: inputs (mic, curve, sensitivity), the mic library, the
/// sensitivity calibrations, the typed edit and what the view says.
pub(super) fn cal_page(app: &App, ui: &mut egui::Ui, v: &CalView, ch: &Chrome) {
    let Some(st) = app.state.daemon() else {
        ui.label(RichText::new("not connected to a daemon").color(ch.dim));
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
                let resp = ui.label(RichText::new(&r.title).color(if focused {
                    ch.focus
                } else {
                    ch.text
                }));
                ui.label(RichText::new(&r.detail).color(if r.warn { ch.armed } else { ch.text }));
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
                "Nothing stored and no session open: open a session (Audio page), name the mic \
                 on its input (N), import its curve (I).",
            )
            .color(ch.dim),
        );
    }
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
}

/// The acoustic or electrical calibration dialog open over the Calibration page.
pub(super) fn cal_dialogs(app: &App, ctx: &egui::Context, ch: &Chrome, v: &CalView) {
    if let Some(d) = &v.electrical {
        electrical(app, ctx, ch, d);
    }
    if let Some(d) = &v.acoustic {
        acoustic(app, ctx, ch, d);
    }
}

/// The acoustic calibration dialog over the view: what to do, the input's level now, the
/// mic, the calibrator's level and tone; the calibration it replaces, if electrical.
fn acoustic(app: &App, ctx: &egui::Context, ch: &Chrome, d: &AcousticDialog) {
    let meter = app
        .state
        .input_meters()
        .get(&d.input)
        .cloned()
        .unwrap_or_else(ac2_scene::meter::MeterReading::none);
    egui::Area::new(egui::Id::new("ac2-acoustic-cal"))
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(640.0);
                ui.label(RichText::new(d.title()).strong().size(16.0));
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(d.instructions()).color(ch.text)).wrap());
                ui.add_space(6.0);
                egui::Grid::new("ac2-acoustic-fields")
                    .num_columns(4)
                    .spacing(egui::vec2(12.0, 6.0))
                    .show(ui, |ui| {
                        let row = |ui: &mut egui::Ui, f: Option<AcousticField>, label: &str| {
                            let focused = f.is_some_and(|f| f == d.focus);
                            ui.label(
                                RichText::new(if focused { "▸" } else { " " })
                                    .color(ch.focus)
                                    .monospace(),
                            );
                            ui.label(RichText::new(label).color(if focused {
                                ch.focus
                            } else {
                                ch.text
                            }));
                            focused
                        };
                        let value = |ui: &mut egui::Ui, text: &str, focused: bool| {
                            ui.label(
                                RichText::new(if focused {
                                    format!("{text}▏")
                                } else {
                                    text.to_owned()
                                })
                                .monospace()
                                .color(if focused {
                                    ch.focus
                                } else {
                                    ch.text
                                }),
                            );
                        };
                        row(ui, None, "Input level now");
                        ui.horizontal(|ui| super::session::meter(ui, &meter, ch));
                        ui.label(
                            RichText::new("dBFS · steady, not clipping")
                                .small()
                                .color(ch.dim),
                        );
                        ui.end_row();
                        let f = row(ui, Some(AcousticField::Mic), "Mic");
                        value(ui, &d.mic, f);
                        ui.label(
                            RichText::new("the name it is stored for, e.g. MM1 34804")
                                .small()
                                .color(ch.dim),
                        );
                        ui.end_row();
                        let f = row(ui, Some(AcousticField::Level), "Calibrator level");
                        value(ui, &d.level, f);
                        ui.label(
                            RichText::new("←/→ 94 / 114 dB, or type")
                                .small()
                                .color(ch.dim),
                        );
                        ui.end_row();
                        let f = row(ui, Some(AcousticField::Freq), "Calibrator tone");
                        value(ui, &d.freq, f);
                        ui.label(RichText::new("←/→ 1 kHz / 250 Hz").small().color(ch.dim));
                        ui.end_row();
                    });
                if let Some(r) = &d.replaces {
                    ui.label(
                        RichText::new(format!(
                            "Replaces the electrical calibration of this mic ({r}): a \
                             calibrator measures the capsule too."
                        ))
                        .small()
                        .color(ch.dim),
                    );
                }
                for (text, color) in [
                    (d.pending.as_ref().map(|_| "reading the input…"), ch.dim),
                    (d.error.as_deref(), ch.fault),
                ] {
                    if let Some(t) = text {
                        ui.label(RichText::new(t).color(color));
                    }
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "↑↓ field · ←→ level / tone · type the values · Enter reads the input \
                         and stores · Esc closes (back to the calibrations)",
                    )
                    .small()
                    .color(ch.dim),
                );
            });
        });
}

/// The electrical calibration dialog over the view: where the voltage is measured (with its
/// safety note, prominent), the input's level now, the voltage, the tone, the sensitivity
/// and where it comes from.
fn electrical(app: &App, ctx: &egui::Context, ch: &Chrome, d: &ElectricalDialog) {
    let meter = app
        .state
        .input_meters()
        .get(&d.input)
        .cloned()
        .unwrap_or_else(ac2_scene::meter::MeterReading::none);
    egui::Area::new(egui::Id::new("ac2-electrical-cal"))
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(720.0);
                ui.label(RichText::new(d.title()).strong().size(16.0));
                ui.add_space(4.0);
                ui.label(RichText::new(d.safety()).strong().color(ch.armed));
                ui.add_space(6.0);
                egui::Grid::new("ac2-electrical-fields")
                    .num_columns(4)
                    .spacing(egui::vec2(12.0, 6.0))
                    .show(ui, |ui| {
                        let row = |ui: &mut egui::Ui, f: Option<Field>, label: &str| {
                            let focused = f.is_some_and(|f| f == d.focus);
                            ui.label(
                                RichText::new(if focused { "▸" } else { " " })
                                    .color(ch.focus)
                                    .monospace(),
                            );
                            ui.label(RichText::new(label).color(if focused {
                                ch.focus
                            } else {
                                ch.text
                            }));
                            focused
                        };
                        let value = |ui: &mut egui::Ui, text: &str, focused: bool| {
                            ui.label(
                                RichText::new(if focused {
                                    format!("{text}▏")
                                } else {
                                    text.to_owned()
                                })
                                .monospace()
                                .color(if focused {
                                    ch.focus
                                } else {
                                    ch.text
                                }),
                            );
                        };
                        row(ui, Some(Field::Method), "Measured");
                        ui.label(RichText::new(d.method_text()).color(ch.text));
                        ui.label(RichText::new("←/→").small().color(ch.dim));
                        ui.end_row();
                        row(ui, None, "Input level now");
                        ui.horizontal(|ui| super::session::meter(ui, &meter, ch));
                        ui.label(
                            RichText::new("dBFS · steady, not clipping")
                                .small()
                                .color(ch.dim),
                        );
                        ui.end_row();
                        let f = row(ui, Some(Field::Volts), "Voltage measured (RMS)");
                        value(ui, &d.volts, f);
                        ui.label(
                            RichText::new("as the meter shows it, e.g. 15.03 mV")
                                .small()
                                .color(ch.dim),
                        );
                        ui.end_row();
                        let f = row(ui, Some(Field::Freq), "Tone");
                        value(ui, &d.freq, f);
                        ui.label(
                            RichText::new("1 kHz: where mic sensitivities are stated")
                                .small()
                                .color(ch.dim),
                        );
                        ui.end_row();
                        let f = row(ui, Some(Field::Sensitivity), "Mic sensitivity");
                        value(ui, &d.sensitivity, f);
                        ui.label(RichText::new(d.sensitivity_source()).small().color(ch.dim));
                        ui.end_row();
                    });
                if let Some(n) = &d.no_data_sheet {
                    ui.label(RichText::new(n).small().color(ch.dim));
                }
                for (text, color) in [
                    (d.pending.as_ref().map(|_| "reading the input…"), ch.dim),
                    (d.notice.as_deref(), ch.armed),
                    (d.error.as_deref(), ch.fault),
                ] {
                    if let Some(t) = text {
                        ui.label(RichText::new(t).color(color));
                    }
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Keep the gain you will measure with. ↑↓ field · ←→ in-line / \
                         injected · type the values · Enter reads the input and stores · \
                         Esc closes (back to the calibrations)",
                    )
                    .small()
                    .color(ch.dim),
                );
            });
        });
}
