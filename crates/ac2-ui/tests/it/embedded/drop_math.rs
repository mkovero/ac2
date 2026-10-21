//! A curve's row dropped onto another's makes a math channel, from an empty daemon.

use super::harness::{Driver, NAME, R, measure_from_empty};
use ac2_proto::model::{MathExpr, MathOp, MeasKind, Operand, TraceKind, TraceOwner};
use ac2_scene::meas_list::TreeKey;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::FormKind;
use ac2_ui::state::{DragMsg, Msg, Overlay};

/// From an empty daemon: a capture of the first transfer measurement, then Z's loudness
/// shift, which is listed under that measurement (not Imported); the shift's row dropped
/// on the capture's opens the math dialog as `capture × loudness`, and Enter makes that
/// math channel under the same measurement.
#[test]
fn a_loudness_shift_dropped_on_a_capture_makes_its_math_channel() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let m = d.st.selected_meas().map(|m| m.id).ok_or("measurement")?;
    let owner = TraceOwner::Meas { meas: m };

    d.key("Ctrl+1");
    d.until("the capture in slot 1", |s| {
        s.daemon()
            .is_some_and(|x| x.traces.iter().any(|t| t.edit.slot == Some(1)))
    })?;
    let capture =
        d.st.daemon()
            .and_then(|x| x.traces.iter().find(|t| t.edit.slot == Some(1)))
            .cloned()
            .ok_or("capture")?;
    assert_eq!(capture.edit.owner, owner);

    // Z, the fourth row (loudness), its dialog's defaults.
    d.key("Z");
    for _ in 0..3 {
        d.key("Down");
    }
    d.key("Enter");
    assert!(
        matches!(&d.st.overlay, Overlay::Form(_)),
        "{:?}",
        d.st.overlay
    );
    d.key("Enter");
    let loud_name = "loudness 85 → 100 phon";
    d.until("the loudness shift", |s| {
        s.daemon()
            .is_some_and(|x| x.traces.iter().any(|t| t.edit.name == loud_name))
    })?;
    let loud =
        d.st.daemon()
            .and_then(|x| x.traces.iter().find(|t| t.edit.name == loud_name))
            .cloned()
            .ok_or("loudness")?;
    assert_eq!(loud.kind, TraceKind::Target);
    assert_eq!(
        loud.edit.owner, owner,
        "listed with the measurement it was made on"
    );

    d.send(Msg::RowDrag(DragMsg::Start(TreeKey::Trace(loud.id))));
    d.send(Msg::RowDrag(DragMsg::OverRow(Some(TreeKey::Trace(
        capture.id,
    )))));
    let name = format!("{} × {loud_name}", capture.edit.name);
    assert_eq!(
        d.st.drag_label().as_deref(),
        Some(format!("math channel: {name}").as_str())
    );
    d.send(Msg::RowDrag(DragMsg::Drop { compare: false }));
    assert!(
        matches!(&d.st.overlay, Overlay::Form(f) if f.kind == FormKind::Math),
        "{:?}",
        d.st.overlay
    );
    // Nothing is made until Enter.
    assert!(
        !d.st
            .measurements()
            .iter()
            .any(|x| matches!(x.config.kind, MeasKind::Math { .. }))
    );
    d.key("Enter");
    d.until("the math channel", |s| {
        s.measurements()
            .iter()
            .any(|x| matches!(x.config.kind, MeasKind::Math { .. }))
    })?;
    let made =
        d.st.measurements()
            .into_iter()
            .find(|x| matches!(x.config.kind, MeasKind::Math { .. }))
            .cloned()
            .ok_or("math channel")?;
    assert_eq!(made.config.name, name);
    let MeasKind::Math { config } = &made.config.kind else {
        return Err("not math".into());
    };
    assert_eq!(config.owner, owner);
    assert_eq!(
        config.expr,
        MathExpr::Binary {
            a: Operand::Trace { trace: capture.id },
            op: MathOp::Multiply,
            b: Operand::Trace { trace: loud.id },
        }
    );
    Ok(())
}
