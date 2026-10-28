//! EQ what-if sets (`docs/design/eq-overlay.md` §2): `meas.eq`, the locked filter curve each
//! set keeps, the predictions (`curve × set` math channels) and the `eq_in` a capture
//! records, so a curve that already contains the set is never filtered by it twice.

use ac2_proto::event::{Change, Patch};
use ac2_proto::model::{
    CalState, EqBody, EqEdit, EqRealisation, EqSet, EqSnapshot, LogGridSpec, MathConfig,
    MathDomain, MathExpr, MathOp, MeasConfig, MeasKind, MeasScope, Measurement, Operand, PeakQ,
    TraceKind, TraceMeta, TraceOwner, TraceSource,
};
use ac2_proto::units::{ClientId, EqSetId, Hz, MeasId, Seconds, TraceId, WallNs};
use ac2_proto::{Command, ErrorCode, GridDef, ProtoError, ReplyBody};
use ac2_traces::Columns;
use ac2_traces::eq;
use ac2_traces::math::trace_on_grid;
use ac2_traces::meta;
use num_complex::Complex64;

use super::{Control, static_grid};
use crate::jobs::math::Held;
use crate::util::{perr, wall_ns};

/// The rate a new digital set assumes without a session: the common processor rate.
const DEFAULT_RATE: f64 = 48_000.0;
/// Points per octave of a sweep measurement's EQ curve (a transfer's is its own grid).
const SWEEP_EQ_PPO: u32 = 48;

fn invalid(m: impl Into<String>) -> ProtoError {
    perr(ErrorCode::Invalid, m.into())
}

/// Where a curve after the set is evaluated for display: a transfer's own grid, else ten
/// octaves at a fine resolution.
fn eq_grid(kind: &MeasKind) -> GridDef {
    static_grid(kind).unwrap_or_else(|| {
        let g = LogGridSpec::ten_octaves(SWEEP_EQ_PPO);
        GridDef::Log {
            ppo: g.ppo,
            k_min: g.k_min,
            k_max: g.k_max,
        }
    })
}

fn can_hold_eq(kind: &MeasKind) -> bool {
    matches!(kind, MeasKind::Transfer { .. } | MeasKind::Sweep { .. })
}

/// The math channel predicting `curve` after set `set` of `meas`.
fn prediction_expr(meas: MeasId, set: EqSetId, curve: Operand) -> MathExpr {
    MathExpr::Binary {
        a: curve,
        op: MathOp::Multiply,
        b: Operand::Eq { meas, set },
    }
}

/// The curve a prediction filters, if `config` is one of set `set`.
fn predicted_curve(config: &MathConfig, set: Operand) -> Option<Operand> {
    match config.expr {
        MathExpr::Binary {
            a,
            op: MathOp::Multiply,
            b,
        } if b == set => Some(a),
        _ => None,
    }
}

impl Control {
    fn eq_meas(&self, meas: MeasId) -> Result<Measurement, ProtoError> {
        let m = self.meas(meas)?.clone();
        if !can_hold_eq(&m.config.kind) {
            return Err(invalid(format!(
                "{} is not a transfer or sweep measurement: EQ predicts transfer functions",
                m.config.name
            )));
        }
        Ok(m)
    }

    fn eq_set(m: &Measurement, set: EqSetId) -> Result<&EqSet, ProtoError> {
        m.eq.iter().find(|s| s.id == set).ok_or_else(|| {
            perr(
                ErrorCode::NotFound,
                format!("{} has no EQ set {}", m.config.name, set.0),
            )
        })
    }

    /// A `response` section's trace on `grid`: a stored transfer function that is not
    /// itself an EQ curve (a set filtering by its own curve would be circular).
    fn eq_response(
        &self,
        id: TraceId,
        grid: &GridDef,
        freqs: &[f64],
    ) -> Result<Vec<Complex64>, String> {
        let t = self.stored(id).map_err(|e| e.msg)?;
        if matches!(t.meta.source, TraceSource::Eq { .. }) {
            return Err(format!(
                "{} is an EQ curve, not a response",
                t.meta.edit.name
            ));
        }
        if !matches!(t.meta.kind, TraceKind::Transfer | TraceKind::Sweep) {
            return Err(format!("{} is not a transfer function", t.meta.edit.name));
        }
        Ok(eq::complex(&trace_on_grid(&t, grid, freqs)))
    }

    fn eq_check(&self, body: &EqBody, grid: &GridDef) -> Result<(), ProtoError> {
        if body.name.trim().is_empty() {
            return Err(invalid("an EQ set needs a name"));
        }
        if body.sections.len() > EqSet::MAX_SECTIONS {
            return Err(invalid(format!(
                "an EQ set holds at most {} sections",
                EqSet::MAX_SECTIONS
            )));
        }
        if let EqRealisation::Digital { rate } = body.realisation
            && !(rate.0.is_finite() && rate.0 > 0.0)
        {
            return Err(invalid("the processor's sample rate must be positive"));
        }
        let freqs = grid.centres();
        let resp = |id| self.eq_response(id, grid, &freqs);
        eq::validate(body, &resp).map_err(|e| invalid(e.to_string()))
    }

    /// The set's filter curve on `grid`.
    fn eq_columns(&self, body: &EqBody, grid: &GridDef) -> Result<Columns, ProtoError> {
        let freqs = grid.centres();
        let resp = |id| self.eq_response(id, grid, &freqs);
        eq::response(body, &freqs, &resp)
            .map(|h| eq::columns(&h))
            .map_err(|e| invalid(e.to_string()))
    }

    /// Stores (or restores) set `set`'s filter curve as its locked trace, under its rev.
    fn eq_store_trace(&mut self, m: &Measurement, set: &EqSet) -> Result<(), ProtoError> {
        let grid = eq_grid(&m.config.kind);
        let columns = self.eq_columns(&set.body, &grid)?;
        let source = TraceSource::Eq {
            meas: m.id,
            set: set.id,
            rev: set.rev,
        };
        let existing = self
            .store
            .state()
            .traces
            .iter()
            .find(|t| t.id == set.trace)
            .cloned();
        match existing {
            Some(mut t) => {
                let gid = self.register_grid(grid.clone());
                self.traces.insert(t.id, grid, columns, None, None, None);
                t.source = source;
                t.grid_id = gid;
                t.edit.name.clone_from(&set.body.name);
                self.commit(Change::Trace(Patch::Set(t)));
            }
            None => {
                let mut edit = meta::new_edit(
                    set.trace,
                    set.body.name.clone(),
                    None,
                    TraceOwner::Meas { meas: m.id },
                );
                // The daemon keeps it in step with the set; no one edits its curve.
                edit.locked = true;
                let t = TraceMeta {
                    id: set.trace,
                    edit,
                    kind: TraceKind::Transfer,
                    source,
                    grid_id: grid.id(),
                    delay: Seconds(0.0),
                    depth: None,
                    cal: CalState::Uncalibrated,
                    mic: None,
                    mic_curve: None,
                    created_at: WallNs(wall_ns()),
                    eq_in: Vec::new(),
                    position: None,
                    captured_unsettled: false,
                    superseded: false,
                };
                self.add_trace(t, grid, columns, None, None, None);
            }
        }
        Ok(())
    }

    fn next_eq_id(&self) -> EqSetId {
        let max = self
            .store
            .state()
            .measurements
            .iter()
            .flat_map(|m| m.eq.iter().map(|s| s.id.0))
            .max()
            .unwrap_or(0);
        EqSetId(max + 1)
    }

    fn free_eq_name(m: &Measurement) -> String {
        (1..)
            .map(|n| format!("EQ {n}"))
            .find(|n| !m.eq.iter().any(|s| s.body.name == *n))
            .unwrap_or_default()
    }

    fn session_rate(&self) -> f64 {
        self.session
            .as_ref()
            .map_or(DEFAULT_RATE, |rt| f64::from(rt.sample_rate))
    }

    /// The prediction math channels of set `set` of `meas`, with the curve each filters.
    fn predictions(&self, meas: MeasId, set: EqSetId) -> Vec<(MeasId, Operand)> {
        let o = Operand::Eq { meas, set };
        self.maths_naming(o)
            .into_iter()
            .filter_map(|m| match &m.config.kind {
                MeasKind::Math { config } => predicted_curve(config, o).map(|c| (m.id, c)),
                _ => None,
            })
            .collect()
    }

    /// The EQ curves and prediction channels of `meas`: they exist only with its sets, so
    /// they go with it whatever happens to its other traces.
    pub(super) fn eq_owned(&self, meas: MeasId) -> (Vec<TraceId>, Vec<MeasId>) {
        let Ok(m) = self.meas(meas) else {
            return (Vec::new(), Vec::new());
        };
        let traces = m.eq.iter().map(|s| s.trace).collect();
        let maths =
            m.eq.iter()
                .flat_map(|s| self.predictions(meas, s.id))
                .map(|(id, _)| id)
                .collect();
        (traces, maths)
    }

    fn commit_meas(&mut self, m: &Measurement) {
        self.commit(Change::Measurement(Patch::Set(m.clone())));
    }

    /// `meas.eq`.
    pub(super) fn meas_eq(
        &mut self,
        client: &ClientId,
        meas: MeasId,
        edit: EqEdit,
    ) -> Result<ReplyBody, ProtoError> {
        let mut m = self.eq_meas(meas)?;
        match edit {
            EqEdit::New {
                name,
                realisation,
                peak_q,
                sections,
            } => {
                if m.eq.len() >= EqSet::MAX_SETS {
                    return Err(perr(
                        ErrorCode::Refused,
                        format!(
                            "{} holds {} EQ sets already, the most it holds",
                            m.config.name,
                            EqSet::MAX_SETS
                        ),
                    ));
                }
                let body = EqBody {
                    name: name.unwrap_or_else(|| Self::free_eq_name(&m)),
                    realisation: realisation.unwrap_or(EqRealisation::Digital {
                        rate: Hz(self.session_rate()),
                    }),
                    peak_q: peak_q.unwrap_or(PeakQ::HalfGain),
                    enabled: true,
                    sections,
                };
                let id = self.add_eq(&mut m, body)?;
                if matches!(m.config.kind, MeasKind::Transfer { .. }) {
                    self.eq_predict(client, &m, id, Operand::Meas { meas }, true)?;
                }
            }
            EqEdit::Put { set, body } => {
                Self::eq_set(&m, set)?;
                self.eq_check(&body, &eq_grid(&m.config.kind))?;
                let s = m.eq.iter_mut().find(|s| s.id == set).expect("checked");
                s.body = body;
                s.rev += 1;
                let s = s.clone();
                self.eq_store_trace(&m, &s)?;
                self.commit_meas(&m);
                self.restart_maths_naming(Operand::Eq { meas, set });
            }
            EqEdit::Delete { set } => {
                let trace = Self::eq_set(&m, set)?.trace;
                for (id, _) in self.predictions(meas, set) {
                    self.remove_meas(id);
                }
                self.traces.remove(trace);
                self.commit(Change::Trace(Patch::Deleted(trace)));
                m.eq.retain(|s| s.id != set);
                self.commit_meas(&m);
            }
            EqEdit::Mark { set, clear } => {
                Self::eq_set(&m, set)?;
                let s = m.eq.iter_mut().find(|s| s.id == set).expect("checked");
                s.in_console = (!clear).then(|| s.snapshot());
                self.commit_meas(&m);
                self.restart_maths_naming(Operand::Eq { meas, set });
            }
            EqEdit::Predict { set, curve, on } => {
                Self::eq_set(&m, set)?;
                self.eq_predict(client, &m, set, curve, on)?;
            }
            EqEdit::Copy { set, to } => {
                let body = Self::eq_set(&m, set)?.body.clone();
                let mut target = self.eq_meas(to)?;
                if target.eq.len() >= EqSet::MAX_SETS {
                    return Err(perr(
                        ErrorCode::Refused,
                        format!(
                            "{} holds {} EQ sets already, the most it holds",
                            target.config.name,
                            EqSet::MAX_SETS
                        ),
                    ));
                }
                let id = self.add_eq(&mut target, body)?;
                if matches!(target.config.kind, MeasKind::Transfer { .. }) {
                    self.eq_predict(client, &target, id, Operand::Meas { meas: to }, true)?;
                }
            }
        }
        Ok(ReplyBody::Measurement(self.meas(meas)?.clone()))
    }

    /// Adds a set holding `body` to `m` (committed), with its curve.
    fn add_eq(&mut self, m: &mut Measurement, body: EqBody) -> Result<EqSetId, ProtoError> {
        self.eq_check(&body, &eq_grid(&m.config.kind))?;
        let set = EqSet {
            id: self.next_eq_id(),
            rev: 1,
            body,
            in_console: None,
            trace: self.traces.alloc(),
        };
        self.eq_store_trace(m, &set)?;
        let id = set.id;
        m.eq.push(set);
        self.commit_meas(m);
        Ok(id)
    }

    /// Shows (`on`) or stops showing `curve` of `m` after set `set`.
    fn eq_predict(
        &mut self,
        client: &ClientId,
        m: &Measurement,
        set: EqSetId,
        curve: Operand,
        on: bool,
    ) -> Result<(), ProtoError> {
        let existing = self
            .predictions(m.id, set)
            .into_iter()
            .find(|(_, c)| *c == curve)
            .map(|(id, _)| id);
        if !on {
            if let Some(id) = existing {
                self.remove_meas(id);
            }
            return Ok(());
        }
        if existing.is_some() {
            return Ok(());
        }
        let smoothing = match curve {
            Operand::Meas { meas } if meas == m.id => match &m.config.kind {
                MeasKind::Transfer { config } => config.smoothing,
                _ => {
                    return Err(invalid(format!(
                        "{} is a sweep measurement: predict one of its runs",
                        m.config.name
                    )));
                }
            },
            Operand::Trace { trace } => {
                let t = self.stored(trace)?.meta;
                let mine = t.edit.owner == TraceOwner::Meas { meas: m.id };
                if !mine
                    || matches!(t.source, TraceSource::Eq { .. })
                    || !matches!(t.kind, TraceKind::Transfer | TraceKind::Sweep)
                {
                    return Err(invalid(format!(
                        "{} is not a transfer function of {}",
                        t.edit.name, m.config.name
                    )));
                }
                t.edit.smoothing
            }
            _ => {
                return Err(invalid(format!(
                    "an EQ set of {} predicts that measurement or one of its stored transfer \
                     functions",
                    m.config.name
                )));
            }
        };
        let set_name = Self::eq_set(m, set)?.body.name.clone();
        let owner = TraceOwner::Meas { meas: m.id };
        let mut config = MathConfig::of(
            owner,
            MathDomain::Transfer,
            prediction_expr(m.id, set, curve),
        );
        config.smoothing = smoothing;
        let name = format!("{} after {set_name}", self.operand_name(curve));
        let reply = self.execute(
            client,
            Command::MeasCreate {
                config: MeasConfig {
                    name,
                    kind: MeasKind::Math { config },
                    scope: MeasScope::Session,
                },
            },
        )?;
        if let ReplyBody::Measurement(p) = reply {
            self.execute(client, Command::MeasStart { meas: p.id })?;
        }
        Ok(())
    }

    /// What `curve` already contains of set `set`: the console mark for the live
    /// measurement, the snapshot a stored trace recorded.
    fn eq_in_of(&self, set: &EqSet, curve: Operand) -> Option<EqSnapshot> {
        match curve {
            Operand::Meas { .. } => set.in_console.clone(),
            Operand::Trace { trace } => self
                .store
                .state()
                .traces
                .iter()
                .find(|t| t.id == trace)
                .and_then(|t| t.eq_in.iter().find(|s| s.set == set.id).cloned()),
            Operand::Eq { .. } => None,
        }
    }

    /// The held operand `Operand::Eq` of prediction `c` stands for on `grid`: the factor its
    /// curve is multiplied by. A filter is exact and shares no arrival with anything.
    pub(super) fn eq_held(
        &self,
        meas: MeasId,
        set: EqSetId,
        c: &MathConfig,
        grid: &GridDef,
    ) -> Result<Held, ProtoError> {
        let m = self.meas(meas)?;
        let s = Self::eq_set(m, set)?;
        let curve = c
            .expr
            .operands()
            .first()
            .copied()
            .ok_or_else(|| invalid("a prediction needs a curve"))?;
        let eq_in = self.eq_in_of(s, curve);
        let freqs = grid.centres();
        let resp = |id| self.eq_response(id, grid, &freqs);
        let (_, h) =
            eq::prediction(s, eq_in.as_ref(), &freqs, &resp).map_err(|e| invalid(e.to_string()))?;
        Ok(Held {
            columns: eq::columns(&h),
            delay: 0.0,
            time_base: None,
            scale: None,
            level: None,
            mic_curve: false,
        })
    }

    /// What a capture of `meas`'s live curve records: every set marked in the console.
    pub(super) fn eq_in_live(&self, meas: MeasId) -> Vec<EqSnapshot> {
        self.meas(meas)
            .map(|m| m.eq.iter().filter_map(|s| s.in_console.clone()).collect())
            .unwrap_or_default()
    }

    /// What a capture of math channel `c` records: a prediction's curve contains its set
    /// as it is now (plus whatever its curve recorded of the other sets).
    pub(super) fn eq_in_math(&self, c: &MathConfig) -> Vec<EqSnapshot> {
        let MathExpr::Binary {
            a,
            op: MathOp::Multiply,
            b: Operand::Eq { meas, set },
        } = c.expr
        else {
            return Vec::new();
        };
        let Ok(m) = self.meas(meas) else {
            return Vec::new();
        };
        let mut out: Vec<EqSnapshot> =
            m.eq.iter()
                .filter(|s| s.id != set)
                .filter_map(|s| self.eq_in_of(s, a))
                .collect();
        if let Ok(s) = Self::eq_set(m, set) {
            out.push(s.snapshot());
        }
        out
    }

    /// Refuses turning `meas` into a kind that holds no EQ while it holds sets.
    pub(super) fn check_eq_update(&self, meas: MeasId, new: &MeasKind) -> Result<(), ProtoError> {
        let m = self.meas(meas)?;
        if m.eq.is_empty() || can_hold_eq(new) {
            return Ok(());
        }
        Err(perr(
            ErrorCode::Refused,
            format!(
                "{} holds EQ sets: it stays a transfer or sweep measurement; delete them first",
                m.config.name
            ),
        ))
    }

    /// Recomputes the curves of `meas`'s sets (its grid changed, or a session was loaded);
    /// a set that no longer evaluates keeps its old curve and says why in the log.
    pub(super) fn eq_refresh(&mut self, meas: MeasId) {
        let Ok(m) = self.meas(meas).cloned() else {
            return;
        };
        for s in &m.eq {
            if let Err(e) = self.eq_store_trace(&m, s) {
                tracing::warn!(
                    "EQ set {} of {} not recomputed: {}",
                    s.id.0,
                    m.config.name,
                    e.msg
                );
            }
        }
    }

    /// Refuses deleting a trace that is an EQ curve or a set's `response` section.
    pub(super) fn check_eq_trace_delete(&self, trace: TraceId) -> Result<(), ProtoError> {
        let st = self.store.state();
        if let Some(t) = st.traces.iter().find(|t| t.id == trace)
            && let TraceSource::Eq { set, .. } = t.source
        {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "{} is EQ set {}'s curve; delete the set instead",
                    t.edit.name, set.0
                ),
            ));
        }
        for m in &st.measurements {
            for s in &m.eq {
                let named = s.body.sections.iter().any(|x| {
                    matches!(x.shape, ac2_proto::model::EqShape::Response { trace: t } if t == trace)
                });
                if named {
                    return Err(perr(
                        ErrorCode::Refused,
                        format!(
                            "trace {trace} is a response section of {} on {}; remove it there \
                             first",
                            s.body.name, m.config.name
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}
