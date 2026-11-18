//! The walk setup window and the plan editor: their rows and notes as
//! `ac2_scene::walk::setup` words them, and a plan view the mouse edits. Nothing here computes
//! a value; the mouse's pixels become plan metres through the scene's own mapping. The walk
//! pane's control bar: the buttons `ac2_scene::walk::controls` words, sizes and dims, each
//! sending its action's command.

use std::sync::Arc;

use eframe::egui::{self, RichText};

use super::overlays::{backdrop, card};
use crate::app::App;
use crate::keys::CommandId;
use crate::plot::{PlotSlot, paint};
use crate::state::{Msg, Overlay, PaneId};
use crate::theme::Chrome;
use crate::walk_setup::PlanMsg;
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;
use ac2_scene::walk::controls::Action;
use ac2_scene::walk::setup::{EditorInput, Layer, Row, SetupInput, editor_view, setup_view};
use ac2_scene::walk::{DotLook, PlanDot, PlanPicture, plan_scene};

/// The plan view's renderer: no pane's (pane slots are their ids, counted from 0).
const PLAN_SLOT: PlotSlot = PlotSlot(u32::MAX - 1);

/// How near the pointer a point must be to be the one pressed, pixels.
const REACH_PX: f64 = 10.0;

const PLAN_SIZE: egui::Vec2 = egui::vec2(520.0, 400.0);
const ROWS_WIDTH: f32 = 360.0;

fn rows(ui: &mut egui::Ui, rows: &[Row], ch: &Chrome) {
    egui::Grid::new("walk-rows")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            for r in rows {
                ui.label(RichText::new(&r.label).color(ch.dim));
                let v = RichText::new(&r.value).size(14.0);
                ui.label(if r.focused {
                    v.color(ch.text).background_color(ch.focus)
                } else {
                    v.color(ch.text)
                });
                ui.end_row();
            }
        });
}

/// The plan view of `pic`: draws it and turns the mouse on it into plan messages.
fn plan_view(ui: &mut egui::Ui, app: &App, pic: &PlanPicture<'_>) -> Vec<PlanMsg> {
    let (rect, resp) = ui.allocate_exact_size(PLAN_SIZE, egui::Sense::click_and_drag());
    let theme = app.theme();
    let (scene, map) = plan_scene(pic, Viewport::new(PLAN_SIZE.x, PLAN_SIZE.y), &theme);
    paint(ui, PLAN_SLOT, rect, Arc::new(scene));
    let metres = |p: egui::Pos2| map.to_m([p.x - rect.min.x, p.y - rect.min.y]);
    let reach_m = REACH_PX / map.px_per_m();
    let mut out = Vec::new();
    let pointer = resp.interact_pointer_pos();
    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(p) = ui.input(|i| i.pointer.press_origin())
    {
        out.push(PlanMsg::Press {
            at: metres(p),
            reach_m,
        });
    }
    if resp.dragged_by(egui::PointerButton::Primary)
        && let Some(p) = pointer
    {
        out.push(PlanMsg::Drag { at: metres(p) });
    }
    if resp.drag_stopped() {
        out.push(PlanMsg::Release);
    }
    if resp.clicked()
        && let Some(p) = pointer
    {
        out.push(PlanMsg::Press {
            at: metres(p),
            reach_m,
        });
        out.push(PlanMsg::Release);
    }
    if resp.secondary_clicked()
        && let Some(p) = pointer
    {
        out.push(PlanMsg::Remove {
            at: metres(p),
            reach_m,
        });
    }
    out
}

fn window(ctx: &egui::Context, id: &str, ch: &Chrome, body: impl FnOnce(&mut egui::Ui)) {
    backdrop(ctx);
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 60.0))
        .show(ctx, |ui| card(ch).show(ui, body));
}

fn footer(ui: &mut egui::Ui, lines: &[String], error: Option<&str>, hint: &str, ch: &Chrome) {
    for n in lines {
        ui.label(RichText::new(n).size(13.0).color(ch.warn));
    }
    if let Some(e) = error {
        ui.label(RichText::new(e).size(13.0).color(ch.fault));
    }
    ui.add_space(4.0);
    ui.label(RichText::new(hint).small().color(ch.dim));
}

pub(super) fn setup(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    let Overlay::WalkSetup(s) = &app.state.overlay else {
        return;
    };
    let s = s.as_ref().clone();
    let st = &app.state;
    let plans = st.walk_plan_choices();
    let plan = s.plan_in(&plans).cloned();
    let labels = s.labels();
    let breaks = plan.as_ref().map(|p| s.breaks(p)).unwrap_or_default();
    let mics: Vec<String> = s
        .mics
        .iter()
        .filter_map(|&m| st.meas(m))
        .map(|m| match &m.config.kind {
            ac2_proto::model::MeasKind::Sweep { config } => {
                ac2_scene::walk::setup::sweep_mics(&m.config.name, config.measurement_inputs.len())
            }
            _ => m.config.name.clone(),
        })
        .collect();
    let mic_names: Vec<&str> = mics.iter().map(String::as_str).collect();
    let v = setup_view(&SetupInput {
        name: &s.name,
        plan: plan.as_ref(),
        zone: s.zone,
        preset: s.preset,
        reverb_s: s.reverb_s,
        n: s.count(),
        // A count typed answers what unset seats leave open.
        default_n: s.default_n.or(s.n),
        seed: s.seed,
        mics: &mic_names,
        add_mic: s.add_mic,
        focus: s.focus,
        typed: s.typed.as_deref(),
        relaxed: &s.relaxed,
        labels: &labels,
        breaks: &breaks,
        error: s.error.as_deref(),
    });
    let mut msgs = Vec::new();
    window(ctx, "ac2-walk-setup", ch, |ui| {
        ui.label(RichText::new(v.title).strong().size(16.0));
        ui.add_space(6.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(ROWS_WIDTH);
                rows(ui, &v.rows, ch);
            });
            match &plan {
                Some(plan) => {
                    let dots = labels
                        .iter()
                        .zip(&s.points)
                        .enumerate()
                        .map(|(i, (l, p))| PlanDot {
                            label: l.clone(),
                            at: [p[0], p[1]],
                            look: DotLook::Pending,
                            current: s.drag == Some(i),
                            next: false,
                            broken: breaks.get(i).is_some_and(|b| !b.is_empty()),
                            mic: 0,
                        })
                        .collect();
                    let pic = PlanPicture {
                        plan,
                        dots,
                        editing: None,
                        coverage: None,
                    };
                    msgs = plan_view(ui, app, &pic);
                }
                None => {
                    ui.allocate_ui(PLAN_SIZE, |ui| {
                        ui.label(
                            RichText::new("New plan…: Enter opens the plan editor").color(ch.dim),
                        );
                    });
                }
            }
        });
        ui.add_space(6.0);
        footer(ui, &v.notes, v.error.as_deref(), &v.hint, ch);
    });
    for m in msgs {
        app.dispatch(Msg::Plan(m));
    }
}

pub(super) fn editor(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    let Overlay::PlanEditor(e) = &app.state.overlay else {
        return;
    };
    let e = e.as_ref().clone();
    let v = editor_view(&EditorInput {
        plan: &e.plan,
        layer: e.layer,
        focus: e.focus,
        typed: e.typed.as_deref(),
        verdict: e.verdict(),
        row_error: e.row_error.as_deref(),
    });
    let focused_row = match e.focus {
        ac2_scene::walk::setup::EditorField::Row(k) => Some(k),
        _ => None,
    };
    let mut msgs = Vec::new();
    window(ctx, "ac2-plan-editor", ch, |ui| {
        ui.label(RichText::new(v.title).strong().size(16.0));
        ui.add_space(6.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(ROWS_WIDTH);
                egui::ScrollArea::vertical()
                    .max_height(PLAN_SIZE.y)
                    .show(ui, |ui| rows(ui, &v.rows, ch));
            });
            let points = e.layer_points();
            let dots = points
                .iter()
                .enumerate()
                .map(|(i, p)| PlanDot {
                    label: (i + 1).to_string(),
                    at: *p,
                    look: DotLook::Vertex,
                    current: focused_row == Some(i),
                    next: false,
                    broken: false,
                    mic: 0,
                })
                .collect();
            let pic = PlanPicture {
                plan: &e.plan,
                dots,
                editing: (e.layer != Layer::Markers).then_some(points),
                coverage: None,
            };
            msgs = plan_view(ui, app, &pic);
        });
        ui.add_space(6.0);
        let verdict = RichText::new(&v.verdict).size(13.0);
        ui.label(if v.refused {
            verdict.color(ch.warn)
        } else {
            verdict.color(ch.ok)
        });
        footer(ui, &[], v.row_error.as_deref(), &v.hint, ch);
    });
    for m in msgs {
        app.dispatch(Msg::Plan(m));
    }
}

/// The command a button of the walk pane's bar sends: the one the palette runs.
pub(crate) fn action_command(a: Action) -> CommandId {
    match a {
        Action::Prev => CommandId::WalkPrev,
        Action::Next => CommandId::WalkNext,
        Action::Capture => CommandId::WalkCapture,
        Action::Skip => CommandId::WalkSkip,
        Action::Redo => CommandId::WalkRedo,
        Action::Swap => CommandId::WalkSwap,
    }
}

/// The control bar of walk pane `id` along the bottom of `rect`; returns what is left of
/// `rect` above it (all of it when the pane shows no walk). The bar is a control, not plot
/// chrome: it stays whatever the pane's chrome hides, and takes its strip off the plot so no
/// button covers the plan.
pub(super) fn control_bar(
    app: &mut App,
    ui: &egui::Ui,
    id: PaneId,
    rect: egui::Rect,
    theme: &Theme,
    ch: &Chrome,
) -> egui::Rect {
    let Some(bar) = crate::scenes::walk_controls(&app.state, id, theme, rect.width()) else {
        return rect;
    };
    let above = egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.max.y - bar.height));
    // A pane too short for the bar and anything above it keeps its picture: the walk is
    // still read there, and the palette runs every action.
    if above.height() < bar.height {
        return rect;
    }
    // A window over the panes owns the mouse.
    let live = !app.state.window_over_panes();
    let mut pressed = None;
    for b in &bar.buttons {
        let r = egui::Rect::from_min_size(
            above.left_bottom() + egui::vec2(b.rect.x, b.rect.y),
            egui::vec2(b.rect.w, b.rect.h),
        );
        let enabled = b.blocked.is_none();
        let sense = if enabled && live {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let resp = ui.interact(r, ui.id().with(("walk-bar", id.0, b.action)), sense);
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, b.action.label())
        });
        let primary = b.action.primary();
        let fill = match (primary, enabled) {
            (true, true) => ch.focus,
            (true, false) => ch.focus.gamma_multiply(0.35),
            (false, _) => ch.panel,
        };
        let edge = if enabled && resp.hovered() {
            ch.text
        } else if enabled {
            ch.border
        } else {
            ch.border.gamma_multiply(0.5)
        };
        let painter = ui.painter().with_clip_rect(r);
        painter.rect_filled(r, 5.0, fill);
        painter.rect_stroke(
            r,
            5.0,
            egui::Stroke::new(if primary { 1.5 } else { 1.0 }, edge),
            egui::StrokeKind::Inside,
        );
        if enabled && resp.is_pointer_button_down_on() {
            painter.rect_filled(r, 5.0, ch.text.gamma_multiply(0.12));
        }
        let text = RichText::new(&b.label).size(b.font_size).color(if enabled {
            ch.text
        } else {
            ch.dim.gamma_multiply(0.6)
        });
        let text = if primary { text.strong() } else { text };
        let galley = egui::WidgetText::from(text).into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Button,
        );
        painter.galley(r.center() - galley.size() / 2.0, galley, ch.text);
        if resp.clicked() {
            pressed = Some(b.action);
        }
        if let Some(tip) = b.tip() {
            resp.on_hover_text(tip);
        }
    }
    if let Some(a) = pressed {
        // The commands act on the focused pane's walk: the pane pressed is that one.
        app.dispatch(Msg::FocusPane(id));
        app.dispatch(Msg::Command(action_command(a)));
    }
    above
}
