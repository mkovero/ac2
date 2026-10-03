//! The pane grid: transfer function across the top, spectrum / IR / SPL below; one plot
//! renderer slot per pane.

use std::sync::Arc;

use ac2_scene::axis::Mapping;
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;
use eframe::egui;

use crate::app::{App, CachedScene};
use crate::plot::{self, PlotSlot};
use crate::scenes;
use crate::state::{Msg, Overlay, PaneKind};
use crate::theme::Chrome;
use ac2_proto::units::MeasId;

/// Share of the height the transfer pane takes when other panes are shown below it.
const TF_SHARE: f32 = 0.62;
const GAP: f32 = 6.0;
const TITLE_H: f32 = 20.0;

/// Pane rectangles inside `area` for the visible panes.
pub(crate) fn layout(visible: &[PaneKind], area: egui::Rect) -> Vec<(PaneKind, egui::Rect)> {
    let has_tf = visible.contains(&PaneKind::Transfer);
    let others: Vec<PaneKind> = visible
        .iter()
        .copied()
        .filter(|p| *p != PaneKind::Transfer)
        .collect();
    let mut out = Vec::new();
    let mut row = area;
    if has_tf {
        if others.is_empty() {
            return vec![(PaneKind::Transfer, area)];
        }
        let h = (area.height() - GAP) * TF_SHARE;
        out.push((
            PaneKind::Transfer,
            egui::Rect::from_min_size(area.min, egui::vec2(area.width(), h)),
        ));
        row = egui::Rect::from_min_max(egui::pos2(area.min.x, area.min.y + h + GAP), area.max);
    }
    let n = others.len() as f32;
    if n > 0.0 {
        let w = (row.width() - GAP * (n - 1.0)) / n;
        for (i, p) in others.into_iter().enumerate() {
            let x = row.min.x + i as f32 * (w + GAP);
            out.push((
                p,
                egui::Rect::from_min_size(egui::pos2(x, row.min.y), egui::vec2(w, row.height())),
            ));
        }
    }
    out
}

fn slot(p: PaneKind) -> PlotSlot {
    PlotSlot(p as u32)
}

/// Scene for `pane` at `size`, from the cache when nothing changed.
fn scene_for(
    app: &mut App,
    pane: PaneKind,
    size: egui::Vec2,
    theme: &Theme,
) -> Option<(Arc<ac2_plot::Scene>, Option<Mapping>)> {
    if let Some(c) = app.scenes.get(&pane)
        && c.generation == app.generation
        && c.size == size
        && c.theme == app.state.theme
    {
        return Some((c.scene.clone(), c.x_axis));
    }
    let vp = Viewport {
        width: size.x,
        height: size.y,
    };
    let now = super::now();
    let st = &app.state;
    let (scene, x_axis) = match pane {
        PaneKind::Transfer => {
            let s = scenes::transfer(st, theme, vp, now);
            (s.scene, Some(s.x_axis.mapping))
        }
        PaneKind::Spectrum => {
            let s = scenes::spectrum(st, theme, vp, now);
            (s.scene, Some(s.x_axis.mapping))
        }
        PaneKind::Ir => (scenes::ir(st, theme, vp, now)?.scene, None),
        PaneKind::Spl => (scenes::spl(st, theme, vp, now)?.scene, None),
    };
    let scene = Arc::new(scene);
    app.scenes.insert(
        pane,
        CachedScene {
            generation: app.generation,
            size,
            theme: app.state.theme,
            scene: scene.clone(),
            x_axis,
        },
    );
    Some((scene, x_axis))
}

fn placeholder(pane: PaneKind, app: &App) -> &'static str {
    match pane {
        PaneKind::Ir if scenes::focus_tf(&app.state).is_none() => "no transfer measurement",
        PaneKind::Ir => "no IR frame yet",
        PaneKind::Spl => "no SPL meter",
        _ => "",
    }
}

pub(super) fn panes(app: &mut App, ui: &mut egui::Ui, theme: &Theme, ch: &Chrome) {
    let area = ui.available_rect_before_wrap();
    let visible = app.state.layout.visible();
    for (pane, rect) in layout(&visible, area) {
        let focused = app.state.layout.focus == pane;
        let stroke = if focused {
            egui::Stroke::new(1.5, ch.focus)
        } else {
            egui::Stroke::new(1.0, ch.border)
        };
        let painter = ui.painter();
        painter.rect_filled(rect, 4.0, ch.raised);
        painter.rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);
        let title = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), TITLE_H));
        let n = PaneKind::ALL.iter().position(|p| *p == pane).unwrap_or(0) + 1;
        let label = painter.text(
            title.left_center() + egui::vec2(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            format!("{n}  {}", pane.title()),
            egui::FontId::proportional(12.0),
            if focused { ch.text } else { ch.dim },
        );
        let plot_rect = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + 2.0, rect.min.y + TITLE_H),
            egui::pos2(rect.max.x - 2.0, rect.max.y - 2.0),
        );
        let resp = ui.interact(
            rect,
            ui.id().with(("pane", pane as u32)),
            egui::Sense::click_and_drag(),
        );
        // After the pane's own response, so a click on the chip is the chip's.
        title_chip(app, ui, pane, title, label.right() + 10.0, ch);
        if plot_rect.width() < 8.0 || plot_rect.height() < 8.0 {
            continue;
        }
        let built = if app.plots {
            scene_for(app, pane, plot_rect.size(), theme)
        } else {
            None
        };
        match &built {
            Some((scene, _)) => plot::paint(ui, slot(pane), plot_rect, scene.clone()),
            None => {
                let text = if app.plots {
                    placeholder(pane, app)
                } else {
                    "plots need the wgpu renderer"
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
        if pane == PaneKind::Transfer
            && app.state.overlay == crate::state::Overlay::None
            && let Some(hint) = app.state.empty_hint(&app.keymap)
        {
            empty_hint(ui, plot_rect, &hint, ch);
        }
        navigate(app, ui, &resp, pane, plot_rect, built.and_then(|b| b.1));
    }
    ui.allocate_rect(area, egui::Sense::hover());
}

/// The chip in a pane's title naming the measurement the pane shows (a click opens the list
/// of measurements it can show), and on the transfer and spectrum panes the smoothing of
/// what is drawn there.
fn title_chip(
    app: &mut App,
    ui: &egui::Ui,
    pane: PaneKind,
    title: egui::Rect,
    x: f32,
    ch: &Chrome,
) {
    let st = &app.state;
    let Some(m) = st.pane_meas(pane) else {
        return;
    };
    let font = egui::FontId::proportional(12.0);
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(m.config.name.clone(), font.clone(), ch.text);
    let r = egui::Rect::from_min_size(
        egui::pos2(x, title.min.y + 2.0),
        egui::vec2(galley.size().x + 24.0, TITLE_H - 3.0),
    );
    let chip = ui.interact(
        r,
        ui.id().with(("pane-chip", pane as u32)),
        egui::Sense::click(),
    );
    let name = m.config.name.clone();
    chip.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
    let open = matches!(st.overlay, Overlay::PaneMenu(pm) if pm.pane == pane);
    let fill = if open || chip.hovered() {
        ch.focus.gamma_multiply(0.35)
    } else {
        ch.panel
    };
    painter.rect_filled(r, 3.0, fill);
    painter.rect_stroke(
        r,
        3.0,
        egui::Stroke::new(1.0, ch.border),
        egui::StrokeKind::Inside,
    );
    let gh = galley.size().y;
    painter.galley(
        egui::pos2(r.min.x + 7.0, r.center().y - gh / 2.0),
        galley,
        ch.text,
    );
    // A small ▾, drawn: it opens a list.
    let (cx, cy) = (r.max.x - 9.0, r.center().y);
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(cx - 3.5, cy - 1.5),
            egui::pos2(cx + 3.5, cy - 1.5),
            egui::pos2(cx, cy + 2.5),
        ],
        ch.dim,
        egui::Stroke::NONE,
    ));
    if let Some(c) = st.smoothing_caption(pane) {
        painter.text(
            r.right_center() + egui::vec2(10.0, 0.0),
            egui::Align2::LEFT_CENTER,
            c,
            font,
            ch.dim,
        );
    }
    if chip.clicked() {
        app.dispatch(Msg::PaneMenu(pane));
    }
    pane_menu(
        app,
        ui,
        pane,
        r.left_bottom() + egui::vec2(0.0, 2.0),
        &chip,
        ch,
    );
}

/// The open measurement list of `pane`, under its chip.
fn pane_menu(
    app: &mut App,
    ui: &egui::Ui,
    pane: PaneKind,
    at: egui::Pos2,
    chip: &egui::Response,
    ch: &Chrome,
) {
    let Overlay::PaneMenu(menu) = app.state.overlay else {
        return;
    };
    if menu.pane != pane {
        return;
    }
    let rows: Vec<(MeasId, String)> = app
        .state
        .pane_candidates(pane)
        .iter()
        .map(|m| {
            (
                m.id,
                format!(
                    "{}  {}",
                    super::chrome::kind_tag(&m.config.kind),
                    m.config.name
                ),
            )
        })
        .collect();
    let mut picked = None;
    let area = egui::Area::new(ui.id().with(("pane-menu", pane as u32)))
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .show(ui.ctx(), |ui| {
            super::overlays::card(ch)
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.set_min_width(200.0);
                    for (i, (id, text)) in rows.iter().enumerate() {
                        let b = egui::Button::selectable(i == menu.index, text.as_str());
                        if ui.add(b).clicked() {
                            picked = Some(*id);
                        }
                    }
                    ui.label(
                        egui::RichText::new("↑/↓ · Enter shows · Esc closes")
                            .small()
                            .color(ch.dim),
                    );
                });
        });
    if let Some(id) = picked {
        app.dispatch(Msg::PaneShow(pane, id));
    } else if area.response.clicked_elsewhere() && !chip.clicked() {
        app.dispatch(Msg::PaneMenu(pane));
    }
}

/// First-run guidance over the (empty) transfer plot: what to do to get a measurement.
fn empty_hint(ui: &egui::Ui, plot_rect: egui::Rect, hint: &str, ch: &Chrome) {
    let painter = ui.painter_at(plot_rect);
    let galley = painter.layout(
        hint.to_owned(),
        egui::FontId::proportional(15.0),
        ch.text,
        (plot_rect.width() - 40.0).max(100.0),
    );
    let pad = egui::vec2(16.0, 10.0);
    let rect = egui::Rect::from_center_size(plot_rect.center(), galley.size() + 2.0 * pad);
    painter.rect_filled(rect, 6.0, ch.panel);
    painter.rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(1.0, ch.focus),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.min + pad, galley, ch.text);
}

/// Click focuses (and on a frequency axis places the cursor); wheel zooms about the
/// pointer; drag pans. Navigation only: values stay as received.
fn navigate(
    app: &mut App,
    ui: &egui::Ui,
    resp: &egui::Response,
    pane: PaneKind,
    plot_rect: egui::Rect,
    x: Option<Mapping>,
) {
    if resp.clicked() || resp.drag_started() {
        app.dispatch(Msg::FocusPane(pane));
    }
    let Some(m) = x else {
        return;
    };
    let hz_at = |pos: egui::Pos2| m.from_px(pos.x - plot_rect.min.x);
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        app.dispatch(Msg::CursorAt(Some(hz_at(p))));
    }
    if resp.dragged() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_tf_on_top() {
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 606.0));
        let l = layout(&PaneKind::ALL, area);
        assert_eq!(l.len(), 4);
        assert_eq!(l[0].0, PaneKind::Transfer);
        assert!((l[0].1.height() - 372.0).abs() < 1e-3);
        // Bottom row splits evenly, no overlap.
        for w in l[1..].windows(2) {
            assert!(w[0].1.max.x <= w[1].1.min.x);
            assert!((w[0].1.width() - w[1].1.width()).abs() < 1e-3);
        }
        assert_eq!(
            layout(&[PaneKind::Transfer], area),
            vec![(PaneKind::Transfer, area)]
        );
        let only = layout(&[PaneKind::Spectrum, PaneKind::Spl], area);
        assert_eq!(only[0].1.height(), area.height());
    }
}
