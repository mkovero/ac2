//! Top bar (link, session, stimulus) and the measurement list.

use ac2_scene::autosave::AutosaveTone;
use ac2_scene::format;
use ac2_scene::meas_list::{Mark, TreeKey};
use ac2_scene::recording::RecordingTone;
use eframe::egui::{self, Color32, RichText};

use crate::app::App;
use crate::keys::{CommandId, STOP_ANYWHERE, Scope};
use crate::marquee::Marquee;
use crate::state::{ConnState, DragMsg, Msg, StimPhase, outputs_text};
use crate::theme::Chrome;

use super::segment::{Part, SEGMENT_MIN_W, draw_segment, health_part, segment_width};

mod tree_row;
use tree_row::{RowTips, tree_row};

fn dot(ui: &mut egui::Ui, c: Color32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), 4.5, c);
}

/// A small ▾ after an item that opens a list, as on a pane's chip; drawn, since the UI
/// font has no such glyph.
fn chevron(ui: &mut egui::Ui, c: Color32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(BAR_DOT, 10.0), egui::Sense::hover());
    let (cx, cy) = (r.center().x, r.center().y);
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(cx - 3.5, cy - 1.5),
            egui::pos2(cx + 3.5, cy - 1.5),
            egui::pos2(cx, cy + 2.5),
        ],
        c,
        egui::Stroke::NONE,
    ));
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
    variants: Vec<egui::WidgetText>,
    /// May disappear entirely once its shortest text does not fit.
    droppable: bool,
    /// A status dot before the text.
    dot: Option<Color32>,
    /// A ▾ after the text: the item opens a list.
    chevron: Option<Color32>,
    /// A separator after it (on its far side from the bar's edge).
    sep: bool,
    hover: Option<String>,
    /// A click (or tap) runs it.
    click: Option<CommandId>,
    /// The rigs drop-down: the menu opens under it.
    rigs: bool,
}

impl Item {
    fn new(prio: u8, variants: Vec<RichText>) -> Self {
        Self::of(prio, variants.into_iter().map(Into::into).collect())
    }

    fn of(prio: u8, variants: Vec<egui::WidgetText>) -> Self {
        Self {
            prio,
            variants,
            droppable: true,
            dot: None,
            chevron: None,
            sep: false,
            hover: None,
            click: None,
            rigs: false,
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
pub(super) const BAR_GAP: f32 = 8.0;
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

pub(super) fn text_width(ui: &egui::Ui, t: &egui::WidgetText) -> f32 {
    t.clone()
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
/// Draws `it` in its variant `k`; returns its command when it was clicked.
fn draw_item(ui: &mut egui::Ui, it: &Item, k: usize, rtl: bool) -> Option<CommandId> {
    draw_item_at(ui, it, k, rtl).0
}

/// [`draw_item`], with where its text went.
fn draw_item_at(
    ui: &mut egui::Ui,
    it: &Item,
    k: usize,
    rtl: bool,
) -> (Option<CommandId>, egui::Rect) {
    let mut clicked = false;
    let mut at = egui::Rect::NOTHING;
    let mut text = |ui: &mut egui::Ui| {
        let mut l = egui::Label::new(it.variants[k].clone());
        if it.click.is_some() {
            l = l.sense(egui::Sense::click());
        }
        let mut r = ui.add(l);
        if it.click.is_some() {
            r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        if let Some(h) = &it.hover {
            r = r.on_hover_text(h);
        }
        clicked = r.clicked();
        at = r.rect;
    };
    if rtl {
        if let Some(c) = it.chevron {
            chevron(ui, c);
        }
        text(ui);
        if let Some(c) = it.dot {
            dot(ui, c);
        }
    } else {
        if let Some(c) = it.dot {
            dot(ui, c);
        }
        text(ui);
        if let Some(c) = it.chevron {
            chevron(ui, c);
        }
    }
    if it.sep {
        ui.separator();
    }
    (it.click.filter(|_| clicked), at)
}

pub(super) fn top_bar(app: &mut App, ui: &mut egui::Ui, ch: &Chrome) {
    let st = &app.state;
    let now = std::time::Instant::now();
    let link_state = match &st.rig.conn {
        ConnState::Connecting { target } => ac2_scene::chrome::Link::Connecting { target },
        ConnState::Failed { target, error } => ac2_scene::chrome::Link::Failed { target, error },
        ConnState::Connected { target, server, .. } => ac2_scene::chrome::Link::Connected {
            target,
            server,
            responding: st.rig.mirror.as_ref().is_some_and(|m| m.responding(now)),
            synced: st.rig.mirror.as_ref().is_some_and(|m| m.synced()),
        },
    };
    let link_line = ac2_scene::chrome::link_line(link_state);
    let color = match link_line.tone {
        ac2_scene::chrome::LinkTone::Ok => ch.ok,
        ac2_scene::chrome::LinkTone::Warn => ch.warn,
        ac2_scene::chrome::LinkTone::Fault => ch.fault,
    };
    let (link_full, link_short) = (link_line.full, link_line.short);
    let dim = |s: String| RichText::new(s).color(ch.dim);
    let stopped = crate::scenes::audio_stopped(st, super::now().wall);
    // The session line in full: when it is wider than its room the segment scrolls.
    let (session_prio, session) = match (st.daemon().and_then(|s| s.session.open.as_ref()), stopped)
    {
        (Some(_), Some(t)) => (
            75,
            Part::single(
                ui,
                RichText::new(t.bar.first().cloned().unwrap_or_default())
                    .color(ch.fault)
                    .into(),
                Some(ac2_scene::chrome::stopped_hover(&t.banner, &t.detail)),
            ),
        ),
        (Some(o), None) => {
            let l = ac2_scene::audio::session_line(o);
            let color = if l.changed { ch.warn } else { ch.dim };
            (
                40,
                Part::single(ui, RichText::new(l.full).color(color).into(), l.detail),
            )
        }
        (None, _) if st.daemon().is_some() => (
            60,
            Part::single(ui, dim(st.no_audio_line(&app.keymap)).into(), None),
        ),
        (None, _) => (40, Part::single(ui, dim("—".into()).into(), None)),
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
    let working = st.daemon().map(|_| {
        let l = st.working_label(super::now().wall);
        let name = ac2_scene::sessions::origin_name(st.working().origin.as_ref());
        let color = if l.unsaved { ch.warn } else { ch.text };
        let short = if l.unsaved {
            ac2_scene::chrome::unsaved_short(&name)
        } else {
            name
        };
        let mut it = Item::new(
            if l.unsaved { 80 } else { 50 },
            vec![
                RichText::new(l.text).color(color),
                RichText::new(short).color(color),
            ],
        )
        .sep();
        it.hover = Some(l.detail);
        it.click = Some(CommandId::Sessions);
        it
    });
    let mut segment = vec![session];
    segment.extend(health_part(app, ui, ch, now));
    let mut left = vec![
        Item::new(100, vec![RichText::new("ac2").strong().size(16.0)])
            .kept()
            .sep(),
        link,
    ];
    left.extend(working);

    // Right side, from the edge inwards.
    let [keys_full, keys_short] = ac2_scene::chrome::keys_hint(
        &key_hint(app, CommandId::Help),
        &key_hint(app, CommandId::Palette),
    );
    let mut keys = Item::new(10, vec![dim(keys_full), dim(keys_short)]).sep();
    keys.hover = Some(ac2_scene::chrome::keys_tip(
        &key_hint(app, CommandId::Help),
        &key_hint(app, CommandId::Palette),
        &key_hint(app, CommandId::KeyHints),
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
    // Innermost, right after the session line: the rig everything else in the bar is of.
    right.push(rig_item(st, ch));

    let segment_w = segment_width(&segment);
    let item_w = |it: &Item| {
        let extra = BAR_GAP
            + it.dot.map_or(0.0, |_| BAR_DOT + BAR_GAP)
            + it.chevron.map_or(0.0, |_| BAR_DOT + BAR_GAP)
            + if it.sep { BAR_SEP + 2.0 * BAR_GAP } else { 0.0 };
        let v = it
            .variants
            .iter()
            .map(|t| text_width(ui, t) + extra)
            .collect();
        (it.prio, v, it.droppable)
    };
    // The segment takes all the room between the two groups. It enters the fit as two
    // entries: the width past its scrolling minimum, given up early (at the session line's
    // own priority: scrolling beats dropping other items), and the minimum itself, given
    // up only after the link has shortened to its dot (the segment says what the audio
    // does; the dot's colour still says how the link is).
    let min_w = segment_w.min(SEGMENT_MIN_W);
    let widths: Vec<(u8, Vec<f32>, bool)> = left
        .iter()
        .map(item_w)
        .chain([
            (session_prio, vec![segment_w - min_w, 0.0], false),
            (
                session_prio.max(SEGMENT_KEEP_PRIO),
                vec![min_w + BAR_GAP],
                true,
            ),
        ])
        .chain(right.iter().map(item_w))
        .collect();
    // A little slack: egui rounds widget sizes to whole pixels; the gear keeps its place.
    let pick = fit_bar(&widths, ui.available_width() - 4.0 - GEAR_W);
    let (lp, rest) = pick.split_at(left.len());
    let (segment_shown, rp) = (rest[1].is_some(), &rest[2..]);
    let settings_tip = format!("Settings ({})", key_hint(app, CommandId::Settings));
    let mut open_settings = false;
    let mut run = None;
    let fade = ch.panel;
    let marquee = &mut app.marquee;
    let mut wake = None;
    let mut rig_at = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = BAR_GAP;
        for (it, p) in left.iter().zip(lp) {
            if let Some(k) = p {
                run = run.or(draw_item(ui, it, *k, false));
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
                    let (c, at) = draw_item_at(ui, it, *k, true);
                    if it.rigs {
                        rig_at = Some(at);
                    }
                    run = run.or(c);
                }
            }
            if segment_shown {
                let room = ui.available_rect_before_wrap();
                wake = draw_segment(ui, room, &segment, marquee, fade);
            } else {
                *marquee = Marquee::default();
            }
        });
    });
    if let Some(w) = wake {
        // At the marquee's own frame period, not a predicted frame earlier.
        let dt = ui.input(|i| i.predicted_dt);
        let w = std::time::Duration::from_secs_f64(w);
        ui.ctx()
            .request_repaint_after(crate::pace::repaint_delay(w, dt));
    }
    app.rig_menu_at = rig_at;
    if open_settings {
        app.dispatch(crate::state::Msg::Command(CommandId::Settings));
    }
    if let Some(c) = run {
        app.dispatch(crate::state::Msg::Command(c));
    }
}

/// The rigs drop-down: the rig shown, by name (also with one rig, so the bar says whose
/// it is). Greyed and inert while a window is open: the window acts on the rig shown.
fn rig_item(st: &crate::state::AppState, ch: &Chrome) -> Item {
    let name = st.rig_name(st.active());
    let free = matches!(
        st.overlay,
        crate::state::Overlay::None | crate::state::Overlay::RigMenu(_)
    );
    let color = if free { ch.text } else { ch.dim };
    let mut it = Item::new(68, vec![RichText::new(name).color(color)]).sep();
    it.hover = Some(if free {
        ac2_scene::chrome::RIG_MENU_TIP.to_owned()
    } else {
        ac2_scene::chrome::RIG_SWITCH_WINDOW_OPEN.to_owned()
    });
    it.click = free.then_some(CommandId::Rigs);
    it.rigs = true;
    it.chevron = Some(ch.dim);
    it
}

/// The rigs drop-down's list, under its top-bar item: each rig (greyed when down), then
/// Connect… and closing the rig shown.
pub(super) fn rig_menu(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    let crate::state::Overlay::RigMenu(menu) = app.state.overlay else {
        return;
    };
    let Some(at) = app.rig_menu_at else {
        return;
    };
    let rows = app.state.rig_menu_rows();
    let mut picked = None;
    let area = egui::Area::new(egui::Id::new("ac2-rig-menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(at.left_bottom() + egui::vec2(0.0, 4.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(200.0);
                for (i, row) in rows.iter().enumerate() {
                    let (text, color) = rig_row_text(&app.state, *row, ch);
                    let r = ui.add(
                        egui::Button::selectable(i == menu.index, RichText::new(text).color(color))
                            .min_size(egui::vec2(200.0, 0.0)),
                    );
                    if r.clicked() {
                        picked = Some(*row);
                    }
                }
            });
        });
    let outside = ctx.input(|i| i.pointer.any_click())
        && !area.response.contains_pointer()
        && !at.contains(ctx.pointer_interact_pos().unwrap_or_default());
    if let Some(r) = picked {
        app.dispatch(crate::state::Msg::RigPick(r));
    } else if outside {
        app.dispatch(crate::state::Msg::Command(CommandId::Rigs));
    }
}

/// A row of the rigs drop-down as drawn.
fn rig_row_text(
    st: &crate::state::AppState,
    row: crate::state::RigRow,
    ch: &Chrome,
) -> (String, Color32) {
    use crate::state::RigRow;
    match row {
        RigRow::Rig(id) => {
            let up = st
                .find_rig(id)
                .is_some_and(|r| matches!(r.conn, ConnState::Connected { .. }));
            let text = ac2_scene::chrome::rig_row(&st.rig_name(id), up);
            (text, if up { ch.text } else { ch.dim })
        }
        RigRow::Connect => (ac2_scene::chrome::RIG_CONNECT.to_owned(), ch.text),
        RigRow::Close(id) => (ac2_scene::chrome::rig_close(&st.rig_name(id)), ch.text),
    }
}

/// The scrolling segment's minimum gives way after the link (70); a stopped stream's banner
/// keeps its own, higher priority.
const SEGMENT_KEEP_PRIO: u8 = 72;

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
    // A lit badge is a filled chip: banner fills with their own text colour.
    let firing = (ch.fault_fill, ch.on_fault_fill);
    let armed = (ch.warn_fill, ch.on_warn_fill);
    let (badge, (fill, ink)) = match (
        generator.map(|g| (g.armed, g.firing)),
        st.rig.stimulus.phase,
    ) {
        (Some((_, true)), _) => ("FIRING", firing),
        (Some((true, false)), StimPhase::FireRequested) => ("FIRE…", armed),
        (Some((true, false)), _) => ("ARMED", armed),
        (_, StimPhase::Arming) => ("ARMING…", armed),
        (_, StimPhase::Stopping) => ("STOPPING…", (ch.dim, ch.panel)),
        _ => ("STIM OFF", (Color32::TRANSPARENT, ch.dim)),
    };
    let level = st.rig.stimulus.level.map_or_else(
        || "no level".to_string(),
        |l| format!("{} dBFS", format::signed(l.0, 1)),
    );
    let run = st
        .daemon()
        .and_then(|s| s.sweep.as_ref())
        .filter(|r| r.active());
    let sweep = st.rig.sweep.plan.is_some();
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
            let level_key = key_hint(app, CommandId::StimulusLevel);
            if st.many_rigs() {
                let rig = st.rig_name(st.active());
                ac2_scene::stimulus::rig_hint(&rig, next, &what, &level_key)
            } else {
                ac2_scene::stimulus::hint(next, &what, &level_key)
            }
        });
    let elsewhere = st.live_elsewhere();
    // The level and outputs are the rig shown's: with more than one, say whose.
    let rig = if st.many_rigs() {
        format!("{}: ", st.rig_name(st.active()))
    } else {
        String::new()
    };
    // While anything is armed or playing: the stop that works from anywhere, windows
    // included (Esc stops too while no window is open).
    let live = run.is_some() || st.stimulus_live() || !elsewhere.is_empty();
    let badge_text = RichText::new(badge)
        .strong()
        .color(ink)
        .background_color(fill);
    let prefix = if sweep { "sweep " } else { "" };
    let mut v = vec![
        Item::new(100, vec![badge_text]).kept(),
        Item::new(
            90,
            vec![
                RichText::new(format!(
                    "{rig}{prefix}{level} → {}",
                    ac2_scene::rig::stimulus_outputs(
                        &st.rig.stimulus.outputs,
                        st.daemon()
                            .map(|s| s.outputs.as_slice())
                            .unwrap_or_default()
                    )
                )),
                RichText::new(format!(
                    "{rig}{prefix}{level} → out {}",
                    outputs_text(&st.rig.stimulus.outputs)
                )),
                RichText::new(format!("{rig}{prefix}{level}")),
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
        it.hover = Some(if st.many_rigs() {
            ac2_scene::stimulus::stop_every_rig_tip(&stop)
        } else {
            format!(
                "{stop} stops and disarms the stimulus from anywhere, also with a window open; \
                 with no window open Esc does too"
            )
        });
        v.push(it);
        if !elsewhere.is_empty() {
            v.push(
                Item::new(
                    96,
                    vec![
                        RichText::new(ac2_scene::stimulus::live_elsewhere(&elsewhere))
                            .strong()
                            .color(ch.fault),
                    ],
                )
                .kept(),
            );
        }
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
                    .color(ch.on_fault_fill),
            )
            .fill(ch.fault_fill)
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

pub(super) fn sidebar(app: &mut App, ui: &mut egui::Ui, ch: &Chrome) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| sidebar_lists(app, ui, ch));
}

fn sidebar_lists(app: &mut App, ui: &mut egui::Ui, ch: &Chrome) {
    let tips = RowTips {
        meas: format!(
            "Click shows it in the focused pane · the arrow folds it · {} / {} step through \
             the list · {} shows / hides its curve, {} everything under it · {} \
             deletes it (asks first) · drag onto a pane to show it there, onto another row \
             to combine the two in a math channel",
            key_hint(app, CommandId::NextInTree),
            key_hint(app, CommandId::PrevInTree),
            key_hint(app, CommandId::ToggleSelected),
            key_hint(app, CommandId::HideGroup),
            key_hint(app, CommandId::DeleteSelected)
        ),
        eye: key_hint(app, CommandId::ToggleSelected),
        select: format!(
            "Click selects it (again: deselects) · double click renames (an EQ set: opens \
             it) · {} moves it · {} / {} step through the shown traces · drag onto another \
             row to combine the two in a math channel",
            key_hint(app, CommandId::MoveTrace),
            key_hint(app, CommandId::NextTrace),
            key_hint(app, CommandId::PrevTrace)
        ),
    };
    if let Some(m) = super::inputs_list::inputs(app, ui, ch) {
        app.dispatch(m);
    }
    let mut msg = None;
    let (mut under, mut row_over) = (None, None);
    let reveal = std::mem::take(&mut app.state.tree_reveal);
    let dragging = app.state.row_drag_held();
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
            // What the rows' dots say, drawn as the rows draw them so the key matches.
            for (shown, word) in [(true, "shown"), (false, "hidden")] {
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 12.0), egui::Sense::hover());
                let c = r.center();
                if shown {
                    ui.painter().circle_filled(c, 4.0, ch.dim);
                } else {
                    ui.painter()
                        .circle_stroke(c, 3.25, egui::Stroke::new(1.25, ch.dim));
                }
                ui.label(RichText::new(word).small().color(ch.dim));
            }
        });
        ui.add_space(4.0);
        let rows = st.tree_rows();
        let theme = ac2_scene::theme::Theme::by_name(st.theme);
        if rows.is_empty() {
            ui.label(RichText::new(st.empty_list_text()).color(ch.dim));
        }
        let drag = st.row_drag().map(|d| (d.item, d.onto));
        let pointer = ui.ctx().pointer_latest_pos();
        for row in &rows {
            let reveal = reveal
                && row.mark == Mark::Active
                && matches!(
                    row.key,
                    TreeKey::Meas(_) | TreeKey::Live(_) | TreeKey::Math(_)
                );
            let (m, rect) = tree_row(ui, row, &tips, ch, &theme, reveal, dragging);
            if let Some(m) = m {
                msg = Some(m);
            }
            // A row dragged over another: the drop combines them (the math dialog opens),
            // shaded as a pane a drop would take is.
            if let Some((item, onto)) = drag {
                if pointer.is_some_and(|p| rect.contains(p))
                    && row.key != item
                    && !row.key.is_heading()
                {
                    under = Some(row.key);
                }
                if onto == Some(row.key) {
                    let r = rect.expand(1.0);
                    let painter = ui.painter();
                    painter.rect_filled(r, 3.0, crate::theme::c32(theme.unresolved_shade));
                    let e = theme.unresolved_edge;
                    let stroke = egui::Stroke::new(e.width, crate::theme::c32(e.color));
                    let (on, off) = e.dash.map_or((4.0, 4.0), |d| (d.on, d.off));
                    let outline = [
                        r.left_top(),
                        r.right_top(),
                        r.right_bottom(),
                        r.left_bottom(),
                        r.left_top(),
                    ];
                    painter.extend(egui::Shape::dashed_line(&outline, stroke, on, off));
                }
            }
        }
        if let Some((_, onto)) = drag
            && onto != under
        {
            row_over = Some(Msg::RowDrag(DragMsg::OverRow(under)));
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
    if let Some(m) = row_over {
        app.dispatch(m);
    }
    if let Some(m) = msg {
        app.dispatch(m);
    }
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
