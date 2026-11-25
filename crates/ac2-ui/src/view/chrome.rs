//! The top bar, the rigs drop-down, the progress strip and the measurement list.

use eframe::egui::{self, Color32, RichText};

use crate::app::App;
use crate::keys::{CommandId, STOP_ANYWHERE};
use crate::state::Msg;
use crate::theme::Chrome;

use super::segment::{LINE_MIN_W, draw_line, lay_out_line};
use super::top_bar::key_hint;

mod sidebar;
mod tree_row;
pub(super) use sidebar::sidebar;
pub(super) use tree_row::ROW_DRAG_START;

fn dot(ui: &mut egui::Ui, c: Color32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), 4.5, c);
}

/// A small ▾ after an item that opens a list, as on the rig drop-down; drawn, since the UI
/// font has no such glyph.
fn chevron(ui: &mut egui::Ui, c: Color32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(BAR_DOT, 10.0), egui::Sense::hover());
    let (cx, cy) = (r.center().x, r.center().y);
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(cx - 3.5, cy - 1.5),
            egui::pos2(cx + 3.5, cy - 1.5),
            egui::pos2(cx, cy + 2.5),
        ],
        c,
        egui::Stroke::NONE,
    ));
}

/// Item spacing in the bar.
pub(super) const BAR_GAP: f32 = 8.0;
/// A separator's own width (it also takes a gap on each side).
const BAR_SEP: f32 = 6.0;
/// The status dot.
const BAR_DOT: f32 = 10.0;
/// Room the Settings gear takes at the bar's right edge.
const GEAR_W: f32 = 28.0;

pub(super) fn text_width(ui: &egui::Ui, t: &egui::WidgetText) -> f32 {
    t.clone()
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
        .size()
        .x
}

/// A label at one of the bar's fixed ends, with its tooltip.
fn fixed(ui: &mut egui::Ui, text: RichText, hover: Option<&str>) {
    let r = ui.label(text);
    if let Some(h) = hover {
        r.on_hover_text(h);
    }
}

/// The stimulus item's parts left to right, each in its colour: the state, who holds it,
/// the keys; each part after the state is led by ` · `, so the parts read as one item.
fn stim_parts(s: &ac2_scene::stim_item::StimItem, ch: &Chrome) -> Vec<egui::WidgetText> {
    let mut state = RichText::new(&s.state).color(super::segment::tone_color(ch, s.tone));
    if s.strong {
        state = state.strong();
    }
    let mut v: Vec<egui::WidgetText> = vec![state.into()];
    if let Some(h) = &s.held {
        v.push(RichText::new(format!("· {h}")).color(ch.warn).into());
    }
    if let Some(k) = &s.keys {
        v.push(RichText::new(format!("· {k}")).color(ch.dim).into());
    }
    v
}

/// The top bar: the rig shown as the rigs drop-down, with its link's dot, fixed at the
/// left; the stimulus with the stop (while anything is armed or playing), the key hints and
/// the Settings gear fixed at the right; between them the line of [`ac2_scene::top_bar`],
/// which scrolls when it is wider than the room left.
pub(super) fn top_bar(app: &mut App, ui: &mut egui::Ui, ch: &Chrome) {
    let bar = app.top_bar(std::time::Instant::now());
    let stop = app.bar_stop();
    let stim = app.bar_stim();
    let st = &app.state;
    let line = lay_out_line(ui, ch, &bar);
    let dot_color = match bar.tone {
        ac2_scene::chrome::LinkTone::Ok => ch.ok,
        ac2_scene::chrome::LinkTone::Warn => ch.warn,
        ac2_scene::chrome::LinkTone::Fault => ch.fault,
    };
    // Greyed and inert while a window is open: the window acts on the rig shown.
    let free = matches!(
        st.overlay,
        crate::state::Overlay::None | crate::state::Overlay::RigMenu(_)
    );
    let rig_text: egui::WidgetText = RichText::new(&bar.rig)
        .strong()
        .color(if free { ch.text } else { ch.dim })
        .into();
    let rig_tip = if free {
        ac2_scene::chrome::RIG_MENU_TIP
    } else {
        ac2_scene::chrome::RIG_SWITCH_WINDOW_OPEN
    };
    let keys: egui::WidgetText = RichText::new(ac2_scene::chrome::keys_hint(
        &key_hint(app, CommandId::Help),
        &key_hint(app, CommandId::Palette),
    ))
    .color(ch.dim)
    .into();
    let keys_tip = ac2_scene::chrome::keys_tip(
        &key_hint(app, CommandId::Help),
        &key_hint(app, CommandId::Palette),
        &key_hint(app, CommandId::KeyHints),
    );
    let stop_text =
        |s: &str, c: Color32| -> egui::WidgetText { RichText::new(s).strong().color(c).into() };
    // What the fixed ends take; the key hints give their room to a line that would
    // otherwise have too little to be followed.
    let sep_w = BAR_SEP + 2.0 * BAR_GAP;
    let left_w =
        BAR_DOT + BAR_GAP + text_width(ui, &rig_text) + BAR_GAP + BAR_DOT + BAR_GAP + sep_w;
    let stop_w = stop.as_ref().map_or(0.0, |s| {
        let chips: f32 = s
            .elsewhere
            .iter()
            .map(|(t, _)| text_width(ui, &stop_text(t, ch.fault)) + BAR_GAP)
            .sum();
        chips + text_width(ui, &stop_text(&s.label, ch.fault)) + BAR_GAP + sep_w
    });
    let stim_width = |s: &ac2_scene::stim_item::StimItem| -> f32 {
        let w: f32 = stim_parts(s, ch)
            .iter()
            .map(|t| text_width(ui, t) + BAR_GAP)
            .sum();
        // Its own separator unless the stop's stands beside it.
        w + if stop.is_some() { 0.0 } else { sep_w }
    };
    // The stimulus is never scrolled away: on a narrow bar it gives up its keys, then the
    // holder, then what plays, until the line keeps room to be followed.
    let fixed_room = ui.available_width() - left_w - stop_w - GEAR_W - 4.0;
    let stim = stim.map(|s| {
        (0..ac2_scene::stim_item::StimItem::STEPS)
            .map(|n| s.narrowed(n))
            .find(|n| fixed_room - stim_width(n) >= LINE_MIN_W)
            .unwrap_or_else(|| s.narrowed(ac2_scene::stim_item::StimItem::STEPS - 1))
    });
    let stim_w = stim.as_ref().map_or(0.0, stim_width);
    let keys_w = text_width(ui, &keys) + BAR_GAP + sep_w;
    let room_with_keys = ui.available_width() - left_w - stop_w - stim_w - GEAR_W - keys_w - 4.0;
    let show_keys = room_with_keys >= line.width.min(LINE_MIN_W);
    let settings_tip = format!("Settings ({})", key_hint(app, CommandId::Settings));
    let mut open_settings = false;
    let mut run = None;
    let marquee = &mut app.marquee;
    let mut wake = None;
    let mut rig_at = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = BAR_GAP;
        dot(ui, dot_color);
        let mut l = egui::Label::new(rig_text);
        if free {
            l = l.sense(egui::Sense::click());
        }
        let mut r = ui.add(l);
        if free {
            r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        rig_at = Some(r.rect);
        if r.on_hover_text(rig_tip).clicked() {
            run = Some(CommandId::Rigs);
        }
        chevron(ui, ch.dim);
        ui.separator();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let gear = ui
                .add(
                    egui::Button::new(RichText::new("⚙").size(16.0).color(ch.text))
                        .frame(false)
                        .min_size(egui::vec2(GEAR_W - BAR_GAP, 18.0)),
                )
                .on_hover_text(settings_tip);
            open_settings = gear.clicked();
            if show_keys {
                ui.label(keys).on_hover_text(keys_tip);
                ui.separator();
            }
            if let Some(s) = &stop {
                fixed(
                    ui,
                    RichText::new(&s.label).strong().color(ch.fault),
                    Some(&s.tip),
                );
            }
            // The stimulus of the rig shown, just before its stop: the two read as one
            // item, `ARMED pink −20.0 dBFS → out 1 · Enter fires · Space disarms · ■ Stop
            // stimulus: Shift+Esc`.
            if let Some(s) = &stim {
                let parts = stim_parts(s, ch);
                for t in parts.into_iter().rev() {
                    ui.label(t).on_hover_text(&s.hover);
                }
            }
            if let Some(s) = &stop {
                // A chip per rig not shown that is armed or playing, the stop beside them.
                for (text, firing) in &s.elsewhere {
                    let c = if *firing { ch.fault } else { ch.warn };
                    fixed(ui, RichText::new(text).strong().color(c), None);
                }
            }
            if stim.is_some() || stop.is_some() {
                ui.separator();
            }
            let room = ui.available_rect_before_wrap();
            let drawn = draw_line(ui, room, &line, marquee, ch);
            wake = drawn.wake;
            run = run.or(drawn.clicked);
        });
    });
    if let Some(w) = wake {
        // At the marquee's own frame period, not a predicted frame earlier.
        let dt = ui.input(|i| i.predicted_dt);
        let w = std::time::Duration::from_secs_f64(w);
        ui.ctx()
            .request_repaint_after(crate::pace::repaint_delay(w, dt));
    }
    app.rig_menu_at = rig_at;
    if open_settings {
        app.dispatch(crate::state::Msg::Command(CommandId::Settings));
    }
    if let Some(c) = run {
        app.dispatch(crate::state::Msg::Command(c));
    }
}

/// The rigs drop-down's list, under its top-bar item: each rig (greyed when down), then
/// Connect… and closing the rig shown.
pub(super) fn rig_menu(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    let crate::state::Overlay::RigMenu(menu) = app.state.overlay else {
        return;
    };
    let Some(at) = app.rig_menu_at else {
        return;
    };
    let rows = app.state.rig_menu_rows();
    let mut picked = None;
    let area = egui::Area::new(egui::Id::new("ac2-rig-menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(at.left_bottom() + egui::vec2(0.0, 4.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(200.0);
                for (i, row) in rows.iter().enumerate() {
                    let (text, color) = rig_row_text(&app.state, *row, ch);
                    let r = ui.add(
                        egui::Button::selectable(i == menu.index, RichText::new(text).color(color))
                            .min_size(egui::vec2(200.0, 0.0)),
                    );
                    if r.clicked() {
                        picked = Some(*row);
                    }
                }
            });
        });
    let outside = ctx.input(|i| i.pointer.any_click())
        && !area.response.contains_pointer()
        && !at.contains(ctx.pointer_interact_pos().unwrap_or_default());
    if let Some(r) = picked {
        app.dispatch(crate::state::Msg::RigPick(r));
    } else if outside {
        app.dispatch(crate::state::Msg::Command(CommandId::Rigs));
    }
}

/// A row of the rigs drop-down as drawn.
fn rig_row_text(
    st: &crate::state::AppState,
    row: crate::state::RigRow,
    ch: &Chrome,
) -> (String, Color32) {
    use crate::state::RigRow;
    match row {
        RigRow::Rig(id) => {
            let down = st.find_rig(id).and_then(|r| r.conn.down());
            let text = ac2_scene::chrome::rig_row(&st.rig_name(id), down);
            (text, if down.is_none() { ch.text } else { ch.dim })
        }
        RigRow::Connect => (ac2_scene::chrome::RIG_CONNECT.to_owned(), ch.text),
        RigRow::Close(id) => (ac2_scene::chrome::rig_close(&st.rig_name(id)), ch.text),
    }
}

/// The running operation: what, which step, a bar, time left, and Stop.
/// The progress strip drawn over the bottom of the pane area `over`, translucent so the
/// panes keep their size and place under it.
pub(super) fn progress_overlay(
    app: &mut App,
    ctx: &egui::Context,
    ch: &Chrome,
    p: &ac2_scene::progress::Progress,
    over: egui::Rect,
) {
    let w = (over.width() - 24.0).clamp(1.0, 1100.0);
    let fill = Color32::from_rgba_unmultiplied(ch.panel.r(), ch.panel.g(), ch.panel.b(), 242);
    egui::Area::new(egui::Id::new("ac2-progress"))
        .order(egui::Order::Middle)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(egui::pos2(over.center().x, over.bottom() - 8.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, ch.armed))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_width(w - 20.0);
                    progress(app, ui, ch, p);
                });
        });
}

fn progress(app: &mut App, ui: &mut egui::Ui, ch: &Chrome, p: &ac2_scene::progress::Progress) {
    let stop_key = STOP_ANYWHERE.label();
    let mut stop = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(RichText::new(&p.title).strong().size(15.0).color(ch.text));
        ui.label(RichText::new(&p.detail).color(ch.dim));
        ui.label(RichText::new(&p.step).strong().size(15.0).color(ch.armed));
        ui.add(
            egui::ProgressBar::new(p.fraction)
                .desired_width(260.0)
                .fill(ch.armed),
        );
        if let Some(r) = &p.remaining {
            ui.label(RichText::new(r).color(ch.text));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let b = egui::Button::new(
                RichText::new(format!("■ Stop ({stop_key})"))
                    .strong()
                    .color(ch.on_fault_fill),
            )
            .fill(ch.fault_fill)
            .min_size(egui::vec2(96.0, 22.0));
            if ui
                .add(b)
                .on_hover_text(format!(
                    "Fades the output out, disarms and discards this run · {stop_key} from \
                     anywhere (Esc too while no window is open)"
                ))
                .clicked()
            {
                stop = true;
            }
        });
    });
    if stop {
        app.dispatch(Msg::Command(CommandId::StimulusStop));
    }
}
