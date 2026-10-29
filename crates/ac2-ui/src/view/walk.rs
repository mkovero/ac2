//! The walk setup window and the plan editor: their rows and notes as
//! `ac2_scene::walk::setup` words them, and a plan view the mouse edits. Nothing here computes
//! a value; the mouse's pixels become plan metres through the scene's own mapping.

use std::sync::Arc;

use eframe::egui::{self, RichText};

use super::overlays::{backdrop, card};
use crate::app::App;
use crate::plot::{PlotSlot, paint};
use crate::state::{Msg, Overlay};
use crate::theme::Chrome;
use crate::walk_setup::PlanMsg;
use ac2_scene::primitives::Viewport;
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
    let (scene, map) = plan_scene(
        pic,
        Viewport {
            width: PLAN_SIZE.x,
            height: PLAN_SIZE.y,
        },
        &theme,
    );
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
        .map(|m| m.config.name.clone())
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
