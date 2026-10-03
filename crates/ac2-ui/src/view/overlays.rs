//! Help overlay, command palette, text prompt and toasts.

use eframe::egui::{self, Color32, Key, RichText, text::LayoutJob};

use crate::app::App;
use crate::forms::Value;
use crate::keys::{CommandId, Keymap, Scope};
use crate::state::{FormMsg, Msg, Overlay};
use crate::theme::Chrome;

const PALETTE_ROWS: usize = 12;

pub(super) fn draw(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    match app.state.overlay.clone() {
        Overlay::None => {}
        Overlay::Help => help(app, ctx, ch),
        Overlay::Palette(_) => palette(app, ctx, ch),
        Overlay::Prompt(_) => prompt(app, ctx, ch),
        Overlay::DelayPick(_) => delay_pick(app, ctx, ch),
        Overlay::Form(_) => form(app, ctx, ch),
        Overlay::Session(_) => super::session::session(app, ctx, ch),
        Overlay::Offer(_) => super::session::offer(app, ctx, ch),
        // Drawn by its pane, under the title chip.
        Overlay::PaneMenu(_) => {}
    }
    toasts(app, ctx, ch);
}

pub(super) fn backdrop(ctx: &egui::Context) {
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

pub(super) fn card(ch: &Chrome) -> egui::Frame {
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
    use CommandId as C;
    let groups: [([CommandId; 9], &str); 2] = [
        (
            [
                C::Slot1,
                C::Slot2,
                C::Slot3,
                C::Slot4,
                C::Slot5,
                C::Slot6,
                C::Slot7,
                C::Slot8,
                C::Slot9,
            ],
            "Capture selected measurement to slot 1…9",
        ),
        (
            [
                C::ShowSlot1,
                C::ShowSlot2,
                C::ShowSlot3,
                C::ShowSlot4,
                C::ShowSlot5,
                C::ShowSlot6,
                C::ShowSlot7,
                C::ShowSlot8,
                C::ShowSlot9,
            ],
            "Show / hide slot 1…9",
        ),
    ];
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
    let mut rows = Vec::new();
    for scope in scopes {
        let live = scope == Scope::Global || scope == active;
        rows.push(HelpRow::Header {
            title: scope.title(),
            live,
        });
        // A group bound to one modifier + digit 1…9 collapses into one row.
        let collapsed: Vec<bool> = groups
            .iter()
            .map(|(cmds, _)| {
                let chords: Vec<Vec<crate::keys::Chord>> =
                    cmds.iter().map(|c| keymap.chords(*c, scope)).collect();
                chords.iter().zip(digits).all(|(v, d)| {
                    v.len() == 1 && v[0].key == d && {
                        let first = &chords[0][0];
                        (v[0].command, v[0].alt, v[0].shift)
                            == (first.command, first.alt, first.shift)
                    }
                })
            })
            .collect();
        for c in CommandId::ALL {
            let chords = keymap.chords(*c, scope);
            if chords.is_empty() {
                continue;
            }
            if let Some(g) = groups
                .iter()
                .zip(&collapsed)
                .position(|((cmds, _), col)| *col && cmds.contains(c))
            {
                let (cmds, title) = &groups[g];
                if *c == cmds[0] {
                    let one = chords[0].label();
                    rows.push(HelpRow::Bind {
                        keys: format!("{one}…9"),
                        title: (*title).into(),
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
    // Explicit position and width: centring by anchor uses the previous frame's size, so an
    // overlay wider than its first measurement slid off the left edge.
    let width = (screen.width() - 40.0).clamp(320.0, 1200.0);
    let left = screen.center().x - width / 2.0;
    egui::Area::new(egui::Id::new("ac2-help"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(left.max(screen.min.x), screen.min.y + 40.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(width - 24.0);
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
                // Wrap within the column so a long title never widens the overlay.
                ui.add(
                    egui::Label::new(RichText::new(title).color(if *live {
                        ch.text
                    } else {
                        ch.dim
                    }))
                    .wrap(),
                );
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

/// The new-measurement dialogs: one row per field, the focused row highlighted; inputs by
/// name with their meters; keys drive it, the mouse can too.
fn form(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    let Overlay::Form(f) = &app.state.overlay else {
        return;
    };
    let f = f.clone();
    let meters = app.state.input_meters();
    backdrop(ctx);
    let mut msg = None;
    egui::Area::new(egui::Id::new("ac2-form"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(600.0);
                ui.label(RichText::new(f.kind.title()).strong().size(16.0));
                ui.add_space(8.0);
                egui::Grid::new("ac2-form-grid")
                    .num_columns(3)
                    .spacing(egui::vec2(12.0, 6.0))
                    .min_col_width(0.0)
                    .show(ui, |ui| {
                        for (i, field) in f.fields.iter().enumerate() {
                            let focused = i == f.focus;
                            let label = RichText::new(field.label).color(if focused {
                                ch.text
                            } else {
                                ch.dim
                            });
                            if ui
                                .add(egui::Label::new(label).sense(egui::Sense::click()))
                                .clicked()
                            {
                                msg = Some(FormMsg::Focus(i));
                            }
                            ui.horizontal(|ui| {
                                ui.set_min_width(250.0);
                                match &field.value {
                                    Value::Text(t) => {
                                        let selected = focused && f.selected && !t.is_empty();
                                        let shown = if selected {
                                            t.clone()
                                        } else if focused {
                                            format!("{t}▏")
                                        } else if t.is_empty() {
                                            "—".to_owned()
                                        } else {
                                            t.clone()
                                        };
                                        let mut text = RichText::new(shown).monospace();
                                        // Selected: typing replaces it, shown inverted.
                                        text = if selected {
                                            text.color(ch.panel).background_color(ch.focus)
                                        } else {
                                            text.color(ch.text)
                                        };
                                        if ui.add(egui::Button::selectable(focused, text)).clicked()
                                        {
                                            msg = Some(FormMsg::Focus(i));
                                        }
                                    }
                                    Value::Choice { options, .. }
                                    | Value::Channel { options, .. } => {
                                        let shown = field.display();
                                        let shown = if options.is_empty() {
                                            "—".to_owned()
                                        } else {
                                            shown
                                        };
                                        if ui.small_button("‹").clicked() {
                                            msg = Some(FormMsg::Cycle(i, -1));
                                        }
                                        let text = RichText::new(shown).color(ch.text);
                                        let b = egui::Button::selectable(focused, text);
                                        let r = if matches!(field.value, Value::Channel { .. }) {
                                            // Fixed width: the meters line up.
                                            ui.add_sized([170.0, 20.0], b)
                                        } else {
                                            ui.add(b)
                                        };
                                        if r.clicked() {
                                            msg = Some(FormMsg::Cycle(i, 1));
                                        }
                                        if ui.small_button("›").clicked() {
                                            msg = Some(FormMsg::Cycle(i, 1));
                                        }
                                        if let Some(c) = field
                                            .channel_value()
                                            .filter(|_| field.id != crate::forms::FieldId::Output)
                                        {
                                            let m = meters.get(&c).cloned().unwrap_or_else(
                                                ac2_scene::meter::MeterReading::none,
                                            );
                                            super::session::meter(ui, &m, ch);
                                        }
                                    }
                                }
                            });
                            ui.label(RichText::new(&field.hint).small().color(ch.dim));
                            ui.end_row();
                        }
                    });
                if let Some(e) = &f.error {
                    ui.add_space(4.0);
                    ui.label(RichText::new(e).color(ch.fault));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let verb = f.kind.verb();
                    if ui.button(verb).clicked() {
                        msg = Some(FormMsg::Submit);
                    }
                    if ui.button("Cancel").clicked() {
                        msg = Some(FormMsg::Cancel);
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "{} · ↑↓ field · ←→ choose (inputs by name) · Esc closes (and stops stimulus)",
                        f.kind.submit()
                    ))
                    .small()
                    .color(ch.dim),
                );
            });
        });
    if let Some(m) = msg {
        app.dispatch(Msg::Form(m));
    }
}

/// The candidate list of an ambiguous delay finding, over the top of the transfer pane. No
/// backdrop: the IR and transfer plots it is chosen from stay visible.
fn delay_pick(app: &App, ctx: &egui::Context, ch: &Chrome) {
    let Overlay::DelayPick(c) = &app.state.overlay else {
        return;
    };
    egui::Area::new(egui::Id::new("ac2-delay-pick"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
        .interactable(false)
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(420.0);
                ui.label(RichText::new(format!("{}: pick the delay", c.name)).strong());
                ui.label(
                    RichText::new(ac2_scene::finding::outcome_text(&c.finding.outcome))
                        .small()
                        .color(ch.armed),
                );
                if let Some(note) = ac2_scene::finding::ambiguity_note(&c.finding.outcome) {
                    ui.label(RichText::new(note).small().color(ch.text));
                }
                ui.add_space(6.0);
                let rows = c.rows();
                for r in &rows {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&r.key).monospace().strong().color(ch.focus));
                        ui.label(RichText::new(&r.text).monospace().color(ch.text));
                    });
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        ac2_scene::finding::band_text(c.finding.band),
                        ac2_scene::finding::confidence_text(&c.finding.confidence)
                    ))
                    .small()
                    .color(ch.dim),
                );
                ui.label(
                    RichText::new(format!(
                        "{} inserts · Shift+X strongest · Esc closes (and stops stimulus)",
                        ac2_scene::finding::pick_keys(rows.len())
                    ))
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
        // One row per bound (command, scope), each group of nine slot keys folded into one.
        let mut pairs: Vec<(CommandId, Scope)> =
            k.bindings().iter().map(|b| (b.command, b.scope)).collect();
        pairs.sort();
        pairs.dedup();
        assert_eq!(binds, pairs.len() - 16);
        let one = crate::keys::Chord::command(Key::Num1).label();
        assert!(rows.contains(&HelpRow::Bind {
            keys: format!("{one}…9"),
            title: "Capture selected measurement to slot 1…9".into(),
            live: true,
        }));
        assert!(rows.contains(&HelpRow::Bind {
            keys: "1…9".into(),
            title: "Show / hide slot 1…9".into(),
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
