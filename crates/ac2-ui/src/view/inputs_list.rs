//! The sidebar's Inputs list: a row per session input or pair, each bar reading what the
//! Programme pane's per-input bar reads (`ac2_scene::programme::row_block`), its number in
//! a fixed slot beside it, then each side's max, the true peak where the bar reads
//! something else, a pair's correlation and a row's note. Only the bar fills and ticks
//! move as values do: every text slot keeps its width, so nothing in the list jumps.

use ac2_scene::meter::{InputUse, MeterState};
use ac2_scene::programme::{INPUTS_LOUDNESS_TIP, RowSide, inputs_header, row_block};
use eframe::egui::{self, RichText};

use crate::app::App;
use crate::keys::{CommandId, Scope};
use crate::state::Msg;
use crate::theme::Chrome;

/// A bar line's height, logical pixels.
const BAR_H: f32 = 10.0;
/// The narrowest a bar gets in a narrow sidebar: below it the fill no longer reads.
const BAR_MIN_W: f32 = 24.0;

/// Every input of the open session, metered all the time: the operator sees what reaches
/// the mic and the reference before and during any measurement. A click picks a row for
/// the Inputs keys (reset its max, open it in a Programme pane): the message it makes.
pub(super) fn inputs(app: &App, ui: &mut egui::Ui, ch: &Chrome) -> Option<Msg> {
    let rows = app.state.session_inputs();
    if rows.is_empty() {
        return None;
    }
    let key = |c: CommandId| {
        app.keymap
            .chords(c, Scope::Inputs)
            .first()
            .map_or_else(|| "—".into(), |k| k.label())
    };
    // The rig's setting, not the frame's: the header reads before any frame arrives, and
    // each number carries its own unit through the one frame where the two differ.
    let rig = crate::state::ballistic(
        app.state
            .daemon()
            .map(|s| s.rig.levels.ballistic)
            .unwrap_or_default(),
    );
    let mut tip = format!(
        "Click picks a row · {} resets its max, overs and loudness · {} meters it in a new \
         Programme pane",
        key(CommandId::LevelsReset),
        key(CommandId::ProgrammePane)
    );
    if rig.is_loudness() {
        tip = format!("{tip} · {INPUTS_LOUDNESS_TIP}");
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Inputs").strong())
            .on_hover_text(&tip);
        let header = inputs_header(rig, &key(CommandId::LevelsReset));
        ui.add(
            egui::Label::new(RichText::new(header).small().color(ch.dim))
                .wrap_mode(egui::TextWrapMode::Truncate)
                .selectable(false),
        )
        .on_hover_text(&tip);
    });
    ui.add_space(2.0);
    let small = egui::TextStyle::Small.resolve(ui.style()).size;
    let mono = |t: &str| {
        RichText::new(t)
            .family(egui::FontFamily::Monospace)
            .size(small)
    };
    let line = |ui: &mut egui::Ui, t: RichText| {
        ui.add(
            egui::Label::new(t)
                .wrap_mode(egui::TextWrapMode::Extend)
                .selectable(false),
        );
    };
    // One width for every row's bar lines, taken once: a row's own width can differ by
    // the frame's layout pass, and the bars and numbers must line up down the list.
    let row_w = ui.available_width();
    let mut msg = None;
    for r in &rows {
        let picked = app.state.rig.input_pick == Some(r.channel);
        let Some(p) = app
            .state
            .row_meters(r.channel)
            .map(|m| app.state.programme_of(m))
        else {
            continue;
        };
        let block = row_block(
            &p.inputs,
            p.ballistic,
            p.pair.as_ref().map(|x| x.correlation),
        );
        let row = ui.vertical(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if let Some(u) = r.used {
                    let color = match u {
                        InputUse::Reference => ch.focus,
                        InputUse::Measurement => ch.ok,
                    };
                    ui.label(
                        RichText::new(u.tag())
                            .small()
                            .strong()
                            .color(ch.panel)
                            .background_color(color),
                    );
                }
                // Wrapped, never cut: the label names the mic curve or line calibration.
                ui.add(
                    egui::Label::new(RichText::new(r.label_line()).color(if picked {
                        ch.focus
                    } else {
                        ch.text
                    }))
                    .wrap_mode(egui::TextWrapMode::Wrap)
                    .selectable(false),
                );
            });
            for s in &block.sides {
                side(ui, s, ch, small, row_w, &mono);
            }
            for l in &block.lines {
                line(ui, mono(&l.text).color(flag(l.flagged, ch)));
            }
            // Negative correlation reads in the warning colours.
            if let Some(chip) = &block.correlation {
                let (fg, bg) = if chip.warning {
                    (ch.on_fault_fill, ch.fault_fill)
                } else {
                    (ch.dim, ch.panel)
                };
                line(ui, mono(&chip.text).color(fg).background_color(bg));
            }
            if let Some(n) = &block.note {
                line(ui, mono(n).color(ch.dim));
            }
        });
        let rect = row.response.rect;
        if picked {
            ui.painter().rect_stroke(
                rect.expand(2.0),
                3.0,
                egui::Stroke::new(1.0, ch.focus),
                egui::StrokeKind::Outside,
            );
        }
        let id = ui.id().with(("input-row", r.channel));
        if ui.interact(rect, id, egui::Sense::click()).clicked() {
            msg = Some(Msg::InputPick(r.channel));
        }
        ui.add_space(2.0);
    }
    ui.add_space(10.0);
    msg
}

fn flag(flagged: bool, ch: &Chrome) -> egui::Color32 {
    if flagged { ch.fault } else { ch.dim }
}

/// One side's bar line in `row_w`: its mark, the bar in the width the fixed text leaves,
/// the number.
fn side(
    ui: &mut egui::Ui,
    s: &RowSide,
    ch: &Chrome,
    small: f32,
    row_w: f32,
    mono: &dyn Fn(&str) -> RichText,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.add(egui::Label::new(mono(s.mark).color(ch.dim)).selectable(false));
        // The text's room is its slots' (a fixed count of characters) in a digit's width,
        // not the drawn text's: a sign or dash the monospace font lacks would otherwise
        // change the bar's width with the value.
        let slots = "0".repeat(s.reading.text.chars().count());
        let text_w = egui::WidgetText::from(mono(&slots))
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Small,
            )
            .size()
            .x;
        let used = ui.cursor().left() - ui.max_rect().left();
        let w = (row_w - used - text_w - 4.0).max(BAR_MIN_W);
        let (r, _) = ui.allocate_exact_size(egui::vec2(w, BAR_H), egui::Sense::hover());
        let (t, _) = ui.allocate_exact_size(egui::vec2(text_w, BAR_H), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(r, 2.0, ch.panel);
        painter.rect_stroke(
            r,
            2.0,
            egui::Stroke::new(1.0, ch.border),
            egui::StrokeKind::Inside,
        );
        // The pane's peak-bar colours: level, hot near full scale, the fault colour on CLIP.
        let color = match s.state {
            MeterState::NoData | MeterState::Silent => ch.dim,
            MeterState::Signal => ch.ok,
            MeterState::Hot => ch.armed,
            MeterState::Clip => ch.fault,
        };
        if s.fill > 0.0 {
            let mut bar = r.shrink(1.0);
            bar.set_width(bar.width() * s.fill.min(1.0));
            painter.rect_filled(bar, 1.0, color);
        }
        if let Some(t) = s.tick {
            let x = r.left() + 1.0 + (r.width() - 2.0) * t.clamp(0.0, 1.0);
            let tick = if s.state == MeterState::Clip {
                ch.fault
            } else {
                ch.text
            };
            painter.line_segment(
                [
                    egui::pos2(x, r.top() + 1.0),
                    egui::pos2(x, r.bottom() - 1.0),
                ],
                egui::Stroke::new(2.0, tick),
            );
        }
        let color = if s.reading.flagged { ch.fault } else { ch.text };
        painter.text(
            t.left_center(),
            egui::Align2::LEFT_CENTER,
            &s.reading.text,
            egui::FontId::monospace(small),
            color,
        );
    });
}
