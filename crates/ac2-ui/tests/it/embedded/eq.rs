//! The EQ what-if editor from an empty daemon: Alt+E makes `EQ 1` on the transfer
//! measurement, a peaking section is added and set by keys, the legend says what each
//! prediction assumes is in, B / Shift+B bypass, P predicts a capture, M + M marks the set
//! as in the console and the live prediction turns into `no change`.

use super::harness::{Driver, NAME, R, measure_from_empty};
use ac2_proto::model::{EqShape, MeasKind, Measurement, Operand};
use ac2_proto::units::MeasId;
use ac2_scene::theme::Theme;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::state::{AppState, Msg, Overlay, StimPhase};
use std::time::Instant;

/// Every legend line of the transfer pane, without the `· indep.` of a line that keeps
/// its own delay.
fn legend(s: &AppState) -> Vec<String> {
    ac2_ui::scenes::transfer(
        s,
        crate::common::pane(s, ac2_ui::state::PaneKind::Transfer),
        &Theme::dark(),
        ac2_scene::primitives::Viewport {
            width: 1200.0,
            height: 600.0,
        },
        ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: ac2_proto::units::WallNs(0),
        },
    )
    .legend
    .iter()
    .map(|e| e.text.trim_end_matches(" · indep.").to_owned())
    .collect()
}

fn meas(s: &AppState, id: MeasId) -> Option<&Measurement> {
    s.measurements().into_iter().find(|m| m.id == id)
}

/// The prediction of `curve`, if made.
fn prediction(s: &AppState, curve: &Operand) -> Option<MeasId> {
    s.measurements().iter().find_map(|m| match &m.config.kind {
        MeasKind::Math { config } => ac2_scene::eq::prediction(&config.expr)
            .filter(|(_, _, c)| *c == curve)
            .map(|_| m.id),
        _ => None,
    })
}

fn editor_bottom(s: &AppState) -> String {
    let Overlay::Eq(e) = &s.overlay else {
        return String::new();
    };
    let Some(m) = meas(s, e.meas) else {
        return String::new();
    };
    ac2_ui::eq_editor::view(e, m, &s.trace_list(), &s.measurements()).bottom
}

fn has_legend(s: &AppState, line: &str) -> bool {
    legend(s).iter().any(|l| l == line)
}

#[test]
fn eq_editor_from_an_empty_daemon() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let m = d.st.selected_meas().cloned().ok_or("measurement")?;
    let live = Operand::Meas { meas: m.id };

    // The level is still set from the first run: Space arms, Enter fires.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|d| d.generator.firing))?;
    d.key("Alt+E");
    assert!(matches!(d.st.overlay, Overlay::Eq(_)), "{:?}", d.st.overlay);
    d.until("EQ 1 and its prediction", |s| {
        meas(s, m.id).is_some_and(|x| x.eq.len() == 1) && prediction(s, &live).is_some()
    })?;
    let pred = prediction(&d.st, &live).ok_or("prediction")?;

    // Insert adds a peaking section at 1 kHz; Tab to its gain, typed −6.
    d.key("Insert");
    d.key("Tab");
    d.send(Msg::Text("-6".into()));
    d.until("the peaking −6 dB section", |s| {
        meas(s, m.id).is_some_and(|x| {
            matches!(
                x.eq[0].body.sections.as_slice(),
                [s] if matches!(s.shape, EqShape::Peaking { gain, .. } if gain.0 == -6.0)
            )
        })
    })?;
    // Its Q a step narrower by ←/→ on the Q field (Tab once more).
    d.key("Tab");
    d.key("Right");
    d.until("Q narrowed", |s| {
        meas(s, m.id).is_some_and(|x| {
            matches!(x.eq[0].body.sections[0].shape,
                EqShape::Peaking { bw: ac2_proto::model::Bandwidth::Q { value }, .. } if value > 1.0)
        })
    })?;

    d.tf_frames(m.id, 240)?;
    let name = format!("{} after EQ 1 · assumes none of it in", m.config.name);
    d.until("the prediction's legend", |s| {
        legend(s).iter().any(|l| l.starts_with(&name))
    })?;
    d.until("the EQ curve's legend", |s| has_legend(s, "EQ 1"))?;

    // B bypasses the section, Shift+B the whole set.
    d.key("B");
    d.until("the section off", |s| {
        meas(s, m.id).is_some_and(|x| !x.eq[0].body.sections[0].enabled)
    })?;
    d.key("B");
    d.key("Shift+B");
    d.until("the set bypassed", |s| has_legend(s, "EQ 1 · bypassed"))?;
    d.key("Shift+B");
    d.until("the set back", |s| has_legend(s, "EQ 1"))?;

    // A capture, then P on its row predicts it too.
    d.key("Escape");
    assert!(!matches!(d.st.overlay, Overlay::Eq(_)));
    d.key("Ctrl+1");
    d.until("the capture", |s| {
        s.trace_list().iter().any(|t| {
            t.edit.owner == ac2_proto::model::TraceOwner::Meas { meas: m.id }
                && matches!(t.source, ac2_proto::model::TraceSource::Captured { .. })
        })
    })?;
    let capture =
        d.st.trace_list()
            .iter()
            .find(|t| matches!(t.source, ac2_proto::model::TraceSource::Captured { .. }))
            .map(|t| (t.id, t.edit.name.clone()))
            .ok_or("capture")?;
    d.key("Alt+E");
    // Header, the section, the live curve, the capture.
    for _ in 0..3 {
        d.key("Down");
    }
    d.key("P");
    let cap = Operand::Trace { trace: capture.0 };
    d.until("the capture's prediction", |s| {
        prediction(s, &cap).is_some()
    })?;

    // M asks, M marks: the live curve now contains the set as it is.
    d.key("M");
    assert!(
        editor_bottom(&d.st).starts_with("Mark EQ 1 rev "),
        "{}",
        editor_bottom(&d.st)
    );
    d.key("M");
    d.until("the mark", |s| {
        meas(s, m.id).is_some_and(|x| x.eq[0].in_console.is_some())
    })?;
    let no_change = format!("{} after EQ 1 · EQ 1 in · no change", m.config.name);
    d.until("no change", |s| {
        legend(s).iter().any(|l| l.starts_with(&no_change))
    })?;
    // The capture was taken before the mark: it still contains none of it.
    let cap_name = format!("{} after EQ 1 · assumes none of it in", capture.1);
    d.until("the capture's prediction legend", |s| {
        legend(s).iter().any(|l| l.starts_with(&cap_name))
    })?;
    assert!(meas(&d.st, pred).is_some());
    d.key("Escape");
    d.stop()
}
