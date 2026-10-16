//! The Rig monitor page and the cue chip the tree and the page draw. Every text comes from
//! `ac2_scene::osc` and [`crate::settings::RigPage`].

use ac2_proto::model::{CueState, Rig};
use ac2_scene::osc;
use ac2_scene::theme::Theme;
use eframe::egui::{self, RichText};

use crate::settings::{RigRow, Settings};
use crate::state::{Msg, SettingsMsg};
use crate::theme::{Chrome, c32};

/// A cue state as a small chip, coloured as the Leq tiles colour the state.
pub(super) fn cue_chip(ui: &mut egui::Ui, s: CueState, theme: &Theme) -> egui::Response {
    let (bg, fg) = osc::cue_colors(s, theme);
    let t = RichText::new(osc::state_word(s))
        .small()
        .strong()
        .color(c32(fg))
        .background_color(c32(bg));
    ui.add(egui::Label::new(t).selectable(false))
}

pub(super) fn rig_page(
    ui: &mut egui::Ui,
    s: &Settings,
    rig: &Rig,
    meters: &[&ac2_proto::model::Measurement],
    theme: &Theme,
    ch: &Chrome,
    msg: &mut Option<Msg>,
) {
    let p = &s.rig;
    for row in RigRow::ALL {
        let focused = p.focus == row;
        ui.horizontal(|ui| {
            let marker = if focused { "▸" } else { " " };
            ui.label(RichText::new(marker).color(ch.focus).monospace());
            ui.allocate_ui_with_layout(
                egui::vec2(170.0, 20.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_width(170.0);
                    ui.label(RichText::new(row.title()).strong().color(if focused {
                        ch.focus
                    } else {
                        ch.text
                    }));
                },
            );
            match row {
                RigRow::Target | RigRow::Test => {
                    let text = if row == RigRow::Target {
                        &p.target
                    } else {
                        &p.test
                    };
                    let sel = focused && p.selected && !text.is_empty();
                    let shown = if sel {
                        text.clone()
                    } else if focused {
                        format!("{text}▏")
                    } else if text.is_empty() {
                        "off".to_owned()
                    } else {
                        text.clone()
                    };
                    let mut t = RichText::new(shown).monospace();
                    t = if sel {
                        t.color(ch.panel).background_color(ch.focus)
                    } else {
                        t.color(ch.text)
                    };
                    let r = ui.add_sized([220.0, 20.0], egui::Button::selectable(focused, t));
                    if r.clicked() {
                        *msg = Some(Msg::Settings(SettingsMsg::RigFocus(row)));
                    }
                    let action = if row == RigRow::Target {
                        "Apply"
                    } else {
                        "Send"
                    };
                    if ui.button(action).clicked() {
                        *msg = Some(Msg::Settings(SettingsMsg::RigApply(row)));
                    }
                }
                RigRow::Monitor => {
                    let on = rig.monitor_at_start;
                    if ui
                        .add(egui::Button::selectable(
                            focused,
                            RichText::new(osc::monitor_text(on)).color(ch.text),
                        ))
                        .clicked()
                    {
                        *msg = Some(Msg::Settings(SettingsMsg::RigMonitor(!on)));
                    }
                }
            }
        });
        let note = match row {
            RigRow::Target => Some(osc::TARGET_NOTE),
            RigRow::Test => Some(osc::TEST_NOTE),
            RigRow::Monitor => None,
        };
        if let Some(n) = note {
            ui.horizontal(|ui| {
                ui.add_space(190.0);
                ui.label(RichText::new(n).small().color(ch.dim));
            });
        }
        ui.add_space(4.0);
    }
    if let Some(e) = &p.error {
        ui.label(RichText::new(e).color(ch.fault));
    }
    ui.add_space(8.0);
    let status_color = if rig.osc.last_error.is_some() {
        ch.warn
    } else {
        ch.dim
    };
    ui.label(RichText::new(osc::status_line(rig)).color(status_color));
    ui.add_space(10.0);
    ui.label(RichText::new(osc::METERS_TITLE).strong().color(ch.text));
    if meters.is_empty() {
        ui.label(RichText::new(osc::NO_RIG_METERS).small().color(ch.dim));
    }
    for m in meters {
        let state = osc::meter_state(rig, m.id);
        ui.horizontal(|ui| {
            if let Some(st) = state {
                cue_chip(ui, st, theme);
            }
            ui.label(RichText::new(osc::meter_line(m, state)).color(ch.text));
        });
    }
}
