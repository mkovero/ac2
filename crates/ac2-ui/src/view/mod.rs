//! Drawing: top bar, measurement list, panes, overlays. Reads the state, emits messages
//! (clicks, wheel, drags) through [`App::dispatch`]; never computes a measurement value.

mod autosave;
mod cal;
mod chrome;
mod leq;
mod overlays;
mod panes;
mod recording;
mod session;
mod settings;

use ac2_scene::theme::Theme;
use eframe::egui;

use crate::app::App;
use crate::scenes::Now;

pub(crate) fn now() -> Now {
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos().min(u128::from(u64::MAX)) as u64);
    Now {
        instant: std::time::Instant::now(),
        wall: ac2_proto::units::WallNs(wall),
    }
}

pub(crate) fn draw(app: &mut App, ui: &mut egui::Ui, theme: &Theme) {
    let ch = crate::theme::chrome(theme);
    if app.state.stage_view() {
        egui::CentralPanel::no_frame()
            .frame(egui::Frame::new().fill(ch.panel))
            .show(ui, |ui| panes::panes(app, ui, theme, &ch));
        let top = ui.ctx().content_rect().min.y;
        overlays::draw(app, ui.ctx(), &ch, top);
        return;
    }
    let mut top = egui::Panel::top("ac2-top")
        .frame(
            egui::Frame::new()
                .fill(ch.panel)
                .inner_margin(egui::Margin::symmetric(10, 6)),
        )
        .show(ui, |ui| chrome::top_bar(app, ui, &ch))
        .response
        .rect
        .bottom();
    // Outside the panes: visible whichever pane is maximised.
    if let Some(p) = app.state.operation() {
        top = egui::Panel::top("ac2-progress")
            .frame(
                egui::Frame::new()
                    .fill(ch.panel)
                    .stroke(egui::Stroke::new(1.0, ch.armed))
                    .inner_margin(egui::Margin::symmetric(10, 6)),
            )
            .show(ui, |ui| chrome::progress(app, ui, &ch, &p))
            .response
            .rect
            .bottom();
    }
    egui::Panel::left("ac2-measurements")
        .resizable(false)
        .exact_size(230.0)
        .frame(
            egui::Frame::new()
                .fill(ch.panel)
                .inner_margin(egui::Margin::symmetric(10, 8)),
        )
        .show(ui, |ui| chrome::sidebar(app, ui, &ch));
    egui::CentralPanel::no_frame()
        .frame(
            egui::Frame::new()
                .fill(ch.panel)
                .inner_margin(egui::Margin::same(6)),
        )
        .show(ui, |ui| panes::panes(app, ui, theme, &ch));
    overlays::draw(app, ui.ctx(), &ch, top);
}
