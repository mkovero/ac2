//! The top bar at every width.

use super::*;
use crate::harness::R;
use ac2_proto::model::{GeneratorDesired, GeneratorSettings, Signal};
use ac2_proto::units::Dbfs;

/// A set of two sweeps run by another client while the app watches: the progress strip
/// shows which sweep of how many and the time left, drawn over the panes (the sweep starting
/// moves no pane: the transfer title stays where it was), still there with the transfer
/// pane maximised; its Stop button stops the stimulus on the daemon, ending the run.
#[test]
fn sweep_progress_strip() -> R {
    use ac2_proto::model::{
        EssSpec, LfHarmonics, MeasConfig, MeasKind, MeasScope, Resolution, SweepConfig,
    };
    use ac2_proto::units::{Hz, Seconds};
    use egui_kittest::kittest::{By, Queryable as _};
    if !have_gpu("sweep_progress_strip") {
        return Ok(());
    }
    let rig = Rig::start()?;
    let mut h = harness(options(&rig));
    step_until(&mut h, "live frames", live);
    // The pane's title naming its measurement (the tree has rows of that name too).
    let title = |h: &Harness<'_, App>| {
        h.query_all(By::new().label_contains(" · Main L"))
            .map(|n| n.rect())
            .filter(|r| r.min.x > 230.0)
            .fold(egui::Rect::NOTHING, |a, r| a.union(r))
    };
    let before = title(&h);
    assert!(before.is_positive(), "no measurement title in the pane");

    let sweep = EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(20.0));
    let level = Dbfs(-30.0);
    let ReplyBody::Measurement(m) = rig.cli.call(Command::MeasCreate {
        config: MeasConfig {
            scope: MeasScope::Session,
            name: "Sweep 1".into(),
            kind: MeasKind::Sweep {
                config: SweepConfig {
                    reference_input: 0,
                    measurement_inputs: vec![1],
                    outputs: vec![0],
                    level,
                    sweep,
                    repeats: 2,
                    gate: None,
                    tail: None,
                    lf_harmonics: LfHarmonics::Standard,
                    resolution: Resolution::FortyEighth,
                },
            },
        },
    })?
    else {
        return Err("no measurement".into());
    };
    let lease = rig.cli.lease(GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Ess { sweep },
            level,
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing: false,
    })?;
    rig.cli.call(Command::SweepRun(ac2_proto::SweepRunArgs {
        lease_token: lease.token(),
        meas: m.id,
        name: None,
    }))?;
    step_until(&mut h, "the progress strip", |a| {
        a.state
            .operation()
            .is_some_and(|p| p.step == "sweep 1 of 2" && p.remaining.is_some())
    });
    settle(&mut h, 2);
    assert_eq!(title(&h), before, "the strip moved the panes");
    h.key_press(Key::W);
    step_until(&mut h, "maximized", |a| a.state.layout.maximized);
    assert!(h.state().state.operation().is_some(), "the strip stays");
    h.get_by_label_contains("Stop (").click();
    step_until(&mut h, "stopped on the daemon", |a| {
        a.state
            .daemon()
            .is_some_and(|s| !s.generator.firing && !s.generator.armed)
    });
    settle(&mut h, 3);
    let st = &h.state().state;
    // The run discarded: the daemon says why it ended.
    let status = st.daemon().and_then(|s| s.sweep.clone()).map(|r| r.status);
    assert!(
        matches!(
            status,
            Some(ac2_proto::model::SweepStatus::Failed {
                reason: ac2_proto::model::SweepFailure::Stopped,
                ..
            })
        ),
        "{status:?}"
    );
    assert!(st.operation().is_none(), "{:?}", st.operation());
    drop(lease);
    Ok(())
}

/// With the stimulus armed by another client (said in brackets, with who holds it) and a
/// stream open, at every width from wide to narrow: the rig drop-down and the stop are
/// whole and in the window, every label in the bar keeps its own space, and the daemon's
/// software name is not in the bar. Wide, the whole line is there.
#[test]
fn top_bar_never_overlaps() -> R {
    use egui::accesskit::Role;
    use egui_kittest::kittest::{By, NodeT, Queryable as _};
    if !have_gpu("top_bar_never_overlaps") {
        return Ok(());
    }
    let rig = Rig::start()?;
    let mut h = harness(options(&rig));
    step_until(&mut h, "live frames", live);
    let _held = rig.cli.lease(GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Pink,
            level: Dbfs(-30.0),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing: false,
    })?;
    // A client on the rig's own host has only a routing tag for an id: named by the rig,
    // which saw it connect locally.
    let holder = "held by an app on Simulated rig".to_owned();
    step_until(&mut h, "armed by the other client", |a| {
        a.state
            .daemon()
            .is_some_and(|s| s.generator.armed && s.generator.owner_local)
    });
    for width in [2400.0, 1600.0, 1290.0, 1100.0, 900.0, 760.0, 640.0] {
        h.set_size(egui::vec2(width, 800.0));
        for _ in 0..3 {
            h.step();
        }
        let labels: Vec<(String, egui::Rect)> = h
            .query_all(By::new().predicate(|n| n.role() == Role::Label))
            .map(|n| {
                let a = n.accesskit_node();
                (
                    a.label().or_else(|| a.value()).unwrap_or_default(),
                    n.rect(),
                )
            })
            .filter(|(_, r)| r.max.y <= 34.0)
            .collect();
        let has = |s: &str| labels.iter().any(|(t, _)| t.contains(s));
        // Fixed at the bar's ends, whatever the line between them does.
        for s in ["Simulated rig", "■ Stop"] {
            assert!(has(s), "{width} px: no {s:?} in {labels:?}");
        }
        assert!(!has(DESCRIBE), "{width} px: {labels:?}");
        if width >= 2400.0 {
            // Wide: the line in full, and the key hints.
            for s in ["local", "(ARMED", holder.as_str(), "frames", "commands"] {
                assert!(has(s), "{width} px: no {s:?} in {labels:?}");
            }
        }
        if width <= 640.0 {
            // Narrow: the key hints have given their room to the line.
            assert!(!has("commands"), "{width} px: {labels:?}");
        }
        for (i, (a, ra)) in labels.iter().enumerate() {
            assert!(
                ra.min.x >= 0.0 && ra.max.x <= width,
                "{width} px: {a:?} {ra:?} outside the window"
            );
            for (b, rb) in &labels[i + 1..] {
                let o = ra.intersect(*rb);
                assert!(
                    o.width() <= 0.5 || o.height() <= 0.5,
                    "{width} px: {a:?} {ra:?} overlaps {b:?} {rb:?}"
                );
            }
        }
    }
    Ok(())
}
