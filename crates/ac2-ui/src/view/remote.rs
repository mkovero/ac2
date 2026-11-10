//! Remote inputs on the Audio I/O page (each under its rig's heading, with that rig's row
//! text, a meter and its state in words), the add dialog, and the Connection page's
//! *Between rigs* section. Every word comes from `ac2-scene`.

use eframe::egui::{self, RichText};

use ac2_scene::remote_inputs as words;

use crate::app::App;
use crate::remote_inputs::PickLine;
use crate::session_dialog::{Row, SessionDialog};
use crate::settings::Settings;
use crate::state::SessionMsg;
use crate::theme::Chrome;

/// The remote inputs, the add row, and the add dialog while it is open.
pub(super) fn remote_section(
    app: &App,
    ui: &mut egui::Ui,
    s: &Settings,
    focus: Option<Row>,
    ch: &Chrome,
    msg: &mut Option<SessionMsg>,
) {
    let d: &SessionDialog = &s.session;
    super::settings::section(ui, words::SECTION, "set on each input's own rig", ch);
    let rows = app.state.remote_rows();
    let mut heading: Option<&str> = None;
    for (i, r) in d.remote.iter().enumerate() {
        let Some(row) = rows.iter().find(|x| x.slot == r.slot) else {
            continue;
        };
        if heading != Some(row.rig.as_str()) {
            heading = Some(row.rig.as_str());
            ui.label(RichText::new(&row.rig).strong().color(ch.text));
        }
        let focused = focus == Some(Row::Remote(i));
        let pending = s.remote.confirm == Some(r.slot);
        ui.horizontal(|ui| {
            let marker = if focused { "▸" } else { " " };
            ui.label(RichText::new(marker).color(ch.focus).monospace());
            let t = RichText::new(&row.text).color(if focused { ch.focus } else { ch.text });
            if ui.add(egui::Button::selectable(focused, t)).clicked() {
                *msg = Some(SessionMsg::Focus(Row::Remote(i)));
            }
            super::session::meter(ui, &row.reading, ch);
            let remove = RichText::new("Remove").color(if pending { ch.fault } else { ch.dim });
            if ui
                .add(egui::Button::new(remove).small())
                .on_hover_text("Delete (twice)")
                .clicked()
            {
                *msg = Some(SessionMsg::RemoveRemote(r.slot));
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            ui.label(RichText::new(&row.state).small().color(if row.problem {
                ch.fault
            } else {
                ch.dim
            }));
        });
    }
    let focused = focus == Some(Row::AddRemote);
    ui.horizontal(|ui| {
        let marker = if focused { "▸" } else { " " };
        ui.label(RichText::new(marker).color(ch.focus).monospace());
        let t = RichText::new(format!("{}  Enter", words::ADD)).color(if focused {
            ch.focus
        } else {
            ch.text
        });
        if ui.add(egui::Button::selectable(focused, t)).clicked() {
            *msg = Some(SessionMsg::AddRemote);
        }
    });
    if let Some(p) = &s.remote.pick {
        pick_window(app, ui, p, ch, msg);
    }
}

fn pick_window(
    app: &App,
    ui: &mut egui::Ui,
    p: &crate::remote_inputs::RemotePick,
    ch: &Chrome,
    msg: &mut Option<SessionMsg>,
) {
    let lines = app.state.remote_pick_lines();
    let n = lines.iter().filter(|l| l.pick().is_some()).count();
    let focus = p.focus.min(n.saturating_sub(1));
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, ch.focus))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.label(RichText::new(words::ADD).strong().color(ch.text));
            let mut k = 0;
            for l in &lines {
                match l {
                    PickLine::Text { text, problem } => {
                        ui.label(RichText::new(text).color(if *problem {
                            ch.fault
                        } else {
                            ch.dim
                        }));
                    }
                    PickLine::Input { text, added, .. } => {
                        let pick = l.pick();
                        let focused = pick.is_some() && k == focus;
                        ui.horizontal(|ui| {
                            let marker = if focused { "▸" } else { " " };
                            ui.label(RichText::new(marker).color(ch.focus).monospace());
                            let color = match (focused, added) {
                                (true, _) => ch.focus,
                                (false, true) => ch.dim,
                                (false, false) => ch.text,
                            };
                            let t = RichText::new(text).color(color);
                            if ui.add(egui::Button::selectable(focused, t)).clicked()
                                && let Some((rig, c)) = pick
                            {
                                *msg = Some(SessionMsg::PickRemote(rig, c));
                            }
                        });
                        if pick.is_some() {
                            k += 1;
                        }
                    }
                }
            }
            if let Some(e) = &p.error {
                ui.label(RichText::new(e).color(ch.fault));
            }
            ui.label(
                RichText::new("↑↓ move · Enter takes the input · Esc closes")
                    .small()
                    .color(ch.dim),
            );
        });
}

/// The Connection page's *Between rigs*: per rig the session takes inputs from, the clocks
/// and the link.
pub(super) fn between_rigs(app: &App, ui: &mut egui::Ui, ch: &Chrome) {
    let blocks = app.state.between_rigs();
    if blocks.is_empty() {
        return;
    }
    super::settings::section(ui, words::BETWEEN_RIGS, "", ch);
    for b in blocks {
        for (i, l) in b.iter().enumerate() {
            let t = RichText::new(l);
            ui.label(if i == 0 {
                t.strong().color(ch.text)
            } else {
                t.color(ch.text)
            });
        }
        ui.add_space(4.0);
    }
}
