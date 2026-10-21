//! A list row dropped onto another row: the math dialog opens with the two combined
//! (`ac2_scene::math_drop`), A the row dropped on, B the row dragged, the operator the
//! reading B's kind asks for. Nothing is made until Enter: the operator, operands and name
//! can still be changed, and Esc drops the idea.

use ac2_proto::model::{MathDomain, MathOp, Operand, TargetCurve, TraceSource};
use ac2_scene::math_drop::{self, DropCurve};
use ac2_scene::meas_list::TreeKey;

use super::*;

/// What a drop of one row onto another opens.
#[derive(Clone, Debug, PartialEq)]
pub struct DropPlan {
    pub a: Operand,
    pub op: MathOp,
    pub b: Operand,
    /// Where the channel is listed: with A, as a math channel made on A's measurement is.
    pub owner: TraceOwner,
    /// The label by the pointer: `math channel: TF 1 S1 × loudness 85 → 100 phon`.
    pub label: String,
}

/// One side of a drop as the list knows it.
struct Side {
    name: String,
    domain: Option<MathDomain>,
    curve: DropCurve,
    operand: Option<Operand>,
    /// The group it is listed in.
    group: TraceOwner,
}

impl AppState {
    fn drop_side(&self, key: TreeKey) -> Option<Side> {
        let ms = self.measurements();
        match key {
            TreeKey::Meas(id) | TreeKey::Live(id) | TreeKey::Math(id) => {
                let m = self.meas(id)?;
                let (domain, curve) = match &m.config.kind {
                    MeasKind::Transfer { .. } => (Some(MathDomain::Transfer), DropCurve::Live),
                    MeasKind::Spectrum { .. } => (Some(MathDomain::Spectrum), DropCurve::Live),
                    MeasKind::Rta { .. } => (Some(MathDomain::Rta), DropCurve::Live),
                    MeasKind::Math { config } => (Some(config.domain), DropCurve::Math),
                    MeasKind::Spl { .. } | MeasKind::Sweep { .. } => (None, DropCurve::Live),
                };
                Some(Side {
                    name: m.config.name.clone(),
                    domain,
                    curve,
                    operand: Some(Operand::Meas { meas: id }),
                    group: ac2_scene::meas_list::meas_group(m, &ms),
                })
            }
            TreeKey::Trace(id) => {
                let t = self.trace_meta(id).ok()?;
                let (curve, operand) = match (&t.source, t.kind) {
                    (TraceSource::Eq { meas, set, .. }, _) => (
                        DropCurve::Eq,
                        Operand::Eq {
                            meas: *meas,
                            set: *set,
                        },
                    ),
                    (
                        TraceSource::Generated {
                            curve: TargetCurve::LoudnessShift { .. },
                        },
                        _,
                    ) => (DropCurve::Loudness, Operand::Trace { trace: id }),
                    (_, TraceKind::Target) => (DropCurve::Target, Operand::Trace { trace: id }),
                    _ => (DropCurve::Stored, Operand::Trace { trace: id }),
                };
                let domain = match t.kind {
                    TraceKind::Transfer | TraceKind::Sweep | TraceKind::Target => {
                        MathDomain::Transfer
                    }
                    TraceKind::Spectrum { .. } => MathDomain::Spectrum,
                    TraceKind::Rta { .. } => MathDomain::Rta,
                };
                Some(Side {
                    name: t.edit.name.clone(),
                    domain: Some(domain),
                    curve,
                    operand: Some(operand),
                    group: ac2_scene::meas_list::group_of(&t, &ms),
                })
            }
            TreeKey::Imported | TreeKey::Rig | TreeKey::Session => None,
        }
    }

    /// What dropping row `dragged` on row `onto` opens, or why the two do not combine.
    pub fn drop_math_plan(&self, dragged: TreeKey, onto: TreeKey) -> Result<DropPlan, String> {
        let (Some(mut a), Some(mut b)) = (self.drop_side(onto), self.drop_side(dragged)) else {
            return Err("that row is gone".into());
        };
        fn view(s: &Side) -> math_drop::DropSide<'_> {
            math_drop::DropSide {
                name: &s.name,
                domain: s.domain,
                curve: s.curve,
            }
        }
        if math_drop::swaps(&view(&a), &view(&b)) {
            std::mem::swap(&mut a, &mut b);
        }
        let op = math_drop::plan(&view(&a), &view(&b))?;
        let (Some(ao), Some(bo)) = (a.operand, b.operand) else {
            return Err("that row is gone".into());
        };
        Ok(DropPlan {
            a: ao,
            op,
            b: bo,
            owner: a.group,
            label: math_drop::label(&a.name, op, &b.name, b.curve == DropCurve::Eq),
        })
    }

    /// Row `dragged` dropped on row `onto`: the math dialog, prefilled; a toast says why
    /// when the two do not combine.
    pub(super) fn drop_math(&mut self, dragged: TreeKey, onto: TreeKey) {
        let p = match self.drop_math_plan(dragged, onto) {
            Ok(p) => p,
            Err(why) => {
                self.warn(why);
                return;
            }
        };
        let candidates = self.math_candidates(Some(p.owner));
        match Form::math_drop(candidates, p.a, p.op, p.b, p.owner) {
            Some(f) => self.overlay = Overlay::Form(Box::new(f)),
            None => self.warn(format!(
                "{}: not offered by the math dialog",
                p.label.trim_start_matches("math channel: ")
            )),
        }
    }
}
