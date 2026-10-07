//! Top bar (link, session, stimulus) and the measurement list.

use ac2_proto::model::TraceOwner;
use ac2_scene::autosave::AutosaveTone;
use ac2_scene::format;
use ac2_scene::meas_list::{Mark, TreeKey, TreeRow};
use ac2_scene::recording::RecordingTone;
use eframe::egui::{self, Color32, RichText};

use crate::app::App;
use crate::keys::{CommandId, STOP_ANYWHERE, Scope};
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
    let stopped = crate::scenes::audio_stopped(st, super::now().wall);
    let session = match (st.daemon().and_then(|s| s.session.open.as_ref()), stopped) {
        (Some(_), Some(t)) => {
            let mut it = Item::new(
                75,
                t.bar
                    .iter()
                    .map(|b| RichText::new(b.clone()).color(ch.fault))
                    .collect(),
            );
            it.hover = Some(format!("{} · {}", t.banner, t.detail));
            it
        }
        (Some(o), None) => {
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
        (None, _) if st.daemon().is_some() => Item::new(
            60,
            vec![
                dim(format!(
                    "no audio session · {} opens one",
                    key_hint(app, CommandId::OpenSession)
                )),
                dim("no audio session".into()),
            ],
        ),
        (None, _) => Item::new(40, vec![dim("—".into())]),
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
    if let Some(l) = st.recording_label() {
        let color = match l.tone {
            RecordingTone::Recording => ch.fault,
            RecordingTone::Quiet => ch.dim,
            RecordingTone::Warning => ch.warn,
        };
        let active = l.tone != RecordingTone::Quiet;
        let mut it = Item::new(
            if active { 70 } else { 25 },
            vec![RichText::new(l.text).color(color)],
        )
        .sep();
        it.hover = Some(l.detail);
        right.push(it);
    }
    if let Some(t) = st.replay_label() {
        let mut it = Item::new(70, vec![RichText::new(t).color(ch.armed)]).sep();
        it.hover = Some(ac2_scene::recording::REPLAY_DETAIL.to_owned());
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
    // A little slack: egui rounds widget sizes to whole pixels; the gear keeps its place.
    let pick = fit_bar(&widths, ui.available_width() - 4.0 - GEAR_W);
    let (lp, rp) = pick.split_at(left.len());
    let settings_tip = format!("Settings ({})", key_hint(app, CommandId::Settings));
    let mut open_settings = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = BAR_GAP;
        for (it, p) in left.iter().zip(lp) {
            if let Some(k) = p {
                draw_item(ui, it, *k, false);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let gear = ui
                .add(
                    egui::Button::new(RichText::new("⚙").size(16.0).color(ch.text))
                        .frame(false)
                        .min_size(egui::vec2(GEAR_W - BAR_GAP, 18.0)),
                )
                .on_hover_text(settings_tip);
            open_settings = gear.clicked();
            for (it, p) in right.iter().zip(rp) {
                if let Some(k) = p {
                    draw_item(ui, it, *k, true);
                }
            }
        });
    });
    if open_settings {
        app.dispatch(crate::state::Msg::Command(CommandId::Settings));
    }
}

/// Room the Settings gear takes at the bar's right edge.
const GEAR_W: f32 = 28.0;

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
    // What the keys do next, as the focused view decides. With a window open the window
    // has Space, Enter and Esc; only the missing level is said then.
    let window = st.overlay != crate::state::Overlay::None;
    let hint = st
        .stimulus_next()
        .filter(|_| run.is_none())
        .filter(|(next, what)| {
            !window
                || matches!(
                    (next, what),
                    (
                        ac2_scene::stimulus::Next::Space,
                        ac2_scene::stimulus::Stimulus::Generator { level: None, .. }
                    )
                )
        })
        .map(|(next, what)| {
            ac2_scene::stimulus::hint(next, &what, &key_hint(app, CommandId::StimulusLevel))
        });
    // While anything is armed or playing: the stop that works from anywhere, windows
    // included (Esc stops too while no window is open).
    let live = run.is_some() || st.stimulus_live();
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
                    "{prefix}{level} → {}",
                    ac2_scene::rig::stimulus_outputs(
                        &st.stimulus.outputs,
                        st.daemon()
                            .map(|s| s.outputs.as_slice())
                            .unwrap_or_default()
                    )
                )),
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
    if let Some((long, short)) = hint {
        v.push(Item::new(20, vec![dim_text(long, ch), dim_text(short, ch)]));
    }
    if live {
        let stop = STOP_ANYWHERE.label();
        let mut it = Item::new(
            95,
            vec![
                RichText::new(format!("■ Stop: {stop}"))
                    .strong()
                    .color(ch.fault),
                RichText::new(format!("■ {stop}")).strong().color(ch.fault),
            ],
        )
        .kept();
        it.hover = Some(format!(
            "{stop} stops and disarms the stimulus from anywhere, also with a window open; \
             with no window open Esc does too"
        ));
        v.push(it);
    }
    // From the bar's edge inwards: the hints are outermost, the badge innermost.
    v.reverse();
    v
}

fn dim_text(s: String, ch: &Chrome) -> RichText {
    RichText::new(s).color(ch.dim)
}

/// The running operation: what, which step, a bar, time left, and Stop.
/// The progress strip drawn over the bottom of the pane area `over`, translucent so the
/// panes keep their size and place under it.
pub(super) fn progress_overlay(
    app: &mut App,
    ctx: &egui::Context,
    ch: &Chrome,
    p: &ac2_scene::progress::Progress,
    over: egui::Rect,
) {
    let w = (over.width() - 24.0).clamp(1.0, 1100.0);
    let fill = Color32::from_rgba_unmultiplied(ch.panel.r(), ch.panel.g(), ch.panel.b(), 242);
    egui::Area::new(egui::Id::new("ac2-progress"))
        .order(egui::Order::Middle)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(egui::pos2(over.center().x, over.bottom() - 8.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, ch.armed))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_width(w - 20.0);
                    progress(app, ui, ch, p);
                });
        });
}

fn progress(app: &mut App, ui: &mut egui::Ui, ch: &Chrome, p: &ac2_scene::progress::Progress) {
    let stop_key = STOP_ANYWHERE.label();
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
                    "Fades the output out, disarms and discards this run · {stop_key} from \
                     anywhere (Esc too while no window is open)"
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
    let tips = RowTips {
        meas: format!(
            "Click selects it · the arrow folds it · {} / {} step through the focused pane's \
             measurements · {} shows / hides its curve, {} everything under it · {} deletes it \
             (asks first)",
            key_hint(app, CommandId::NextMeasurement),
            key_hint(app, CommandId::PrevMeasurement),
            key_hint(app, CommandId::ToggleSelected),
            key_hint(app, CommandId::HideGroup),
            key_hint(app, CommandId::DeleteSelected)
        ),
        eye: key_hint(app, CommandId::ToggleSelected),
        select: format!(
            "Click selects it (again: deselects) · double click renames · {} moves it · {} / {} \
             step through the shown traces",
            key_hint(app, CommandId::MoveTrace),
            key_hint(app, CommandId::NextTrace),
            key_hint(app, CommandId::PrevTrace)
        ),
    };
    inputs(app, ui, ch);
    let mut msg = None;
    {
        let st = &app.state;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Measurements").strong());
            ui.label(
                RichText::new(format!(
                    "{} selects a trace · {} shows / hides",
                    key_hint(app, CommandId::NextTrace),
                    key_hint(app, CommandId::ToggleSelected)
                ))
                .small()
                .color(ch.dim),
            );
        });
        ui.add_space(4.0);
        let rows = st.tree_rows();
        if rows.is_empty() {
            ui.label(RichText::new("none").color(ch.dim));
        }
        for row in &rows {
            if let Some(m) = tree_row(ui, row, &tips, ch) {
                msg = Some(m);
            }
        }
        if !rows.iter().any(|r| matches!(r.key, TreeKey::Trace(_))) && !rows.is_empty() {
            ui.label(
                RichText::new(format!(
                    "{} captures the selected TF under it",
                    key_hint(app, CommandId::Slot1)
                ))
                .small()
                .color(ch.dim),
            );
        }
    }
    if let Some(m) = msg {
        app.dispatch(m);
    }
}

/// The tooltips of the tree's rows, with the keys that do the same.
struct RowTips {
    meas: String,
    /// The key that shows / hides the selected curve.
    eye: String,
    select: String,
}

/// Width of a row's colour dot, which is also its show / hide toggle.
const EYE_W: f32 = 18.0;
/// How far a row under a header is indented.
const INDENT: f32 = 14.0;

/// One row of the measurement tree: a header (its fold arrow, its tag and name over its
/// state) or a row under it (its tree line, its dot when it has a curve to show or hide,
/// its name over what it is).
fn tree_row(ui: &mut egui::Ui, row: &TreeRow, tips: &RowTips, ch: &Chrome) -> Option<Msg> {
    let mut click = None;
    let group = match row.key {
        TreeKey::Meas(meas) => Some(TraceOwner::Meas { meas }),
        TreeKey::Imported => Some(TraceOwner::Imported),
        _ => None,
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if let (Some(folded), Some(g)) = (row.collapsed, group) {
            let what = if folded { "Unfold" } else { "Fold" };
            let (rect, r) = ui.allocate_exact_size(egui::vec2(EYE_W, 22.0), egui::Sense::click());
            let label = format!("{what} {}", row.name);
            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
            // Drawn, not a glyph: the arrow characters are missing from the UI font.
            let c = egui::pos2(rect.center().x, rect.min.y + 9.0);
            let color = if r.hovered() { ch.text } else { ch.dim };
            let points = if folded {
                vec![
                    c + egui::vec2(-2.5, -4.5),
                    c + egui::vec2(3.5, 0.0),
                    c + egui::vec2(-2.5, 4.5),
                ]
            } else {
                vec![
                    c + egui::vec2(-4.5, -2.5),
                    c + egui::vec2(4.5, -2.5),
                    c + egui::vec2(0.0, 3.5),
                ]
            };
            ui.painter().add(egui::Shape::convex_polygon(
                points,
                color,
                egui::Stroke::NONE,
            ));
            let r = r.on_hover_text(label);
            if r.clicked() {
                click = Some(Msg::ToggleGroup(g));
            }
        } else {
            ui.add_space(INDENT);
            ui.label(
                RichText::new(if row.last { "└" } else { "├" })
                    .color(ch.dim)
                    .monospace(),
            );
        }
        // A curve's dot: filled when shown, a ring when hidden; a click shows or hides it.
        let dot = match (row.key, row.dot) {
            (TreeKey::Trace(_), Some((c, shown))) => Some((to_color32(c), shown)),
            (TreeKey::Live(_), _) => Some((ch.text, !row.hidden)),
            _ => None,
        };
        if let Some((color, shown)) = dot {
            let (r, eye) = ui.allocate_exact_size(egui::vec2(EYE_W, 22.0), egui::Sense::click());
            let what = if shown { "Hide" } else { "Show" };
            let label = format!("{what} {}", row.name);
            eye.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
            let centre = egui::pos2(r.center().x, r.min.y + 9.0);
            if shown {
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
                "{what} this curve · {} shows / hides the selected one",
                tips.eye
            );
            if eye.on_hover_text(tip).clicked() {
                click = Some(match row.key {
                    TreeKey::Trace(id) => Msg::ToggleShown(id),
                    TreeKey::Live(id) => Msg::ToggleMeasShown(id),
                    _ => return,
                });
            }
        }
        // The longest detail line that fits beside the name, measured as drawn.
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
        let name_color = match row.dot {
            _ if row.hidden => ch.dim,
            Some((c, _)) => to_color32(c),
            None => ch.text,
        };
        let mut job = egui::text::LayoutJob::default();
        let body = egui::TextStyle::Body.resolve(ui.style());
        let mut fmt = egui::TextFormat::simple(body, name_color);
        if row.depth == 0 {
            fmt.font_id.size += 0.5;
        }
        job.append(&row.name, 0.0, fmt);
        job.append("\n", 0.0, egui::TextFormat::simple(small.clone(), ch.dim));
        job.append(&detail, 0.0, egui::TextFormat::simple(small, ch.dim));
        job.wrap.max_width = room;
        // Filled: the keys act on it. Outlined: still the selected measurement, while a
        // stored trace selected after it has the keys.
        let b = match row.mark {
            Mark::Active => egui::Button::selectable(true, job),
            Mark::Selected => egui::Button::new(job)
                .fill(Color32::TRANSPARENT)
                .stroke(egui::Stroke::new(1.0, ch.focus)),
            Mark::None => egui::Button::selectable(false, job),
        };
        let tip = match row.key {
            TreeKey::Trace(_) => format!("{}\n{}", row.describe, tips.select),
            _ => format!("{}\n{}", row.describe, tips.meas),
        };
        let r = ui
            .add(b.wrap_mode(egui::TextWrapMode::Wrap))
            .on_hover_text(tip);
        // A double click's second click also reads as a click: check it first, or it would
        // deselect the row the first click selected.
        if let TreeKey::Trace(id) = row.key
            && r.double_clicked()
        {
            click = Some(Msg::RenameTrace(id));
        } else if r.clicked() {
            click = Some(match row.key {
                TreeKey::Meas(id) | TreeKey::Live(id) | TreeKey::Math(id) => Msg::SelectMeas(id),
                TreeKey::Trace(id) => Msg::SelectTrace(id),
                TreeKey::Imported => Msg::ToggleGroup(TraceOwner::Imported),
            });
        }
    });
    click
}

fn to_color32(c: ac2_scene::primitives::Color) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
        255,
    )
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
