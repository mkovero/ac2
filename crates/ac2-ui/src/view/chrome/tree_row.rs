//! One row of the measurement tree beside the panes.

use ac2_proto::model::TraceOwner;
use ac2_scene::meas_list::{Mark, TreeKey, TreeRow};
use eframe::egui::{self, Color32, RichText};

use crate::state::{DragMsg, Msg};
use crate::theme::Chrome;

/// The tooltips of the tree's rows, with the keys that do the same.
pub(super) struct RowTips {
    pub(super) meas: String,
    /// The key that shows / hides the selected curve.
    pub(super) eye: String,
    pub(super) select: String,
}

/// Width of a row's colour dot, which is also its show / hide toggle.
const EYE_W: f32 = 18.0;
/// How far a row under a header is indented.
const INDENT: f32 = 14.0;

/// How far (points) the pointer moves with the button held on a row before the row is
/// dragged: a click that wobbles a little stays a click.
const ROW_DRAG_START: f32 = 6.0;

/// One row of the measurement tree: a header (its fold arrow, its tag and name over its
/// state) or a row under it (its tree line, its dot when it has a curve to show or hide,
/// its name over what it is). `reveal`: the tree scrolls the row into view. Pressed and
/// moved, a row other than Imported's starts a drag onto the panes or another row (unless
/// one is `dragging` already). Returns the message and where the row was drawn.
pub(super) fn tree_row(
    ui: &mut egui::Ui,
    row: &TreeRow,
    tips: &RowTips,
    ch: &Chrome,
    theme: &ac2_scene::theme::Theme,
    reveal: bool,
    dragging: bool,
) -> (Option<Msg>, egui::Rect) {
    let mut click = None;
    let group = match row.key {
        TreeKey::Meas(meas) => Some(TraceOwner::Meas { meas }),
        TreeKey::Imported => Some(TraceOwner::Imported),
        _ => None,
    };
    let rect = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if let (Some(folded), Some(g)) = (row.collapsed, group) {
                let what = if folded { "Unfold" } else { "Fold" };
                let (rect, r) =
                    ui.allocate_exact_size(egui::vec2(EYE_W, 22.0), egui::Sense::click());
                let label = format!("{what} {}", row.name);
                r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
                // Drawn, not a glyph: the arrow characters are missing from the UI font.
                let c = egui::pos2(rect.center().x, rect.min.y + 9.0);
                let color = if r.hovered() { ch.text } else { ch.dim };
                let points = if folded {
                    vec![
                        c + egui::vec2(-2.5, -4.5),
                        c + egui::vec2(3.5, 0.0),
                        c + egui::vec2(-2.5, 4.5),
                    ]
                } else {
                    vec![
                        c + egui::vec2(-4.5, -2.5),
                        c + egui::vec2(4.5, -2.5),
                        c + egui::vec2(0.0, 3.5),
                    ]
                };
                ui.painter().add(egui::Shape::convex_polygon(
                    points,
                    color,
                    egui::Stroke::NONE,
                ));
                let r = r.on_hover_text(label);
                if r.clicked() {
                    click = Some(Msg::ToggleGroup(g));
                }
            } else if row.depth == 0 {
                // A header with nothing to fold (the rig's, the session's).
                ui.add_space(EYE_W);
            } else {
                ui.add_space(INDENT);
                ui.label(
                    RichText::new(if row.last { "└" } else { "├" })
                        .color(ch.dim)
                        .monospace(),
                );
            }
            if let Some(c) = row.chip {
                super::super::rig::cue_chip(ui, c, theme)
                    .on_hover_text(format!("OSC cue {}", ac2_scene::osc::state_word(c)));
            }
            // A curve's dot: filled when shown, a ring when hidden; a click shows or hides it.
            if let Some((color, shown)) = row.dot.map(|(c, shown)| (to_color32(c), shown)) {
                let (r, eye) =
                    ui.allocate_exact_size(egui::vec2(EYE_W, 22.0), egui::Sense::click());
                let what = if shown { "Hide" } else { "Show" };
                let label = format!("{what} {}", row.name);
                eye.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label)
                });
                let centre = egui::pos2(r.center().x, r.min.y + 9.0);
                if shown {
                    ui.painter().circle_filled(centre, 5.5, color);
                } else {
                    ui.painter()
                        .circle_stroke(centre, 4.5, egui::Stroke::new(1.5, color));
                }
                if eye.hovered() {
                    ui.painter()
                        .circle_stroke(centre, 8.0, egui::Stroke::new(1.0, ch.border));
                }
                let tip = format!(
                    "{what} this curve · {} shows / hides the selected one",
                    tips.eye
                );
                if eye.on_hover_text(tip).clicked() {
                    click = Some(match row.key {
                        TreeKey::Trace(id) => Msg::ToggleShown(id),
                        TreeKey::Live(id) | TreeKey::Math(id) => Msg::ToggleMeasShown(id),
                        TreeKey::Meas(_) | TreeKey::Imported | TreeKey::Rig | TreeKey::Session => {
                            return;
                        }
                    });
                }
            }
            // The longest detail line that fits beside the name, measured as drawn.
            let room = ui.available_width() - 2.0 * ui.spacing().button_padding.x;
            let small = egui::TextStyle::Small.resolve(ui.style());
            let detail = row
                .details
                .iter()
                .find(|d| {
                    ui.painter()
                        .layout_no_wrap((*d).clone(), small.clone(), ch.dim)
                        .size()
                        .x
                        <= room
                })
                .or(row.details.last())
                .cloned()
                .unwrap_or_default();
            let name_color = match row.dot {
                _ if row.hidden => ch.dim,
                Some((c, _)) => to_color32(c),
                None => ch.text,
            };
            let mut job = egui::text::LayoutJob::default();
            let body = egui::TextStyle::Body.resolve(ui.style());
            let mut fmt = egui::TextFormat::simple(body, name_color);
            if row.depth == 0 {
                fmt.font_id.size += 0.5;
            }
            job.append(&row.name, 0.0, fmt);
            job.append("\n", 0.0, egui::TextFormat::simple(small.clone(), ch.dim));
            job.append(&detail, 0.0, egui::TextFormat::simple(small, ch.dim));
            job.wrap.max_width = room;
            // Filled: the keys act on it. Outlined: still the selected measurement, while a
            // stored trace selected after it has the keys.
            let b = match row.mark {
                Mark::Active => egui::Button::selectable(true, job),
                Mark::Selected => egui::Button::new(job)
                    .fill(Color32::TRANSPARENT)
                    .stroke(egui::Stroke::new(1.0, ch.focus)),
                Mark::None => egui::Button::selectable(false, job),
            };
            let tip = match row.key {
                TreeKey::Trace(_) => format!("{}\n{}", row.describe, tips.select),
                _ => format!("{}\n{}", row.describe, tips.meas),
            };
            let draggable = !row.key.is_heading();
            let sense = if draggable {
                egui::Sense::click_and_drag()
            } else {
                egui::Sense::click()
            };
            let r = ui.add(b.wrap_mode(egui::TextWrapMode::Wrap).sense(sense));
            // No tooltip while the row is held: it would cover the drag label.
            let r = if r.is_pointer_button_down_on() {
                r
            } else {
                r.on_hover_text(tip)
            };
            if reveal {
                r.scroll_to_me(None);
            }
            let moved = ui.input(|i| {
                i.pointer
                    .press_origin()
                    .zip(i.pointer.latest_pos())
                    .map_or(0.0, |(a, b)| a.distance(b))
            });
            if draggable && !dragging && r.dragged() && moved >= ROW_DRAG_START {
                click = Some(Msg::RowDrag(DragMsg::Start(row.key)));
            }
            // A double click's second click also reads as a click: check it first, or it would
            // deselect the row the first click selected.
            if let TreeKey::Trace(id) = row.key
                && r.double_clicked()
            {
                click = Some(Msg::RenameTrace(id));
            } else if r.clicked() {
                click = Some(match row.key {
                    TreeKey::Meas(id) => Msg::SelectMeas(id),
                    TreeKey::Math(id) => Msg::SelectMath(id),
                    TreeKey::Live(id) => Msg::SelectLive(id),
                    TreeKey::Trace(id) => Msg::SelectTrace(id),
                    TreeKey::Imported => Msg::ToggleGroup(TraceOwner::Imported),
                    TreeKey::Rig => Msg::OpenRig,
                    TreeKey::Session => return,
                });
            }
        })
        .response
        .rect;
    (click, rect)
}

fn to_color32(c: ac2_scene::primitives::Color) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
        255,
    )
}
