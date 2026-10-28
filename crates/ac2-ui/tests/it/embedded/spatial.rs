//! The spatial average from an empty daemon: two transfer measurements (two mic positions)
//! made through the app, then the New measurement chooser's Spatial average row.

use super::harness::{Driver, NAME, R, measure_from_empty};
use ac2_proto::model::{AverageMethod, MathExpr, MeasKind, Operand};
use ac2_scene::new_meas::{NewKind, SPATIAL_AVERAGE_NEEDS};
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::{FieldId, FormKind};
use ac2_ui::state::{AppState, ChoicePrompt, ChoicePurpose, Overlay, PaneKind};

/// The open chooser.
fn chooser(s: &AppState) -> Option<&ChoicePrompt> {
    match &s.overlay {
        Overlay::Choose(c) if c.purpose == ChoicePurpose::NewMeasurement => Some(c),
        _ => None,
    }
}

fn transfers(s: &AppState) -> Vec<ac2_proto::units::MeasId> {
    s.measurements()
        .iter()
        .filter(|m| matches!(m.config.kind, MeasKind::Transfer { .. }))
        .map(|m| m.id)
        .collect()
}

/// From an empty daemon: with one transfer measurement the Spatial average row is dimmed
/// with its reason; a second made from the chooser's Transfer row; then the Spatial average
/// row opens the math dialog on the power average of both, named `Spatial average`, and
/// Enter makes it in a pane of its own, the transfer pane still showing what it showed.
#[test]
fn spatial_average_from_an_empty_daemon() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let row = NewKind::ALL
        .iter()
        .position(|k| *k == NewKind::SpatialAverage)
        .ok_or("spatial average row")?;

    // One position: the row says what it needs.
    d.key("Alt+1");
    d.key("Ctrl+M");
    let c = chooser(&d.st).ok_or("the chooser")?;
    assert!(c.choices[row].label.starts_with("Spatial average — "));
    assert_eq!(
        c.choices[row].blocked.as_deref(),
        Some(SPATIAL_AVERAGE_NEEDS)
    );
    assert_eq!(c.index, 0, "Transfer, on the transfer pane");

    // A second position: Enter on Transfer, Enter on its form.
    d.key("Enter");
    d.until(
        "the transfer form",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Transfer),
    )?;
    d.key("Enter");
    d.until("two transfer measurements", |s| transfers(s).len() == 2)?;
    let both = transfers(&d.st);

    d.key("Alt+1");
    let tf = d.st.layout.focus;
    assert_eq!(d.st.layout.kind(tf), PaneKind::Transfer);
    let shown = d.st.pane_meas(tf).map(|m| m.id);
    let panes = d.st.layout.views.len();
    d.key("Ctrl+M");
    let c = chooser(&d.st).ok_or("the chooser")?;
    assert_eq!(c.choices[row].blocked, None);
    for _ in 0..row {
        d.key("ArrowDown");
    }
    assert_eq!(chooser(&d.st).map(|c| c.index), Some(row));
    d.key("Enter");
    let Overlay::Form(f) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    assert_eq!(f.kind, FormKind::Math);
    let rows: Vec<(String, String)> = f
        .fields
        .iter()
        .map(|x| (x.label.clone(), x.display()))
        .collect();
    assert_eq!(rows[0], ("Operator".into(), "average of several".into()));
    assert_eq!(rows[1].1, "in the average", "{rows:?}");
    assert_eq!(rows[2].1, "in the average", "{rows:?}");
    assert!(rows[3].1.starts_with("power"), "{rows:?}");
    assert_eq!(f.text(FieldId::Name), "Spatial average");

    d.key("Enter");
    d.until("the spatial average, running", |s| {
        s.measurements()
            .iter()
            .any(|m| m.running && m.config.name == "Spatial average")
    })?;
    let avg =
        d.st.measurements()
            .iter()
            .find(|m| m.config.name == "Spatial average")
            .map(|m| (*m).clone())
            .ok_or("spatial average")?;
    let MeasKind::Math { config } = &avg.config.kind else {
        return Err(format!("{:?}", avg.config.kind).into());
    };
    // Both positions (the selected one's curve listed first), power.
    let MathExpr::Average { of, method } = &config.expr else {
        return Err(format!("{:?}", config.expr).into());
    };
    assert_eq!(*method, AverageMethod::Power);
    let mut of = of.clone();
    of.sort_by_key(|o| match o {
        Operand::Meas { meas } => meas.0,
        _ => u32::MAX,
    });
    assert_eq!(
        of,
        both.iter()
            .map(|m| Operand::Meas { meas: *m })
            .collect::<Vec<_>>()
    );
    d.until("a pane of its own", |s| s.layout.views.len() == panes + 1)?;
    assert_eq!(
        d.st.pane_meas(tf).map(|m| m.id),
        shown,
        "the transfer pane kept"
    );
    let v = d.st.layout.view(d.st.layout.focus).ok_or("focused pane")?;
    assert_eq!(v.kind, PaneKind::Transfer);
    assert_eq!(v.meas, Some(avg.id));
    d.stop()
}
