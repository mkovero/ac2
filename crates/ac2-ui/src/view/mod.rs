//! Drawing: top bar, measurement list, panes, overlays. Reads the state, emits messages
//! (clicks, wheel, drags) through [`App::dispatch`]; never computes a measurement value.

mod autosave;
mod band_transfer;
mod cal;
mod chrome;
mod eq;
mod help;
mod inputs_list;
mod leq;
mod overlays;
mod panes;
mod recording;
mod rig;
mod segment;
mod session;
mod sessions;
mod settings;

use ac2_scene::theme::Theme;
use eframe::egui;

use crate::app::App;
use crate::scenes::Now;

/// A focusable cell of a dialog: whether it has the focus, where it is (what a click on it
/// focuses), where its message goes, and the colours it is drawn in.
struct CellCtx<'a, A, M> {
    focused: bool,
    at: A,
    msg: &'a mut Option<M>,
    ch: &'a crate::theme::Chrome,
}

/// A button's key cue (its hover text): the window's fixed keys that do what it does, as
/// this keyboard labels them (`Enter`, `↩` on macOS). A command's key comes from the keymap
/// (`Keymap::key_hint`) instead, so the cue follows a rebinding.
pub(crate) fn keys_cue(keys: &[egui::Key]) -> String {
    keys.iter()
        .map(|k| crate::keys::Chord::key(*k).label())
        .collect::<Vec<_>>()
        .join(" or ")
}

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
        panes::row_drag(app, ui.ctx(), theme, &ch);
        let top = ui.ctx().content_rect().min.y;
        overlays::draw(app, ui.ctx(), &ch, top);
        return;
    }
    let top = egui::Panel::top("ac2-top")
        .frame(
            egui::Frame::new()
                .fill(ch.panel)
                .inner_margin(egui::Margin::symmetric(10, 6)),
        )
        .show(ui, |ui| chrome::top_bar(app, ui, &ch))
        .response
        .rect
        .bottom();
    egui::Panel::left("ac2-measurements")
        .resizable(false)
        .exact_size(230.0)
        .frame(
            egui::Frame::new()
                .fill(ch.panel)
                .inner_margin(egui::Margin::symmetric(10, 8)),
        )
        .show(ui, |ui| chrome::sidebar(app, ui, &ch));
    let panes = egui::CentralPanel::no_frame()
        .frame(
            egui::Frame::new()
                .fill(ch.panel)
                .inner_margin(egui::Margin::same(6)),
        )
        .show(ui, |ui| panes::panes(app, ui, theme, &ch))
        .response
        .rect;
    // Over the panes, never beside them: a sweep starting or ending must not resize or
    // move what the operator is reading. It sits at the bottom of the pane area, over the
    // focused pane's key hints and the lowest axis labels, clear of the banners, legends
    // and traces at the top and middle; whichever pane is maximised, it is there.
    if let Some(p) = app.state.operation() {
        chrome::progress_overlay(app, ui.ctx(), &ch, &p, panes);
    }
    panes::row_drag(app, ui.ctx(), theme, &ch);
    overlays::draw(app, ui.ctx(), &ch, top);
}
