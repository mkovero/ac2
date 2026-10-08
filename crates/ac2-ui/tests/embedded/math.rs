//! Math channels on the transfer and spectrum panes.

use crate::harness::{Driver, NAME, R, measure_from_empty};
use ac2_proto::FrameData;
use ac2_proto::model::MeasKind;
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::MeasId;
use ac2_scene::theme::Theme;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::CommandId;
use ac2_ui::state::{AppState, Msg, Overlay, StimPhase};
use std::time::Instant;

/// The legend text of live measurement `id` on the transfer pane.
fn tf_legend(s: &AppState, id: MeasId) -> String {
    let size = ac2_scene::primitives::Viewport {
        width: 1200.0,
        height: 600.0,
    };
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    ac2_ui::scenes::transfer(s, &Theme::dark(), size, now)
        .legend
        .iter()
        .find(|e| e.key == ac2_scene::trace::TraceKey::Live(id))
        .map(|e| e.text.clone())
        .unwrap_or_default()
}

/// The math channel of the mirrored state, if there is one running.
fn running_math(s: &AppState) -> Option<(MeasId, String)> {
    s.measurements()
        .iter()
        .find(|m| matches!(m.config.kind, MeasKind::Math { .. }) && m.running)
        .map(|m| (m.id, m.config.name.clone()))
}

/// From an empty daemon: two transfer measurements (two positions of the room mic); the
/// math channel dialog by name (Shift+M) makes their average, drawn as a transfer curve whose
/// legend counts its positions; Ctrl+1 captures it into a stored trace naming the
/// expression; edited into A ÷ B it reads 0 dB (one path over itself); a position stopped
/// is named in a banner and the ratio says it has no result.
#[test]
fn math_channels_from_an_empty_daemon() -> R {
    use ac2_proto::Command;
    use ac2_proto::frame::OperandStatus;
    use ac2_proto::model::{MathExpr, MathOp, TraceSource};
    use ac2_ui::conn::Request;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let first = d.st.selected_meas().map(|m| m.id).ok_or("measurement")?;

    // One transfer function: nothing to combine it with yet, and the dialog says so.
    d.key("Shift+M");
    assert!(!matches!(d.st.overlay, Overlay::Form(_)));
    assert!(
        d.st.toasts
            .iter()
            .any(|t| t.text.contains("two transfer functions, spectra or RTAs")),
        "{:?}",
        d.st.toasts
    );

    // A second position, from the transfer dialog.
    d.send(Msg::Command(CommandId::NewTransfer));
    d.key("Enter");
    d.until("two transfer measurements", |s| {
        s.measurements()
            .iter()
            .filter(|m| matches!(m.config.kind, MeasKind::Transfer { .. }) && m.running)
            .count()
            == 2
    })?;
    let second =
        d.st.measurements()
            .iter()
            .map(|m| m.id)
            .find(|id| *id != first)
            .ok_or("second")?;

    // A, the operator and B by name; the operator steps on to the average, which takes both
    // positions; Enter makes and starts it.
    d.key("Shift+M");
    let Overlay::Form(f) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    assert_eq!(f.kind, FormKind::Math);
    let rows: Vec<(String, String)> = f
        .fields
        .iter()
        .map(|x| (x.label.clone(), x.display()))
        .collect();
    assert_eq!(rows[0].0, "A");
    assert!(rows[0].1.ends_with("(live)"), "{rows:?}");
    assert_eq!(rows[1], ("Operator".into(), "÷  A relative to B".into()));
    assert_eq!(rows[2].0, "B");
    d.key("Down");
    for _ in 0..4 {
        d.key("Right");
    }
    let Overlay::Form(f) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    let rows: Vec<(String, String)> = f
        .fields
        .iter()
        .map(|x| (x.label.clone(), x.display()))
        .collect();
    assert_eq!(rows[0], ("Operator".into(), "average of several".into()));
    assert_eq!(rows[1].1, "in the average");
    assert_eq!(rows[2].1, "in the average");
    assert!(
        rows.iter()
            .any(|r| r == &("Name".into(), "Average of 2".into())),
        "{rows:?}"
    );
    d.key("Enter");
    d.until("the math channel, running", |s| running_math(s).is_some())?;
    let (avg, name) = running_math(&d.st).ok_or("math channel")?;
    assert_eq!(name, "Average of 2");

    // Two positions of the same −6 dB path average to −6 dB (the level is still typed).
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|d| d.generator.firing))?;
    d.tf_frames(avg, 240)?;
    assert!(
        tf_legend(&d.st, avg).starts_with("Average of 2 · 2 positions · power avg"),
        "{}",
        tf_legend(&d.st, avg)
    );

    // Ctrl+1 captures it: a stored trace in slot 1 naming the expression and its operands.
    d.send(Msg::SelectMeas(avg));
    d.key("Ctrl+1");
    d.until("the capture in slot 1", |s| {
        s.daemon().is_some_and(|x| {
            x.traces.iter().any(|t| {
                t.edit.slot == Some(1)
                    && matches!(
                        &t.source,
                        TraceSource::Math { expr: MathExpr::Average { .. }, operands, .. }
                            if operands.len() == 2
                    )
            })
        })
    })?;

    // Edited in the same dialog into A ÷ B: one path over itself is 0 dB.
    d.key("Ctrl+K");
    d.send(Msg::Text("edit the selected math".into()));
    d.key("Enter");
    assert!(
        matches!(&d.st.overlay, Overlay::Form(f) if f.kind == FormKind::MathEdit),
        "{:?}",
        d.st.overlay
    );
    for _ in 0..4 {
        d.key("Left");
    }
    d.key("Enter");
    let topic = Topic::Data {
        meas: avg,
        stream: Stream::Tf,
    };
    d.until("the ratio at 0 dB", |s| {
        let ratio = s.meas(avg).is_some_and(|m| {
            matches!(
                &m.config.kind,
                MeasKind::Math { config } if matches!(
                    config.expr,
                    MathExpr::Binary { op: MathOp::Divide, .. }
                )
            )
        });
        ratio
            && s.data.as_ref().is_some_and(|x| {
                x.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                    FrameData::Tf(tf) => tf
                        .mag
                        .get(240)
                        .is_some_and(|m| m.is_finite() && m.abs() < 0.5),
                    _ => false,
                })
            })
    })?;
    assert!(
        tf_legend(&d.st, avg).contains(" ÷ "),
        "{}",
        tf_legend(&d.st, avg)
    );

    // The ratio's phase keeps A's arrival relative to B: its legend says how far apart they
    // are, to 0.1 µs, by the delays each one's phase is referred to — the second position,
    // over the capture of the average taken at no delay.
    d.conn.send(Request::Call {
        cmd: Command::DelaySet {
            meas: second,
            delay: ac2_proto::units::Seconds(12.3e-6),
        },
        what: "delay".into(),
    });
    d.until("the delay set", |s| {
        s.meas(second)
            .and_then(|m| m.delay.as_ref())
            .is_some_and(|x| (x.applied.0 - 12.3e-6).abs() < 1e-12)
    })?;
    assert!(
        tf_legend(&d.st, avg).contains(" ÷ Average of 2 S1 · arrival Δ +12.3 µs · +4.2 mm @ 20 °C"),
        "{}",
        tf_legend(&d.st, avg)
    );

    // One operand stopped: no ratio, and the banner says which and why.
    d.conn.send(Request::Call {
        cmd: Command::MeasStop { meas: second },
        what: "stopped".into(),
    });
    d.until("the ratio without its second operand", |s| {
        s.data.as_ref().is_some_and(|x| {
            x.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                FrameData::Tf(tf) => tf.meta.math.as_ref().is_some_and(|a| {
                    a.operands
                        .iter()
                        .any(|m| m.status == OperandStatus::Stopped)
                }),
                _ => false,
            })
        })
    })?;
    let second_name =
        d.st.measurements()
            .iter()
            .find(|m| m.id == second)
            .map(|m| m.config.name.clone())
            .unwrap_or_default();
    d.st.pane_meas
        .insert(ac2_ui::state::PaneKind::Transfer, avg);
    let size = ac2_scene::primitives::Viewport {
        width: 1200.0,
        height: 600.0,
    };
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let scene = ac2_ui::scenes::transfer(&d.st, &Theme::dark(), size, now);
    let banners: Vec<(String, Option<String>)> = scene
        .banners
        .iter()
        .map(|b| (b.text.clone(), b.detail.clone()))
        .collect();
    assert!(
        banners.contains(&(
            "NO RESULT · 1 OF 2 OPERANDS".into(),
            Some(format!("Average of 2: left out {second_name}: stopped"))
        )),
        "{banners:?}"
    );
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// Spectrum math lands on the spectrum pane, never the transfer pane: two spectra of the
/// room mic, A − B by the dialog with the spectrum pane focused, both operands in its frames.
#[test]
fn spectrum_math_lands_on_the_spectrum_pane() -> R {
    use ac2_proto::model::MathDomain;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    for n in 1..=2 {
        d.send(Msg::Command(CommandId::NewSpectrum));
        d.key("Enter");
        d.until("the spectrum, running", |s| {
            s.measurements()
                .iter()
                .filter(|m| matches!(m.config.kind, MeasKind::Spectrum { .. }) && m.running)
                .count()
                == n
        })?;
    }
    d.key("Alt+2");
    d.key("Shift+M");
    let Overlay::Form(f) = &d.st.overlay else {
        return Err(format!("{:?}", d.st.overlay).into());
    };
    let op = f
        .fields
        .iter()
        .find(|x| x.label == "Operator")
        .map(|x| x.display())
        .unwrap_or_default();
    assert_eq!(op, "−  level difference (dB)");
    d.key("Enter");
    d.until("the spectrum math, running", |s| running_math(s).is_some())?;
    let (id, name) = running_math(&d.st).ok_or("math channel")?;
    assert_eq!(name, "Spectrum 2 − Spectrum 1");
    assert!(d.st.meas(id).is_some_and(|m| matches!(
        &m.config.kind,
        MeasKind::Math { config } if config.domain == MathDomain::Spectrum
    )));
    // The level is still typed: Space arms, Enter fires.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|d| d.generator.firing))?;
    let topic = Topic::Data {
        meas: id,
        stream: Stream::Spec,
    };
    // Both spectra went in, on the spectrum stream (the levels are checked against
    // analytic values in the daemon's tests: here each operand is its own unaveraged FFT).
    d.until("the difference on the spectrum stream", |s| {
        s.data.as_ref().is_some_and(|x| {
            x.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                FrameData::Spec(sp) => {
                    sp.meta.math.as_ref().is_some_and(|m| m.included() == 2)
                        && sp.level.iter().filter(|v| v.is_finite()).count() > 100
                }
                _ => false,
            })
        })
    })?;
    let size = ac2_scene::primitives::Viewport {
        width: 1200.0,
        height: 600.0,
    };
    let now = || ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let live = ac2_scene::trace::TraceKey::Live(id);
    d.st.pane_meas.insert(ac2_ui::state::PaneKind::Spectrum, id);
    let spec = ac2_ui::scenes::spectrum(&d.st, &Theme::dark(), size, now());
    assert!(
        spec.legend.iter().any(|e| e.key == live),
        "not on the spectrum pane"
    );
    // Shown by the pane, its caption is the expression and what it means.
    assert!(
        spec.caption
            .contains("Spectrum 2 − Spectrum 1 · level difference"),
        "{}",
        spec.caption
    );
    let tf = ac2_ui::scenes::transfer(&d.st, &Theme::dark(), size, now());
    assert!(
        tf.legend.iter().all(|e| e.key != live),
        "spectrum math on the transfer pane"
    );
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}
