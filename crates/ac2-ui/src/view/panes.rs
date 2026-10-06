//! The pane grid: transfer function across the top, spectrum / IR / SPL below; one plot
//! renderer slot per pane.

use std::sync::Arc;

use ac2_scene::axis::Mapping;
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;
use eframe::egui;

use ac2_scene::view::{DistortionUnit, SplMode};

use crate::app::{App, CachedScene};
use crate::hints::{self, KeyHint};
use crate::keys::{CommandId, Scope};
use crate::plot::{self, PlotSlot};
use crate::scenes;
use crate::state::{HintPlace, Msg, Overlay, PaneKind};
use crate::theme::Chrome;
use ac2_proto::units::MeasId;

/// Share of the height the transfer pane takes when other panes are shown below it.
const TF_SHARE: f32 = 0.62;
const GAP: f32 = 6.0;
const TITLE_H: f32 = 20.0;
/// The focused pane's key-hint line, under its plot.
pub(crate) const HINT_H: f32 = 18.0;
/// Text size of the hint line.
const HINT_FONT: f32 = 11.5;
/// Left and right inset of the hint line's text.
const HINT_PAD: f32 = 8.0;

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

/// Height of a plot's caption in the stage view.
const STAGE_CAPTION_H: f32 = 22.0;

/// The stage view's caption of `pane`: `Transfer · Main L`; none for the SPL pane, whose
/// meter and Leq windows name themselves.
fn stage_caption(st: &crate::state::AppState, pane: PaneKind) -> Option<String> {
    if pane == PaneKind::Spl {
        return None;
    }
    let parts: Vec<String> = [
        Some(pane.title().to_owned()),
        st.pane_meas(pane).map(|m| m.config.name.clone()),
        st.pane_caption(pane),
    ]
    .into_iter()
    .flatten()
    .collect();
    Some(parts.join(" · "))
}

fn slot(p: PaneKind) -> PlotSlot {
    PlotSlot(p as u32)
}

/// The axes of a pane's scene the mouse navigates: frequency, and level in dB.
#[derive(Clone, Copy, Debug, Default)]
struct Axes {
    x: Option<Mapping>,
    y_level: Option<Mapping>,
    /// The spectrograph's time axis (seconds before the newest frame), when shown: a
    /// click there puts the cursor on a time as well as a frequency.
    time: Option<Mapping>,
}

/// Scene for `pane` at `size`, from the cache when nothing changed.
fn scene_for(
    app: &mut App,
    pane: PaneKind,
    size: egui::Vec2,
    theme: &Theme,
) -> Option<(Arc<ac2_plot::Scene>, Axes)> {
    if let Some(c) = app.scenes.get(&pane)
        && c.generation == app.pane_generation(pane)
        && c.size == size
        && c.theme == app.state.theme
    {
        let axes = Axes {
            x: c.x_axis,
            y_level: c.y_level,
            time: c.time_axis,
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
    let (scene, axes) = match pane {
        PaneKind::Transfer => {
            let s = scenes::transfer(st, theme, vp, now);
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
                },
            )
        }
        PaneKind::Spectrum if st.view.spectrum.mode.spectrograph() => {
            let s = scenes::spectrograph(st, theme, vp, now);
            if let Some(sp) = &s.spectrum {
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
                },
            )
        }
        PaneKind::Spectrum => {
            let s = scenes::spectrum(st, theme, vp, now);
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
                },
            )
        }
        PaneKind::Ir => (scenes::ir(st, theme, vp, now)?.scene, Axes::default()),
        PaneKind::Spl => (
            scenes::spl_pane(st, &app.keymap, theme, vp, now)?,
            Axes::default(),
        ),
        PaneKind::Distortion => {
            let s = scenes::sweep(st, theme, vp, now);
            let axes = Axes {
                x: s.x_axis(),
                y_level: s.y_level(st.view.distortion.unit),
                time: None,
            };
            (s.scene(), axes)
        }
    };
    let scene = Arc::new(scene);
    app.scenes.insert(
        pane,
        CachedScene {
            generation: app.pane_generation(pane),
            size,
            theme: app.state.theme,
            scene: scene.clone(),
            x_axis: axes.x,
            y_level: axes.y_level,
            time_axis: axes.time,
            unit_tip,
        },
    );
    Some((scene, axes))
}

fn placeholder(pane: PaneKind, app: &App) -> &'static str {
    match pane {
        PaneKind::Ir if scenes::focus_tf(&app.state).is_none() => "no transfer measurement",
        PaneKind::Ir => "no IR frame yet",
        PaneKind::Spl if app.state.view.spl.mode == SplMode::Leq && scenes::has_spl(&app.state) => {
            "no Leq windows yet: they show once the meter has measured a second"
        }
        PaneKind::Spl if scenes::has_spl(&app.state) => "no SPL frame yet",
        PaneKind::Spl => "no SPL meter",
        _ => "",
    }
}

pub(super) fn panes(app: &mut App, ui: &mut egui::Ui, theme: &Theme, ch: &Chrome) {
    let area = ui.available_rect_before_wrap();
    let visible = app.state.layout.visible();
    // The stage view is the pane's picture alone: no frame, no title.
    let stage = app.state.stage_view();
    for (pane, rect) in layout(&visible, area) {
        if stage {
            // The SPL meter and its Leq windows carry their own captions; a plot gets a slim
            // one naming the pane and its measurement, and nothing else.
            let rect = match stage_caption(&app.state, pane) {
                Some(text) => {
                    let strip = egui::Rect::from_min_size(
                        rect.min,
                        egui::vec2(rect.width(), STAGE_CAPTION_H),
                    );
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
                ui.id().with(("stage", pane as u32)),
                egui::Sense::click_and_drag(),
            );
            let built = if app.plots {
                scene_for(app, pane, rect.size(), theme)
            } else {
                None
            };
            let axes = built.as_ref().map(|b| b.1).unwrap_or_default();
            match built {
                Some((scene, _)) => plot::paint(ui, slot(pane), rect, scene),
                None => {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        placeholder(pane, app),
                        egui::FontId::proportional(13.0),
                        ch.dim,
                    );
                }
            }
            // Full screen keeps the mouse: wheel zooms frequency, Ctrl/Shift+wheel the level
            // axis, as in the split layout.
            navigate(app, ui, &resp, pane, rect, axes);
            continue;
        }
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
        let style = crate::keys::label_style();
        let line = app.state.key_hint_line(&app.keymap, pane, style);
        // The hint line takes its strip off the plot's bottom: it never covers a curve.
        let bottom = rect.max.y - 2.0 - if line.is_some() { HINT_H } else { 0.0 };
        let plot_rect = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + 2.0, rect.min.y + TITLE_H),
            egui::pos2(rect.max.x - 2.0, bottom),
        );
        let resp = ui.interact(
            rect,
            ui.id().with(("pane", pane as u32)),
            egui::Sense::click_and_drag(),
        );
        // After the pane's own response, so a hover or click there is theirs.
        let all = app.state.pane_hints(&app.keymap, pane, style);
        let name_rect =
            egui::Rect::from_min_max(title.min, egui::pos2(label.right() + 4.0, title.max.y));
        let name = ui.interact(
            name_rect,
            ui.id().with(("pane-title", pane as u32)),
            egui::Sense::hover(),
        );
        name.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Label,
                true,
                format!("{n}  {}", pane.title()),
            )
        });
        name.on_hover_ui(|ui| hints_tooltip(ui, &app.keymap, pane, &all, ch));
        if let Some(items) = &line {
            let strip = egui::Rect::from_min_max(
                egui::pos2(rect.min.x + 2.0, bottom),
                egui::pos2(rect.max.x - 2.0, rect.max.y - 2.0),
            );
            hint_line(ui, strip, items, ch);
            ui.interact(
                strip,
                ui.id().with(("pane-hints", pane as u32)),
                egui::Sense::hover(),
            )
            .on_hover_ui(|ui| hints_tooltip(ui, &app.keymap, pane, &all, ch));
        }
        let mut x = label.right() + 10.0;
        if let Some(chip) = title_chip(app, ui, pane, title, x, ch) {
            x = chip.right() + 10.0;
        }
        let mut right = title.right() - 8.0;
        if pane == PaneKind::Distortion
            && !app.state.view.distortion.show_ir
            && let Some(left) = unit_toggle(app, ui, title, x, ch)
        {
            right = left - 10.0;
        }
        let caption_end = caption(app, ui, pane, title, x, right, ch);
        if plot_rect.width() < 8.0 || plot_rect.height() < 8.0 {
            continue;
        }
        let built = if app.plots {
            scene_for(app, pane, plot_rect.size(), theme)
        } else {
            None
        };
        // What the level axis means, over its unit (a spectrum's per-bin levels).
        let tip = app.scenes.get(&pane).and_then(|c| c.unit_tip.clone());
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
            match hint.place {
                HintPlace::Centre => empty_hint(ui, plot_rect, &hint.text, ch),
                HintPlace::Title => title_hint(ui, title, caption_end + 16.0, &hint.text, ch),
            }
        }
        navigate(
            app,
            ui,
            &resp,
            pane,
            plot_rect,
            built.map(|b| b.1).unwrap_or_default(),
        );
    }
    ui.allocate_rect(area, egui::Sense::hover());
}

/// The chip in a pane's title naming the measurement the pane shows (a click opens the list
/// of measurements it can show); where it was drawn.
fn title_chip(
    app: &mut App,
    ui: &egui::Ui,
    pane: PaneKind,
    title: egui::Rect,
    x: f32,
    ch: &Chrome,
) -> Option<egui::Rect> {
    let st = &app.state;
    let m = st.pane_meas(pane)?;
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
    let next = app
        .keymap
        .first_chord(CommandId::NextMeasurement, pane.scope())
        .map(|c| c.label());
    let chip = chip.on_hover_text(match next {
        Some(k) => format!("{name}: click to choose what this pane shows · {k} the next one"),
        None => format!("{name}: click to choose what this pane shows"),
    });
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
    Some(r)
}

/// The pane's caption (the selected stored trace, smoothing, mic curve) between `left` and
/// `right` of its title: the longest variant that fits, else the shortest cut with `…`, so
/// the title always names the trace the keys act on. Returns where the caption ends.
fn caption(
    app: &App,
    ui: &egui::Ui,
    pane: PaneKind,
    title: egui::Rect,
    left: f32,
    right: f32,
    ch: &Chrome,
) -> f32 {
    let variants = app.state.pane_caption_variants(pane);
    let room = right - left;
    if variants.is_empty() || room < 24.0 {
        return left;
    }
    let font = egui::FontId::proportional(12.0);
    let painter = ui.painter();
    let fits = variants
        .iter()
        .map(|v| painter.layout_no_wrap(v.clone(), font.clone(), ch.dim))
        .find(|g| g.size().x <= room);
    let galley = fits.unwrap_or_else(|| {
        let mut job = egui::text::LayoutJob::simple_singleline(
            variants.last().cloned().unwrap_or_default(),
            font,
            ch.dim,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(room);
        painter.layout_job(job)
    });
    let w = galley.size().x;
    painter.galley(
        egui::pos2(left, title.center().y - galley.size().y / 2.0),
        galley,
        ch.dim,
    );
    left + w
}

/// The key-hint line in `strip`: `key name` pairs, the keys in the accent colour, as many as
/// fit (the least used go first; the help key stays).
fn hint_line(ui: &egui::Ui, strip: egui::Rect, items: &[KeyHint], ch: &Chrome) {
    let painter = ui.painter_at(strip);
    painter.line_segment(
        [strip.left_top(), strip.right_top()],
        egui::Stroke::new(1.0, ch.border),
    );
    let (galleys, sep) = hint_galleys(ui.ctx(), items, ch);
    let widths: Vec<(u8, f32)> = items
        .iter()
        .zip(&galleys)
        .map(|(h, g)| (h.priority, g.size().x))
        .collect();
    let placed = place_hints(&widths, sep.size().x, strip.min.x, strip.width());
    for (n, (i, x)) in placed.into_iter().enumerate() {
        let g = galleys[i].clone();
        let y = strip.center().y - g.size().y / 2.0;
        if n > 0 {
            painter.galley(egui::pos2(x - sep.size().x, y), sep.clone(), ch.dim);
        }
        painter.galley(egui::pos2(x, y), g, ch.dim);
    }
}

/// Where each kept hint of `widths` (priority, width) starts in a strip from `left`,
/// `width` wide: `(index, x)`, left to right, a separator before each but the first.
fn place_hints(widths: &[(u8, f32)], sep: f32, left: f32, width: f32) -> Vec<(usize, f32)> {
    let keep = hints::fit(widths, sep, width - 2.0 * HINT_PAD);
    let mut x = left + HINT_PAD;
    let mut out = Vec::new();
    for (i, ((_, w), k)) in widths.iter().zip(keep).enumerate() {
        if !k {
            continue;
        }
        if !out.is_empty() {
            x += sep;
        }
        out.push((i, x));
        x += w;
    }
    out
}

/// Each hint laid out as drawn (key in the accent colour, name dimmed), and the separator.
pub(crate) fn hint_galleys(
    ctx: &egui::Context,
    items: &[KeyHint],
    ch: &Chrome,
) -> (
    Vec<std::sync::Arc<egui::Galley>>,
    std::sync::Arc<egui::Galley>,
) {
    let font = egui::FontId::proportional(HINT_FONT);
    let fmt = |color| egui::TextFormat::simple(font.clone(), color);
    ctx.fonts_mut(|f| {
        let galleys = items
            .iter()
            .map(|h| {
                let mut job = egui::text::LayoutJob::default();
                job.append(&h.keys, 0.0, fmt(ch.focus));
                job.append(&format!(" {}", h.name), 0.0, fmt(ch.dim));
                f.layout_job(job)
            })
            .collect();
        let sep = f.layout_no_wrap(hints::SEP.to_owned(), font.clone(), ch.border);
        (galleys, sep)
    })
}

/// The tooltip of a pane's title and hint line: every hint with the command's full title,
/// and how to hide the line.
fn hints_tooltip(
    ui: &mut egui::Ui,
    keymap: &crate::keys::Keymap,
    pane: PaneKind,
    all: &[KeyHint],
    ch: &Chrome,
) {
    ui.label(egui::RichText::new(format!("{} — most used keys", pane.title())).strong());
    egui::Grid::new(("hints-tip", pane as u32))
        .num_columns(2)
        .spacing(egui::vec2(12.0, 3.0))
        .show(ui, |ui| {
            for h in all {
                ui.label(egui::RichText::new(&h.keys).monospace().color(ch.focus));
                let title = if h.command == CommandId::Help {
                    "Every key, in every pane"
                } else {
                    h.title()
                };
                ui.label(title);
                ui.end_row();
            }
        });
    if let Some(k) = keymap.first_chord(CommandId::KeyHints, Scope::Global) {
        ui.label(
            egui::RichText::new(format!("{} hides or shows the hint line", k.label()))
                .small()
                .color(ch.dim),
        );
    }
}

/// The distortion pane's unit, `dB | %`, at the right end of its title: the shown one
/// highlighted, a click shows the other (as the key does, which the tooltip names). Left
/// out when the title is too narrow to hold it beside the pane's name (from `min_x`).
/// Returns its left edge when drawn.
fn unit_toggle(
    app: &mut App,
    ui: &egui::Ui,
    title: egui::Rect,
    min_x: f32,
    ch: &Chrome,
) -> Option<f32> {
    let current = app.state.view.distortion.unit;
    let tip = match app
        .keymap
        .key_hint(CommandId::DistortionUnit, Scope::Distortion)
    {
        Some(k) => format!("Distortion in dB re fundamental or percent of it · {k}"),
        None => "Distortion in dB re fundamental or percent of it".to_string(),
    };
    let font = egui::FontId::proportional(12.0);
    let pad = 7.0;
    let painter = ui.painter();
    let items: Vec<(DistortionUnit, &str, std::sync::Arc<egui::Galley>)> =
        [(DistortionUnit::Db, "dB"), (DistortionUnit::Percent, "%")]
            .into_iter()
            .map(|(u, t)| {
                let color = if u == current { ch.text } else { ch.dim };
                (
                    u,
                    t,
                    painter.layout_no_wrap(t.to_string(), font.clone(), color),
                )
            })
            .collect();
    let total: f32 = items.iter().map(|(_, _, g)| g.size().x + 2.0 * pad).sum();
    let mut x = title.max.x - 6.0 - total;
    if x < min_x {
        return None;
    }
    let left = x;
    let (y, h) = (title.min.y + 2.0, TITLE_H - 3.0);
    let mut picked = None;
    for (unit, text, galley) in items {
        let r =
            egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(galley.size().x + 2.0 * pad, h));
        x = r.max.x;
        let resp = ui.interact(
            r,
            ui.id().with(("distortion-unit", text)),
            egui::Sense::click(),
        );
        let selected = unit == current;
        resp.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, text)
        });
        if selected {
            painter.rect_filled(r, 3.0, ch.focus.gamma_multiply(0.35));
        } else if resp.hovered() {
            painter.rect_filled(r, 3.0, ch.panel);
        }
        let gh = galley.size().y;
        painter.galley(
            egui::pos2(r.min.x + pad, r.center().y - gh / 2.0),
            galley,
            ch.text,
        );
        if resp.clicked() && !selected {
            picked = Some(unit);
        }
        resp.on_hover_text(tip.as_str());
    }
    let whole = egui::Rect::from_min_max(
        egui::pos2(title.max.x - 6.0 - total, y),
        egui::pos2(x, y + h),
    );
    painter.rect_stroke(
        whole,
        3.0,
        egui::Stroke::new(1.0, ch.border),
        egui::StrokeKind::Inside,
    );
    if let Some(u) = picked {
        app.dispatch(Msg::DistortionUnit(u));
    }
    Some(left)
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
                        egui::RichText::new("↑/↓ · wheel · Enter shows · Esc closes")
                            .small()
                            .color(ch.dim),
                    );
                });
        });
    let wheel = area
        .response
        .contains_pointer()
        .then(|| super::overlays::wheel_rows(ui.ctx()))
        .flatten();
    if let Some(id) = picked {
        app.dispatch(Msg::PaneShow(pane, id));
    } else if let Some(rows) = wheel {
        app.dispatch(Msg::Wheel { rows });
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

/// The same guidance as one line at the right of the pane's title strip, while the plot
/// below shows stored curves: still in view, never over a curve. Elided when the strip is
/// short.
fn title_hint(ui: &egui::Ui, title: egui::Rect, left: f32, hint: &str, ch: &Chrome) {
    let room = title.right() - 8.0 - left;
    if room < 40.0 {
        return;
    }
    let mut job = egui::text::LayoutJob::simple_singleline(
        hint.to_owned(),
        egui::FontId::proportional(12.0),
        ch.focus,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(room);
    let painter = ui.painter();
    let galley = painter.layout_job(job);
    let pos = egui::pos2(
        title.right() - 8.0 - galley.size().x,
        title.center().y - galley.size().y / 2.0,
    );
    painter.galley(pos, galley, ch.focus);
}

/// Click focuses (and on a frequency axis places the cursor); wheel zooms the frequency
/// axis about the pointer, Ctrl+wheel the level axis, Shift+wheel pans the level axis; drag
/// pans the frequency axis. Navigation only: values stay as received.
fn navigate(
    app: &mut App,
    ui: &egui::Ui,
    resp: &egui::Response,
    pane: PaneKind,
    plot_rect: egui::Rect,
    axes: Axes,
) {
    // A window over the panes owns the mouse: its wheel scrolls the window, never zooms a
    // plot behind it.
    if app.state.window_over_panes() {
        return;
    }
    if resp.clicked() || resp.drag_started() {
        app.dispatch(Msg::FocusPane(pane));
    }
    if resp.hovered()
        && crate::state::level_range(
            &app.state.view,
            pane,
            crate::scenes::spectrum_scale(&app.state),
        )
        .is_some()
        && !(pane == PaneKind::Distortion && app.state.view.distortion.show_ir)
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
        let about_db = axes
            .y_level
            .zip(pos)
            .map(|(m, p)| m.from_px(p.y - plot_rect.min.y))
            .filter(|v| v.is_finite());
        if zoom != 1.0 {
            app.dispatch(Msg::LevelZoom {
                pane,
                about_db,
                factor: f64::from(zoom),
            });
        }
        if shift
            && dx != 0.0
            && let Some(r) = crate::state::level_range(
                &app.state.view,
                pane,
                crate::scenes::spectrum_scale(&app.state),
            )
        {
            // Wheel up (positive delta) shows higher levels; about a tenth of the span per
            // notch (50 px of scroll).
            app.dispatch(Msg::LevelPan {
                pane,
                db: f64::from(dx / 500.0) * r.span(),
            });
        }
    }
    let Some(m) = axes.x else {
        return;
    };
    let hz_at = |pos: egui::Pos2| m.from_px(pos.x - plot_rect.min.x);
    if resp.clicked()
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
    use crate::keys::{Keymap, LabelStyle};

    /// Every pane's line at every width from 300 to 1920 px, measured with egui's fonts:
    /// nothing drawn past the strip, no two hints overlapping, the help key always there,
    /// and the whole line once the pane is wide enough.
    #[test]
    fn hint_lines_fit_every_width() {
        let ctx = egui::Context::default();
        crate::app::install_fonts(&ctx);
        let ch = crate::theme::chrome(&Theme::dark());
        let keymap = Keymap::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let ctx = ui.ctx();
            for scope in Scope::ALL {
                for style in [LabelStyle::Pc, LabelStyle::Mac] {
                    let items = hints::line(&keymap, scope, style, |_| false);
                    let (galleys, sep) = hint_galleys(ctx, &items, &ch);
                    let widths: Vec<(u8, f32)> = items
                        .iter()
                        .zip(&galleys)
                        .map(|(h, g)| (h.priority, g.size().x))
                        .collect();
                    let sep = sep.size().x;
                    let mut shown_before = 0;
                    for w in (300..=1920).step_by(10) {
                        let w = w as f32;
                        let placed = place_hints(&widths, sep, 0.0, w);
                        let help = items.len() - 1;
                        assert_eq!(placed.last().map(|p| p.0), Some(help), "{scope:?} {w}");
                        for pair in placed.windows(2) {
                            let ((i, x), (_, next)) = (pair[0], pair[1]);
                            assert!(x + widths[i].1 + sep <= next + 0.01, "{scope:?} {w}");
                        }
                        let (i, x) = placed[placed.len() - 1];
                        assert!(x + widths[i].1 <= w - HINT_PAD + 0.01, "{scope:?} {w}");
                        // Wider never shows fewer.
                        assert!(placed.len() >= shown_before, "{scope:?} {w}");
                        shown_before = placed.len();
                    }
                    assert_eq!(shown_before, items.len(), "{scope:?}: all at 1920 px");
                    // At the narrowest the most used still shows beside the help key.
                    let narrow = place_hints(&widths, sep, 0.0, 300.0);
                    let want = if scope == Scope::Global { 1 } else { 3 };
                    assert!(narrow.len() >= want, "{scope:?} {style:?}: {narrow:?}");
                }
            }
        });
        out.textures_delta.clear();
    }

    #[test]
    fn layout_tf_on_top() {
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 606.0));
        let l = layout(&PaneKind::ALL, area);
        assert_eq!(l.len(), 5);
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
