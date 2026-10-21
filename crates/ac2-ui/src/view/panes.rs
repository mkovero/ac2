//! The pane tree on screen: each pane's rectangle from the tree (`crate::state::PaneNode`),
//! its frame, title and plot; one plot renderer slot per pane.

use std::sync::Arc;

use ac2_scene::axis::Mapping;
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;
use eframe::egui;

use ac2_scene::view::{DistortionUnit, IrPane};

use crate::app::{App, CachedScene, PlateDrag, PlateGrab};
use crate::hints::{self, KeyHint};
use crate::keys::{CommandId, Scope};
use crate::plot::{self, PlotSlot};
use crate::scenes;
use crate::state::{
    Axis, DragMsg, HintPlace, IrNavMsg, Msg, Overlay, PaneId, PaneKind, PaneMenuRow, PaneRect,
    PlateMsg, StripMsg, TransferView, drop_target,
};
use crate::theme::Chrome;

const GAP: f32 = 6.0;
const TITLE_H: f32 = 20.0;
/// The focused pane's key-hint line, under its plot.
pub(crate) const HINT_H: f32 = 18.0;
/// Text size of the hint line.
const HINT_FONT: f32 = 11.5;
/// Left and right inset of the hint line's text.
const HINT_PAD: f32 = 8.0;

/// Pane rectangles inside `area` for the panes drawn now.
pub(crate) fn layout(st: &crate::state::AppState, area: egui::Rect) -> Vec<(PaneId, egui::Rect)> {
    let r = PaneRect::new(area.min.x, area.min.y, area.width(), area.height());
    st.pane_rects(r, GAP)
        .into_iter()
        .map(|(id, r)| {
            (
                id,
                egui::Rect::from_min_size(egui::pos2(r.x, r.y), egui::vec2(r.w, r.h)),
            )
        })
        .collect()
}

/// Height of a plot's caption in the stage view.
const STAGE_CAPTION_H: f32 = 22.0;

/// The stage view's caption of `pane`: `Transfer · Main L`; none for the SPL pane, whose
/// meter and Leq windows name themselves.
fn stage_caption(st: &crate::state::AppState, id: PaneId) -> Option<String> {
    let view = st.layout.view(id)?;
    if view.kind == PaneKind::Spl {
        return None;
    }
    let parts: Vec<String> = [
        Some(view.title().to_owned()),
        st.pane_meas(id).map(|m| m.config.name.clone()),
        st.pane_caption(id),
    ]
    .into_iter()
    .flatten()
    .collect();
    Some(parts.join(" · "))
}

fn slot(p: PaneId) -> PlotSlot {
    PlotSlot(p.0)
}

/// The axes of a pane's scene the mouse navigates: frequency, and level in dB (or, on a
/// log scale, in percent of the fundamental); or an IR picture's time and value axes.
#[derive(Clone, Copy, Debug, Default)]
struct Axes {
    x: Option<Mapping>,
    y_level: Option<Mapping>,
    /// The spectrograph's time axis (seconds before the newest frame), when shown: a
    /// click there puts the cursor on a time as well as a frequency.
    time: Option<Mapping>,
    /// An IR picture: which one, its time axis and its value axis.
    ir: Option<(IrPane, Mapping, Mapping)>,
    /// An SPL pane's Leq history strip: where and over what time.
    strip: Option<scenes::StripPlace>,
}

/// Scene for `pane` at `size`, from the cache when nothing changed.
fn scene_for(
    app: &mut App,
    id: PaneId,
    size: egui::Vec2,
    theme: &Theme,
) -> Option<(Arc<ac2_plot::Scene>, Axes)> {
    let view = app.state.layout.view(id)?.to_owned();
    let pane = view.kind;
    let freq_right = app.state.column_right(id);
    if let Some(c) = app.scenes.get(&id)
        && c.view == view
        && c.generation == app.pane_generation(pane)
        && c.size == size
        && c.freq_right == freq_right
        && c.theme == app.state.theme
    {
        let axes = Axes {
            x: c.x_axis,
            y_level: c.y_level,
            time: c.time_axis,
            ir: c.ir_axes,
            strip: c.strip,
        };
        return Some((c.scene.clone(), axes));
    }
    let vp = Viewport {
        width: size.x,
        height: size.y,
    };
    let now = super::now();
    let st = &app.state;
    let mut unit_tip = None;
    let mut legend = None;
    let mut plates = Vec::new();
    let mut legend_hits = Vec::new();
    let (scene, axes) = match pane {
        PaneKind::Transfer if view.shows_ir() => {
            let s = scenes::ir(st, id, &app.keymap, theme, vp, now)?;
            // Navigation needs an IR: the empty pane's axes only frame its reason.
            let ir = st
                .ir_extent(IrPane::Live)
                .map(|_| (IrPane::Live, s.x_axis.mapping, s.y_axis.mapping));
            (
                s.scene,
                Axes {
                    ir,
                    ..Axes::default()
                },
            )
        }
        PaneKind::Transfer if view.modes.transfer == TransferView::Alignment => {
            let s = scenes::alignment(st, id, theme, vp, now);
            (
                s.scene,
                Axes {
                    x: Some(s.x_axis.mapping),
                    ..Axes::default()
                },
            )
        }
        PaneKind::Transfer => {
            let s = scenes::transfer(st, id, theme, vp, now);
            plates = s.plates();
            legend = s.legend_box;
            legend_hits = s.legend_hits;
            let y = s
                .panes
                .iter()
                .find(|p| p.kind == ac2_scene::tf::TfPaneKind::Magnitude)
                .map(|p| p.y_axis.mapping);
            (
                s.scene,
                Axes {
                    x: Some(s.x_axis.mapping),
                    y_level: y,
                    time: None,
                    ir: None,
                    strip: None,
                },
            )
        }
        PaneKind::Spectrum if view.modes.spectrum.spectrograph() => {
            let s = scenes::spectrograph(st, id, theme, vp, now);
            if let Some(sp) = &s.spectrum {
                plates = sp.plates();
                legend_hits = sp.legend_hits.clone();
                let r = sp.unit_rect;
                unit_tip = sp.unit_help.clone().map(|h| {
                    (
                        egui::Rect::from_min_size(egui::pos2(r.x, r.y), egui::vec2(r.w, r.h)),
                        h,
                    )
                });
            }
            (
                s.scene,
                Axes {
                    x: Some(s.x_axis.mapping),
                    y_level: s.spectrum.as_ref().map(|sp| sp.y_axis.mapping),
                    time: Some(s.time_axis.mapping),
                    ir: None,
                    strip: None,
                },
            )
        }
        PaneKind::Spectrum => {
            let s = scenes::spectrum(st, id, theme, vp, now);
            plates = s.plates();
            legend_hits = s.legend_hits;
            let r = s.unit_rect;
            unit_tip = s.unit_help.map(|h| {
                (
                    egui::Rect::from_min_size(egui::pos2(r.x, r.y), egui::vec2(r.w, r.h)),
                    h,
                )
            });
            (
                s.scene,
                Axes {
                    x: Some(s.x_axis.mapping),
                    y_level: Some(s.y_axis.mapping),
                    time: None,
                    ir: None,
                    strip: None,
                },
            )
        }
        PaneKind::Spl => {
            let (scene, strip) = scenes::spl_pane(st, id, &app.keymap, theme, vp, now)?;
            (
                scene,
                Axes {
                    strip,
                    ..Axes::default()
                },
            )
        }
        PaneKind::Distortion => {
            let s = scenes::sweep(st, id, theme, vp, now);
            let axes = Axes {
                x: s.x_axis(),
                y_level: s.y_level(),
                time: None,
                ir: s.ir_axes().map(|(x, y)| (IrPane::Sweep, x, y)),
                strip: None,
            };
            (s.scene(), axes)
        }
    };
    let scene = Arc::new(scene);
    app.scenes.insert(
        id,
        CachedScene {
            view,
            generation: app.pane_generation(pane),
            size,
            freq_right,
            theme: app.state.theme,
            scene: scene.clone(),
            x_axis: axes.x,
            y_level: axes.y_level,
            time_axis: axes.time,
            ir_axes: axes.ir,
            strip: axes.strip,
            unit_tip,
            legend,
            plates,
            legend_hits,
        },
    );
    Some((scene, axes))
}

fn placeholder(id: PaneId, app: &App) -> String {
    let modes = app.state.pane_modes(id);
    match app.state.layout.kind(id) {
        PaneKind::Transfer if modes.transfer.ir().is_some() => "no transfer measurement".into(),
        PaneKind::Spl => scenes::spl_placeholder(&app.state, id, &app.keymap),
        _ => String::new(),
    }
}

pub(super) fn panes(app: &mut App, ui: &mut egui::Ui, theme: &Theme, ch: &Chrome) {
    let area = ui.available_rect_before_wrap();
    // The reducer lays the tree out in this area to split the focused pane along its longer
    // side and to refuse a split too small to read.
    app.state.pane_area = (area.width(), area.height());
    // The stage view is the pane's picture alone: no frame, no title.
    let stage = app.state.stage_view();
    // Numbered in reading order, as Alt+1 … 9 count them.
    let order = app.state.layout.panes();
    app.pane_rects = layout(&app.state, area);
    for (id, rect) in layout(&app.state, area) {
        // Each pane's widgets live under its own id and are clipped to its rectangle: two
        // panes of one kind draw the same controls, which must stay distinct widgets, and a
        // title that runs long must neither paint nor take clicks past its pane.
        let mut pane_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(("pane-scope", id.0))
                .max_rect(rect),
        );
        pane_ui.set_clip_rect(rect.intersect(ui.clip_rect()));
        pane(app, &mut pane_ui, theme, ch, id, rect, stage, &order);
    }
    if !stage {
        split_gaps(app, ui, area);
    }
    ui.allocate_rect(area, egui::Sense::hover());
}

/// One pane in `rect`: its frame, title strip, plot and hint line (or, on the stage, its
/// picture alone).
#[allow(clippy::too_many_arguments)]
fn pane(
    app: &mut App,
    ui: &mut egui::Ui,
    theme: &Theme,
    ch: &Chrome,
    id: PaneId,
    rect: egui::Rect,
    stage: bool,
    order: &[PaneId],
) {
    let pane = app.state.layout.kind(id);
    let pane_title = app
        .state
        .layout
        .view(id)
        .map_or(pane.title(), |v| v.title());
    if stage {
        // The SPL meter and its Leq windows carry their own captions; a plot gets a slim
        // one naming the pane and its measurement, and nothing else.
        let rect = match stage_caption(&app.state, id) {
            Some(text) => {
                let strip =
                    egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), STAGE_CAPTION_H));
                ui.painter().text(
                    strip.left_center() + egui::vec2(10.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    text,
                    egui::FontId::proportional(13.0),
                    ch.dim,
                );
                egui::Rect::from_min_max(
                    egui::pos2(rect.min.x, rect.min.y + STAGE_CAPTION_H),
                    rect.max,
                )
            }
            None => rect,
        };
        let resp = ui.interact(
            rect,
            ui.id().with(("stage", id.0)),
            egui::Sense::click_and_drag(),
        );
        let built = if app.plots {
            scene_for(app, id, rect.size(), theme)
        } else {
            None
        };
        let axes = built.as_ref().map(|b| b.1).unwrap_or_default();
        match built {
            Some((scene, _)) => plot::paint(ui, slot(id), rect, scene),
            None => {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    placeholder(id, app),
                    egui::FontId::proportional(13.0),
                    ch.dim,
                );
            }
        }
        // Full screen keeps the mouse: wheel zooms frequency, Ctrl/Shift+wheel the level
        // axis, as in the split layout.
        let took = plates_mouse(app, ui, &resp, id, rect);
        navigate(app, ui, &resp, id, rect, axes, took);
        return;
    }
    let focused = app.state.layout.focus == id;
    let stroke = if focused {
        egui::Stroke::new(1.5, ch.focus)
    } else {
        egui::Stroke::new(1.0, ch.border)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, ch.raised);
    painter.rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);
    let title = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), TITLE_H));
    let n = order.iter().position(|p| *p == id).unwrap_or(0) + 1;
    let label = painter.text(
        title.left_center() + egui::vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        format!("{n}  {pane_title}"),
        egui::FontId::proportional(12.0),
        if focused { ch.text } else { ch.dim },
    );
    let style = crate::keys::label_style();
    let line = app.state.key_hint_line(&app.keymap, id, style);
    // The hint line takes its strip off the plot's bottom: it never covers a curve.
    let bottom = rect.max.y - 2.0 - if line.is_some() { HINT_H } else { 0.0 };
    let plot_rect = egui::Rect::from_min_max(
        egui::pos2(rect.min.x + 2.0, rect.min.y + TITLE_H),
        egui::pos2(rect.max.x - 2.0, bottom),
    );
    let resp = ui.interact(
        rect,
        ui.id().with(("pane", id.0)),
        egui::Sense::click_and_drag(),
    );
    // After the pane's own response, so a hover or click there is theirs.
    let all = app.state.pane_hints(&app.keymap, id, style);
    let name_rect =
        egui::Rect::from_min_max(title.min, egui::pos2(label.right() + 4.0, title.max.y));
    let name = ui.interact(
        name_rect,
        ui.id().with(("pane-title", id.0)),
        egui::Sense::hover(),
    );
    name.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, format!("{n}  {pane_title}"))
    });
    // No tooltip on the name: it would open over the top of the plot (where the legend
    // sits) whenever the pointer crosses the title; the hint line carries it instead.
    if let Some(items) = &line {
        let strip = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + 2.0, bottom),
            egui::pos2(rect.max.x - 2.0, rect.max.y - 2.0),
        );
        hint_line(ui, strip, items, ch);
        let line = ui.interact(
            strip,
            ui.id().with(("pane-hints", id.0)),
            egui::Sense::hover(),
        );
        line.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Label, true, format!("{pane_title} keys"))
        });
        line.on_hover_ui(|ui| hints_tooltip(ui, &app.keymap, pane, pane_title, &all, ch));
    }
    let mut x = label.right() + 10.0;
    let mut right = title.right() - 8.0;
    // The unit toggle keeps its place beside the shortest chip; the chip takes what is left.
    let chip_reserve = if app.state.pane_meas(id).is_some() {
        CHIP_MIN_W + 10.0
    } else {
        0.0
    };
    if pane == PaneKind::Distortion
        && app.state.pane_modes(id).sweep == ac2_scene::view::SweepMode::Response
        && let Some(left) = unit_toggle(app, ui, title, x + chip_reserve, ch)
    {
        right = left - 10.0;
    }
    let has_caption = !app.state.pane_caption_variants(id).is_empty();
    let chip_max = chip_budget(right - x, has_caption);
    if let Some(chip) = title_chip(app, ui, id, title, x, chip_max, ch) {
        x = chip.right() + 10.0;
    }
    let caption_end = caption(app, ui, id, title, x, right, ch);
    if plot_rect.width() < 8.0 || plot_rect.height() < 8.0 {
        return;
    }
    let built = if app.plots {
        scene_for(app, id, plot_rect.size(), theme)
    } else {
        None
    };
    // What the level axis means, over its unit (a spectrum's per-bin levels).
    let tip = app.scenes.get(&id).and_then(|c| c.unit_tip.clone());
    let resp = match tip {
        Some((r, text))
            if built.is_some()
                && resp
                    .hover_pos()
                    .is_some_and(|p| r.translate(plot_rect.min.to_vec2()).contains(p)) =>
        {
            resp.on_hover_text(text)
        }
        _ => resp,
    };
    match &built {
        Some((scene, _)) => plot::paint(ui, slot(id), plot_rect, scene.clone()),
        None => {
            let text = if app.plots {
                placeholder(id, app)
            } else {
                "plots need the wgpu renderer".to_owned()
            };
            ui.painter().text(
                plot_rect.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(13.0),
                ch.dim,
            );
        }
    }
    // Hidden under a dialog, which is already the next step.
    if app.state.overlay == crate::state::Overlay::None
        && let Some(hint) = app.state.empty_hint(id, &app.keymap)
    {
        match hint.place {
            HintPlace::Centre => empty_hint(ui, plot_rect, &hint.text, ch),
            HintPlace::Title => title_hint(ui, title, caption_end + 16.0, &hint.text, ch),
        }
    }
    let took = plates_mouse(app, ui, &resp, id, plot_rect);
    navigate(
        app,
        ui,
        &resp,
        id,
        plot_rect,
        built.map(|b| b.1).unwrap_or_default(),
        took,
    );
}

/// How far past the gap's own strip the mouse still grabs it: the gap is a few points
/// wide, too narrow to hit at a glance.
const GAP_GRAB: f32 = 3.0;

/// The gaps between panes: dragging one moves its split live, where the reducer lays the
/// tree out. Registered after the panes so the strip's slight overlap with their edges is
/// the gap's.
fn split_gaps(app: &mut App, ui: &egui::Ui, area: egui::Rect) {
    if app.state.window_over_panes() {
        return;
    }
    let r = PaneRect::new(area.min.x, area.min.y, area.width(), area.height());
    for g in app.state.visible_tree().split_gaps(r, GAP) {
        let rect =
            egui::Rect::from_min_size(egui::pos2(g.gap.x, g.gap.y), egui::vec2(g.gap.w, g.gap.h))
                .expand(GAP_GRAB);
        let resp = ui.interact(
            rect,
            ui.id().with(("split_gap", &g.path)),
            egui::Sense::drag(),
        );
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(match g.axis {
                Axis::Row => egui::CursorIcon::ResizeHorizontal,
                Axis::Column => egui::CursorIcon::ResizeVertical,
            });
        }
        if resp.dragged()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let pos = match g.axis {
                Axis::Row => p.x,
                Axis::Column => p.y,
            };
            let ratio = g.ratio_at(pos);
            app.dispatch(Msg::DragSplit {
                path: g.path,
                ratio,
            });
        }
    }
}

/// A list row dragged over the panes: the pane (or the half of it a drop near its side
/// splits off) shaded faintly with a dashed edge, as the resolution marker shades a range,
/// and a label by the pointer naming where the row goes. Dropped on release, Shift held
/// comparing; off the panes, nothing happens.
pub(super) fn row_drag(app: &mut App, ctx: &egui::Context, theme: &Theme, ch: &Chrome) {
    if !app.state.row_drag_held() {
        return;
    }
    let (pos, down, shift) = ctx.input(|i| {
        (
            i.pointer.latest_pos(),
            i.pointer.primary_down(),
            i.modifiers.shift,
        )
    });
    let over = if app.state.window_over_panes() {
        None
    } else {
        let rects: Vec<(PaneId, PaneRect)> = app
            .pane_rects
            .iter()
            .map(|(id, r)| (*id, PaneRect::new(r.min.x, r.min.y, r.width(), r.height())))
            .collect();
        pos.and_then(|p| drop_target(&rects, p.x, p.y))
    };
    if !down {
        app.dispatch(Msg::RowDrag(DragMsg::Drop { compare: shift }));
        return;
    }
    if app
        .state
        .row_drag()
        .is_some_and(|d| d.over != over || d.compare != shift)
    {
        app.dispatch(Msg::RowDrag(DragMsg::Over {
            target: over,
            compare: shift,
        }));
    }
    if app.state.row_drag().is_none() {
        return;
    }
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("row-drag"),
    ));
    if let Some((pane, edge)) = app.state.drop_highlight()
        && let Some(r) = app
            .pane_rects
            .iter()
            .find(|(id, _)| *id == pane)
            .map(|x| x.1)
    {
        use ac2_scene::pane_edit::Edge;
        let (w, h) = (r.width() / 2.0, r.height() / 2.0);
        let r = match edge {
            None => r,
            Some(Edge::Top) => egui::Rect::from_min_size(r.min, egui::vec2(r.width(), h)),
            Some(Edge::Bottom) => egui::Rect::from_min_max(egui::pos2(r.min.x, r.min.y + h), r.max),
            Some(Edge::Left) => egui::Rect::from_min_size(r.min, egui::vec2(w, r.height())),
            Some(Edge::Right) => egui::Rect::from_min_max(egui::pos2(r.min.x + w, r.min.y), r.max),
        }
        .shrink(3.0);
        painter.rect_filled(r, 4.0, crate::theme::c32(theme.unresolved_shade));
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
    if let (Some(text), Some(p)) = (app.state.drag_label(), pos) {
        let galley = painter.layout_no_wrap(text, egui::FontId::proportional(13.0), ch.text);
        let at = p + egui::vec2(16.0, 12.0);
        let plate = egui::Rect::from_min_size(at, galley.size()).expand2(egui::vec2(6.0, 3.0));
        painter.rect_filled(plate, 4.0, ch.raised);
        painter.rect_stroke(
            plate,
            4.0,
            egui::Stroke::new(1.0, ch.border),
            egui::StrokeKind::Outside,
        );
        painter.galley(at, galley, ch.text);
    }
}

mod mouse;
mod title;
use mouse::*;
use title::*;

#[cfg(test)]
mod tests;
