//! The Leq windows dialog: the preset and horizon on top, one row per window (length and
//! weighting by name, limit and warn margin typed), keys drive it, the mouse can too.

use eframe::egui::{self, RichText};

use crate::app::App;
use crate::leq_dialog::{Col, Extra, Focus, LeqDialog};
use crate::state::{LeqMsg, Msg, Overlay};
use crate::theme::Chrome;

use super::overlays::{backdrop, card};

/// A choice cell: ‹ value › — clicking the value steps forward.
fn choice(
    ui: &mut egui::Ui,
    text: String,
    focused: bool,
    at: Focus,
    msg: &mut Option<LeqMsg>,
    ch: &Chrome,
) {
    if ui.small_button("‹").clicked() {
        *msg = Some(LeqMsg::Cycle(at, -1));
    }
    let t = RichText::new(text).color(ch.text);
    if ui.add(egui::Button::selectable(focused, t)).clicked() {
        *msg = Some(LeqMsg::Cycle(at, 1));
    }
    if ui.small_button("›").clicked() {
        *msg = Some(LeqMsg::Cycle(at, 1));
    }
}

/// A typed cell; the selected text (typing replaces it) shows inverted.
#[allow(clippy::too_many_arguments)]
fn text_cell(
    ui: &mut egui::Ui,
    text: &str,
    focused: bool,
    selected: bool,
    empty: &str,
    at: Focus,
    msg: &mut Option<LeqMsg>,
    ch: &Chrome,
) {
    let sel = focused && selected && !text.is_empty();
    let shown = if sel {
        text.to_owned()
    } else if focused {
        format!("{text}▏")
    } else if text.is_empty() {
        empty.to_owned()
    } else {
        text.to_owned()
    };
    let mut t = RichText::new(shown).monospace();
    t = if sel {
        t.color(ch.panel).background_color(ch.focus)
    } else if text.is_empty() && !focused {
        t.color(ch.dim)
    } else {
        t.color(ch.text)
    };
    if ui
        .add_sized([90.0, 20.0], egui::Button::selectable(focused, t))
        .clicked()
    {
        *msg = Some(LeqMsg::Focus(at));
    }
}

/// The SPL / Leq page: the Leq windows and limits of the SPL pane's meter.
pub(super) fn leq_page(ui: &mut egui::Ui, d: &LeqDialog, ch: &Chrome) -> Option<LeqMsg> {
    let mut msg = None;
    ui.set_max_width(760.0);
    ui.label(
        RichText::new(format!("Leq windows and limits — {}", d.name))
            .strong()
            .size(16.0),
    );
    if !d.calibrated {
        ui.label(
            RichText::new(
                "Not calibrated: values read dBFS and limits are not judged until \
                 the input has an SPL calibration.",
            )
            .small()
            .color(ch.warn),
        );
    }
    ui.add_space(8.0);
    egui::Grid::new("ac2-leq-top")
        .num_columns(2)
        .spacing(egui::vec2(12.0, 6.0))
        .show(ui, |ui| {
            let f = d.focus == Focus::Preset;
            ui.label(RichText::new("Preset").color(if f { ch.text } else { ch.dim }));
            ui.horizontal(|ui| {
                choice(ui, d.preset_text(), f, Focus::Preset, &mut msg, ch);
            });
            ui.end_row();
            if let Some(s) = d.preset_source() {
                ui.label("");
                ui.add(egui::Label::new(RichText::new(s).small().color(ch.dim)).wrap());
                ui.end_row();
            }
            ui.label("");
            ui.add(egui::Label::new(RichText::new(d.preset_note()).small().color(ch.dim)).wrap());
            ui.end_row();
            let f = d.focus == Focus::Horizon;
            ui.label(RichText::new("Headroom over").color(if f { ch.text } else { ch.dim }));
            ui.horizontal(|ui| {
                choice(
                    ui,
                    format!(
                        "the next {}",
                        ac2_scene::leq::length(f64::from(d.horizon_s()))
                    ),
                    f,
                    Focus::Horizon,
                    &mut msg,
                    ch,
                );
            });
            ui.end_row();
        });
    ui.add_space(8.0);
    egui::Grid::new("ac2-leq-windows")
        .num_columns(4)
        .spacing(egui::vec2(12.0, 6.0))
        .show(ui, |ui| {
            for c in [Col::Length, Col::Weighting, Col::Limit, Col::Margin] {
                ui.label(RichText::new(c.title()).small().color(ch.dim));
            }
            ui.end_row();
            for (row, r) in d.rows.iter().enumerate() {
                let at = |col| Focus::Window { row, col };
                let fo = |col| d.focus == at(col);
                ui.horizontal(|ui| {
                    ui.set_min_width(190.0);
                    choice(
                        ui,
                        r.cell(Col::Length),
                        fo(Col::Length),
                        at(Col::Length),
                        &mut msg,
                        ch,
                    );
                });
                ui.horizontal(|ui| {
                    choice(
                        ui,
                        r.cell(Col::Weighting),
                        fo(Col::Weighting),
                        at(Col::Weighting),
                        &mut msg,
                        ch,
                    );
                });
                text_cell(
                    ui,
                    &r.limit,
                    fo(Col::Limit),
                    d.selected,
                    "no limit",
                    at(Col::Limit),
                    &mut msg,
                    ch,
                );
                text_cell(
                    ui,
                    &r.margin,
                    fo(Col::Margin),
                    d.selected,
                    "0",
                    at(Col::Margin),
                    &mut msg,
                    ch,
                );
                ui.end_row();
            }
        });
    if d.rows.is_empty() {
        ui.label(RichText::new("No windows: Insert adds one.").color(ch.dim));
    }
    ui.add_space(8.0);
    egui::Grid::new("ac2-leq-extra")
        .num_columns(3)
        .spacing(egui::vec2(12.0, 6.0))
        .show(ui, |ui| {
            for e in Extra::ALL {
                let at = Focus::Extra(e);
                let f = d.focus == at;
                ui.label(RichText::new(e.title()).color(if f { ch.text } else { ch.dim }));
                text_cell(
                    ui,
                    &d.extra_text(e),
                    f,
                    d.selected,
                    e.empty(),
                    at,
                    &mut msg,
                    ch,
                );
                ui.add(egui::Label::new(RichText::new(e.note()).small().color(ch.dim)).wrap());
                ui.end_row();
            }
        });
    if let Some(e) = &d.error {
        ui.add_space(4.0);
        ui.label(RichText::new(e).color(ch.fault));
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Apply").clicked() {
            msg = Some(LeqMsg::Submit);
        }
        if ui.button("Add a window").clicked() {
            msg = Some(LeqMsg::Add);
        }
        if ui.button("Remove this window").clicked() {
            msg = Some(LeqMsg::Remove);
        }
        if ui.button("Cancel").clicked() {
            msg = Some(LeqMsg::Cancel);
        }
    });
    msg
}

/// The confirmation before a new SPL log: what ends, what starts over, what is kept.
pub(super) fn new_log(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    let Overlay::NewLog(p) = &app.state.overlay else {
        return;
    };
    let k = p.confirm.clone();
    backdrop(ctx);
    let mut msg = None;
    egui::Area::new(egui::Id::new("ac2-new-log"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(520.0);
                ui.label(RichText::new(&k.title).strong().size(16.0));
                ui.add_space(6.0);
                for (i, l) in k.lines.iter().enumerate() {
                    let t = RichText::new(l);
                    ui.label(if i == 0 {
                        t.color(ch.warn)
                    } else {
                        t.color(ch.text)
                    });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(RichText::new("Start a new log").strong())
                        .clicked()
                    {
                        msg = Some(true);
                    }
                    if ui.button("Keep the current log").clicked() {
                        msg = Some(false);
                    }
                });
                ui.label(RichText::new(&k.hint).small().color(ch.dim));
            });
        });
    if let Some(m) = msg {
        app.dispatch(Msg::NewLog(m));
    }
}
