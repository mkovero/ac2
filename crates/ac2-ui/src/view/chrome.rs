//! Top bar (link, session, stimulus) and the measurement list.

use ac2_proto::model::MeasKind;
use ac2_scene::autosave::AutosaveTone;
use ac2_scene::format;
use eframe::egui::{self, Color32, RichText};

use crate::app::App;
use crate::keys::{CommandId, Scope};
use crate::state::{ConnState, Msg, StimPhase, outputs_text};
use crate::theme::Chrome;

fn dot(ui: &mut egui::Ui, c: Color32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), 4.5, c);
}

/// The first key of a global command, as shown in the bar.
fn key_hint(app: &App, c: CommandId) -> String {
    app.keymap
        .chords(c, Scope::Global)
        .first()
        .map_or_else(|| "—".into(), |k| k.label())
}

/// One thing in the top bar: its texts from the longest to the shortest, and how much it
/// matters. The bar is fitted before it is drawn, so nothing overlaps at any width: the
/// least important items shorten first, then go.
struct Item {
    /// Higher stays longer.
    prio: u8,
    /// Longest first; the bar shows one of them.
    variants: Vec<RichText>,
    /// May disappear entirely once its shortest text does not fit.
    droppable: bool,
    /// A status dot before the text.
    dot: Option<Color32>,
    /// A separator after it (on its far side from the bar's edge).
    sep: bool,
    hover: Option<String>,
}

impl Item {
    fn new(prio: u8, variants: Vec<RichText>) -> Self {
        Self {
            prio,
            variants,
            droppable: true,
            dot: None,
            sep: false,
            hover: None,
        }
    }

    fn kept(mut self) -> Self {
        self.droppable = false;
        self
    }

    fn sep(mut self) -> Self {
        self.sep = true;
        self
    }
}

/// Item spacing in the bar.
const BAR_GAP: f32 = 8.0;
/// A separator's own width (it also takes a gap on each side).
const BAR_SEP: f32 = 6.0;
/// The status dot.
const BAR_DOT: f32 = 10.0;

/// Which text of each item the bar shows (`None`: dropped) so that the sum of `widths`
/// (per item: each variant's width, extras included) fits `available`. Starting from the
/// longest texts, the lowest-priority item that can still give way shortens or, past its
/// shortest text, goes, until everything fits or nothing more may give way.
fn fit_bar(widths: &[(u8, Vec<f32>, bool)], available: f32) -> Vec<Option<usize>> {
    let mut pick: Vec<Option<usize>> = widths
        .iter()
        .map(|(_, v, _)| (!v.is_empty()).then_some(0))
        .collect();
    let total = |pick: &[Option<usize>]| -> f32 {
        widths
            .iter()
            .zip(pick)
            .filter_map(|((_, v, _), p)| p.map(|i| v[i]))
            .sum()
    };
    while total(&pick) > available {
        let next = widths
            .iter()
            .enumerate()
            .filter(|(i, (_, v, drop))| match pick[*i] {
                Some(k) => k + 1 < v.len() || *drop,
                None => false,
            })
            .min_by_key(|(_, (p, _, _))| *p)
            .map(|(i, _)| i);
        let Some(i) = next else { break };
        let n = widths[i].1.len();
        pick[i] = match pick[i] {
            Some(k) if k + 1 < n => Some(k + 1),
            _ => None,
        };
    }
    pick
}

fn text_width(ui: &egui::Ui, t: &RichText) -> f32 {
    egui::WidgetText::from(t.clone())
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
        .size()
        .x
}

/// Draws item `it` with its text `k`. Right to left, the code order flips: the text first,
/// then the dot (shown before it), then the separator (shown inwards of it).
fn draw_item(ui: &mut egui::Ui, it: &Item, k: usize, rtl: bool) {
    let text = |ui: &mut egui::Ui| {
        let r = ui.label(it.variants[k].clone());
        if let Some(h) = &it.hover {
            r.on_hover_text(h);
        }
    };
    if rtl {
        text(ui);
        if let Some(c) = it.dot {
            dot(ui, c);
        }
    } else {
        if let Some(c) = it.dot {
            dot(ui, c);
        }
        text(ui);
    }
    if it.sep {
        ui.separator();
    }
}

pub(super) fn top_bar(app: &mut App, ui: &mut egui::Ui, ch: &Chrome) {
    let st = &app.state;
    let now = std::time::Instant::now();
    let (color, link_full, link_short) = match &st.conn {
        ConnState::Connecting { target } => (
            ch.warn,
            format!("connecting to {target}…"),
            "connecting…".to_owned(),
        ),
        ConnState::Failed { target, error } => (
            ch.fault,
            format!("{target}: {error} · retrying"),
            "link failed · retrying".to_owned(),
        ),
        ConnState::Connected { target, server, .. } => {
            let responding = st.mirror.as_ref().is_some_and(|m| m.responding(now));
            let synced = st.mirror.as_ref().is_some_and(|m| m.synced());
            if !responding {
                (
                    ch.fault,
                    format!("{server} · {target} · not responding"),
                    "not responding".to_owned(),
                )
            } else if !synced {
                (
                    ch.warn,
                    format!("{server} · {target} · syncing"),
                    "syncing".to_owned(),
                )
            } else {
                (ch.ok, format!("{server} · {target}"), target.clone())
            }
        }
    };
    let dim = |s: String| RichText::new(s).color(ch.dim);
    let session = match st.daemon().and_then(|s| s.session.open.as_ref()) {
        Some(o) => {
            let rate = format::fixed(f64::from(o.sample_rate_hz) / 1000.0, 1);
            Item::new(
                40,
                vec![
                    dim(format!(
                        "{} · {rate} kHz · {} frames",
                        o.input_device.0, o.buffer_frames
                    )),
                    dim(format!("{rate} kHz · {} frames", o.buffer_frames)),
                ],
            )
        }
        None if st.daemon().is_some() => Item::new(
            60,
            vec![
                dim(format!(
                    "no audio session · {} opens one",
                    key_hint(app, CommandId::OpenSession)
                )),
                dim("no audio session".into()),
            ],
        ),
        None => Item::new(40, vec![dim("—".into())]),
    };
    let mut link = Item::new(
        70,
        vec![
            RichText::new(link_full),
            RichText::new(link_short),
            RichText::new(""),
        ],
    )
    .kept()
    .sep();
    link.dot = Some(color);
    let left = [
        Item::new(100, vec![RichText::new("ac2").strong().size(16.0)])
            .kept()
            .sep(),
        link,
        session,
    ];

    // Right side, from the edge inwards.
    let mut keys = Item::new(
        10,
        vec![
            dim(format!(
                "{} keys · {} commands",
                key_hint(app, CommandId::Help),
                key_hint(app, CommandId::Palette)
            )),
            dim(format!("{} keys", key_hint(app, CommandId::Help))),
        ],
    )
    .sep();
    keys.hover = Some(format!(
        "{} shows every key · {} finds every command by name · {} hides or shows the panes' key hints",
        key_hint(app, CommandId::Help),
        key_hint(app, CommandId::Palette),
        key_hint(app, CommandId::KeyHints)
    ));
    let mut right = vec![keys];
    if let Some(l) = st.autosave_label(super::now().wall) {
        let color = match l.tone {
            AutosaveTone::Quiet | AutosaveTone::Busy => ch.dim,
            AutosaveTone::Warning => ch.warn,
        };
        let mut it = Item::new(
            if l.tone == AutosaveTone::Warning {
                65
            } else {
                30
            },
            vec![RichText::new(l.text).color(color)],
        )
        .sep();
        it.hover = Some(l.detail);
        right.push(it);
    }
    right.extend(stimulus(app, ch));

    let items: Vec<&Item> = left.iter().chain(right.iter()).collect();
    let widths: Vec<(u8, Vec<f32>, bool)> = items
        .iter()
        .map(|it| {
            let extra = BAR_GAP
                + it.dot.map_or(0.0, |_| BAR_DOT + BAR_GAP)
                + if it.sep { BAR_SEP + 2.0 * BAR_GAP } else { 0.0 };
            let v = it
                .variants
                .iter()
                .map(|t| text_width(ui, t) + extra)
                .collect();
            (it.prio, v, it.droppable)
        })
        .collect();
    // A little slack: egui rounds widget sizes to whole pixels.
    let pick = fit_bar(&widths, ui.available_width() - 4.0);
    let (lp, rp) = pick.split_at(left.len());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = BAR_GAP;
        for (it, p) in left.iter().zip(lp) {
            if let Some(k) = p {
                draw_item(ui, it, *k, false);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (it, p) in right.iter().zip(rp) {
                if let Some(k) = p {
                    draw_item(ui, it, *k, true);
                }
            }
        });
    });
}

/// The stimulus items, from the bar's right edge inwards: what the keys do next, who holds
/// the generator, the level and outputs, the state badge.
fn stimulus(app: &App, ch: &Chrome) -> Vec<Item> {
    let st = &app.state;
    let generator = st.daemon().map(|s| &s.generator);
    let mine = generator
        .and_then(|g| g.owner.as_ref())
        .zip(st.my_client_id())
        .is_some_and(|(o, me)| o == me);
    let other = generator
        .and_then(|g| g.owner.as_ref())
        .filter(|_| !mine)
        .map(|o| o.0.clone());
    // The mirrored generator is the truth; the local phase only adds "requested".
    let (badge, color) = match (generator.map(|g| (g.armed, g.firing)), st.stimulus.phase) {
        (Some((_, true)), _) => ("FIRING", ch.fault),
        (Some((true, false)), StimPhase::FireRequested) => ("FIRE…", ch.armed),
        (Some((true, false)), _) => ("ARMED", ch.armed),
        (_, StimPhase::Arming) => ("ARMING…", ch.armed),
        (_, StimPhase::Stopping) => ("STOPPING…", ch.dim),
        _ => ("STIM OFF", ch.dim),
    };
    let level = st.stimulus.level.map_or_else(
        || "no level".to_string(),
        |l| format!("{} dBFS", format::signed(l.0, 1)),
    );
    let run = st
        .daemon()
        .and_then(|s| s.sweep.as_ref())
        .filter(|r| r.active());
    let sweep = st.sweep.plan.is_some();
    // A running sweep's progress and Stop are on the strip below the bar.
    let hint = match (run, st.stimulus.phase) {
        (Some(_), _) => format!("{} stops", key_hint(app, CommandId::StimulusStop)),
        (None, StimPhase::Idle) if st.stimulus.level.is_none() => "L types a level".to_string(),
        (None, StimPhase::Idle) => "Space arms".to_string(),
        (None, StimPhase::Armed) if sweep => "Enter plays the sweep · Esc stops".to_string(),
        (None, StimPhase::Armed) => "Enter fires · Esc stops".to_string(),
        _ => "Esc stops".to_string(),
    };
    let off = badge == "STIM OFF";
    let badge_text = RichText::new(badge)
        .strong()
        .color(if off { ch.dim } else { Color32::BLACK })
        .background_color(if off { Color32::TRANSPARENT } else { color });
    let prefix = if sweep { "sweep " } else { "" };
    let mut v = vec![
        Item::new(100, vec![badge_text]).kept(),
        Item::new(
            90,
            vec![
                RichText::new(format!(
                    "{prefix}{level} → out {}",
                    outputs_text(&st.stimulus.outputs)
                )),
                RichText::new(format!("{prefix}{level}")),
            ],
        )
        .kept(),
    ];
    if let Some(o) = other {
        v.push(Item::new(
            85,
            vec![
                RichText::new(format!("held by {o}")).color(ch.warn),
                RichText::new("held").color(ch.warn),
            ],
        ));
    }
    v.push(Item::new(20, vec![dim_text(hint, ch)]));
    // From the bar's edge inwards: the hint is outermost, the badge innermost.
    v.reverse();
    v
}

fn dim_text(s: String, ch: &Chrome) -> RichText {
    RichText::new(s).color(ch.dim)
}

/// The running operation: what, which step, a bar, time left, and Stop.
pub(super) fn progress(
    app: &mut App,
    ui: &mut egui::Ui,
    ch: &Chrome,
    p: &ac2_scene::progress::Progress,
) {
    let stop_key = key_hint(app, CommandId::StimulusStop);
    let mut stop = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(RichText::new(&p.title).strong().size(15.0).color(ch.text));
        ui.label(RichText::new(&p.detail).color(ch.dim));
        ui.label(RichText::new(&p.step).strong().size(15.0).color(ch.armed));
        ui.add(
            egui::ProgressBar::new(p.fraction)
                .desired_width(260.0)
                .fill(ch.armed),
        );
        if let Some(r) = &p.remaining {
            ui.label(RichText::new(r).color(ch.text));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let b = egui::Button::new(
                RichText::new(format!("■ Stop ({stop_key})"))
                    .strong()
                    .color(Color32::BLACK),
            )
            .fill(ch.fault)
            .min_size(egui::vec2(96.0, 22.0));
            if ui
                .add(b)
                .on_hover_text(format!(
                    "Fades the output out, disarms and discards this run · {stop_key}"
                ))
                .clicked()
            {
                stop = true;
            }
        });
    });
    if stop {
        app.dispatch(Msg::Command(CommandId::StimulusStop));
    }
}

/// Short kind tag of a measurement in lists.
pub(super) fn kind_tag(k: &MeasKind) -> &'static str {
    match k {
        MeasKind::Transfer { .. } => "TF",
        MeasKind::Spectrum { .. } => "FFT",
        MeasKind::Rta { .. } => "RTA",
        MeasKind::Spl { .. } => "SPL",
    }
}

/// Every input of the open session, metered all the time: the operator sees what reaches
/// the mic and the reference before and during any measurement.
fn inputs(app: &App, ui: &mut egui::Ui, ch: &Chrome) {
    let rows = app.state.session_inputs();
    if rows.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Inputs").strong());
        ui.label(RichText::new("dBFS RMS · peak tick").small().color(ch.dim));
    });
    ui.add_space(2.0);
    for r in &rows {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if let Some(u) = r.used {
                let color = match u {
                    ac2_scene::meter::InputUse::Reference => ch.focus,
                    ac2_scene::meter::InputUse::Measurement => ch.ok,
                };
                ui.label(
                    RichText::new(u.tag())
                        .small()
                        .strong()
                        .color(ch.panel)
                        .background_color(color),
                );
            }
            // Wrapped, never cut: the label names the mic curve in use.
            ui.add(
                egui::Label::new(RichText::new(&r.label).color(ch.text))
                    .wrap_mode(egui::TextWrapMode::Wrap),
            );
        });
        ui.horizontal(|ui| {
            super::session::meter_sized(ui, &r.reading, ch, 136.0);
        });
        ui.add_space(2.0);
    }
    ui.add_space(10.0);
}

pub(super) fn sidebar(app: &mut App, ui: &mut egui::Ui, ch: &Chrome) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| sidebar_lists(app, ui, ch));
}

fn sidebar_lists(app: &mut App, ui: &mut egui::Ui, ch: &Chrome) {
    let mut clicked = None;
    let mut clicked_trace = None;
    let mut renamed_trace = None;
    let mut toggled_trace = None;
    let tips = RowTips {
        meas: format!(
            "Click selects it · {} / {} step through the focused pane's measurements",
            key_hint(app, CommandId::NextMeasurement),
            key_hint(app, CommandId::PrevMeasurement)
        ),
        eye: key_hint(app, CommandId::ToggleTrace),
        select: format!(
            "Click selects it (again: deselects) · double click renames · {} / {} step through the shown traces",
            key_hint(app, CommandId::NextTrace),
            key_hint(app, CommandId::PrevTrace)
        ),
    };
    inputs(app, ui, ch);
    {
        let st = &app.state;
        ui.label(RichText::new("Measurements").strong());
        ui.add_space(4.0);
        let ms = st.measurements();
        if ms.is_empty() {
            ui.label(RichText::new("none").color(ch.dim));
        }
        for m in ms {
            let selected = st.selected == Some(m.id) && st.selected_trace.is_none();
            let kind = kind_tag(&m.config.kind);
            let state = match (m.running, m.frozen) {
                (_, true) => "frozen",
                (true, false) => "running",
                (false, false) => "stopped",
            };
            let mut text = format!("{kind}  {}\n     {state}", m.config.name);
            if let Some(d) = &m.delay {
                // Distance stays in the transfer legend's reference line.
                text.push_str(&format!(" · {}", format::ms(d.applied.0, 2)));
                if d.tracking && d.awaiting_pick {
                    text.push_str(" · tracking paused");
                } else if d.tracking {
                    text.push_str(" · tracking");
                }
            }
            match &m.config.kind {
                MeasKind::Transfer { config } if config.smoothing.is_some() => {
                    text.push_str(&format!(" · {}", format::smoothing(config.smoothing)));
                }
                MeasKind::Spectrum { config } => {
                    if let Some(f) = config.smoothing {
                        text.push_str(&format!(" · smoothed {}", format::octave_fraction(f)));
                    }
                }
                _ => {}
            }
            let e = st.edit(m.id);
            if e.inverted {
                text.push_str(" · inv");
            }
            if e.offset_db != 0.0 {
                text.push_str(&format!(" · {}", format::db_readout(e.offset_db)));
            }
            let r = ui
                .add(egui::Button::selectable(selected, text).wrap_mode(egui::TextWrapMode::Wrap))
                .on_hover_text(&tips.meas);
            if r.clicked() {
                clicked = Some(m.id);
            }
        }
        ui.add_space(12.0);
        traces_header(app, ui, ch);
        ui.add_space(4.0);
        let rows = st.trace_rows();
        if rows.is_empty() {
            ui.label(
                RichText::new(format!(
                    "{} captures the selected TF",
                    key_hint(app, CommandId::Slot1)
                ))
                .color(ch.dim),
            );
        }
        for row in &rows {
            match trace_row(ui, row, &tips, ch) {
                Some(RowClick::Select) => clicked_trace = Some(row.id),
                Some(RowClick::Eye) => toggled_trace = Some(row.id),
                Some(RowClick::Rename) => renamed_trace = Some(row.id),
                None => {}
            }
        }
    }
    if let Some(id) = clicked {
        app.dispatch(Msg::SelectMeas(id));
    }
    if let Some(id) = clicked_trace {
        app.dispatch(Msg::SelectTrace(id));
    }
    if let Some(id) = toggled_trace {
        app.dispatch(Msg::ToggleShown(id));
    }
    if let Some(id) = renamed_trace {
        app.dispatch(Msg::RenameTrace(id));
    }
}

/// "Traces" and the keys that act on the list.
fn traces_header(app: &App, ui: &mut egui::Ui, ch: &Chrome) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Traces").strong());
        ui.label(
            RichText::new(format!(
                "{} selects · {} shows / hides",
                key_hint(app, CommandId::NextTrace),
                key_hint(app, CommandId::ToggleTrace)
            ))
            .small()
            .color(ch.dim),
        );
    });
}

/// The tooltips of the list's rows, with the keys that do the same.
struct RowTips {
    meas: String,
    /// The key that shows / hides the selected trace.
    eye: String,
    select: String,
}

/// What a click on a trace row did.
enum RowClick {
    /// The row: select (again: deselect).
    Select,
    /// Its colour dot: show / hide.
    Eye,
    /// A double click on the row: rename.
    Rename,
}

/// Width of a row's colour dot, which is also its show / hide toggle.
const EYE_W: f32 = 18.0;

/// One stored trace: its colour dot (filled when shown, a ring when hidden; a click shows
/// or hides it) and its name over what it is, highlighted when selected.
fn trace_row(
    ui: &mut egui::Ui,
    row: &ac2_scene::trace_list::TraceRow,
    tips: &RowTips,
    ch: &Chrome,
) -> Option<RowClick> {
    let c = row.color;
    let color = Color32::from_rgba_unmultiplied(
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
        255,
    );
    let mut click = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let (r, eye) = ui.allocate_exact_size(egui::vec2(EYE_W, 22.0), egui::Sense::click());
        let what = if row.shown { "Hide" } else { "Show" };
        let label = format!("{what} {}", row.name);
        eye.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
        let centre = egui::pos2(r.center().x, r.min.y + 9.0);
        if row.shown {
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
            "{what} this trace · {} shows / hides the selected one",
            tips.eye
        );
        if eye.on_hover_text(tip).clicked() {
            click = Some(RowClick::Eye);
        }
        // The longest detail line that fits beside the dot, measured as drawn.
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
        let mut job = egui::text::LayoutJob::default();
        let body = egui::TextStyle::Body.resolve(ui.style());
        job.append(
            &row.name,
            0.0,
            egui::TextFormat::simple(body, if row.shown { color } else { ch.dim }),
        );
        job.append("\n", 0.0, egui::TextFormat::simple(small.clone(), ch.dim));
        job.append(&detail, 0.0, egui::TextFormat::simple(small, ch.dim));
        job.wrap.max_width = room;
        let r = ui
            .add(egui::Button::selectable(row.selected, job).wrap_mode(egui::TextWrapMode::Wrap))
            .on_hover_text(format!("{}\n{}", row.describe, tips.select));
        // A double click's second click also reads as a click: check it first, or it would
        // deselect the row the first click selected.
        if r.double_clicked() {
            click = Some(RowClick::Rename);
        } else if r.clicked() {
            click = Some(RowClick::Select);
        }
    });
    click
}

#[cfg(test)]
mod tests {
    use super::fit_bar;

    #[test]
    fn the_bar_shortens_then_drops_the_least_important_first() {
        // (priority, widths of its texts longest first, may go)
        let items = vec![
            (100, vec![30.0], false),
            (70, vec![200.0, 80.0, 20.0], false),
            (40, vec![250.0, 120.0], true),
            (10, vec![150.0, 60.0], true),
            (100, vec![60.0], false),
        ];
        assert_eq!(
            fit_bar(&items, 1000.0),
            vec![Some(0), Some(0), Some(0), Some(0), Some(0)]
        );
        // The keys hint shortens first, then goes, then the session text shortens.
        assert_eq!(
            fit_bar(&items, 650.0),
            vec![Some(0), Some(0), Some(0), Some(1), Some(0)]
        );
        assert_eq!(
            fit_bar(&items, 545.0),
            vec![Some(0), Some(0), Some(0), None, Some(0)]
        );
        assert_eq!(
            fit_bar(&items, 420.0),
            vec![Some(0), Some(0), Some(1), None, Some(0)]
        );
        // Kept items never go: at their shortest they stay even when nothing fits.
        assert_eq!(
            fit_bar(&items, 50.0),
            vec![Some(0), Some(2), None, None, Some(0)]
        );
    }
}
