//! Help overlay, command palette, text prompt and toasts.

use eframe::egui::{self, Color32, Key, RichText, text::LayoutJob};

use crate::app::App;
use crate::forms::Value;
use crate::keys::{CommandId, Keymap, STOP_ANYWHERE, Scope};
use crate::state::{FormMsg, HELP_LINE, Msg, Overlay};
use crate::theme::{Chrome, c32};

use crate::palette::PALETTE_ROWS;

/// Draws the open window; a full-window view (Settings) starts at `top`, under the bars
/// that say what drives the speakers.
pub(super) fn draw(app: &mut App, ctx: &egui::Context, ch: &Chrome, top: f32) {
    match app.state.overlay.clone() {
        Overlay::None => {}
        Overlay::Help => help(app, ctx, ch),
        Overlay::Notifications => notifications(app, ctx, ch),
        Overlay::Palette(_) => palette(app, ctx, ch),
        Overlay::Prompt(_) => prompt(app, ctx, ch),
        Overlay::DelayPick(_) => delay_pick(app, ctx, ch),
        Overlay::Form(_) => form(app, ctx, ch),
        Overlay::Settings(_) => super::settings::settings(app, ctx, ch, top),
        Overlay::Offer(_) => super::session::offer(app, ctx, ch),
        Overlay::NewLog(_) => super::leq::new_log(app, ctx, ch),
        Overlay::Delete(_) => delete(app, ctx, ch),
        Overlay::Choose(_) => choose(app, ctx, ch),
        // Drawn by its pane, under the title chip.
        Overlay::PaneMenu(_) => {}
    }
    toasts(app, ctx, top);
}

/// The confirmation before the selected measurement or stored trace is deleted: which
/// one, and what deleting it means; or why it cannot be.
fn delete(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    let Overlay::Delete(p) = &app.state.overlay else {
        return;
    };
    let k = p.confirm.clone();
    backdrop(ctx);
    let mut msg = None;
    egui::Area::new(egui::Id::new("ac2-delete"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(480.0);
                ui.label(RichText::new(&k.title).strong().size(16.0));
                ui.add_space(6.0);
                for (i, l) in k.lines.iter().enumerate() {
                    let t = RichText::new(l);
                    ui.label(if i == 0 {
                        t.color(ch.dim)
                    } else {
                        t.color(ch.warn)
                    });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if k.refused {
                        if ui.button("Close").clicked() {
                            msg = Some(false);
                        }
                        return;
                    }
                    if ui.button(RichText::new("Delete").strong()).clicked() {
                        msg = Some(true);
                    }
                    if ui.button("Keep it").clicked() {
                        msg = Some(false);
                    }
                });
                ui.label(RichText::new(&k.hint).small().color(ch.dim));
            });
        });
    if let Some(m) = msg {
        app.dispatch(Msg::Delete(m));
    }
}

/// A question with a few answers: what deleting a measurement does with its traces, where
/// a trace moves. The highlighted answer is what Enter takes; one that cannot be taken says
/// why under it.
fn choose(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    let Overlay::Choose(p) = &app.state.overlay else {
        return;
    };
    let p = p.as_ref().clone();
    backdrop(ctx);
    let mut msg = None;
    egui::Area::new(egui::Id::new("ac2-choose"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(520.0);
                ui.label(RichText::new(&p.title).strong().size(16.0));
                ui.add_space(6.0);
                for (i, l) in p.lines.iter().enumerate() {
                    let t = RichText::new(l);
                    ui.label(if i == 0 {
                        t.color(ch.dim)
                    } else {
                        t.color(ch.warn)
                    });
                }
                ui.add_space(8.0);
                for (i, c) in p.choices.iter().enumerate() {
                    let mut text = RichText::new(&c.label);
                    if c.blocked.is_some() {
                        text = text.color(ch.dim);
                    } else if i == p.index {
                        text = text.strong();
                    }
                    let r = ui.add_sized(
                        [ui.available_width(), 24.0],
                        egui::Button::selectable(i == p.index, text),
                    );
                    if r.clicked() {
                        msg = Some(Some(i));
                    }
                    if let Some(why) = &c.blocked {
                        ui.label(
                            RichText::new(format!("    not possible: {why}"))
                                .small()
                                .color(ch.dim),
                        );
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    // A question whose answers include Cancel needs no second one.
                    if !p.choices.iter().any(|c| c.label == "Cancel")
                        && ui.button("Cancel").clicked()
                    {
                        msg = Some(None);
                    }
                    ui.label(RichText::new(&p.hint).small().color(ch.dim));
                });
            });
        });
    if let Some(m) = msg {
        app.dispatch(Msg::Choose(m));
    }
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
            if let Some((a, _, title, keys)) = PAIRS
                .iter()
                .filter(|(a, b, _)| *a == *c || *b == *c)
                .find_map(|(a, b, t)| pair_keys(keymap, scope, *a, *b).map(|k| (*a, *b, t, k)))
            {
                if a == *c {
                    rows.push(HelpRow::Bind {
                        keys,
                        title: (*title).into(),
                        live,
                    });
                }
                continue;
            }
            // Keys that do not fit the key column side by side go one per line.
            let labels: Vec<String> = chords.iter().map(|k| k.label()).collect();
            let one_line = labels.join(" ");
            let keys = if one_line.chars().count() <= KEY_COLUMN_CHARS {
                one_line
            } else {
                labels.join("\n")
            };
            rows.push(HelpRow::Bind {
                keys,
                title: c.title().into(),
                live,
            });
        }
    }
    rows
}

/// Opposite steps shown on one row (`↑/↓ Stimulus level +1 / −1 dB`) when both have one key
/// with the same modifiers.
const PAIRS: [(CommandId, CommandId, &str); 10] = [
    (
        CommandId::LevelUp,
        CommandId::LevelDown,
        "Stimulus level +1 / −1 dB",
    ),
    (
        CommandId::LevelUpCoarse,
        CommandId::LevelDownCoarse,
        "Stimulus level +3 / −3 dB",
    ),
    (
        CommandId::ZoomIn,
        CommandId::ZoomOut,
        "Zoom frequency in / out (IR: time)",
    ),
    (
        CommandId::PanLeft,
        CommandId::PanRight,
        "Pan frequency down / up (IR: time)",
    ),
    (
        CommandId::CursorLeft,
        CommandId::CursorRight,
        "Cursor 1/12 octave down / up (IR: a step)",
    ),
    (
        CommandId::NudgeEarlier,
        CommandId::NudgeLater,
        "Delay −0.1 / +0.1 ms of the measurement or the selected stored trace",
    ),
    (
        CommandId::OffsetUp,
        CommandId::OffsetDown,
        "Display offset +1 / −1 dB of the selected curve",
    ),
    (
        CommandId::OffsetUpCoarse,
        CommandId::OffsetDownCoarse,
        "Display offset +3 / −3 dB of the selected curve",
    ),
    (
        CommandId::LevelZoomIn,
        CommandId::LevelZoomOut,
        "Zoom level axis in / out (vertical; IR: amplitude or dB)",
    ),
    (
        CommandId::LevelPanUp,
        CommandId::LevelPanDown,
        "Pan level axis up / down (IR: amplitude or dB)",
    ),
];

/// The widest key text the help's key column holds (`Alt+Shift+V`).
const KEY_COLUMN_CHARS: usize = 11;

/// `Shift+↑/↓` for a pair bound to one key each with the same modifiers in `scope`, short
/// enough for the key column; else `None` and the two get a row each.
fn pair_keys(keymap: &Keymap, scope: Scope, a: CommandId, b: CommandId) -> Option<String> {
    let (ka, kb) = (keymap.chords(a, scope), keymap.chords(b, scope));
    let ([x], [y]) = (ka.as_slice(), kb.as_slice()) else {
        return None;
    };
    if (x.command, x.alt, x.shift) != (y.command, y.alt, y.shift) {
        return None;
    }
    let text = format!("{}/{}", x.label(), crate::keys::Chord::key(y.key).label());
    (text.chars().count() <= KEY_COLUMN_CHARS).then_some(text)
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

fn help(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
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
                            "focused pane: {} · ↑↓ PgUp PgDn scroll · {} or Esc closes · {} \
                             stops the stimulus from any window",
                            active.title(),
                            app.keymap
                                .chords(CommandId::Help, Scope::Global)
                                .first()
                                .map(|k| k.label())
                                .unwrap_or_default(),
                            STOP_ANYWHERE.label()
                        ))
                        .color(ch.dim),
                    );
                });
                ui.add_space(6.0);
                // The keys scroll it (the window owns ↑/↓), the wheel too; the offset lives
                // in the state so the reducer moves it and the view keeps it in range.
                let out = egui::ScrollArea::vertical()
                    .max_height((screen.height() - 170.0).max(120.0))
                    .auto_shrink([false, true])
                    .vertical_scroll_offset(app.state.help_scroll)
                    .show(ui, |ui| {
                        ui.columns(cols.len().max(1), |uis| {
                            for (ui, col) in uis.iter_mut().zip(&cols) {
                                for row in col {
                                    help_row(ui, row, ch);
                                }
                            }
                        });
                    });
                let max = (out.content_size.y - out.inner_rect.height()).max(0.0);
                app.state.help_scroll = out.state.offset.y.clamp(0.0, max);
                app.state.help_page = (out.inner_rect.height() - HELP_LINE).max(HELP_LINE);
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
                let lines = keys.lines().count().max(1) as f32;
                let h = ui.text_style_height(&egui::TextStyle::Body) * lines;
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
                    RichText::new(format!(
                        "↑↓ PgUp PgDn choose · Enter runs · Esc closes · {} stops the stimulus",
                        STOP_ANYWHERE.label()
                    ))
                    .small()
                    .color(ch.dim),
                );
            });
        });
    // The wheel moves the highlight as ↑/↓ do (the list shows a window of it).
    if let Some(rows) = wheel_rows(ctx) {
        app.dispatch(Msg::Wheel { rows });
    }
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
                    RichText::new("Enter applies · Esc cancels")
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
                            let label = RichText::new(field.label.as_str()).color(if focused {
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
                        "{} · ↑↓ field · ←→ choose (inputs by name) · Esc closes",
                        f.kind.submit()
                    ))
                    .small()
                    .color(ch.dim),
                );
                if let Some(note) = f.kind.close_note() {
                    ui.label(RichText::new(note).small().color(ch.armed));
                }
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
                for (i, r) in rows.iter().enumerate() {
                    let on = i == c.selected;
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(if on { "▸" } else { " " })
                                .monospace()
                                .color(ch.focus),
                        );
                        ui.label(RichText::new(&r.key).monospace().strong().color(ch.focus));
                        ui.label(RichText::new(&r.text).monospace().color(if on {
                            ch.focus
                        } else {
                            ch.text
                        }));
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
                        "{} or ↑↓ Enter inserts · Shift+X strongest · Esc closes",
                        ac2_scene::finding::pick_keys(rows.len())
                    ))
                    .small()
                    .color(ch.dim),
                );
            });
        });
}

/// Whole rows the wheel moved this frame (down positive), for lists that show a window of
/// their rows: a row per line of a wheel notch, at least one when it moved at all so a
/// touchpad moves the list too.
pub(super) fn wheel_rows(ctx: &egui::Context) -> Option<i32> {
    let lines: f32 = ctx.input(|i| {
        i.raw
            .events
            .iter()
            .map(|e| match e {
                egui::Event::MouseWheel { unit, delta, .. } => match unit {
                    egui::MouseWheelUnit::Line => delta.y,
                    egui::MouseWheelUnit::Point => delta.y / 50.0,
                    egui::MouseWheelUnit::Page => delta.y * PALETTE_ROWS as f32,
                },
                _ => 0.0,
            })
            .sum()
    });
    if lines == 0.0 {
        return None;
    }
    // Content moving down (positive) is the list scrolling up.
    let rows = (lines.abs().round() as i32).max(1);
    Some(if lines > 0.0 { -rows } else { rows })
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

/// Under the toasts: the panes' bottom margin (6) and the hint strip's inset (2) around the
/// focused pane's key-hint line.
const TOAST_BOTTOM_RESERVED: f32 = super::panes::HINT_H + 8.0;

/// The toasts, laid out by [`ac2_scene::toast`] with the drawn font's measures: a click
/// dismisses one, the pointer resting on them holds them all.
fn toasts(app: &mut App, ctx: &egui::Context, top: f32) {
    let mut held = false;
    let mut dismiss = None;
    if !app.state.toasts.is_empty() {
        let theme = app.theme();
        let screen = ctx.content_rect();
        let font = egui::FontId::proportional(theme.font_size + 1.0);
        let line_h = ctx.fonts_mut(|f| f.row_height(&font));
        let measure = |s: &str| {
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE)
                    .size()
                    .x
            })
        };
        let area = ac2_scene::toast::area(
            screen.width(),
            screen.height(),
            top - screen.min.y,
            TOAST_BOTTOM_RESERVED,
        );
        let texts: Vec<&str> = app.state.toasts.iter().map(|t| t.text.as_str()).collect();
        let boxes = ac2_scene::toast::stack(&texts, area, line_h, &measure);
        let n = boxes.len();
        for (k, b) in boxes.into_iter().enumerate() {
            let t = &app.state.toasts[b.index];
            let c = ac2_scene::toast::colors(t.severity, &theme);
            let min = screen.min + egui::vec2(b.rect.x, b.rect.y);
            let size = egui::vec2(b.rect.w, b.rect.h);
            let text = t.text.clone();
            let id = t.id;
            // Keyed by place from the newest, not by toast: an area egui has not seen
            // before is drawn invisibly for a pass to measure it, and a toast must show
            // the moment it comes.
            let resp = egui::Area::new(egui::Id::new(("ac2-toast", n - 1 - k)))
                .order(egui::Order::Tooltip)
                .fixed_pos(min)
                .fade_in(false)
                .show(ctx, |ui| {
                    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
                    let p = ui.painter();
                    p.rect_filled(rect, 4.0, c32(c.background));
                    p.rect_stroke(
                        rect,
                        4.0,
                        egui::Stroke::new(1.0, c32(c.border)),
                        egui::StrokeKind::Inside,
                    );
                    for (i, line) in b.lines.iter().enumerate() {
                        p.text(
                            rect.min
                                + egui::vec2(
                                    ac2_scene::toast::PAD_X,
                                    ac2_scene::toast::PAD_Y + i as f32 * line_h,
                                ),
                            egui::Align2::LEFT_TOP,
                            line,
                            font.clone(),
                            c32(c.text),
                        );
                    }
                    resp.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &text)
                    });
                    resp
                })
                .inner;
            held |= resp.hovered();
            if resp.clicked() {
                dismiss = Some(id);
            }
        }
    }
    if let Some(id) = dismiss {
        app.dispatch(Msg::DismissToast(id));
    } else if held != app.state.toasts_held {
        app.dispatch(Msg::ToastsHeld(held));
    }
}

/// The recent notifications, newest first, each with its severity and how long ago it
/// came; scrolled by the keys like the help.
fn notifications(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    backdrop(ctx);
    let theme = app.theme();
    let screen = ctx.content_rect();
    let width = (screen.width() - 40.0).clamp(320.0, 760.0);
    let left = screen.center().x - width / 2.0;
    let now = app.state.now_s;
    egui::Area::new(egui::Id::new("ac2-notifications"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(left.max(screen.min.x), screen.min.y + 40.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(width - 28.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Recent notifications").strong().size(16.0));
                    ui.label(
                        RichText::new(format!(
                            "newest first · the last {} · ↑↓ PgUp PgDn scroll · Esc closes",
                            ac2_scene::toast::LOG_LEN
                        ))
                        .color(ch.dim),
                    );
                });
                ui.add_space(6.0);
                if app.state.notices.is_empty() {
                    ui.label(RichText::new("nothing yet").color(ch.dim));
                }
                let out = egui::ScrollArea::vertical()
                    .max_height((screen.height() - 150.0).max(120.0))
                    .auto_shrink([false, true])
                    .vertical_scroll_offset(app.state.help_scroll)
                    .show(ui, |ui| {
                        for n in app.state.notices.iter().rev() {
                            notice_row(ui, n, now, &theme, ch);
                        }
                    });
                let max = (out.content_size.y - out.inner_rect.height()).max(0.0);
                app.state.help_scroll = out.state.offset.y.clamp(0.0, max);
                app.state.help_page = (out.inner_rect.height() - HELP_LINE).max(HELP_LINE);
            });
        });
}

fn notice_row(
    ui: &mut egui::Ui,
    n: &crate::state::Notice,
    now: f64,
    theme: &ac2_scene::Theme,
    ch: &Chrome,
) {
    use ac2_scene::banner::Severity;
    let c = ac2_scene::toast::colors(n.severity, theme);
    let word = match n.severity {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Fault => "error",
    };
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(" {word} "))
                .small()
                .color(c32(c.text))
                .background_color(c32(c.background)),
        );
        let mut when = ac2_scene::format::ago(now - n.at_s);
        if n.count > 1 {
            when.push_str(&format!(" · ×{}", n.count));
        }
        ui.label(RichText::new(when).small().color(ch.dim));
    });
    ui.add(egui::Label::new(RichText::new(&n.text).color(ch.text)).wrap());
    ui.add_space(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_lists_every_binding_once() {
        // Which opposite steps fit the key column depends on how keys are named; pin the PC way.
        crate::keys::set_label_style(crate::keys::LabelStyle::Pc);
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
        // Opposite steps share a row where their keys fit the key column: all but the
        // display offset's ±3 dB (Alt+Shift+↑/↓ is too wide).
        assert_eq!(binds, pairs.len() - 16 - (PAIRS.len() - 1));
        assert!(rows.contains(&HelpRow::Bind {
            keys: "↑/↓".into(),
            title: "Stimulus level +1 / −1 dB".into(),
            live: true,
        }));
        let alt = crate::keys::Chord::alt(Key::ArrowUp).label();
        assert!(rows.contains(&HelpRow::Bind {
            keys: format!("{alt}/↓"),
            title: "Display offset +1 / −1 dB of the selected curve".into(),
            live: true,
        }));
        assert!(rows.iter().any(|r| matches!(
            r,
            HelpRow::Bind { title, .. } if title == CommandId::OffsetUpCoarse.title()
        )));
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
        // Too wide side by side: one key per line.
        assert!(rows.contains(&HelpRow::Bind {
            keys: "Delete\nBackspace".into(),
            title: CommandId::DeleteSelected.title().into(),
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
