//! The top bar at every width.

use super::*;
use crate::harness::R;
use ac2_proto::model::{GeneratorDesired, GeneratorSettings, Signal};
use ac2_proto::units::Dbfs;

/// With the stimulus armed by another client (the "held by" warning shown, its level, the
/// hint) and a session open, at every width from wide to narrow every label in the bar keeps
/// its own space inside the window; lower-priority texts shorten or go first, the state
/// badge always stays.
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
    let holder = format!("held by {}", rig.cli.client_id().0);
    step_until(&mut h, "armed by the other client", |a| {
        a.state.daemon().is_some_and(|s| s.generator.armed)
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
        assert!(
            labels.iter().any(|(t, _)| t == "ARMED"),
            "{width} px: no badge in {labels:?}"
        );
        let has = |s: &str| labels.iter().any(|(t, _)| t.contains(s));
        if width >= 2400.0 {
            // Wide: everything in full.
            for s in [holder.as_str(), "commands", "frames", DESCRIBE] {
                assert!(has(s), "{width} px: no {s:?} in {labels:?}");
            }
        }
        if width >= 1600.0 {
            for s in [holder.as_str(), DESCRIBE] {
                assert!(has(s), "{width} px: no {s:?} in {labels:?}");
            }
        }
        if width <= 640.0 {
            // Narrow: the key help and the hint have given way to the stimulus state.
            assert!(!has("commands"), "{width} px: {labels:?}");
            assert!(has("dBFS") || has("no level"), "{width} px: {labels:?}");
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
