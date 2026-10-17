//! Panes and keys: hints, zoom, delete, export, subscriptions, spectrograph, record and replay.

mod edit;
mod focus;
mod own;
mod streams;

use super::harness::{Driver, R};
use ac2_proto::model::MeasKind;
use ac2_proto::units::MeasId;
use ac2_scene::theme::Theme;
use ac2_scene::view::SpectrumMode;
use ac2_ui::forms::FormKind;
use ac2_ui::keys::Keymap;
use ac2_ui::state::{AppState, Msg, Overlay};
use std::time::Instant;

/// The hint line of the focused pane, as the app draws it (PC labels).
pub(super) fn hint_line(s: &AppState) -> Vec<String> {
    let focus = s.layout.focus;
    let keys = Keymap::default();
    let style = ac2_ui::keys::LabelStyle::Pc;
    // Only the focused pane has one.
    for p in s.layout.panes().into_iter().filter(|p| *p != focus) {
        assert_eq!(s.key_hint_line(&keys, p, style), None);
    }
    s.key_hint_line(&keys, focus, style)
        .map(|v| v.iter().map(ac2_ui::hints::KeyHint::text).collect())
        .unwrap_or_default()
}

/// A spectrum made from the palette's dialog, on the dialog's input or (`other`) the
/// session's other one: its id once it runs.
pub(super) fn new_spectrum(d: &mut Driver, other: bool) -> R<MeasId> {
    let before: Vec<MeasId> = d.st.measurements().iter().map(|m| m.id).collect();
    d.key("Ctrl+K");
    d.send(Msg::Text("new spectrum".into()));
    d.key("Enter");
    d.until(
        "the spectrum dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Spectrum),
    )?;
    if other {
        // The input is the dialog's first row, on the mic (the session's last input): ←
        // steps it to the input before.
        d.key("ArrowLeft");
    }
    d.key("Enter");
    d.until("the new spectrum, running", |s| {
        s.measurements().iter().any(|m| {
            !before.contains(&m.id)
                && matches!(m.config.kind, MeasKind::Spectrum { .. })
                && m.running
        })
    })?;
    d.st.measurements()
        .iter()
        .find(|m| !before.contains(&m.id))
        .map(|m| m.id)
        .ok_or_else(|| "the new spectrum".into())
}

/// The spectrum panes in reading order: the measurement each shows, its view, and the
/// spectrograph's caption as drawn.
pub(super) fn spectrum_panes(s: &AppState) -> Vec<(Option<MeasId>, SpectrumMode, String)> {
    let size = ac2_scene::primitives::Viewport {
        width: 1000.0,
        height: 600.0,
    };
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    s.visible_panes()
        .into_iter()
        .filter(|p| s.layout.kind(*p) == ac2_ui::state::PaneKind::Spectrum)
        .map(|p| {
            let caption = ac2_ui::scenes::spectrograph(s, p, &Theme::dark(), size, now).caption;
            (
                s.pane_meas(p).map(|m| m.id),
                s.pane_modes(p).spectrum,
                caption,
            )
        })
        .collect()
}
