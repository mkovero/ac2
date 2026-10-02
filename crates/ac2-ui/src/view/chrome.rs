//! Top bar (link, session, stimulus) and the measurement list.

use ac2_proto::model::MeasKind;
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

pub(super) fn top_bar(app: &mut App, ui: &mut egui::Ui, ch: &Chrome) {
    let st = &app.state;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new("ac2").strong().size(16.0));
        ui.separator();
        let now = std::time::Instant::now();
        let (color, text) = match &st.conn {
            ConnState::Connecting { target } => (ch.warn, format!("connecting to {target}…")),
            ConnState::Failed { target, error } => {
                (ch.fault, format!("{target}: {error} · retrying"))
            }
            ConnState::Connected { target, server, .. } => {
                let responding = st.mirror.as_ref().is_some_and(|m| m.responding(now));
                let synced = st.mirror.as_ref().is_some_and(|m| m.synced());
                if !responding {
                    (ch.fault, format!("{server} · {target} · not responding"))
                } else if !synced {
                    (ch.warn, format!("{server} · {target} · syncing"))
                } else {
                    (ch.ok, format!("{server} · {target}"))
                }
            }
        };
        dot(ui, color);
        ui.label(text);
        ui.separator();
        let session = match st.daemon().and_then(|s| s.session.open.as_ref()) {
            Some(o) => format!(
                "{} · {} kHz · {} frames",
                o.input_device.0,
                format::fixed(f64::from(o.sample_rate_hz) / 1000.0, 1),
                o.buffer_frames
            ),
            None if st.daemon().is_some() => "no audio session".into(),
            None => "—".into(),
        };
        ui.label(RichText::new(session).color(ch.dim));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!(
                    "{} keys · {} commands",
                    key_hint(app, CommandId::Help),
                    key_hint(app, CommandId::Palette)
                ))
                .color(ch.dim),
            );
            ui.separator();
            stimulus(app, ui, ch);
        });
    });
}

fn stimulus(app: &App, ui: &mut egui::Ui, ch: &Chrome) {
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
    let hint = match st.stimulus.phase {
        StimPhase::Idle if st.stimulus.level.is_none() => "L types a level".to_string(),
        StimPhase::Idle => "Space arms".to_string(),
        StimPhase::Armed => "Enter fires · Esc stops".to_string(),
        _ => "Esc stops".to_string(),
    };
    ui.label(RichText::new(hint).color(ch.dim));
    if let Some(o) = other {
        ui.label(RichText::new(format!("held by {o}")).color(ch.warn));
    }
    ui.label(format!(
        "{level} → out {}",
        outputs_text(&st.stimulus.outputs)
    ));
    let badge_text = RichText::new(badge)
        .strong()
        .color(if badge == "STIM OFF" {
            ch.dim
        } else {
            Color32::BLACK
        })
        .background_color(if badge == "STIM OFF" {
            Color32::TRANSPARENT
        } else {
            color
        });
    ui.label(badge_text);
}

pub(super) fn sidebar(app: &mut App, ui: &mut egui::Ui, ch: &Chrome) {
    let mut clicked = None;
    {
        let st = &app.state;
        ui.label(RichText::new("Measurements").strong());
        ui.add_space(4.0);
        let ms = st.measurements();
        if ms.is_empty() {
            ui.label(RichText::new("none").color(ch.dim));
        }
        for m in ms {
            let selected = st.selected == Some(m.id);
            let kind = match m.config.kind {
                MeasKind::Transfer { .. } => "TF",
                MeasKind::Spectrum { .. } => "FFT",
                MeasKind::Rta { .. } => "RTA",
                MeasKind::Spl { .. } => "SPL",
            };
            let state = match (m.running, m.frozen) {
                (_, true) => "frozen",
                (true, false) => "running",
                (false, false) => "stopped",
            };
            let mut text = format!("{kind}  {}\n     {state}", m.config.name);
            if let Some(d) = &m.delay {
                // Distance stays in the transfer legend's reference line.
                text.push_str(&format!(" · {}", format::ms(d.applied.0, 2)));
                if d.tracking {
                    text.push_str(" · tracking");
                }
            }
            let e = st.edit(m.id);
            if e.inverted {
                text.push_str(" · inv");
            }
            if e.offset_db != 0.0 {
                text.push_str(&format!(" · {}", format::db_readout(e.offset_db)));
            }
            let r = ui
                .add(egui::Button::selectable(selected, text).wrap_mode(egui::TextWrapMode::Wrap));
            if r.clicked() {
                clicked = Some(m.id);
            }
        }
        ui.add_space(12.0);
        ui.label(RichText::new("Slots").strong());
        ui.add_space(4.0);
        let mut any = false;
        for (i, s) in st.slots().iter().enumerate() {
            let Some(t) = s else { continue };
            any = true;
            let data = if st.traces.contains_key(&t.id) {
                ""
            } else {
                " (no data)"
            };
            let lock = if t.edit.locked { " 🔒" } else { "" };
            let text = format!("{}  {}{data}{lock}", i + 1, t.edit.name);
            let c = t.edit.color;
            let color = if t.edit.visible {
                egui::Color32::from_rgb(c.r, c.g, c.b)
            } else {
                ch.dim
            };
            ui.label(RichText::new(text).color(color));
        }
        if !any {
            ui.label(
                RichText::new(format!(
                    "{} captures the selected TF",
                    key_hint(app, CommandId::Slot1)
                ))
                .color(ch.dim),
            );
        }
    }
    if let Some(id) = clicked {
        app.dispatch(Msg::SelectMeas(id));
    }
}
