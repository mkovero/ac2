//! The session dialog (backend, device, the channel grids with meters and roles, the
//! reference, rate and buffer) and the input meter shared with the measurement dialogs.

use eframe::egui::{self, Color32, RichText};

use ac2_proto::model::Availability;
use ac2_scene::meter::{MeterReading, MeterState};

use crate::app::App;
use crate::session_dialog::{
    DetectPhase, Edit, InputRole, OutputDevice, RoleKey, Row, SessionDialog, backend_name,
    device_summary,
};
use crate::state::SessionMsg;
use crate::theme::Chrome;

const METER_W: f32 = 150.0;
/// The label column of the device and rate rows, so their values line up.
const LABEL_W: f32 = 104.0;
/// The channel name column, wide enough for a named mic beside its input's name.
const NAME_W: f32 = 230.0;
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
    let t = RichText::new(text).strong();
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
    // The marker in the monospace font, as on every other row: the bold proportional face
    // has no ▸ and drew an empty box.
    ui.label(
        RichText::new(if focused { "▸" } else { " " })
            .color(ch.focus)
            .monospace(),
    );
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    ui.allocate_ui_with_layout(egui::vec2(LABEL_W, 18.0), layout, |ui| {
        ui.set_min_width(LABEL_W);
        ui.add(
            egui::Label::new(
                RichText::new(text)
                    .color(if focused { ch.focus } else { ch.dim })
                    .strong(),
            )
            .sense(egui::Sense::click()),
        )
    })
    .inner
}

/// Each input's meter, and each mic row's curve and calibration text (with whether it needs
/// a look).
type ChannelData = (
    std::collections::BTreeMap<u16, MeterReading>,
    Vec<Option<(String, bool)>>,
);

/// The input meters and each mic row's curve and calibration text, for the channel rows.
fn channel_data(app: &App, d: &SessionDialog) -> ChannelData {
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
    let cal = (0..d.inputs.len())
        .map(|i| {
            app.state
                .daemon()
                .and_then(|st| d.row_cal_text(i, st, now.wall, offset))
        })
        .collect();
    (meters, cal)
}

/// What the session model says: a notice, why meters are missing, why Enter was refused.
pub(super) fn notices(ui: &mut egui::Ui, d: &SessionDialog, ch: &Chrome) {
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
}

/// The Audio & I/O page below the device, in the order the operator decides: the inputs
/// with meters, names and roles; the outputs with their labels and the stimulus; the
/// reference (loopback) with its detection; rate and buffer; then the one button that opens
/// the session. `focus`: the focused row, `None` while the max level row below has it.
pub(super) fn channels_section(
    app: &App,
    ui: &mut egui::Ui,
    d: &SessionDialog,
    focus: Option<Row>,
    ch: &Chrome,
    msg: &mut Option<SessionMsg>,
) {
    if d.backends.is_none() {
        // The device section says the listing is on its way.
        notices(ui, d, ch);
        return;
    }
    let (meters, cal) = channel_data(app, d);
    super::settings::section(ui, "Inputs", "", ch);
    input_grid(ui, d, focus, &meters, &cal, ch, msg);
    super::settings::section(ui, "Outputs", "stimulus: this app", ch);
    output_grid(ui, d, focus, ch, msg);
    super::settings::section(ui, "Reference (loopback)", "", ch);
    ui.label(RichText::new(d.reference_text()).color(ch.text));
    ui.label(
        RichText::new(crate::session_dialog::REFERENCE_HELP)
            .small()
            .color(ch.dim),
    );
    ui.add_space(2.0);
    if ui.button("Detect loopback…  D").clicked() {
        *msg = Some(SessionMsg::Detect);
    }
    detect_panel(ui, d, ch, msg);
    super::settings::section(ui, "Sample rate and buffer", "", ch);
    rate_rows(ui, d, focus, ch, msg);
    ui.add_space(8.0);
    notices(ui, d, ch);
}

/// The page's last row: what Enter does here, set apart from the settings above it.
pub(super) fn open_row(
    ui: &mut egui::Ui,
    d: &SessionDialog,
    new_session: bool,
    ch: &Chrome,
    msg: &mut Option<SessionMsg>,
) {
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let open = if new_session {
            "Open new session  Enter"
        } else if d.is_open_device() {
            "Reopen session  Enter"
        } else {
            "Open session  Enter"
        };
        let primary = egui::Button::new(RichText::new(open).strong().color(ch.panel))
            .fill(ch.focus)
            .min_size(egui::vec2(0.0, 24.0));
        if ui.add(primary).clicked() {
            *msg = Some(SessionMsg::Submit);
        }
        ui.label(
            RichText::new("Esc closes without opening")
                .small()
                .color(ch.dim),
        );
    });
}

/// The Audio & I/O page's device: backend, input and output device.
pub(super) fn device_section(
    ui: &mut egui::Ui,
    d: &SessionDialog,
    focus: Option<Row>,
    ch: &Chrome,
    msg: &mut Option<SessionMsg>,
) {
    backend_rows(ui, d, focus, ch, msg);
}

fn backend_rows(
    ui: &mut egui::Ui,
    d: &SessionDialog,
    focus: Option<Row>,
    ch: &Chrome,
    msg: &mut Option<SessionMsg>,
) {
    let Some(backends) = &d.backends else {
        ui.label(RichText::new("listing backends and devices…").color(ch.dim));
        return;
    };
    ui.horizontal(|ui| {
        if label_col(ui, "Backend", focus == Some(Row::Backend), ch).clicked() {
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
            ui.add_space(LABEL_W + 18.0);
            ui.label(text);
        });
    }
    let (inputs, outputs) = d.device_counts();
    device_row(
        ui,
        "Input device",
        Row::Device,
        d.device_info()
            .map(|dev| (dev.name.clone(), device_summary(dev))),
        d.input_position(),
        inputs,
        focus == Some(Row::Device),
        ch,
        msg,
    );
    let out = match (d.out_device, d.output_info()) {
        (_, None) => None,
        (OutputDevice::SameAsInput, Some(_)) => {
            Some(("same device as the input".to_owned(), String::new()))
        }
        (OutputDevice::Other(_), Some(dev)) => Some((dev.name.clone(), device_summary(dev))),
    };
    device_row(
        ui,
        "Output device",
        Row::OutputDevice,
        out,
        d.output_position(),
        outputs,
        focus == Some(Row::OutputDevice),
        ch,
        msg,
    );
    if let Some(note) = d.clock_note() {
        ui.label(RichText::new(note).small().color(ch.dim));
    }
    if let Some(clocks) = d.clock_sources_text() {
        ui.label(
            RichText::new(format!("Clock source: {clocks}"))
                .small()
                .color(ch.dim),
        );
    }
    if let Some((kind, _)) = d.panel_target() {
        let (button, hint) = crate::session_dialog::panel_label(kind);
        ui.horizontal(|ui| {
            if ui.button(button).clicked() {
                *msg = Some(SessionMsg::Panel);
            }
            ui.label(RichText::new(hint).small().color(ch.dim));
        });
    }
}

/// One device stepper: the name (a click steps on), ‹ › and its place among the choices.
#[allow(clippy::too_many_arguments)]
fn device_row(
    ui: &mut egui::Ui,
    label: &str,
    row: Row,
    shown: Option<(String, String)>,
    at: usize,
    n: usize,
    focused: bool,
    ch: &Chrome,
    msg: &mut Option<SessionMsg>,
) {
    ui.horizontal(|ui| {
        if label_col(ui, label, focused, ch).clicked() {
            *msg = Some(SessionMsg::Focus(row));
        }
        let Some((name, summary)) = shown else {
            ui.label(RichText::new("no device").color(ch.dim));
            return;
        };
        if n > 1 && ui.small_button("‹").clicked() {
            *msg = Some(SessionMsg::Cycle(row, -1));
        }
        let t = RichText::new(name).color(ch.text).strong();
        if ui.add(egui::Button::selectable(focused, t)).clicked() {
            *msg = Some(SessionMsg::Cycle(row, 1));
        }
        if n > 1 && ui.small_button("›").clicked() {
            *msg = Some(SessionMsg::Cycle(row, 1));
        }
        if !summary.is_empty() {
            ui.label(RichText::new(summary).color(ch.dim));
        }
        if n > 1 {
            ui.label(
                RichText::new(format!("({} of {n})", at + 1))
                    .small()
                    .color(ch.dim),
            );
        }
    });
}

/// A channel row's head: the focus marker, the in-session tick and the channel number;
/// whether the tick was clicked.
fn row_head(ui: &mut egui::Ui, focused: bool, on: bool, channel: u16, ch: &Chrome) -> bool {
    ui.horizontal(|ui| {
        let marker = if focused { "▸" } else { " " };
        ui.label(RichText::new(marker).color(ch.focus).monospace());
        let clicked = check(ui, on, ch).clicked();
        ui.label(
            RichText::new(format!("{:>2}", channel + 1))
                .monospace()
                .color(ch.dim),
        );
        clicked
    })
    .inner
}

fn input_grid(
    ui: &mut egui::Ui,
    d: &SessionDialog,
    focus: Option<Row>,
    meters: &std::collections::BTreeMap<u16, MeterReading>,
    cal: &[Option<(String, bool)>],
    ch: &Chrome,
    msg: &mut Option<SessionMsg>,
) {
    egui::Grid::new("ac2-session-inputs")
        .num_columns(5)
        .spacing(egui::vec2(10.0, 4.0))
        .min_col_width(0.0)
        .show(ui, |ui| {
            ui.label("");
            ui.label(RichText::new("name").small().color(ch.dim));
            ui.label(RichText::new("level (dBFS RMS)").small().color(ch.dim));
            ui.label(RichText::new("role").small().color(ch.dim));
            ui.label("");
            ui.end_row();
            for (i, r) in d.inputs.iter().enumerate() {
                let row = Row::Input(i);
                let focused = focus == Some(row);
                if row_head(ui, focused, r.in_session, r.channel, ch) {
                    *msg = Some(SessionMsg::Toggle(row));
                }
                let editing = d.edit == Some(Edit::Mic(i));
                let resp = ui
                    .horizontal(|ui| {
                        ui.set_min_width(NAME_W);
                        let dev = r
                            .device_name
                            .clone()
                            .unwrap_or_else(|| format!("Input {}", r.channel + 1));
                        let name = match r.role {
                            InputRole::Mic if editing => format!("{}▏", r.mic),
                            InputRole::Mic if !r.mic.is_empty() => format!("{} · {dev}", r.mic),
                            _ => dev,
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
                    super::settings::reveal_focused(&resp);
                }
                ui.horizontal(|ui| {
                    let m = meters
                        .get(&r.channel)
                        .cloned()
                        .unwrap_or_else(MeterReading::none);
                    meter(ui, &m, ch);
                });
                ui.horizontal(|ui| {
                    if chip(
                        ui,
                        r.role == InputRole::Reference,
                        "Reference",
                        ch.focus,
                        ch,
                    )
                    .on_hover_text("R: the loopback return of the stimulus")
                    .clicked()
                    {
                        *msg = Some(SessionMsg::Role(row, RoleKey::Reference));
                    }
                    if chip(ui, r.role == InputRole::Mic, "Mic", ch.ok, ch)
                        .on_hover_text("M: a measurement mic")
                        .clicked()
                    {
                        *msg = Some(SessionMsg::Role(row, RoleKey::Mic));
                    }
                });
                // A named mic says which curve and calibration it uses; an unnamed one, how
                // to name it (its name is what every measurement on it reads).
                match cal.get(i).and_then(Option::as_ref) {
                    Some((t, warn)) => {
                        ui.label(RichText::new(t).small().color(if *warn {
                            ch.armed
                        } else {
                            ch.dim
                        }));
                    }
                    None if r.role == InputRole::Mic && !editing => {
                        ui.label(RichText::new("N names the mic").small().color(ch.dim));
                    }
                    None => {
                        ui.label("");
                    }
                }
                ui.end_row();
            }
        });
}

fn output_grid(
    ui: &mut egui::Ui,
    d: &SessionDialog,
    focus: Option<Row>,
    ch: &Chrome,
    msg: &mut Option<SessionMsg>,
) {
    egui::Grid::new("ac2-session-outputs")
        .num_columns(3)
        .spacing(egui::vec2(10.0, 4.0))
        .min_col_width(0.0)
        .show(ui, |ui| {
            for (o, r) in d.outputs.iter().enumerate() {
                let row = Row::Output(o);
                let focused = focus == Some(row);
                let in_session = r.channel < d.out_count;
                if row_head(ui, focused, in_session, r.channel, ch) {
                    *msg = Some(SessionMsg::Toggle(row));
                }
                let name = if d.edit == Some(Edit::OutputLabel(o)) {
                    format!("{}▏", d.label_text)
                } else {
                    match (&r.rig_label, &r.device_name) {
                        (Some(l), Some(dev)) => format!("{l} · {dev}"),
                        _ => r.label(),
                    }
                };
                let resp = ui
                    .horizontal(|ui| {
                        ui.set_min_width(NAME_W);
                        let t =
                            RichText::new(name).color(if in_session { ch.text } else { ch.dim });
                        ui.add(egui::Button::selectable(focused, t))
                    })
                    .inner;
                if resp.clicked() {
                    *msg = Some(SessionMsg::Focus(row));
                }
                if resp.double_clicked() {
                    *msg = Some(SessionMsg::EditMic(row));
                }
                if focused {
                    super::settings::reveal_focused(&resp);
                }
                if chip(ui, r.stimulus, "Stimulus", ch.armed, ch)
                    .on_hover_text("S: plays the stimulus, to the speakers and the loopback")
                    .clicked()
                {
                    *msg = Some(SessionMsg::Role(row, RoleKey::Stimulus));
                }
                ui.end_row();
            }
        });
}

fn rate_rows(
    ui: &mut egui::Ui,
    d: &SessionDialog,
    focus: Option<Row>,
    ch: &Chrome,
    msg: &mut Option<SessionMsg>,
) {
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
        .map_or_else(String::new, |b| format!(" ({b} frames)"));
    for (row, label, text, default, unit) in [
        (Row::Rate, "Sample rate", &d.rate, default_rate, "Hz"),
        (Row::Buffer, "Buffer", &d.buffer, default_buffer, "frames"),
    ] {
        ui.horizontal(|ui| {
            let focused = focus == Some(row);
            if label_col(ui, label, focused, ch).clicked() {
                *msg = Some(SessionMsg::Focus(row));
            }
            let shown = if focused {
                format!("{text}▏ {unit}")
            } else if text.is_empty() {
                format!("device default{default}")
            } else {
                format!("{text} {unit}")
            };
            let t = RichText::new(shown).monospace().color(ch.text);
            let resp = ui.add(egui::Button::selectable(focused, t));
            if resp.clicked() {
                *msg = Some(SessionMsg::Focus(row));
            }
            if focused {
                super::settings::reveal_focused(&resp);
                ui.label(
                    RichText::new("type a value; empty is the device default")
                        .small()
                        .color(ch.dim),
                );
            }
        });
    }
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
                    if ui
                        .button("Play burst")
                        .on_hover_text(super::keys_cue(&[egui::Key::Enter]))
                        .clicked()
                    {
                        *msg = Some(SessionMsg::DetectConfirm);
                    }
                    if ui
                        .button("Cancel")
                        .on_hover_text(super::keys_cue(&[egui::Key::ArrowUp]))
                        .clicked()
                    {
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
