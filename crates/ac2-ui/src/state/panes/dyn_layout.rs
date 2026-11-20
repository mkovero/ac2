//! A Dynamics pane's modes as a measurement's layout stores them and back. Curves A and B
//! name trace ids, which are per daemon: a curve of the pane's own rig is written with that
//! rig's reference, another rig's with its id, as a pane entry is.

use ac2_proto::model::layout as wire;
use ac2_proto::units::RigId;
use ac2_scene::dynamics::{CurveId, DynamicsModes, Side};

use super::meas_layout::WireRigs;

/// Modes `m` of a pane on rig `refs.pane`, as the layout stores them.
pub(in crate::state) fn wire_dynamics(m: &DynamicsModes, refs: &WireRigs) -> wire::DynamicsPane {
    let side = |s: Side| match s {
        Side::Default => wire::DynamicsSide::Default,
        Side::Off => wire::DynamicsSide::Off,
        Side::Curve(c) => wire::DynamicsSide::Curve {
            rig: refs.of(c.rig.unwrap_or(refs.pane)),
            trace: c.trace,
            input: c.input,
        },
    };
    wire::DynamicsPane {
        view: m.view,
        a: side(m.a),
        b: side(m.b),
        step: m.step,
        metric: m.metric,
        thd: m.thd,
        all_levels: m.all_levels,
    }
}

/// Stored modes `p` read for rig `this`, of a pane whose own rig is `pane`.
pub(in crate::state) fn ui_dynamics(
    p: &wire::DynamicsPane,
    this: RigId,
    pane: RigId,
) -> DynamicsModes {
    let side = |s: &wire::DynamicsSide| match s {
        wire::DynamicsSide::Default => Side::Default,
        wire::DynamicsSide::Off => Side::Off,
        wire::DynamicsSide::Curve { rig, trace, input } => {
            let r = match rig {
                wire::RigRef::ThisRig => this,
                wire::RigRef::Rig { id, .. } => *id,
            };
            Side::Curve(CurveId {
                rig: Some(r).filter(|r| *r != pane),
                trace: *trace,
                input: *input,
            })
        }
    };
    DynamicsModes {
        view: p.view,
        a: side(&p.a),
        b: side(&p.b),
        step: p.step,
        metric: p.metric,
        thd: p.thd,
        all_levels: p.all_levels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::units::{Dbfs, Seconds, TraceId};
    use std::collections::BTreeMap;

    #[test]
    fn a_dynamics_pane_round_trips_with_another_rigs_curve() {
        let (this, other) = (RigId(1), RigId(2));
        let names = BTreeMap::from([(other, "pupu".to_string())]);
        let refs = WireRigs {
            this,
            pane: this,
            names: &names,
        };
        let m = DynamicsModes {
            view: wire::DynamicsView::Release,
            a: Side::Curve(CurveId {
                rig: None,
                trace: TraceId(3),
                input: 1,
            }),
            b: Side::Curve(CurveId {
                rig: Some(other),
                trace: TraceId(3),
                input: 0,
            }),
            step: Some(wire::DynamicsStepPick {
                level: Dbfs(-8.0),
                burst: Seconds(1.0),
            }),
            metric: wire::DynamicsMetric::ReleaseTau,
            thd: true,
            all_levels: true,
        };
        let w = wire_dynamics(&m, &refs);
        assert_eq!(
            w.a,
            wire::DynamicsSide::Curve {
                rig: wire::RigRef::ThisRig,
                trace: TraceId(3),
                input: 1
            }
        );
        assert_eq!(ui_dynamics(&w, this, this), m);
    }
}
