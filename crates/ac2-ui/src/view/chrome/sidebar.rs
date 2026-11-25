//! The measurement list beside the panes: the tree's rows with their tips, the hints under
//! its headers, and the imported traces.

use ac2_scene::meas_list::{Mark, TreeKey};
use eframe::egui::{self, RichText};

use crate::app::App;
use crate::keys::{CommandId, Scope};
use crate::state::{DragMsg, Msg};
use crate::theme::Chrome;

use super::key_hint;
use super::tree_row::{RowTips, tree_row};

pub(in crate::view) fn sidebar(app: &mut App, ui: &mut egui::Ui, ch: &Chrome) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| sidebar_lists(app, ui, ch));
}

fn sidebar_lists(app: &mut App, ui: &mut egui::Ui, ch: &Chrome) {
    let edit_key = app
        .keymap
        .chords(CommandId::EditSelected, Scope::Global)
        .first()
        .map(|k| k.label());
    let tips = RowTips {
        edit: ac2_scene::meas_list::edit_tip(edit_key.as_deref()),
        meas: ac2_scene::meas_list::meas_row_tip(edit_key.as_deref()),
        select: ac2_scene::meas_list::TRACE_ROW_TIP.to_owned(),
    };
    if let Some(m) = crate::view::inputs_list::inputs(app, ui, ch) {
        app.dispatch(m);
    }
    let mut msg = None;
    let (mut under, mut row_over) = (None, None);
    let reveal = std::mem::take(&mut app.state.tree_reveal);
    let dragging = app.state.row_drag_held();
    {
        let st = &app.state;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Measurements").strong());
            ui.label(
                RichText::new(format!(
                    "{} selects a trace · {} shows / hides",
                    key_hint(app, CommandId::NextTrace),
                    key_hint(app, CommandId::ToggleSelected)
                ))
                .small()
                .color(ch.dim),
            );
            // What the rows' dots say, drawn as the rows draw them so the key matches.
            for (shown, word) in [(true, "shown"), (false, "hidden")] {
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 12.0), egui::Sense::hover());
                let c = r.center();
                if shown {
                    ui.painter().circle_filled(c, 4.0, ch.dim);
                } else {
                    ui.painter()
                        .circle_stroke(c, 3.25, egui::Stroke::new(1.25, ch.dim));
                }
                ui.label(RichText::new(word).small().color(ch.dim));
            }
        });
        ui.add_space(4.0);
        let rows = st.list_rows();
        let shown = st.active();
        let theme = ac2_scene::theme::Theme::by_name(st.theme);
        // The rig roots alone: nothing measured yet, unless a folded root hides it.
        let listed = rows
            .iter()
            .any(|(_, r)| !matches!(r.key, TreeKey::RigName(_)));
        let drag = st.row_drag().map(|d| (d.item.row(), d.onto));
        let pointer = ui.ctx().pointer_latest_pos();
        for (rig, row) in &rows {
            let (rig, here) = (*rig, (*rig, row.key));
            let reveal = reveal
                && row.mark == Mark::Active
                && matches!(
                    row.key,
                    TreeKey::Meas(_) | TreeKey::Live(_) | TreeKey::Math(_)
                );
            let reveal = reveal && rig == shown;
            let (m, rect) = tree_row(ui, (rig, shown), row, &tips, ch, &theme, reveal, dragging);
            if let Some(m) = m {
                msg = Some(m);
            }
            // A row dragged over another: the drop combines them (the math dialog opens),
            // shaded as a pane a drop would take is.
            if let Some((item, onto)) = drag {
                if pointer.is_some_and(|p| rect.contains(p))
                    && item.is_some_and(|i| i != here)
                    && !row.key.is_heading()
                {
                    under = Some(here);
                }
                if onto == Some(here) {
                    let r = rect.expand(1.0);
                    let painter = ui.painter();
                    painter.rect_filled(r, 3.0, crate::theme::c32(theme.unresolved_shade));
                    let e = theme.unresolved_edge;
                    let stroke = egui::Stroke::new(e.width, crate::theme::c32(e.color));
                    let (on, off) = e.dash.map_or((4.0, 4.0), |d| (d.on, d.off));
                    let outline = [
                        r.left_top(),
                        r.right_top(),
                        r.right_bottom(),
                        r.left_bottom(),
                        r.left_top(),
                    ];
                    painter.extend(egui::Shape::dashed_line(&outline, stroke, on, off));
                }
            }
        }
        if let Some((_, onto)) = drag
            && onto != under
        {
            row_over = Some(Msg::RowDrag(DragMsg::OverRow(under)));
        }
        if !listed && !st.rig_folded(shown) {
            ui.label(RichText::new(st.empty_list_text()).color(ch.dim));
        }
        if !rows.iter().any(|(_, r)| matches!(r.key, TreeKey::Trace(_))) && listed {
            ui.label(
                RichText::new(format!(
                    "{} captures the focused pane's measurement under it",
                    key_hint(app, CommandId::Slot1)
                ))
                .small()
                .color(ch.dim),
            );
        }
    }
    if let Some(m) = row_over {
        app.dispatch(m);
    }
    if let Some(m) = msg {
        app.dispatch(m);
    }
}
