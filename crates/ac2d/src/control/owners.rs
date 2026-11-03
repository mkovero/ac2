//! Who owns what (`docs/design/measurement-tree.md`): every stored trace and every math
//! channel is listed under a measurement or in the imported group. The daemon keeps that
//! true — an owner always exists and is never a math channel — and `meas.delete` says what
//! becomes of what a measurement owns.

use ac2_proto::event::{Change, Patch};
use ac2_proto::model::layout::{EntryRef, PaneContent, RigRef};
use ac2_proto::model::{
    MeasConfig, MeasKind, Measurement, Operand, OwnedTraces, TraceOwner, TraceSource,
};
use ac2_proto::units::{MeasId, TraceId, WalkId};
use ac2_proto::{ErrorCode, ProtoError, ReplyBody};

use super::Control;
use crate::util::perr;

/// Whether `new` is `old` with only a math channel's owner changed.
pub(super) fn owner_only(old: &MeasConfig, new: &MeasConfig) -> bool {
    match (&old.kind, &new.kind) {
        (MeasKind::Math { config: a }, MeasKind::Math { config: b }) => {
            let mut b = b.clone();
            b.owner = a.owner;
            old.name == new.name && *a == b
        }
        _ => false,
    }
}

impl Control {
    /// Refuses an owner that is not an existing measurement other than a math channel.
    pub(super) fn check_owner(&self, owner: TraceOwner) -> Result<(), ProtoError> {
        let Some(meas) = owner.meas() else {
            return Ok(());
        };
        let m = self
            .store
            .state()
            .measurements
            .iter()
            .find(|m| m.id == meas);
        match m.map(|m| &m.config.kind) {
            None => Err(perr(
                ErrorCode::NotFound,
                format!("no measurement {meas} to file it under"),
            )),
            Some(MeasKind::Math { .. }) => Err(perr(
                ErrorCode::Invalid,
                format!("measurement {meas} is a math channel: math owns no traces"),
            )),
            Some(_) => Ok(()),
        }
    }

    /// A math channel's owner must be a measurement that can own it; and a measurement that
    /// owns traces or math channels cannot become a math channel (math owns nothing).
    pub(super) fn check_meas_owner(
        &self,
        own: Option<MeasId>,
        kind: &MeasKind,
    ) -> Result<(), ProtoError> {
        let MeasKind::Math { config } = kind else {
            return Ok(());
        };
        if own.is_some() && config.owner.meas() == own {
            return Err(perr(
                ErrorCode::Invalid,
                "a math channel is listed under a measurement, not under itself",
            ));
        }
        self.check_owner(config.owner)?;
        if let Some(own) = own {
            let (traces, maths) = self.owned(own);
            if !traces.is_empty() || !maths.is_empty() {
                return Err(perr(
                    ErrorCode::Refused,
                    format!(
                        "measurement {own} owns {} trace(s) and {} math channel(s): move them \
                         first (a math channel owns nothing)",
                        traces.len(),
                        maths.len()
                    ),
                ));
            }
        }
        Ok(())
    }

    /// The stored traces and math channels `meas` owns.
    pub(super) fn owned(&self, meas: MeasId) -> (Vec<TraceId>, Vec<MeasId>) {
        let owner = TraceOwner::Meas { meas };
        let st = self.store.state();
        let traces = st
            .traces
            .iter()
            .filter(|t| t.edit.owner == owner)
            .map(|t| t.id)
            .collect();
        let maths = st
            .measurements
            .iter()
            .filter(
                |m| matches!(&m.config.kind, MeasKind::Math { config } if config.owner == owner),
            )
            .map(|m| m.id)
            .collect();
        (traces, maths)
    }

    /// The number of sweep measurement `meas`'s next run: one more than the highest of its
    /// stored runs, wherever they are filed now.
    pub(super) fn next_run_number(&self, meas: MeasId) -> u32 {
        self.store
            .state()
            .traces
            .iter()
            .filter_map(|t| match &t.source {
                TraceSource::Sweep {
                    meas: m, number, ..
                } if *m == meas => Some(*number),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            + 1
    }

    /// `meas.delete`: the measurement goes; what it owns is kept under the imported group or
    /// deleted with it. Everything is checked first, so a refusal changes nothing.
    pub(super) fn meas_delete(
        &mut self,
        meas: MeasId,
        what: OwnedTraces,
    ) -> Result<ReplyBody, ProtoError> {
        let name = self.meas(meas)?.config.name.clone();
        if self.sweep.as_ref().is_some_and(|s| s.run.meas == meas) {
            return Err(perr(
                ErrorCode::Refused,
                format!("a run of {name} is playing; stop it first"),
            ));
        }
        let (traces, maths) = self.owned(meas);
        // Its EQ curves and predictions exist only with its sets: they always go.
        let (eq_traces, eq_maths) = self.eq_owned(meas);
        let traces: Vec<TraceId> = traces
            .into_iter()
            .filter(|t| !eq_traces.contains(t))
            .collect();
        let maths: Vec<MeasId> = maths
            .into_iter()
            .filter(|m| !eq_maths.contains(m))
            .collect();
        let (gone_traces, gone_maths): (Vec<TraceId>, Vec<MeasId>) = match what {
            OwnedTraces::Keep => (eq_traces.clone(), eq_maths.clone()),
            OwnedTraces::Delete => (
                traces.iter().chain(&eq_traces).copied().collect(),
                maths.iter().chain(&eq_maths).copied().collect(),
            ),
        };
        self.check_walk_meas_delete(meas, &gone_traces)?;
        for &m in &gone_maths {
            self.check_walk_meas_delete(m, &[])?;
        }
        // A math channel that stays must not lose an operand.
        let staying = self.store.state().measurements.iter().filter(|m| {
            m.id != meas
                && !gone_maths.contains(&m.id)
                && matches!(m.config.kind, MeasKind::Math { .. })
        });
        for m in staying {
            let MeasKind::Math { config } = &m.config.kind else {
                continue;
            };
            let lost = config.expr.operands().into_iter().find(|o| match o {
                Operand::Meas { meas: x } => *x == meas,
                Operand::Trace { trace } => gone_traces.contains(trace),
                Operand::Eq { meas: x, .. } => *x == meas,
            });
            if let Some(o) = lost {
                let what = match o {
                    Operand::Meas { .. } => name.clone(),
                    Operand::Trace { .. } | Operand::Eq { .. } => self.operand_name(o),
                };
                return Err(perr(
                    ErrorCode::Refused,
                    format!(
                        "{what} is an operand of the math channel {}; edit it there first",
                        m.config.name
                    ),
                ));
            }
        }

        for id in &eq_maths {
            self.remove_meas(*id);
        }
        for id in &eq_traces {
            self.traces.remove(*id);
            self.commit(Change::Trace(Patch::Deleted(*id)));
        }
        match what {
            OwnedTraces::Keep => {
                for id in &traces {
                    if let Some(mut t) = self
                        .store
                        .state()
                        .traces
                        .iter()
                        .find(|t| t.id == *id)
                        .cloned()
                    {
                        t.edit.owner = TraceOwner::Imported;
                        self.commit(Change::Trace(Patch::Set(t)));
                    }
                }
                for id in &maths {
                    if let Some(mut m) = self
                        .store
                        .state()
                        .measurements
                        .iter()
                        .find(|m| m.id == *id)
                        .cloned()
                        && let MeasKind::Math { config } = &mut m.config.kind
                    {
                        config.owner = TraceOwner::Imported;
                        self.commit(Change::Measurement(Patch::Set(m)));
                    }
                }
            }
            OwnedTraces::Delete => {
                // The math channels first: they may compute from the traces.
                for id in &maths {
                    self.remove_meas(*id);
                }
                for id in &traces {
                    self.traces.remove(*id);
                    self.commit(Change::Trace(Patch::Deleted(*id)));
                }
            }
        }
        tracing::info!(
            "measurement {meas} deleted; {} trace(s) and {} math channel(s) {}",
            traces.len(),
            maths.len(),
            match what {
                OwnedTraces::Keep => "moved to the imported group",
                OwnedTraces::Delete => "deleted with it",
            }
        );
        let rev = self.remove_meas(meas);
        let rev = self.prune_layouts().unwrap_or(rev);
        Ok(ReplyBody::Ack { rev })
    }

    /// The measurement whose group draws `what` of this rig, if a group draws it: a live
    /// curve its own measurement's (a math channel's, its owner's), a stored trace the
    /// measurement it is filed under.
    pub(super) fn drawn_by_group(&self, what: EntryRef) -> Option<MeasId> {
        let st = self.store.state();
        match what {
            EntryRef::Group { .. } => None,
            EntryRef::Live { meas } => {
                let m = st.measurements.iter().find(|m| m.id == meas)?;
                match &m.config.kind {
                    MeasKind::Math { config } => config.owner.meas(),
                    _ => Some(meas),
                }
            }
            EntryRef::Trace { trace } => {
                st.traces.iter().find(|t| t.id == trace)?.edit.owner.meas()
            }
        }
    }

    /// Drops from every stored layout (rig meters' too) the references of this rig to
    /// measurements, traces and walks that are gone; a graph pane left with no entry, or a
    /// meter or walk pane whose source went, holds nothing. Never against a client's view:
    /// another rig's references are that rig's to prune. The rev of the last change, if any.
    pub(super) fn prune_layouts(&mut self) -> Option<ac2_proto::units::Rev> {
        let st = self.store.state();
        let meas = |id: MeasId| st.measurements.iter().any(|o| o.id == id);
        let trace = |id: TraceId| st.traces.iter().any(|t| t.id == id);
        let walk = |id: WalkId| st.walks.iter().any(|w| w.id == id);
        let changed: Vec<Measurement> = st
            .measurements
            .iter()
            .filter_map(|m| {
                let mut l = m.layout.clone()?;
                let mut pruned = false;
                for c in l.contents_mut() {
                    let gone = match c {
                        PaneContent::Curves { entries } => {
                            let before = entries.len();
                            entries.retain(|e| {
                                e.rig != RigRef::ThisRig
                                    || match e.what {
                                        EntryRef::Group { meas: id }
                                        | EntryRef::Live { meas: id } => meas(id),
                                        EntryRef::Trace { trace: id } => trace(id),
                                    }
                            });
                            pruned |= entries.len() != before;
                            entries.is_empty()
                        }
                        PaneContent::Meter {
                            rig: RigRef::ThisRig,
                            meas: id,
                        } => !meas(*id),
                        PaneContent::Walk {
                            rig: RigRef::ThisRig,
                            walk: id,
                        } => !walk(*id),
                        _ => false,
                    };
                    if gone {
                        *c = PaneContent::Nothing;
                        pruned = true;
                    }
                }
                pruned.then(|| {
                    let mut m = m.clone();
                    m.layout = Some(l);
                    m
                })
            })
            .collect();
        let mut rev = None;
        for m in changed {
            rev = Some(self.commit(Change::Measurement(Patch::Set(m))));
        }
        rev
    }

    /// Stops measurement `id`'s job, drops what it published and its log, and deletes it.
    pub(super) fn remove_meas(&mut self, id: MeasId) -> ac2_proto::units::Rev {
        self.stop_job(id);
        self.s
            .outbox
            .clear(&ac2_proto::Subscription::Meas(id).prefix());
        self.drop_spl_log(id);
        self.commit(Change::Measurement(Patch::Deleted(id)))
    }
}
