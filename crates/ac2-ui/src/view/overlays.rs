//! Help overlay, command palette, text prompt and toasts.

use eframe::egui::{self, Color32, Key, RichText, text::LayoutJob};

use crate::app::App;
use crate::keys::{CommandId, Keymap, Scope};
use crate::state::{Msg, Overlay};
use crate::theme::Chrome;

const PALETTE_ROWS: usize = 12;

pub(super) fn draw(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    match app.state.overlay.clone() {
        Overlay::None => {}
        Overlay::Help => help(app, ctx, ch),
        Overlay::Palette(_) => palette(app, ctx, ch),
        Overlay::Prompt(_) => prompt(app, ctx, ch),
    }
    toasts(app, ctx, ch);
}

fn backdrop(ctx: &egui::Context) {
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("ac2-backdrop"))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .interactable(false)
        .show(ctx, |ui| {
            ui.painter()
                .rect_filled(screen, 0.0, Color32::from_black_alpha(140));
        });
}

fn card(ch: &Chrome) -> egui::Frame {
    egui::Frame::new()
        .fill(ch.raised)
        .stroke(egui::Stroke::new(1.0, ch.border))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(14))
}

/// One row of the help overlay.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum HelpRow {
    Header {
        title: &'static str,
        live: bool,
    },
    Bind {
        keys: String,
        title: String,
        live: bool,
    },
}

/// Rows for every scope (global and the focused pane's first), with runs of slot commands
/// bound to one modifier + digit collapsed into one row.
pub(crate) fn help_rows(keymap: &Keymap, active: Scope) -> Vec<HelpRow> {
    let mut scopes = vec![Scope::Global, active];
    scopes.extend(
        Scope::ALL
            .into_iter()
            .filter(|s| *s != Scope::Global && *s != active),
    );
    scopes.dedup();
    let slots = [
        CommandId::Slot1,
        CommandId::Slot2,
        CommandId::Slot3,
        CommandId::Slot4,
        CommandId::Slot5,
        CommandId::Slot6,
        CommandId::Slot7,
        CommandId::Slot8,
        CommandId::Slot9,
    ];
    let mut rows = Vec::new();
    for scope in scopes {
        let live = scope == Scope::Global || scope == active;
        rows.push(HelpRow::Header {
            title: scope.title(),
            live,
        });
        let slot_chords: Vec<Vec<crate::keys::Chord>> =
            slots.iter().map(|c| keymap.chords(*c, scope)).collect();
        let digits = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        let collapsed = slot_chords.iter().zip(digits).all(|(v, d)| {
            v.len() == 1 && v[0].key == d && {
                let first = &slot_chords[0][0];
                (v[0].command, v[0].alt, v[0].shift) == (first.command, first.alt, first.shift)
            }
        });
        for c in CommandId::ALL {
            let chords = keymap.chords(*c, scope);
            if chords.is_empty() {
                continue;
            }
            if collapsed && slots.contains(c) {
                if *c == CommandId::Slot1 {
                    let one = chords[0].label();
                    rows.push(HelpRow::Bind {
                        keys: format!("{one}…9"),
                        title: "Capture selected measurement to slot 1…9".into(),
                        live,
                    });
                }
                continue;
            }
            rows.push(HelpRow::Bind {
                keys: chords
                    .iter()
                    .map(|k| k.label())
                    .collect::<Vec<_>>()
                    .join(" "),
                title: c.title().into(),
                live,
            });
        }
    }
    rows
}

/// Splits rows into `n` columns of about equal length, never ending a column on a header.
pub(crate) fn columns(rows: Vec<HelpRow>, n: usize) -> Vec<Vec<HelpRow>> {
    let per = rows.len().div_ceil(n.max(1)).max(1);
    let mut out: Vec<Vec<HelpRow>> = vec![Vec::new()];
    for r in rows {
        let full = out.last().is_some_and(|c| c.len() >= per);
        if full && out.len() < n {
            let last = out.last_mut().map(|c| {
                if matches!(c.last(), Some(HelpRow::Header { .. })) {
                    c.pop()
                } else {
                    None
                }
            });
            out.push(last.flatten().into_iter().collect());
        }
        if let Some(c) = out.last_mut() {
            c.push(r);
        }
    }
    out
}

fn help(app: &App, ctx: &egui::Context, ch: &Chrome) {
    backdrop(ctx);
    let active = app.state.scope();
    let screen = ctx.content_rect();
    let cols = columns(help_rows(&app.keymap, active), 3);
    egui::Area::new(egui::Id::new("ac2-help"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width((screen.width() - 40.0).min(1200.0));
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Keys").strong().size(16.0));
                    ui.label(
                        RichText::new(format!(
                            "focused pane: {} · {} or Esc closes",
                            active.title(),
                            app.keymap
                                .chords(CommandId::Help, Scope::Global)
                                .first()
                                .map(|k| k.label())
                                .unwrap_or_default()
                        ))
                        .color(ch.dim),
                    );
                });
                ui.add_space(6.0);
                ui.columns(cols.len().max(1), |uis| {
                    for (ui, col) in uis.iter_mut().zip(&cols) {
                        for row in col {
                            help_row(ui, row, ch);
                        }
                    }
                });
                ui.add_space(6.0);
                let path = app
                    .keymap_path
                    .as_ref()
                    .map_or_else(|| "no config dir".into(), |p| p.display().to_string());
                ui.label(
                    RichText::new(format!(
                        "overrides: {path} · the palette ({}) lists every command",
                        app.keymap
                            .chords(CommandId::Palette, Scope::Global)
                            .first()
                            .map(|k| k.label())
                            .unwrap_or_default()
                    ))
                    .small()
                    .color(ch.dim),
                );
            });
        });
}

fn help_row(ui: &mut egui::Ui, row: &HelpRow, ch: &Chrome) {
    match row {
        HelpRow::Header { title, live } => {
            ui.add_space(4.0);
            let t = RichText::new(*title).strong();
            ui.label(if *live { t } else { t.color(ch.dim) });
        }
        HelpRow::Bind { keys, title, live } => {
            ui.horizontal(|ui| {
                // Fixed key column so titles line up.
                let h = ui.text_style_height(&egui::TextStyle::Body);
                let (r, _) = ui.allocate_exact_size(egui::vec2(88.0, h), egui::Sense::hover());
                ui.painter().text(
                    r.left_center(),
                    egui::Align2::LEFT_CENTER,
                    keys,
                    egui::TextStyle::Monospace.resolve(ui.style()),
                    ch.focus,
                );
                ui.label(RichText::new(title).color(if *live { ch.text } else { ch.dim }));
            });
        }
    }
}

fn palette(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    let Overlay::Palette(p) = &app.state.overlay else {
        return;
    };
    let active = app.state.scope();
    let entries = p.entries(&app.keymap, active);
    let selected = p.selected.min(entries.len().saturating_sub(1));
    let query = p.query.clone();
    backdrop(ctx);
    let mut run = None;
    egui::Area::new(egui::Id::new("ac2-palette"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 70.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(560.0);
                caret_line(ui, &format!("› {query}"), 16.0, ch);
                ui.separator();
                if entries.is_empty() {
                    ui.label(RichText::new("no matching command").color(ch.dim));
                }
                let first = selected.saturating_sub(PALETTE_ROWS - 1);
                for (i, e) in entries.iter().enumerate().skip(first).take(PALETTE_ROWS) {
                    let mut job = LayoutJob::default();
                    let base = egui::TextFormat {
                        font_id: egui::FontId::proportional(14.0),
                        color: ch.text,
                        ..Default::default()
                    };
                    let hit = egui::TextFormat {
                        color: ch.focus,
                        underline: egui::Stroke::new(1.0, ch.focus),
                        ..base.clone()
                    };
                    for (ci, c) in e.title.chars().enumerate() {
                        let f = if e.positions.contains(&ci) {
                            hit.clone()
                        } else {
                            base.clone()
                        };
                        job.append(&c.to_string(), 0.0, f);
                    }
                    let r = ui
                        .horizontal(|ui| {
                            ui.set_min_width(540.0);
                            let resp = ui.add(egui::Button::selectable(i == selected, job));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if let Some(k) = &e.key {
                                        ui.label(RichText::new(k).monospace().color(ch.dim));
                                    }
                                },
                            );
                            resp
                        })
                        .inner;
                    if r.clicked() {
                        run = Some(e.command);
                    }
                }
                ui.separator();
                ui.label(
                    RichText::new("↑↓ choose · Enter runs · Esc closes (and stops stimulus)")
                        .small()
                        .color(ch.dim),
                );
            });
        });
    if let Some(c) = run {
        app.state.overlay = Overlay::None;
        app.dispatch(Msg::Command(c));
    }
}

fn prompt(app: &App, ctx: &egui::Context, ch: &Chrome) {
    let Overlay::Prompt(p) = &app.state.overlay else {
        return;
    };
    backdrop(ctx);
    egui::Area::new(egui::Id::new("ac2-prompt"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 120.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new(p.kind.label()).strong());
                ui.add_space(4.0);
                caret_line(ui, &p.text, 18.0, ch);
                if let Some(e) = &p.error {
                    ui.label(RichText::new(e).color(ch.fault));
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Enter applies · Esc cancels (and stops stimulus)")
                        .small()
                        .color(ch.dim),
                );
            });
        });
}

/// Text followed by a caret.
fn caret_line(ui: &mut egui::Ui, text: &str, size: f32, ch: &Chrome) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        let r = ui.label(RichText::new(text).size(size).color(ch.text));
        let x = r.rect.right() + 2.0;
        let (y0, y1) = (r.rect.top() + 2.0, r.rect.bottom() - 2.0);
        ui.painter().line_segment(
            [egui::pos2(x, y0), egui::pos2(x, y1)],
            egui::Stroke::new(1.5, ch.focus),
        );
    });
}

fn toasts(app: &App, ctx: &egui::Context, ch: &Chrome) {
    if app.state.toasts.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::new("ac2-toasts"))
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
        .interactable(false)
        .show(ctx, |ui| {
            for t in app.state.toasts.iter().rev().take(4).rev() {
                let frame = egui::Frame::new()
                    .fill(if t.error { ch.fault } else { ch.raised })
                    .stroke(egui::Stroke::new(1.0, ch.border))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(10, 6));
                frame.show(ui, |ui| {
                    let c = if t.error { Color32::WHITE } else { ch.text };
                    ui.label(RichText::new(&t.text).color(c));
                });
                ui.add_space(4.0);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_lists_every_binding_once() {
        let k = Keymap::default();
        let rows = help_rows(&k, Scope::Transfer);
        let binds = rows
            .iter()
            .filter(|r| matches!(r, HelpRow::Bind { .. }))
            .count();
        // One row per bound (command, scope), the nine slot keys folded into one.
        let mut pairs: Vec<(CommandId, Scope)> =
            k.bindings().iter().map(|b| (b.command, b.scope)).collect();
        pairs.sort();
        pairs.dedup();
        assert_eq!(binds, pairs.len() - 8);
        let one = crate::keys::Chord::command(Key::Num1).label();
        assert!(rows.contains(&HelpRow::Bind {
            keys: format!("{one}…9"),
            title: "Capture selected measurement to slot 1…9".into(),
            live: true,
        }));
        // Focused scope right after the global one.
        let headers: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                HelpRow::Header { title, .. } => Some(*title),
                _ => None,
            })
            .collect();
        assert_eq!(headers[..2], ["Everywhere", "Transfer function"]);
    }

    #[test]
    fn columns_balance_and_never_end_on_a_header() {
        let rows = help_rows(&Keymap::default(), Scope::Spectrum);
        let n = rows.len();
        let cols = columns(rows, 3);
        assert_eq!(cols.len(), 3);
        assert_eq!(cols.iter().map(Vec::len).sum::<usize>(), n);
        for c in &cols {
            assert!(!matches!(c.last(), Some(HelpRow::Header { .. })));
            assert!(c.len() <= n.div_ceil(3) + 1);
        }
    }
}
