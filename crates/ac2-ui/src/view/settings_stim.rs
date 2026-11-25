//! This app's stimulus rows of Settings › Audio I/O, after the signal rows and directly
//! above the system max level: whether transfers stop with the stimulus, and the level.

use ac2_scene::stim_rows::{self as text, StimRow};
use eframe::egui::{self, RichText};

use super::settings::title_cell;
use crate::app::App;
use crate::settings::Settings;
use crate::state::{Msg, SettingsMsg};
use crate::theme::Chrome;

pub(super) fn stim_rows(
    app: &App,
    ui: &mut egui::Ui,
    s: &Settings,
    ch: &Chrome,
    msg: &mut Option<Msg>,
) {
    let st = &app.state;
    let level = st.rig.stimulus.level;
    for row in StimRow::ALL {
        let focused = s.signal.app == Some(row);
        ui.horizontal(|ui| {
            let marker = if focused { "▸" } else { " " };
            ui.label(RichText::new(marker).color(ch.focus).monospace());
            title_cell(ui, row.title(), focused, ch);
            match row {
                StimRow::StopTransfers => {
                    if ui.small_button("‹").clicked() {
                        *msg = Some(Msg::Settings(SettingsMsg::Stim(row, -1)));
                    }
                    let on = st.prefs.stop_transfers_with_stimulus;
                    ui.label(RichText::new(text::on_off(on)).color(ch.text));
                    if ui.small_button("›").clicked() {
                        *msg = Some(Msg::Settings(SettingsMsg::Stim(row, 1)));
                    }
                }
                StimRow::Level => {
                    let shown = if focused {
                        text::level_typing(level, &s.signal.text)
                    } else {
                        text::level_value(level)
                    };
                    // Monospace as the page's other typed rows: the caret is its glyph.
                    let t = RichText::new(shown).monospace().color(if focused {
                        ch.focus
                    } else {
                        ch.text
                    });
                    if ui.add(egui::Button::selectable(focused, t)).clicked() {
                        *msg = Some(Msg::Settings(SettingsMsg::Stim(row, 0)));
                    }
                }
            }
        });
        ui.label(RichText::new(row.hint()).small().color(ch.dim));
        if focused {
            ui.scroll_to_cursor(Some(egui::Align::Center));
        }
    }
}
