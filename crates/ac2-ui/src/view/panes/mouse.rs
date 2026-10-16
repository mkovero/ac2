//! The mouse on a pane: the legend, frequency-axis and IR navigation.

use super::*;

/// Click focuses (and on a frequency axis places the cursor); wheel zooms the frequency
/// axis about the pointer, Ctrl+wheel the level axis, Shift+wheel pans the level axis; drag
/// pans the frequency axis. Navigation only: values stay as received.
/// What the legend took of the mouse this pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LegendTook {
    Nothing,
    /// Clicks and drags: the pointer is on the plate, or dragging it.
    Pointer,
    /// The wheel too: its rows scroll.
    PointerAndWheel,
}

/// The mouse on the transfer pane's legend: a drag on the plate moves it, a drag on its
/// bottom-right grip resizes it, the wheel scrolls rows that do not all fit. Places and
/// sizes come from the scene's box, so the reducer only stores them.
pub(super) fn legend_mouse(
    app: &mut App,
    ui: &egui::Ui,
    resp: &egui::Response,
    id: PaneId,
    plot_rect: egui::Rect,
) -> LegendTook {
    use ac2_scene::legend::{LegendHover, ROW};
    if app.state.layout.kind(id) != PaneKind::Transfer {
        return LegendTook::Nothing;
    }
    let b = app.scenes.get(&id).and_then(|c| c.legend.clone());
    let hover_now = app.state.view.tf.legend.hover;
    let origin = plot_rect.min.to_vec2();
    let screen = |r: ac2_scene::primitives::Rect| {
        egui::Rect::from_min_size(egui::pos2(r.x, r.y) + origin, egui::vec2(r.w, r.h))
    };
    app.legend_screen = b.as_ref().map(|b| (screen(b.rect), screen(b.grip)));
    let Some(b) = b.filter(|_| !app.state.window_over_panes()) else {
        app.legend_drag = None;
        if hover_now.is_some() {
            app.dispatch(Msg::Legend(LegendMsg::Hover(None)));
        }
        return LegendTook::Nothing;
    };
    let inside = |r: ac2_scene::primitives::Rect, p: egui::Pos2| {
        (r.x..=r.right()).contains(&p.x) && (r.y..=r.bottom()).contains(&p.y)
    };
    let over_at = |p: egui::Pos2| {
        let p = p - origin;
        if inside(b.grip, p) {
            Some(LegendHover::Grip)
        } else if inside(b.rect, p) {
            Some(LegendHover::Plate)
        } else {
            None
        }
    };
    if resp.drag_started()
        && let Some(p) = ui.input(|i| i.pointer.press_origin())
    {
        let at = p - origin;
        app.legend_drag = match over_at(p) {
            Some(LegendHover::Plate) => Some(LegendDrag::Move {
                grab: at - egui::pos2(b.rect.x, b.rect.y),
            }),
            Some(LegendHover::Grip) => Some(LegendDrag::Resize {
                grab: at - egui::pos2(b.rect.right(), b.rect.bottom()),
            }),
            None => None,
        };
    }
    if let Some(d) = app.legend_drag {
        if !resp.dragged() {
            app.legend_drag = None;
        } else if let Some(p) = resp.interact_pointer_pos() {
            let p = p - origin;
            let (msg, icon, kind) = match d {
                LegendDrag::Move { grab } => {
                    let [x, y] = b.position_for(p.x - grab.x, p.y - grab.y);
                    (
                        LegendMsg::Move { x, y },
                        egui::CursorIcon::Grabbing,
                        LegendHover::Plate,
                    )
                }
                LegendDrag::Resize { grab } => {
                    let [w, h] = b.limits_for(p.x - grab.x, p.y - grab.y);
                    (
                        LegendMsg::Resize {
                            max_width: w,
                            max_height: h,
                        },
                        egui::CursorIcon::ResizeNwSe,
                        LegendHover::Grip,
                    )
                }
            };
            ui.ctx().set_cursor_icon(icon);
            if hover_now != Some(kind) {
                app.dispatch(Msg::Legend(LegendMsg::Hover(Some(kind))));
            }
            app.dispatch(Msg::Legend(msg));
            return LegendTook::Pointer;
        }
    }
    let over = resp.hover_pos().and_then(over_at);
    if over != hover_now {
        app.dispatch(Msg::Legend(LegendMsg::Hover(over)));
    }
    let Some(over) = over else {
        app.legend_wheel = 0.0;
        return LegendTook::Nothing;
    };
    ui.ctx().set_cursor_icon(match over {
        LegendHover::Plate => egui::CursorIcon::Grab,
        LegendHover::Grip => egui::CursorIcon::ResizeNwSe,
    });
    // Ctrl / Shift + wheel still zoom and pan the level axis.
    let (dy, plain) = ui.input(|i| {
        (
            i.smooth_scroll_delta.y,
            !i.modifiers.command && !i.modifiers.shift,
        )
    });
    if !b.scrolls() || !plain {
        return LegendTook::Pointer;
    }
    // Wheel down (negative delta) shows later rows, a row per row height of travel.
    app.legend_wheel -= dy / ROW;
    let rows = app.legend_wheel.trunc();
    if rows != 0.0 {
        app.legend_wheel -= rows;
        let first = b.scrolled(rows as i32);
        if first != app.state.view.tf.legend.first {
            app.dispatch(Msg::Legend(LegendMsg::Scroll { first }));
        }
    }
    LegendTook::PointerAndWheel
}

#[allow(clippy::too_many_arguments)]
pub(super) fn navigate(
    app: &mut App,
    ui: &egui::Ui,
    resp: &egui::Response,
    id: PaneId,
    plot_rect: egui::Rect,
    axes: Axes,
    legend: LegendTook,
) {
    let pane = app.state.layout.kind(id);
    // A window over the panes owns the mouse: its wheel scrolls the window, never zooms a
    // plot behind it.
    if app.state.window_over_panes() {
        return;
    }
    // A click (not a drag: that moves the plate) on a legend row selects that row's curve.
    let row_at = |app: &App, pos: Option<egui::Pos2>| {
        let p = pos? - plot_rect.min.to_vec2();
        let c = app.scenes.get(&id)?;
        let on_grip = c.legend.as_ref().is_some_and(|b| {
            (b.grip.x..=b.grip.right()).contains(&p.x)
                && (b.grip.y..=b.grip.bottom()).contains(&p.y)
        });
        if on_grip {
            return None;
        }
        ac2_scene::legend::hit_at(&c.legend_hits, p.x, p.y)
    };
    if resp.clicked()
        && let Some(key) = row_at(app, resp.interact_pointer_pos())
    {
        app.dispatch(Msg::LegendPick(id, key));
        return;
    }
    if resp.clicked() || resp.drag_started() {
        app.dispatch(Msg::FocusPane(id));
    }
    // The spectrum's legend is no plate to drag: the pointer over an entry says it clicks.
    if legend == LegendTook::Nothing && row_at(app, resp.hover_pos()).is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // A click on the legend is not a cursor placed behind it, a drag on it not a pan; when
    // its rows scroll, the wheel over it is theirs.
    if legend == LegendTook::PointerAndWheel {
        return;
    }
    let free = legend == LegendTook::Nothing;
    if let Some((which, x, y)) = axes.ir {
        navigate_ir(app, ui, resp, plot_rect, which, x, y);
        return;
    }
    if pane == PaneKind::Spl {
        navigate_strip(app, ui, resp, id, plot_rect, axes.strip);
        return;
    }
    if resp.hovered()
        && app.state.level_range(id).is_some()
        && (pane != PaneKind::Distortion
            || app.state.pane_modes(id).sweep == ac2_scene::view::SweepMode::Response)
    {
        // egui turns Ctrl+wheel into a zoom factor and Shift+wheel into horizontal scroll.
        let (zoom, shift, dx, pos) = ui.input(|i| {
            let zoom = if i.modifiers.command {
                i.zoom_delta()
            } else {
                1.0
            };
            (
                zoom,
                i.modifiers.shift,
                i.smooth_scroll_delta.x,
                i.pointer.hover_pos(),
            )
        });
        // A log level axis is the distortion in percent of the fundamental: the level
        // under the pointer is the dB that percent stands for.
        let about_db = axes
            .y_level
            .zip(pos)
            .map(|(m, p)| {
                let v = m.from_px(p.y - plot_rect.min.y);
                match m.scale {
                    ac2_scene::axis::Scale::Linear => v,
                    ac2_scene::axis::Scale::Log => ac2_scene::distortion::db_of_percent(v),
                }
            })
            .filter(|v| v.is_finite());
        if zoom != 1.0 {
            app.dispatch(Msg::LevelZoom {
                pane: id,
                about_db,
                factor: f64::from(zoom),
            });
        }
        if shift
            && dx != 0.0
            && let Some(r) = app.state.level_range(id)
        {
            // Wheel up (positive delta) shows higher levels; about a tenth of the span per
            // notch (50 px of scroll).
            app.dispatch(Msg::LevelPan {
                pane: id,
                db: f64::from(dx / 500.0) * r.span(),
            });
        }
    }
    let Some(m) = axes.x else {
        return;
    };
    let hz_at = |pos: egui::Pos2| m.from_px(pos.x - plot_rect.min.x);
    if free
        && resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let y = p.y - plot_rect.min.y;
        let in_time = axes.time.filter(|t| {
            let (a, b) = (t.px_lo.min(t.px_hi), t.px_lo.max(t.px_hi));
            let x = p.x - plot_rect.min.x;
            (a..=b).contains(&y) && (m.px_lo..=m.px_hi).contains(&x)
        });
        match in_time {
            Some(t) => app.dispatch(Msg::SpectrographCursor {
                hz: hz_at(p),
                before_s: t.from_px(y).max(0.0),
            }),
            None => app.dispatch(Msg::CursorAt(Some(hz_at(p)))),
        }
    }
    if free && resp.dragged() {
        let dx = resp.drag_delta().x;
        let px_per_oct = m.len_px() / (m.range.hi / m.range.lo).log2() as f32;
        if dx != 0.0 && px_per_oct > 0.0 {
            app.dispatch(Msg::Pan {
                octaves: f64::from(-dx / px_per_oct),
            });
        }
    }
    if resp.hovered() {
        let (scroll, pos) = ui.input(|i| (i.smooth_scroll_delta.y, i.pointer.hover_pos()));
        if scroll != 0.0
            && let Some(p) = pos
        {
            app.dispatch(Msg::Zoom {
                about_hz: hz_at(p),
                factor: f64::from((scroll / 200.0).exp()),
            });
        }
    }
}

/// The mouse on an IR picture, as on a frequency pane: wheel zooms time about the pointer,
/// a drag pans it, Ctrl+wheel zooms the value axis about the pointer, Shift+wheel pans it,
/// a click places the cursor.
pub(super) fn navigate_ir(
    app: &mut App,
    ui: &egui::Ui,
    resp: &egui::Response,
    plot_rect: egui::Rect,
    which: IrPane,
    x: Mapping,
    y: Mapping,
) {
    let ms_at = |pos: egui::Pos2| x.from_px(pos.x - plot_rect.min.x);
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        app.dispatch(Msg::IrNav(which, IrNavMsg::Cursor { t_ms: ms_at(p) }));
    }
    if resp.dragged() {
        let dx = resp.drag_delta().x;
        let len = x.len_px();
        if dx != 0.0 && len > 0.0 {
            app.dispatch(Msg::IrNav(
                which,
                IrNavMsg::Pan {
                    ms: -f64::from(dx / len) * x.range.span(),
                },
            ));
        }
    }
    if !resp.hovered() {
        return;
    }
    // egui turns Ctrl+wheel into a zoom factor and Shift+wheel into horizontal scroll.
    let (zoom, shift, dx, scroll, pos) = ui.input(|i| {
        let zoom = if i.modifiers.command {
            i.zoom_delta()
        } else {
            1.0
        };
        (
            zoom,
            i.modifiers.shift,
            i.smooth_scroll_delta.x,
            i.smooth_scroll_delta.y,
            i.pointer.hover_pos(),
        )
    });
    if zoom != 1.0 {
        let about = pos
            .map(|p| y.from_px(p.y - plot_rect.min.y))
            .filter(|v| v.is_finite());
        app.dispatch(Msg::IrNav(
            which,
            IrNavMsg::ValueZoom {
                about,
                factor: f64::from(zoom),
            },
        ));
    }
    if shift && dx != 0.0 {
        // As on the level axes: wheel up shows higher values, a tenth of the span a notch.
        app.dispatch(Msg::IrNav(
            which,
            IrNavMsg::ValuePan {
                by: f64::from(dx / 500.0) * y.range.span(),
            },
        ));
    }
    if scroll != 0.0
        && let Some(p) = pos
    {
        app.dispatch(Msg::IrNav(
            which,
            IrNavMsg::Zoom {
                about_ms: ms_at(p),
                factor: f64::from((scroll / 200.0).exp()),
            },
        ));
    }
}

/// The mouse on an SPL pane's Leq history strip, as on the other graphs' time axes: the
/// wheel zooms time about the pointer, a drag pans it, the pointer reads the windows at its
/// time. Off the strip it does nothing.
pub(super) fn navigate_strip(
    app: &mut App,
    ui: &egui::Ui,
    resp: &egui::Response,
    id: PaneId,
    plot_rect: egui::Rect,
    strip: Option<scenes::StripPlace>,
) {
    let Some(s) = strip else {
        if app.state.leq_cursor.is_some_and(|(p, _)| p == id) {
            app.dispatch(Msg::LeqStrip(id, StripMsg::Hover(None)));
        }
        return;
    };
    let r = egui::Rect::from_min_size(
        plot_rect.min + egui::vec2(s.plot.x, s.plot.y),
        egui::vec2(s.plot.w, s.plot.h),
    );
    let span = s.to - s.from;
    if r.width() <= 1.0 || span.is_nan() || span <= 0.0 {
        return;
    }
    let t_at = |p: egui::Pos2| s.from + f64::from((p.x - r.min.x) / r.width()) * span;
    let shown = (s.from, s.to);
    let width_px = r.width();
    let from_strip = ui
        .input(|i| i.pointer.press_origin())
        .is_some_and(|p| r.contains(p));
    if resp.dragged() && from_strip {
        let dx = resp.drag_delta().x;
        if dx != 0.0 {
            app.dispatch(Msg::LeqStrip(
                id,
                StripMsg::Pan {
                    shown,
                    by: -f64::from(dx / width_px) * span,
                    width_px,
                },
            ));
        }
    }
    let pos = resp.hover_pos().filter(|p| r.contains(*p));
    let t = pos.map(t_at);
    if app
        .state
        .leq_cursor
        .filter(|(p, _)| *p == id)
        .map(|(_, t)| t)
        != t
    {
        app.dispatch(Msg::LeqStrip(id, StripMsg::Hover(t)));
    }
    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
    if scroll != 0.0
        && let Some(about) = t
    {
        app.dispatch(Msg::LeqStrip(
            id,
            StripMsg::Zoom {
                shown,
                about,
                factor: f64::from((scroll / 200.0).exp()),
                width_px,
            },
        ));
    }
}
