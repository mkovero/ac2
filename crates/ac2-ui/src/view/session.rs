//! The session dialog (backend, device, the channel grid with meters and roles), the
//! measurement offer after it, and the input meter shared with the measurement dialogs.

use eframe::egui::{self, Color32, RichText};

use ac2_proto::model::Availability;
use ac2_scene::meter::{MeterReading, MeterState};

use crate::app::App;
use crate::session_dialog::{
    DetectPhase, Edit, InputRole, RoleKey, Row, SessionDialog, backend_name, device_summary,
};
use crate::state::{Msg, Overlay, SessionMsg};
use crate::theme::Chrome;

use super::overlays::{backdrop, card};

const METER_W: f32 = 150.0;
const METER_H: f32 = 10.0;

/// One input meter: the RMS as a bar, the peak as a tick, coloured by state.
pub(super) fn meter(ui: &mut egui::Ui, m: &MeterReading, ch: &Chrome) {
    meter_sized(ui, m, ch, METER_W);
}

/// [`meter`] with a bar `width` wide (the readout beside it keeps its width).
pub(super) fn meter_sized(ui: &mut egui::Ui, m: &MeterReading, ch: &Chrome, width: f32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(width, METER_H), egui::Sense::hover());
    let (t, _) = ui.allocate_exact_size(egui::vec2(64.0, METER_H), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, 2.0, ch.panel);
    p.rect_stroke(
        r,
        2.0,
        egui::Stroke::new(1.0, ch.border),
        egui::StrokeKind::Inside,
    );
    let color = match m.state {
        MeterState::NoData | MeterState::Silent => ch.dim,
        MeterState::Signal => ch.ok,
        MeterState::Hot => ch.armed,
        MeterState::Clip => ch.fault,
    };
    if m.rms_fill > 0.0 {
        let mut bar = r.shrink(1.0);
        bar.set_width(bar.width() * m.rms_fill);
        p.rect_filled(bar, 1.0, color);
    }
    if m.peak_fill > 0.0 {
        let x = r.left() + 1.0 + (r.width() - 2.0) * m.peak_fill;
        p.line_segment(
            [
                egui::pos2(x, r.top() + 1.0),
                egui::pos2(x, r.bottom() - 1.0),
            ],
            egui::Stroke::new(
                2.0,
                if m.state == MeterState::Clip {
                    ch.fault
                } else {
                    ch.text
                },
            ),
        );
    }
    let text = match m.state {
        MeterState::Clip => format!("{} CLIP", m.text),
        _ => m.text.clone(),
    };
    p.text(
        t.right_center(),
        egui::Align2::RIGHT_CENTER,
        text,
        egui::TextStyle::Monospace.resolve(ui.style()),
        if m.state == MeterState::Clip {
            ch.fault
        } else {
            ch.text
        },
    );
}

fn chip(ui: &mut egui::Ui, on: bool, text: &str, color: Color32, ch: &Chrome) -> egui::Response {
    let t = RichText::new(text).monospace().strong();
    let t = if on {
        t.color(ch.panel)
    } else {
        t.color(ch.dim)
    };
    let b = egui::Button::new(t)
        .fill(if on { color } else { Color32::TRANSPARENT })
        .stroke(egui::Stroke::new(1.0, if on { color } else { ch.border }))
        .min_size(egui::vec2(22.0, 18.0));
    ui.add(b)
}

fn check(ui: &mut egui::Ui, on: bool, ch: &Chrome) -> egui::Response {
    let t = RichText::new(if on { "☑" } else { "☐" }).color(if on { ch.text } else { ch.dim });
    ui.add(egui::Button::new(t).frame(false))
}

fn label_col(ui: &mut egui::Ui, text: &str, focused: bool, ch: &Chrome) -> egui::Response {
    let marker = if focused { "▸ " } else { "  " };
    ui.add(
        egui::Label::new(
            RichText::new(format!("{marker}{text}"))
                .color(if focused { ch.focus } else { ch.dim })
                .strong(),
        )
        .sense(egui::Sense::click()),
    )
}

/// The session dialog.
pub(super) fn session(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    let Overlay::Session(d) = &app.state.overlay else {
        return;
    };
    let d: SessionDialog = (**d).clone();
    let meters = app.state.input_meters();
    let now = super::now();
    let offset = ac2_scene::time::ClockOffset(
        app.state
            .mirror
            .as_ref()
            .and_then(|m| m.clock_offset_ns)
            .map_or(0, |o| {
                o.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
            }),
    );
    let cal: Vec<Option<(String, bool)>> = (0..d.inputs.len())
        .map(|i| {
            app.state
                .daemon()
                .and_then(|st| d.row_cal_text(i, st, now.wall, offset))
        })
        .collect();
    backdrop(ctx);
    let mut msg = None;
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("ac2-session"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 40.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(780.0);
                ui.label(RichText::new("Open audio session").strong().size(16.0));
                ui.add_space(6.0);
                backend_rows(ui, &d, ch, &mut msg);
                ui.add_space(4.0);
                ui.separator();
                let grid_h = (screen.height() - 430.0).max(160.0);
                egui::ScrollArea::vertical()
                    .max_height(grid_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| channel_grid(ui, &d, &meters, &cal, ch, &mut msg));
                ui.separator();
                rate_rows(ui, &d, ch, &mut msg);
                detect_panel(ui, &d, ch, &mut msg);
                for (text, color) in [
                    (d.notice.as_deref(), ch.armed),
                    (d.preview_error.as_deref(), ch.dim),
                    (d.error.as_deref(), ch.fault),
                ] {
                    if let Some(t) = text {
                        ui.add_space(2.0);
                        ui.label(RichText::new(t).color(color));
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Open").strong()).clicked() {
                        msg = Some(SessionMsg::Submit);
                    }
                    if ui.button("Detect loopback…").clicked() {
                        msg = Some(SessionMsg::Detect);
                    }
                    if ui.button("Cancel").clicked() {
                        msg = Some(SessionMsg::Cancel);
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "↑↓ move · ←→ backend / device, a mic's curve (off / 0° / 90° …) · \
                         Space in session · R reference · M mic · S stimulus · N names the \
                         mic · D detects the loopback · Enter opens · Esc closes (and stops \
                         the stimulus)",
                    )
                    .small()
                    .color(ch.dim),
                );
            });
        });
    if let Some(m) = msg {
        app.dispatch(Msg::Session(m));
    }
}

fn backend_rows(ui: &mut egui::Ui, d: &SessionDialog, ch: &Chrome, msg: &mut Option<SessionMsg>) {
    let Some(backends) = &d.backends else {
        ui.label(RichText::new("listing backends and devices…").color(ch.dim));
        return;
    };
    ui.horizontal(|ui| {
        if label_col(ui, "Backend ", d.focus == Row::Backend, ch).clicked() {
            *msg = Some(SessionMsg::Focus(Row::Backend));
        }
        for (i, b) in backends.iter().enumerate() {
            let available = b.availability == Availability::Available;
            let name = if available {
                backend_name(b.kind).to_owned()
            } else {
                format!("{} (unavailable)", backend_name(b.kind))
            };
            let t = RichText::new(name).color(if available { ch.text } else { ch.dim });
            if ui
                .add(egui::Button::selectable(i == d.backend, t))
                .clicked()
            {
                *msg = Some(SessionMsg::Cycle(Row::Backend, i as i32 - d.backend as i32));
            }
        }
    });
    if let Some(b) = d.backend_info() {
        let text = match &b.availability {
            Availability::Available => RichText::new(&b.description).small().color(ch.dim),
            Availability::Unavailable { reason } => RichText::new(format!("Unavailable: {reason}"))
                .small()
                .color(ch.fault),
        };
        ui.horizontal(|ui| {
            ui.add_space(72.0);
            ui.label(text);
        });
    }
    ui.horizontal(|ui| {
        if label_col(ui, "Device  ", d.focus == Row::Device, ch).clicked() {
            *msg = Some(SessionMsg::Focus(Row::Device));
        }
        let n = d.backend_info().map_or(0, |b| b.devices.len());
        match d.device_info() {
            None => {
                ui.label(RichText::new("no device").color(ch.dim));
            }
            Some(dev) => {
                if n > 1 && ui.small_button("‹").clicked() {
                    *msg = Some(SessionMsg::Cycle(Row::Device, -1));
                }
                let t = RichText::new(&dev.name).color(ch.text).strong();
                if ui
                    .add(egui::Button::selectable(d.focus == Row::Device, t))
                    .clicked()
                {
                    *msg = Some(SessionMsg::Cycle(Row::Device, 1));
                }
                if n > 1 && ui.small_button("›").clicked() {
                    *msg = Some(SessionMsg::Cycle(Row::Device, 1));
                }
                ui.label(RichText::new(device_summary(dev)).color(ch.dim));
                if n > 1 {
                    ui.label(
                        RichText::new(format!("({} of {n})", d.device + 1))
                            .small()
                            .color(ch.dim),
                    );
                }
            }
        }
    });
}

fn channel_grid(
    ui: &mut egui::Ui,
    d: &SessionDialog,
    meters: &std::collections::BTreeMap<u16, MeterReading>,
    cal: &[Option<(String, bool)>],
    ch: &Chrome,
    msg: &mut Option<SessionMsg>,
) {
    egui::Grid::new("ac2-session-channels")
        .num_columns(5)
        .spacing(egui::vec2(10.0, 4.0))
        .min_col_width(0.0)
        .show(ui, |ui| {
            ui.label(RichText::new("Inputs").strong().color(ch.text));
            ui.label("");
            ui.label(RichText::new("level (dBFS RMS)").small().color(ch.dim));
            ui.label(RichText::new("role").small().color(ch.dim));
            ui.label("");
            ui.end_row();
            for (i, r) in d.inputs.iter().enumerate() {
                let row = Row::Input(i);
                let focused = d.focus == row;
                ui.horizontal(|ui| {
                    let marker = if focused { "▸" } else { " " };
                    ui.label(RichText::new(marker).color(ch.focus).monospace());
                    if check(ui, r.in_session, ch).clicked() {
                        *msg = Some(SessionMsg::Toggle(row));
                    }
                    ui.label(
                        RichText::new(format!("{:>2}", r.channel + 1))
                            .monospace()
                            .color(ch.dim),
                    );
                });
                let editing = d.edit == Some(Edit::Mic(i));
                let resp = ui
                    .horizontal(|ui| {
                        ui.set_min_width(230.0);
                        let dev = r
                            .device_name
                            .clone()
                            .unwrap_or_else(|| format!("Input {}", r.channel + 1));
                        let name = if r.role == InputRole::Mic {
                            if editing {
                                format!("{}▏", r.mic)
                            } else if r.mic.is_empty() {
                                format!("{dev} · mic (N names it)")
                            } else {
                                format!("{} · {dev}", r.mic)
                            }
                        } else {
                            dev
                        };
                        let color = if r.in_session { ch.text } else { ch.dim };
                        let t = RichText::new(name).color(color);
                        let resp = ui.add(egui::Button::selectable(focused, t));
                        if resp.clicked() {
                            *msg = Some(SessionMsg::Focus(row));
                        }
                        if resp.double_clicked() {
                            *msg = Some(SessionMsg::EditMic(row));
                        }
                        resp
                    })
                    .inner;
                if focused {
                    resp.scroll_to_me(None);
                }
                ui.horizontal(|ui| {
                    let m = meters
                        .get(&r.channel)
                        .cloned()
                        .unwrap_or_else(MeterReading::none);
                    meter(ui, &m, ch);
                });
                ui.horizontal(|ui| {
                    if chip(ui, r.role == InputRole::Reference, "R", ch.focus, ch)
                        .on_hover_text("Reference: the loopback return of the stimulus")
                        .clicked()
                    {
                        *msg = Some(SessionMsg::Role(row, RoleKey::Reference));
                    }
                    if chip(ui, r.role == InputRole::Mic, "M", ch.ok, ch)
                        .on_hover_text("Measurement mic")
                        .clicked()
                    {
                        *msg = Some(SessionMsg::Role(row, RoleKey::Mic));
                    }
                });
                let role = match r.role {
                    InputRole::Reference => "Reference (loopback)",
                    InputRole::Mic => "Measurement mic",
                    InputRole::None if r.in_session => "in session",
                    InputRole::None => "",
                };
                // A named mic says which curve and calibration it uses instead.
                match cal.get(i).and_then(Option::as_ref) {
                    Some((t, warn)) => {
                        ui.label(RichText::new(t).small().color(if *warn {
                            ch.armed
                        } else {
                            ch.dim
                        }));
                    }
                    None => {
                        ui.label(RichText::new(role).small().color(ch.dim));
                    }
                }
                ui.end_row();
            }
            ui.label(RichText::new("Outputs").strong().color(ch.text));
            ui.end_row();
            for (o, r) in d.outputs.iter().enumerate() {
                let row = Row::Output(o);
                let focused = d.focus == row;
                let in_session = r.channel < d.out_count;
                ui.horizontal(|ui| {
                    let marker = if focused { "▸" } else { " " };
                    ui.label(RichText::new(marker).color(ch.focus).monospace());
                    if check(ui, in_session, ch).clicked() {
                        *msg = Some(SessionMsg::Toggle(row));
                    }
                    ui.label(
                        RichText::new(format!("{:>2}", r.channel + 1))
                            .monospace()
                            .color(ch.dim),
                    );
                });
                let t = RichText::new(r.label()).color(if in_session { ch.text } else { ch.dim });
                let resp = ui.add(egui::Button::selectable(focused, t));
                if resp.clicked() {
                    *msg = Some(SessionMsg::Focus(row));
                }
                if focused {
                    resp.scroll_to_me(None);
                }
                ui.label("");
                if chip(ui, r.stimulus, "S", ch.armed, ch)
                    .on_hover_text("Stimulus: feeds the speakers and the loopback")
                    .clicked()
                {
                    *msg = Some(SessionMsg::Role(row, RoleKey::Stimulus));
                }
                ui.label(
                    RichText::new(if r.stimulus { "Stimulus" } else { "" })
                        .small()
                        .color(ch.dim),
                );
                ui.end_row();
            }
        });
}

fn rate_rows(ui: &mut egui::Ui, d: &SessionDialog, ch: &Chrome, msg: &mut Option<SessionMsg>) {
    let dir = d
        .device_info()
        .and_then(|x| x.input.as_ref().or(x.output.as_ref()));
    let default_rate = dir
        .and_then(|x| x.default_rate_hz)
        .map_or_else(String::new, |r| {
            format!(" ({})", ac2_scene::format::freq_readout(f64::from(r)))
        });
    let default_buffer = dir
        .and_then(|x| x.default_buffer_frames)
        .map_or_else(String::new, |b| format!(" ({b})"));
    ui.horizontal(|ui| {
        for (row, label, text, default) in [
            (Row::Rate, "Sample rate", &d.rate, default_rate),
            (Row::Buffer, "Buffer (frames)", &d.buffer, default_buffer),
        ] {
            let focused = d.focus == row;
            if label_col(ui, label, focused, ch).clicked() {
                *msg = Some(SessionMsg::Focus(row));
            }
            let shown = if focused {
                format!("{text}▏")
            } else if text.is_empty() {
                format!("device default{default}")
            } else {
                text.clone()
            };
            let t = RichText::new(shown).monospace().color(ch.text);
            if ui.add(egui::Button::selectable(focused, t)).clicked() {
                *msg = Some(SessionMsg::Focus(row));
            }
            ui.add_space(16.0);
        }
    });
}

fn detect_panel(ui: &mut egui::Ui, d: &SessionDialog, ch: &Chrome, msg: &mut Option<SessionMsg>) {
    let Some(p) = &d.detect else {
        return;
    };
    ui.add_space(4.0);
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, ch.armed))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            if let Some(t) = d.detect_text() {
                ui.label(RichText::new(t).color(ch.text));
            }
            if p.phase == DetectPhase::Confirm {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Burst level (dBFS)").color(ch.armed).strong());
                    let shown = if d.edit == Some(Edit::DetectLevel) {
                        format!("{}▏", p.level)
                    } else {
                        p.level.clone()
                    };
                    ui.label(RichText::new(shown).monospace().size(15.0).color(ch.text));
                    if ui.button("Play burst").clicked() {
                        *msg = Some(SessionMsg::DetectConfirm);
                    }
                    if ui.button("Cancel").clicked() {
                        *msg = Some(SessionMsg::DetectCancel);
                    }
                });
                ui.label(
                    RichText::new("This emits sound on that output. Enter plays · ↑ cancels")
                        .small()
                        .color(ch.armed),
                );
            }
            if let Some(e) = &p.error {
                ui.label(RichText::new(e).color(ch.fault));
            }
        });
}

/// The offer after a session opened with a reference and mics.
pub(super) fn offer(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    let Overlay::Offer(o) = &app.state.overlay else {
        return;
    };
    let names: Vec<String> = o.transfers.iter().map(|t| t.name.clone()).collect();
    let mut msg = None;
    egui::Area::new(egui::Id::new("ac2-offer"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(460.0);
                let n = names.len();
                ui.label(
                    RichText::new(if n == 1 {
                        "Create a transfer measurement?".to_owned()
                    } else {
                        format!("Create {n} transfer measurements?")
                    })
                    .strong(),
                );
                ui.add_space(4.0);
                for name in &names {
                    ui.label(RichText::new(format!("• {name}")).color(ch.text));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(RichText::new("Create and start").strong())
                        .clicked()
                    {
                        msg = Some(true);
                    }
                    if ui.button("Not now").clicked() {
                        msg = Some(false);
                    }
                });
                ui.label(
                    RichText::new("Enter creates · N or Esc skips")
                        .small()
                        .color(ch.dim),
                );
            });
        });
    if let Some(m) = msg {
        app.dispatch(Msg::Offer(m));
    }
}
