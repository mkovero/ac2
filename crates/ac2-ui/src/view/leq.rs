//! The Leq windows dialog: the preset and horizon on top, one row per window (length and
//! weighting by name, limit and warn margin typed, − to remove it) under a heading whose +
//! adds one, the band windows likewise (one band each; "+ range…" adds one per band); keys
//! drive it, the mouse can too.

use eframe::egui::{self, RichText};

use crate::app::App;
use crate::caret::{Caret, Shown};
use crate::leq_dialog::{
    BandCol, BandField, BandFocus, Col, CueCol, Extra, Focus, LeqDialog, RangeCol,
};
use crate::state::{LeqMsg, Msg, Overlay};
use crate::theme::Chrome;

use super::overlays::{backdrop, card};

/// Width of a band meter row's title.
const BAND_TITLE_W: f32 = 110.0;

/// A band meter row's title, left in its column.
fn band_title(ui: &mut egui::Ui, title: RichText) {
    ui.allocate_ui_with_layout(
        egui::vec2(BAND_TITLE_W, 20.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(BAND_TITLE_W);
            ui.label(title);
        },
    );
}

/// A choice cell: ‹ value › — clicking the value steps forward.
fn choice(
    ui: &mut egui::Ui,
    text: String,
    focused: bool,
    at: Focus,
    msg: &mut Option<LeqMsg>,
    ch: &Chrome,
) {
    if ui.small_button("‹").clicked() {
        *msg = Some(LeqMsg::Cycle(at, -1));
    }
    let t = RichText::new(text).color(ch.text);
    let r = ui.add(egui::Button::selectable(focused, t));
    if focused {
        // The keys move the focus: the page follows it.
        super::settings::reveal_focused(&r);
    }
    if r.clicked() {
        *msg = Some(LeqMsg::Cycle(at, 1));
    }
    if ui.small_button("›").clicked() {
        *msg = Some(LeqMsg::Cycle(at, 1));
    }
}

/// A section heading with its +: a window added at the section's end.
fn heading(ui: &mut egui::Ui, title: &str, add: Focus, msg: &mut Option<LeqMsg>, ch: &Chrome) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong().color(ch.text));
        let b = ui
            .small_button(RichText::new("+").strong())
            .on_hover_text(format!("add a window to {} (Insert)", title.to_lowercase()));
        if b.clicked() {
            *msg = Some(LeqMsg::Add(add));
        }
    });
}

/// A row's −: the window removed.
fn remove_button(ui: &mut egui::Ui, at: Focus, msg: &mut Option<LeqMsg>) {
    let b = ui
        .small_button(RichText::new("−").strong())
        .on_hover_text("remove this window (Delete)");
    if b.clicked() {
        *msg = Some(LeqMsg::Remove(at));
    }
}

/// A typed cell; the selected text (typing replaces it) shows inverted.
#[allow(clippy::too_many_arguments)]
fn text_cell(
    ui: &mut egui::Ui,
    text: &str,
    focused: bool,
    selected: bool,
    empty: &str,
    at: Focus,
    msg: &mut Option<LeqMsg>,
    ch: &Chrome,
) {
    sized_text_cell(ui, 90.0, text, focused, selected, empty, at, msg, ch);
}

/// [`text_cell`] `width` wide.
#[allow(clippy::too_many_arguments)]
fn sized_text_cell(
    ui: &mut egui::Ui,
    width: f32,
    text: &str,
    focused: bool,
    selected: bool,
    empty: &str,
    at: Focus,
    msg: &mut Option<LeqMsg>,
    ch: &Chrome,
) {
    let sel = focused && selected && !text.is_empty();
    let shown = if sel {
        text.to_owned()
    } else if focused {
        format!("{text}▏")
    } else if text.is_empty() {
        empty.to_owned()
    } else {
        text.to_owned()
    };
    let mut t = RichText::new(shown).monospace();
    t = if sel {
        t.color(ch.panel).background_color(ch.focus)
    } else if text.is_empty() && !focused {
        t.color(ch.dim)
    } else {
        t.color(ch.text)
    };
    let r = ui.add_sized([width, 20.0], egui::Button::selectable(focused, t));
    if focused {
        super::settings::reveal_focused(&r);
    }
    if r.clicked() {
        *msg = Some(LeqMsg::Focus(at));
    }
}

/// A typed line edited at its caret (`Some` while focused), `width` px wide: a line too
/// long for it shows a window around the caret, cut with `…`. Returns the cell and, on a
/// click, the character of the line the click landed before.
pub(super) fn caret_cell(
    ui: &mut egui::Ui,
    width: f32,
    text: &str,
    caret: Option<Caret>,
    empty: &str,
    ch: &Chrome,
) -> (egui::Response, Option<usize>) {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let glyph = ui.fonts_mut(|f| f.glyph_width(&font, '0')).max(1.0);
    let cols = ((width - 2.0 * ui.spacing().button_padding.x) / glyph).floor() as usize;
    let shown = caret.map(|c| Shown::new(text, c, cols));
    let label = match &shown {
        Some(s) => s.text.clone(),
        None if text.is_empty() => empty.to_owned(),
        None if text.chars().count() > cols.max(2) => {
            let mut t: String = text.chars().take(cols.max(2) - 1).collect();
            t.push('…');
            t
        }
        None => text.to_owned(),
    };
    let color = if text.is_empty() && caret.is_none() {
        ch.dim
    } else {
        ch.text
    };
    let r = ui.add_sized(
        [width, 20.0],
        egui::Button::selectable(
            caret.is_some(),
            RichText::new(&label).monospace().color(color),
        ),
    );
    let mut at = None;
    if r.clicked()
        && let Some(p) = r.interact_pointer_pos()
    {
        let galley =
            ui.fonts_mut(|f| f.layout_no_wrap(label.clone(), font.clone(), egui::Color32::WHITE));
        // The button centres its text.
        let left = r.rect.center().x - galley.size().x / 2.0;
        let col = galley.cursor_from_pos(egui::vec2(p.x - left, 0.0)).index.0;
        at = Some(match (&shown, caret) {
            (Some(s), Some(c)) => s.index_at(col, c.index(text)),
            _ if text.is_empty() => 0,
            _ => col,
        });
    }
    (r, at)
}

/// The SPL / Leq page: the Leq windows and limits of the SPL pane's meter.
pub(super) fn leq_page(ui: &mut egui::Ui, d: &LeqDialog, ch: &Chrome) -> Option<LeqMsg> {
    let mut msg = None;
    ui.set_max_width(760.0);
    ui.label(
        RichText::new(format!("Leq windows and limits — {}", d.name))
            .strong()
            .size(16.0),
    );
    if !d.calibrated {
        ui.label(
            RichText::new(
                "Not calibrated: values read dBFS and limits are not judged until \
                 the input has an SPL calibration.",
            )
            .small()
            .color(ch.warn),
        );
    }
    ui.add_space(8.0);
    egui::Grid::new("ac2-leq-top")
        .num_columns(2)
        .spacing(egui::vec2(12.0, 6.0))
        .show(ui, |ui| {
            let f = d.focus == Focus::Preset;
            ui.label(RichText::new("Preset").color(if f { ch.text } else { ch.dim }));
            ui.horizontal(|ui| {
                choice(ui, d.preset_text(), f, Focus::Preset, &mut msg, ch);
            });
            ui.end_row();
            if let Some(s) = d.preset_source() {
                ui.label("");
                ui.add(egui::Label::new(RichText::new(s).small().color(ch.dim)).wrap());
                ui.end_row();
            }
            ui.label("");
            ui.add(egui::Label::new(RichText::new(d.preset_note()).small().color(ch.dim)).wrap());
            ui.end_row();
            let f = d.focus == Focus::Horizon;
            ui.label(RichText::new("Headroom over").color(if f { ch.text } else { ch.dim }));
            ui.horizontal(|ui| {
                choice(
                    ui,
                    format!(
                        "the next {}",
                        ac2_scene::leq::length(f64::from(d.horizon_s()))
                    ),
                    f,
                    Focus::Horizon,
                    &mut msg,
                    ch,
                );
            });
            ui.end_row();
        });
    ui.add_space(8.0);
    // + adds after the last window (or as the first, from the horizon).
    let leq_add = d
        .rows
        .len()
        .checked_sub(1)
        .map_or(Focus::Horizon, |row| Focus::Window {
            row,
            col: Col::Length,
        });
    heading(ui, "Leq windows", leq_add, &mut msg, ch);
    egui::Grid::new("ac2-leq-windows")
        .num_columns(5)
        .spacing(egui::vec2(12.0, 6.0))
        .show(ui, |ui| {
            for c in [Col::Length, Col::Weighting, Col::Limit, Col::Margin] {
                ui.label(RichText::new(c.title()).small().color(ch.dim));
            }
            ui.label("");
            ui.end_row();
            for (row, r) in d.rows.iter().enumerate() {
                let at = |col| Focus::Window { row, col };
                let fo = |col| d.focus == at(col);
                ui.horizontal(|ui| {
                    ui.set_min_width(190.0);
                    choice(
                        ui,
                        r.cell(Col::Length),
                        fo(Col::Length),
                        at(Col::Length),
                        &mut msg,
                        ch,
                    );
                });
                ui.horizontal(|ui| {
                    choice(
                        ui,
                        r.cell(Col::Weighting),
                        fo(Col::Weighting),
                        at(Col::Weighting),
                        &mut msg,
                        ch,
                    );
                });
                text_cell(
                    ui,
                    &r.limit,
                    fo(Col::Limit),
                    d.selected,
                    "no limit",
                    at(Col::Limit),
                    &mut msg,
                    ch,
                );
                text_cell(
                    ui,
                    &r.margin,
                    fo(Col::Margin),
                    d.selected,
                    "0",
                    at(Col::Margin),
                    &mut msg,
                    ch,
                );
                remove_button(ui, at(Col::Length), &mut msg);
                ui.end_row();
            }
        });
    if d.rows.is_empty() {
        ui.label(RichText::new("No windows: + adds one.").color(ch.dim));
    }
    ui.add_space(8.0);
    egui::Grid::new("ac2-leq-extra")
        .num_columns(3)
        .spacing(egui::vec2(12.0, 6.0))
        .show(ui, |ui| {
            for e in Extra::ALL {
                let at = Focus::Extra(e);
                let f = d.focus == at;
                ui.label(RichText::new(e.title()).color(if f { ch.text } else { ch.dim }));
                text_cell(
                    ui,
                    &d.extra_text(e),
                    f,
                    d.selected,
                    e.empty(),
                    at,
                    &mut msg,
                    ch,
                );
                ui.add(egui::Label::new(RichText::new(e.note()).small().color(ch.dim)).wrap());
                ui.end_row();
            }
        });
    ui.add_space(10.0);
    ui.label(RichText::new("Band Leq (1/3-octave bands against per-band limits)").strong());
    band_section(ui, d, &mut msg, ch);
    ui.add_space(10.0);
    rig_section(ui, d, &mut msg, ch);
    if let Some(n) = &d.notice {
        ui.add_space(4.0);
        ui.label(RichText::new(n).color(ch.warn));
    }
    if let Some(e) = &d.error {
        ui.add_space(4.0);
        ui.label(RichText::new(e).color(ch.fault));
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Apply").clicked() {
            msg = Some(LeqMsg::Submit);
        }
        if ui.button("Cancel").clicked() {
            msg = Some(LeqMsg::Cancel);
        }
    });
    msg
}

/// A two-way toggle: its value; a click flips it (→ on, ← off, as the arrows set it).
fn toggle(
    ui: &mut egui::Ui,
    text: &str,
    on: bool,
    focused: bool,
    at: Focus,
    msg: &mut Option<LeqMsg>,
    ch: &Chrome,
) {
    let r = ui.add(egui::Button::selectable(
        focused,
        RichText::new(text).color(ch.text),
    ));
    if focused {
        super::settings::reveal_focused(&r);
    }
    if r.clicked() {
        *msg = Some(LeqMsg::Cycle(at, if on { -1 } else { 1 }));
    }
}

/// The rig's rows: whether the rig keeps the meter, then the cues — folded until shown, one
/// row per cued subject (the meter first): a message per state and the level toggle, under
/// it the DMX levels per state, a bad field's reason under the row.
fn rig_section(ui: &mut egui::Ui, d: &LeqDialog, msg: &mut Option<LeqMsg>, ch: &Chrome) {
    use ac2_scene::{artnet, osc};
    ui.label(RichText::new("Rig").strong());
    egui::Grid::new("ac2-leq-rig")
        .num_columns(2)
        .spacing(egui::vec2(12.0, 6.0))
        .show(ui, |ui| {
            let f = d.focus == Focus::Scope;
            ui.label(RichText::new(osc::KEEP_ON_RIG).color(if f { ch.text } else { ch.dim }));
            toggle(
                ui,
                osc::scope_text(d.on_rig),
                d.on_rig,
                f,
                Focus::Scope,
                msg,
                ch,
            );
            ui.end_row();
            let f = d.focus == Focus::CueFold;
            ui.label(RichText::new(osc::CUES_TITLE).color(if f { ch.text } else { ch.dim }));
            toggle(
                ui,
                &d.cue_fold_text(),
                d.cues_shown,
                f,
                Focus::CueFold,
                msg,
                ch,
            );
            ui.end_row();
        });
    if !d.cues_shown {
        return;
    }
    ui.add(egui::Label::new(RichText::new(osc::CUES_NOTE).small().color(ch.dim)).wrap());
    ui.add(egui::Label::new(RichText::new(osc::APPEND_NOTE).small().color(ch.dim)).wrap());
    egui::Grid::new("ac2-leq-cues")
        .num_columns(6)
        .spacing(egui::vec2(8.0, 6.0))
        .show(ui, |ui| {
            ui.label("");
            for c in CueCol::ALL {
                ui.label(RichText::new(c.title()).small().color(ch.dim));
            }
            ui.end_row();
            for row in d.cue_rows() {
                let Some(c) = d.cue_edit(row) else {
                    continue;
                };
                let error = c.error();
                ui.label(RichText::new(d.cue_name(row)).color(ch.text));
                for col in CueCol::ALL {
                    let at = Focus::Cue { row, col };
                    let f = d.focus == at;
                    match col {
                        CueCol::State(s) => cue_line(ui, d, c.text_of(s), at, msg, ch),
                        CueCol::Append => {
                            toggle(ui, osc::append_text(c.append), c.append, f, at, msg, ch);
                        }
                        CueCol::Dmx(_) => {}
                    }
                }
                ui.end_row();
                ui.label(RichText::new(artnet::CUE_LINE).small().color(ch.dim));
                for col in CueCol::DMX {
                    let CueCol::Dmx(s) = col else { continue };
                    let at = Focus::Cue { row, col };
                    cue_line(ui, d, c.dmx_of(s), at, msg, ch);
                }
                ui.end_row();
                if let Some((col, e)) = error {
                    ui.label("");
                    ui.label(
                        RichText::new(format!("{}: {e}", col.field_name()))
                            .small()
                            .color(ch.fault),
                    );
                    ui.end_row();
                }
            }
        });
}

/// The band meter's rows: the meter, then its windows under a heading with a + and a
/// "+ range…" (each window one band on a row like a Leq window's; the range row, while
/// open, under the heading), then the corrections and the transfer.
fn band_section(ui: &mut egui::Ui, d: &LeqDialog, msg: &mut Option<LeqMsg>, ch: &Chrome) {
    let b = &d.bands;
    // Rows, not a grid: the notes under a row wrap to the page's width.
    let note = |ui: &mut egui::Ui, text: String| {
        ui.horizontal(|ui| {
            ui.add_space(BAND_TITLE_W + 12.0);
            ui.add(egui::Label::new(RichText::new(text).small().color(ch.dim)).wrap());
        });
    };
    let field = |ui: &mut egui::Ui, f: BandField, msg: &mut Option<LeqMsg>| {
        let at = Focus::Band(BandFocus::Field(f));
        let focused = d.focus == at;
        ui.horizontal(|ui| {
            let title = RichText::new(f.title()).color(if focused { ch.text } else { ch.dim });
            band_title(ui, title);
            ui.add_space(12.0);
            choice(ui, b.text(f), focused, at, msg, ch);
        });
    };
    field(ui, BandField::Meter, msg);
    note(ui, b.note(BandField::Meter));
    ui.add_space(4.0);
    // + adds one band after the last window, "+ range…" a band each from … to; either
    // turns the meter on.
    ui.horizontal(|ui| {
        ui.label(RichText::new("Band windows").strong().color(ch.text));
        let add = ui
            .small_button(RichText::new("+").strong())
            .on_hover_text("add a band window: the next band up (Insert)");
        if add.clicked() {
            *msg = Some(LeqMsg::Add(Focus::Band(BandFocus::Field(BandField::Meter))));
        }
        let range = ui
            .small_button(RichText::new("+ range…").strong())
            .on_hover_text(format!(
                "add a band window for each band from … to ({})",
                crate::keys::Chord::shift(egui::Key::Insert).label()
            ));
        if range.clicked() {
            *msg = Some(LeqMsg::OpenRange);
        }
    });
    if !b.is_on() {
        note(
            ui,
            "Off: + adds a band window and turns the band meter on; + range… one per band.".into(),
        );
        return;
    }
    // Fixed widths keep the columns aligned row to row.
    const COLS: [(BandCol, f32); 6] = [
        (BandCol::Band, 104.0),
        (BandCol::Length, 140.0),
        (BandCol::Weighting, 66.0),
        (BandCol::Limit, 92.0),
        (BandCol::DayOffset, 96.0),
        (BandCol::Margin, 92.0),
    ];
    let cell = |ui: &mut egui::Ui, w: f32, add: &mut dyn FnMut(&mut egui::Ui)| {
        ui.allocate_ui_with_layout(
            egui::vec2(w, 20.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(w);
                ui.set_max_width(w);
                add(ui);
            },
        );
    };
    if let Some(r) = &b.range {
        let at = |c| Focus::Band(BandFocus::Range(c));
        ui.horizontal(|ui| {
            ui.label(RichText::new("from").color(ch.dim));
            for (c, w, before) in [
                (RangeCol::From, 104.0, None),
                (RangeCol::To, 104.0, Some("to")),
                (RangeCol::Length, 140.0, None),
                (RangeCol::Weighting, 66.0, None),
            ] {
                if let Some(t) = before {
                    ui.label(RichText::new(t).color(ch.dim));
                }
                cell(ui, w, &mut |ui| {
                    choice(ui, r.cell(c), d.focus == at(c), at(c), msg, ch);
                });
            }
            if ui
                .button(RichText::new("Add").strong())
                .on_hover_text("one band window per band (Enter)")
                .clicked()
            {
                *msg = Some(LeqMsg::AddRange);
            }
            if ui
                .small_button("×")
                .on_hover_text("close without adding (Delete)")
                .clicked()
            {
                *msg = Some(LeqMsg::CloseRange);
            }
        });
        note(ui, r.summary(&b.rows));
    }
    ui.horizontal(|ui| {
        for (c, w) in COLS {
            cell(ui, w, &mut |ui| {
                ui.label(RichText::new(c.title()).small().color(ch.dim));
            });
        }
    });
    for (row, r) in b.rows.iter().enumerate() {
        let at = |col| Focus::Band(BandFocus::Window { row, col });
        let fo = |col| d.focus == at(col);
        ui.horizontal(|ui| {
            for (col, w) in COLS {
                cell(ui, w, &mut |ui| match col {
                    BandCol::Band | BandCol::Length | BandCol::Weighting => {
                        choice(ui, r.cell(col), fo(col), at(col), msg, ch);
                    }
                    BandCol::Limit | BandCol::DayOffset | BandCol::Margin => text_cell(
                        ui,
                        &r.cell(col),
                        fo(col),
                        d.selected,
                        match col {
                            BandCol::Limit => "no limit",
                            BandCol::DayOffset => "day = night",
                            _ => "0",
                        },
                        at(col),
                        msg,
                        ch,
                    ),
                });
            }
            remove_button(ui, at(BandCol::Band), msg);
        });
    }
    if b.rows.is_empty() {
        note(ui, "No band windows: + adds one.".into());
    }
    ui.add_space(4.0);
    field(ui, BandField::Impulse, msg);
    field(ui, BandField::Tonal, msg);
    note(ui, b.note(BandField::Impulse));
    ui.horizontal(|ui| {
        band_title(ui, RichText::new("Transfer").color(ch.dim));
        ui.add_space(12.0);
        ui.label(RichText::new(b.transfer_text()).color(ch.text));
    });
    let per_band = b.transfer_bands();
    if !per_band.is_empty() {
        note(ui, per_band.join(" · "));
    }
    note(ui, b.transfer_hint().into());
}

/// The confirmation before a new SPL log: what ends, what starts over, what is kept.
pub(super) fn new_log(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    let Overlay::NewLog(p) = &app.state.overlay else {
        return;
    };
    let k = p.confirm.clone();
    backdrop(ctx);
    let mut msg = None;
    egui::Area::new(egui::Id::new("ac2-new-log"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(520.0);
                ui.label(RichText::new(&k.title).strong().size(16.0));
                ui.add_space(6.0);
                for (i, l) in k.lines.iter().enumerate() {
                    let t = RichText::new(l);
                    ui.label(if i == 0 {
                        t.color(ch.warn)
                    } else {
                        t.color(ch.text)
                    });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(RichText::new("Start a new log").strong())
                        .clicked()
                    {
                        msg = Some(true);
                    }
                    if ui.button("Keep the current log").clicked() {
                        msg = Some(false);
                    }
                });
                ui.label(RichText::new(&k.hint).small().color(ch.dim));
            });
        });
    if let Some(m) = msg {
        app.dispatch(Msg::NewLog(m));
    }
}

/// A cue line's cell (a message or DMX levels): edited at the caret while focused.
fn cue_line(
    ui: &mut egui::Ui,
    d: &LeqDialog,
    text: &str,
    at: Focus,
    msg: &mut Option<LeqMsg>,
    ch: &Chrome,
) {
    let f = d.focus == at;
    let caret = (f && !d.selected).then_some(d.caret);
    let (r, click) = caret_cell(ui, 130.0, text, caret, "—", ch);
    if f && d.selected && !text.is_empty() {
        // Ctrl+A: the line selected, as other cells show it.
        ui.painter().rect_stroke(
            r.rect,
            2.0,
            egui::Stroke::new(1.0, ch.focus),
            egui::StrokeKind::Inside,
        );
    }
    if f {
        super::settings::reveal_focused(&r);
    }
    if let Some(n) = click {
        *msg = Some(LeqMsg::Caret(at, n));
    }
}
