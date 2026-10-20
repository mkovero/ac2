//! The Rig monitor page and the cue chip the tree and the page draw. Every text comes from
//! `ac2_scene::osc`, `ac2_scene::artnet` and [`crate::settings::RigPage`].

use ac2_proto::model::{CueState, Rig};
use ac2_scene::theme::Theme;
use ac2_scene::{artnet, osc};
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
        if row == RigRow::ArtnetTarget {
            ui.add_space(10.0);
        }
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
                RigRow::Monitor => {
                    let on = rig.monitor_at_start;
                    let r = ui.add(egui::Button::selectable(
                        focused,
                        RichText::new(osc::monitor_text(on)).color(ch.text),
                    ));
                    if focused {
                        super::settings::reveal_focused(&r);
                    }
                    if r.clicked() {
                        *msg = Some(Msg::Settings(SettingsMsg::RigMonitor(!on)));
                    }
                }
                _ => {
                    let text = match row {
                        RigRow::Target => &p.target,
                        RigRow::Test => &p.test,
                        RigRow::ArtnetTarget => &p.artnet,
                        RigRow::Universe => &p.universe,
                        RigRow::Fade => &p.fade,
                        _ => &p.dmx_test,
                    };
                    let caret = focused.then_some(p.caret);
                    let (r, click) = super::leq::caret_cell(ui, 220.0, text, caret, "off", ch);
                    if focused {
                        super::settings::reveal_focused(&r);
                    }
                    if let Some(n) = click {
                        *msg = Some(Msg::Settings(SettingsMsg::RigCaret(row, n)));
                    }
                    let action = match row {
                        RigRow::Test | RigRow::DmxTest => "Send",
                        _ => "Apply",
                    };
                    if ui
                        .button(action)
                        .on_hover_text(super::keys_cue(&[egui::Key::Enter]))
                        .clicked()
                    {
                        *msg = Some(Msg::Settings(SettingsMsg::RigApply(row)));
                    }
                }
            }
        });
        let note = match row {
            RigRow::Target => Some(osc::TARGET_NOTE),
            RigRow::Test => Some(osc::TEST_NOTE),
            RigRow::ArtnetTarget => Some(artnet::TARGET_NOTE),
            RigRow::Universe => Some(artnet::UNIVERSE_NOTE),
            RigRow::Fade => Some(artnet::FADE_NOTE),
            RigRow::DmxTest => Some(artnet::TEST_NOTE),
            RigRow::Monitor => None,
        };
        let error = p.error.as_deref().filter(|_| focused);
        for (text, color) in [(note, ch.dim), (error, ch.fault)] {
            if let Some(n) = text {
                ui.horizontal(|ui| {
                    ui.add_space(190.0);
                    ui.label(RichText::new(n).small().color(color));
                });
            }
        }
        // A kept target's line, under its own group.
        if let Some(n) = &p.notice
            && ((row == RigRow::Test && p.edited) || (row == RigRow::DmxTest && !p.edited))
        {
            ui.label(RichText::new(n).color(ch.warn));
        }
        ui.add_space(4.0);
        let status = |ui: &mut egui::Ui, line: String, err: bool| {
            ui.label(RichText::new(line).color(if err { ch.warn } else { ch.dim }));
        };
        if row == RigRow::Test {
            status(ui, osc::status_line(rig), rig.osc.last_error.is_some());
        } else if row == RigRow::DmxTest {
            status(
                ui,
                artnet::status_line(rig),
                rig.artnet.last_error.is_some(),
            );
            status(ui, artnet::channels_line(rig), false);
        }
    }
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
