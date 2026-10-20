//! The alignment aid from an empty daemon: two transfer curves aligned in their overlap.

use super::harness::{Driver, NAME, R, measure_from_empty};
use ac2_proto::model::{MathExpr, MeasKind};
use ac2_scene::theme::Theme;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::{FieldId, FormKind};
use ac2_ui::state::{AppState, Overlay, PaneKind, StimPhase, TransferView};
use std::time::Instant;

/// The readout of the alignment view the focused pane shows.
fn readout(s: &AppState) -> Option<ac2_scene::align::Readout> {
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let size = ac2_scene::primitives::Viewport {
        width: 1200.0,
        height: 700.0,
    };
    ac2_ui::scenes::alignment(s, s.layout.focus, &Theme::dark(), size, now).readout
}

/// From an empty daemon: a transfer measurement and its capture (Ctrl+1) are two curves of
/// one path. With the stimulus playing again, Alt+M opens the math dialog on align with A
/// the live curve, B the capture and the band prefilled where they are alike; Enter makes
/// it, and it opens in a pane of its own showing the alignment view: no delay, normal
/// polarity, the two adding fully.
#[test]
fn alignment_from_an_empty_daemon() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let live = d.st.selected_meas().map(|m| m.id).ok_or("measurement")?;
    d.tf_frames(live, 240)?;
    d.key("Ctrl+1");
    d.until("the capture in slot 1", |s| {
        s.daemon()
            .is_some_and(|x| x.traces.iter().any(|t| t.edit.slot == Some(1)))
    })?;
    d.until("the capture's curve", |s| !s.traces.is_empty())?;
    let panes = d.st.layout.views.len();
    // A live operand under NO REFERENCE is left out of math (its held average is no longer
    // what the system does), and `measure_from_empty` stopped the stimulus (its level is still
    // set): the live curve needs it playing again for the alignment to have both operands.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;

    d.key("Alt+M");
    let Overlay::Form(f) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    assert_eq!(f.kind, FormKind::Math);
    let rows: Vec<(String, String)> = f
        .fields
        .iter()
        .map(|x| (x.label.clone(), x.display()))
        .collect();
    assert!(rows[0].1.ends_with("(live)"), "{rows:?}");
    assert!(rows[1].1.starts_with("↔  aligned"), "{rows:?}");
    assert!(rows[2].1.ends_with("(stored, S1)"), "{rows:?}");
    assert!(!f.text(FieldId::AlignFrom).is_empty(), "{rows:?}");
    assert!(!f.text(FieldId::AlignTo).is_empty(), "{rows:?}");
    assert!(f.text(FieldId::Name).contains(" ↔ "), "{rows:?}");
    d.key("Enter");
    d.until("the alignment, running", |s| {
        s.measurements().iter().any(|m| {
            m.running
                && matches!(&m.config.kind, MeasKind::Math { config }
                    if matches!(config.expr, MathExpr::Align { .. }))
        })
    })?;
    assert_eq!(d.st.layout.views.len(), panes + 1, "a pane of its own");
    let v = d.st.layout.view(d.st.layout.focus).ok_or("focused pane")?;
    assert_eq!(
        (v.kind, v.modes.transfer),
        (PaneKind::Transfer, TransferView::Alignment)
    );
    d.until("the alignment's fit", |s| readout(s).is_some_and(|r| r.fit))?;
    let r = readout(&d.st).ok_or("readout")?;
    // One path twice: no delay. The live curve keeps averaging fresh noise, so the fit may
    // land a few µs off zero, which the readout's third decimal shows on either curve.
    assert!(
        r.headline == "no delay · polarity normal"
            || (r.headline.contains(": delay 0.00") && r.headline.ends_with("polarity normal")),
        "{r:?}"
    );
    assert!(r.lines[0].contains("summation"), "{r:?}");
    d.stop()
}
