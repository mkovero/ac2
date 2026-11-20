//! The Dynamics panes: the step tables and envelopes held for them, the curves each pane
//! draws, and the keys that change what a pane picks. Every number the pane shows is made
//! by `ac2_scene::dynamics` from these.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ac2_proto::model::layout::{DynamicsMetric, EntryRef};
use ac2_proto::model::{DynamicsEnvelope, DynamicsTable, TraceKind, TraceMeta, TraceOwner};
use ac2_proto::units::{MeasId, TraceId};
use ac2_scene::dynamics::{self as dynamics, CurveId, DynCurve};

use crate::state::*;

/// One step of one input of one trace: what `dynamics.envelope` is asked for.
pub type EnvelopeKey = (TraceId, u16, u32);

/// What this app holds of a rig's dynamics traces.
#[derive(Clone, Debug, Default)]
pub struct DynamicsUi {
    /// Each dynamics trace's step table (`dynamics.table`), fetched as the trace is listed.
    pub tables: BTreeMap<TraceId, Arc<DynamicsTable>>,
    /// The envelopes fetched, for the steps the panes have picked.
    pub envelopes: BTreeMap<EnvelopeKey, Arc<DynamicsEnvelope>>,
    /// Envelopes asked for and not answered (or refused: not asked again).
    pub(super) asked: BTreeSet<EnvelopeKey>,
}

impl DynamicsUi {
    /// Forgets what belongs to traces no longer listed.
    pub(super) fn keep_only(&mut self, listed: &BTreeSet<TraceId>) {
        self.tables.retain(|t, _| listed.contains(t));
        self.envelopes.retain(|(t, _, _), _| listed.contains(t));
        self.asked.retain(|(t, _, _)| listed.contains(t));
    }
}

/// A curve of a Dynamics pane as the app holds it, before it is lent to the scene.
#[derive(Clone, Debug)]
pub struct HeldCurve {
    pub id: CurveId,
    pub name: String,
    pub table: Arc<DynamicsTable>,
}

impl AppState {
    /// The dynamics traces listed, oldest first.
    fn dynamics_traces(&self) -> Vec<&TraceMeta> {
        let mut v: Vec<&TraceMeta> = self
            .daemon()
            .map(|s| {
                s.traces
                    .iter()
                    .filter(|t| t.kind == TraceKind::Dynamics)
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by_key(|t| t.id);
        v
    }

    /// The curves Dynamics pane `pane` draws: every measured input of every dynamics run of
    /// its entries (its measurement's runs, then each entry behind it), in that order. A
    /// run whose table has not arrived yet is not drawn until it has.
    pub fn dynamics_curves(&self, pane: PaneId) -> Vec<HeldCurve> {
        let Some(v) = self
            .layout
            .view(pane)
            .filter(|v| self.ours(v) && v.kind == PaneKind::Dynamics)
        else {
            return Vec::new();
        };
        let ms = self.measurements();
        let runs = self.dynamics_traces();
        let of_meas = |meas: MeasId| -> Vec<&TraceMeta> {
            let mine = TraceOwner::Meas { meas };
            runs.iter()
                .copied()
                .filter(|t| ac2_scene::meas_list::group_of(t, &ms) == mine)
                .collect()
        };
        let mut picked: Vec<&TraceMeta> = Vec::new();
        let lead = self.pane_meas(pane).map(|m| m.id).or(v.meas);
        let entries = lead
            .map(|meas| EntryRef::Group { meas })
            .into_iter()
            .chain(self.compared_into(pane));
        for e in entries {
            let ts = match e {
                EntryRef::Group { meas } => of_meas(meas),
                EntryRef::Trace { trace } => {
                    runs.iter().copied().filter(|t| t.id == trace).collect()
                }
                EntryRef::Live { .. } => Vec::new(),
            };
            for t in ts {
                if !picked.iter().any(|p| p.id == t.id) {
                    picked.push(t);
                }
            }
        }
        let owners: BTreeSet<TraceOwner> = picked
            .iter()
            .map(|t| ac2_scene::meas_list::group_of(t, &ms))
            .collect();
        let several = owners.len() > 1;
        let mut out = Vec::new();
        for t in picked {
            let Some(table) = self.rig.dynamics.tables.get(&t.id) else {
                continue;
            };
            let meas = match ac2_scene::meas_list::group_of(t, &ms) {
                TraceOwner::Meas { meas } => self.meas(meas).map(|m| m.config.name.as_str()),
                TraceOwner::Imported => None,
            };
            for (id, input) in dynamics::curve_ids(None, t.id, table)
                .into_iter()
                .zip(&table.inputs)
            {
                out.push(HeldCurve {
                    id,
                    name: dynamics::curve_name(&input.name, &t.edit.name, meas.filter(|_| several)),
                    table: table.clone(),
                });
            }
        }
        out
    }

    /// The curves of `pane` lent to the scene, each with its fetched envelope of the step
    /// it shows.
    pub fn dynamics_frame_curves<'a>(
        &'a self,
        held: &'a [HeldCurve],
        modes: &dynamics::DynamicsModes,
        theme: &ac2_scene::theme::Theme,
    ) -> Vec<DynCurve<'a>> {
        let mut curves: Vec<DynCurve<'a>> = held
            .iter()
            .enumerate()
            .map(|(i, h)| DynCurve {
                id: h.id,
                name: h.name.clone(),
                color: theme.trace_color(i),
                table: &h.table,
                envelope: None,
            })
            .collect();
        let steps = dynamics::curve_steps(&dynamics::DynamicsFrame {
            curves: &curves,
            modes: *modes,
            pointer: None,
            missing: "",
        });
        for (c, s) in curves.iter_mut().zip(steps) {
            c.envelope = s
                .and_then(|s| {
                    self.rig
                        .dynamics
                        .envelopes
                        .get(&(c.id.trace, c.id.input, s))
                })
                .map(Arc::as_ref);
        }
        curves
    }

    /// Asks for the envelopes the Envelopes views on screen show and this app has not got.
    pub(super) fn want_dynamics_envelopes(&mut self, out: &mut Vec<Request>) {
        let panes: Vec<PaneId> = self
            .layout
            .views
            .iter()
            .filter(|(_, v)| {
                self.ours(v)
                    && v.kind == PaneKind::Dynamics
                    && v.modes.dynamics.view == dynamics::DynamicsView::Envelopes
            })
            .map(|(p, _)| *p)
            .collect();
        let mut want: Vec<EnvelopeKey> = Vec::new();
        for p in panes {
            let held = self.dynamics_curves(p);
            let Some(modes) = self.layout.view(p).map(|v| v.modes.dynamics) else {
                continue;
            };
            let curves: Vec<DynCurve<'_>> = held
                .iter()
                .map(|h| DynCurve {
                    id: h.id,
                    name: String::new(),
                    color: ac2_scene::Color::TRANSPARENT,
                    table: &h.table,
                    envelope: None,
                })
                .collect();
            let steps = dynamics::curve_steps(&dynamics::DynamicsFrame {
                curves: &curves,
                modes,
                pointer: None,
                missing: "",
            });
            for (c, s) in curves.iter().zip(steps) {
                if let Some(s) = s {
                    want.push((c.id.trace, c.id.input, s));
                }
            }
        }
        for k in want {
            let d = &mut self.rig.dynamics;
            if d.envelopes.contains_key(&k) || !d.asked.insert(k) {
                continue;
            }
            out.push(Request::DynamicsEnvelope {
                trace: k.0,
                input: k.1,
                step: k.2,
            });
        }
    }

    /// A step table arrived.
    pub(super) fn dynamics_table_arrived(&mut self, trace: TraceId, t: DynamicsTable) {
        let d = &mut self.rig.dynamics;
        // A trace edited (renamed, its inputs renamed) is the same run: its envelopes stay.
        d.tables.insert(trace, Arc::new(t));
    }

    /// An envelope arrived, or why not: a refusal is kept as asked, so it is not asked
    /// again for every frame; the pane draws that curve without it.
    pub(super) fn dynamics_envelope_arrived(
        &mut self,
        key: EnvelopeKey,
        result: Result<Box<DynamicsEnvelope>, String>,
    ) {
        match result {
            Ok(e) => {
                self.rig.dynamics.envelopes.insert(key, Arc::from(e));
            }
            Err(msg) => tracing::debug!("dynamics.envelope {key:?}: {msg}"),
        }
    }

    /// The focused Dynamics pane's curve A, its index among `held`.
    fn focused_dynamics(&self) -> Option<(Vec<HeldCurve>, Option<usize>, Option<usize>)> {
        let pane = self.layout.focus;
        if self.layout.kind(pane) != PaneKind::Dynamics {
            return None;
        }
        let held = self.dynamics_curves(pane);
        let ids: Vec<CurveId> = held.iter().map(|h| h.id).collect();
        let (a, b) = dynamics::resolve(&self.layout.focused().modes.dynamics, &ids);
        Some((held, a, b))
    }

    /// Z / X (with Shift): the picked step moves among curve A's levels and lengths.
    pub(super) fn dynamics_step(&mut self, levels: i32, lengths: i32) {
        let Some((held, Some(a), _)) = self.focused_dynamics() else {
            self.warn("Dynamics: no run on the pane yet");
            return;
        };
        let info = &held[a].table.info;
        let m = &mut self.modes_mut().dynamics;
        if let Some(p) = dynamics::pick(m, info) {
            m.step = Some(dynamics::step_pick(info, p, levels, lengths));
        }
    }

    /// K / Shift+K: the metric a Steps cell shows, in the table's column order.
    pub(super) fn dynamics_metric(&mut self, forward: bool) {
        let all = DynamicsMetric::ALL;
        let m = &mut self.modes_mut().dynamics;
        let i = all.iter().position(|x| *x == m.metric).unwrap_or(0);
        let n = all.len();
        m.metric = all[if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        }];
    }

    /// B / Shift+B: curve A, or B, becomes the next curve of the pane (B passes through
    /// none).
    pub(super) fn dynamics_cycle(&mut self, is_b: bool) {
        let Some((held, a, b)) = self.focused_dynamics() else {
            return;
        };
        let ids: Vec<CurveId> = held.iter().map(|h| h.id).collect();
        let m = &mut self.modes_mut().dynamics;
        if is_b {
            m.b = dynamics::cycle(m.b, b, &ids, true);
        } else {
            m.a = dynamics::cycle(m.a, a, &ids, false);
        }
    }

    /// D: Envelopes draw THD or gain reduction.
    pub(super) fn dynamics_toggle_thd(&mut self) {
        let m = &mut self.modes_mut().dynamics;
        m.thd = !m.thd;
    }

    /// E: Release draws every level of A, or the picked level of every curve.
    pub(super) fn dynamics_toggle_all_levels(&mut self) {
        let m = &mut self.modes_mut().dynamics;
        m.all_levels = !m.all_levels;
    }

    /// A Steps cell clicked: its step is picked; double-clicked, its envelopes shown.
    pub(super) fn dynamics_pick(
        &mut self,
        pane: PaneId,
        step: dynamics::DynamicsStepPick,
        envelopes: bool,
    ) {
        self.layout.set_focus(pane);
        if let Some(v) = self.layout.view_mut(pane) {
            v.modes.dynamics.step = Some(step);
            if envelopes {
                v.modes.dynamics.view = dynamics::DynamicsView::Envelopes;
            }
        }
    }
}
