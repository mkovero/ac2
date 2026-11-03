//! A pane's title strip: its plain-text title, caption, key-hint line, unit toggle and
//! first-run hints.

use super::*;

/// The narrowest a title is drawn: its number and a few letters, cut short by `…`.
pub(super) const TITLE_MIN_W: f32 = 56.0;

/// What a title leaves for a caption to follow it: a few letters and `…`, so the title
/// still shows that a trace is selected.
const CAPTION_MIN_W: f32 = 32.0;

/// The widest the title may be in `room` (from its left edge to where the title's required
/// controls begin): all of it, or with a caption to follow, all but the caption's least.
pub(super) fn title_budget(room: f32, has_caption: bool) -> f32 {
    let budget = if has_caption {
        room - CAPTION_MIN_W
    } else {
        room
    };
    budget.max(TITLE_MIN_W)
}

/// A pane's title `text` from `left`, at most `max_w` wide, cut short by `…` beyond that
/// (its accessible label keeps the whole text); where it was drawn.
pub(super) fn title_text(
    ui: &egui::Ui,
    text: &str,
    title: egui::Rect,
    left: f32,
    max_w: f32,
    color: egui::Color32,
) -> egui::Rect {
    let font = egui::FontId::proportional(12.0);
    let painter = ui.painter();
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(0.0));
    let galley = painter.layout_job(job);
    let size = galley.size();
    let at = egui::pos2(left, title.center().y - size.y / 2.0);
    painter.galley(at, galley, color);
    egui::Rect::from_min_size(at, size)
}

/// The pane's caption (the selected stored trace, smoothing, mic curve) between `left` and
/// `right` of its title: the longest variant that fits, else the shortest cut with `…`, so
/// the title always names the trace the keys act on. Returns where the caption ends.
pub(super) fn caption(
    app: &App,
    ui: &egui::Ui,
    id: PaneId,
    title: egui::Rect,
    left: f32,
    right: f32,
    ch: &Chrome,
) -> f32 {
    let variants = app.state.pane_caption_variants(id);
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
pub(super) fn hint_line(ui: &egui::Ui, strip: egui::Rect, items: &[KeyHint], ch: &Chrome) {
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
pub(super) fn place_hints(
    widths: &[(u8, f32)],
    sep: f32,
    left: f32,
    width: f32,
) -> Vec<(usize, f32)> {
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

/// The tooltip of a pane's hint line: every hint with the command's full title,
/// and how to hide the line.
pub(super) fn hints_tooltip(
    ui: &mut egui::Ui,
    keymap: &crate::keys::Keymap,
    pane: PaneKind,
    title: &str,
    all: &[KeyHint],
    ch: &Chrome,
) {
    ui.label(egui::RichText::new(format!("{title} — most used keys")).strong());
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
pub(super) fn unit_toggle(
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

/// First-run guidance over the (empty) transfer plot: what to do to get a measurement.
pub(super) fn empty_hint(ui: &egui::Ui, plot_rect: egui::Rect, hint: &str, ch: &Chrome) {
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
pub(super) fn title_hint(ui: &egui::Ui, title: egui::Rect, left: f32, hint: &str, ch: &Chrome) {
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
