//! Dynamics measurements from the app against the real daemon: the chooser's form, the
//! measurement's own Dynamics pane, a run armed with Space and played with Enter, its step
//! table and envelopes in the pane, and a second run replaying the first's rests.

use super::harness::{Driver, R, measuring};
use ac2_proto::model::{MeasKind, Signal, TraceKind};
use ac2_proto::units::{Dbfs, Hz, MeasId};
use ac2_scene::dynamics::DynamicsView;
use ac2_scene::theme::Theme;
use ac2_ui::forms::{FieldId, FormKind};
use ac2_ui::state::{AppState, Msg, Overlay, PaneKind, StimPhase};

fn dynamics_count(s: &AppState) -> usize {
    s.daemon().map_or(0, |x| {
        x.measurements
            .iter()
            .filter(|m| matches!(m.config.kind, MeasKind::Dynamics { .. }))
            .count()
    })
}

fn runs(s: &AppState) -> usize {
    s.daemon().map_or(0, |x| {
        x.traces
            .iter()
            .filter(|t| t.kind == TraceKind::Dynamics)
            .count()
    })
}

/// Ctrl+M, the Dynamics row, the form filled for a short programme on the simulated rig
/// (a loopback reference on in 1, the "processor" on in 2), Enter: the measurement made,
/// on a screen of its own with its Dynamics pane focused.
fn create(d: &mut Driver) -> R<MeasId> {
    let before = dynamics_count(&d.st);
    d.key("Ctrl+M");
    let at = ac2_scene::new_meas::NewKind::ALL
        .iter()
        .position(|k| *k == ac2_scene::new_meas::NewKind::Dynamics)
        .ok_or("the Dynamics row")?;
    d.send(Msg::Choose(Some(at)));
    d.until(
        "the dynamics dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Dynamics),
    )?;
    if let Overlay::Form(f) = &mut d.st.overlay {
        assert_eq!(f.channel(FieldId::Reference), Some(0));
        assert_eq!(f.channel(FieldId::Measurement), Some(1));
        f.set_text(FieldId::Pilot, "-40");
        f.set_text(FieldId::Bursts, "-20");
        f.set_text(FieldId::Durations, "10ms, 100ms");
        f.set_text(FieldId::RestMax, "2s");
    }
    d.key("Enter");
    d.until("the dynamics measurement made", |s| {
        dynamics_count(s) > before
    })?;
    let m =
        d.st.daemon()
            .and_then(|x| {
                x.measurements
                    .iter()
                    .rev()
                    .find(|m| matches!(m.config.kind, MeasKind::Dynamics { .. }))
            })
            .map(|m| m.id)
            .ok_or("the measurement")?;
    d.until("its Dynamics pane focused", |s| {
        s.layout.focus_kind() == PaneKind::Dynamics
            && s.pane_meas(s.layout.focus).map(|x| x.id) == Some(m)
    })?;
    Ok(m)
}

/// Space arms the carrier at the loudest burst; Enter plays the run; the stored trace is
/// selected and the stimulus is off again.
fn run(d: &mut Driver, rests_from_first: bool) -> R {
    let before = runs(&d.st);
    d.key("Space");
    if rests_from_first {
        d.until("the rests question", |s| {
            matches!(s.overlay, Overlay::Choose(_))
        })?;
        if let Overlay::Choose(c) = &d.st.overlay {
            assert!(
                c.choices[1].label.starts_with("Replay the rests of Run 1"),
                "{:?}",
                c.choices
            );
        }
        d.send(Msg::Choose(Some(1)));
    }
    d.until("armed with the carrier", |s| {
        s.rig.stimulus.phase == StimPhase::Armed
            && s.daemon().is_some_and(|x| {
                x.generator.armed
                    && x.generator.settings.as_ref().is_some_and(|g| {
                        g.signal == Signal::Sine { freq: Hz(1000.0) } && g.level == Dbfs(-20.0)
                    })
            })
    })?;
    d.key("Enter");
    d.until("the run stored and selected", |s| {
        runs(s) > before
            && s.rig.dynamics_run.run.is_none()
            && s.rig.selected_trace.is_some_and(|t| {
                s.daemon().is_some_and(|x| {
                    x.traces
                        .iter()
                        .any(|m| m.id == t && m.kind == TraceKind::Dynamics)
                })
            })
    })?;
    d.until("the stimulus off and the lease given back", |s| {
        s.rig.stimulus.phase == StimPhase::Idle
            && s.daemon()
                .is_some_and(|x| !x.generator.armed && !x.generator.firing)
    })?;
    Ok(())
}

fn scene(d: &Driver) -> ac2_scene::dynamics::DynamicsScene {
    let now = ac2_ui::scenes::Now {
        instant: std::time::Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    ac2_ui::scenes::dynamics(
        &d.st,
        d.st.layout.focus,
        &d.keys,
        &Theme::dark(),
        ac2_scene::primitives::Viewport::new(1100.0, 600.0),
        now,
    )
}

/// A dynamics measurement is made without playing: its pane says how to run it, and the
/// generator stays off.
#[test]
fn a_dynamics_measurement_opens_its_own_pane_without_playing() -> R {
    let (_daemon, mut d) = measuring()?;
    create(&mut d)?;
    assert_eq!(crate::common::visible(&d.st), [PaneKind::Dynamics]);
    let s = scene(&d);
    assert!(
        s.note
            .as_deref()
            .is_some_and(|n| n.starts_with("no run of Dynamics 1 yet: Space")),
        "{:?}",
        s.note
    );
    d.synced()?;
    assert!(d.st.daemon().is_some_and(|x| !x.generator.firing));
    assert_eq!(runs(&d.st), 0);
    Ok(())
}

/// Two runs from the app on the simulated rig: the first adapts its rests, the second
/// replays them, so the two compare step by step (no "rests differ"); each view of the
/// pane draws them, and the Envelopes view fetches the picked step's envelopes.
#[test]
fn slow_two_dynamics_runs_compared_in_their_pane() -> R {
    let (_daemon, mut d) = measuring()?;
    create(&mut d)?;
    run(&mut d, false)?;
    d.until("the run's step table", |s| {
        s.dynamics_curves(s.layout.focus).len() == 1
    })?;
    let one = scene(&d);
    assert_eq!(one.a, Some(0));
    assert_eq!(one.cells.len(), 2, "one level × two lengths");
    assert!(one.caption.contains("Run 1"), "{}", one.caption);

    run(&mut d, true)?;
    d.until("both runs' step tables", |s| {
        s.dynamics_curves(s.layout.focus).len() == 2
    })?;
    // Shift+B: B is the second run's curve (the first is A).
    let mut seen_b = scene(&d).b;
    for _ in 0..3 {
        if seen_b == Some(1) {
            break;
        }
        d.key("Shift+B");
        seen_b = scene(&d).b;
    }
    assert_eq!(seen_b, Some(1));
    let both = scene(&d);
    assert_eq!(
        both.note, None,
        "the replayed rests compare: {:?}",
        both.note
    );
    assert!(both.caption.contains(" − "), "{}", both.caption);

    // G: the envelopes of the picked step, fetched for both curves.
    d.key("G");
    assert_eq!(
        d.st.layout.focused().modes.dynamics.view,
        DynamicsView::Envelopes
    );
    d.until("the picked step's envelopes", |s| {
        s.rig.dynamics.envelopes.len() == 2
    })?;
    let env = scene(&d);
    assert!(!env.legend.is_empty());
    // Release, then Table: every step of A, B and A − B under the heading.
    d.key("G");
    d.key("G");
    assert_eq!(
        d.st.layout.focused().modes.dynamics.view,
        DynamicsView::Table
    );
    let t = scene(&d);
    assert_eq!(t.table.len(), 1 + 2 * 3, "{:?}", t.table);
    Ok(())
}
