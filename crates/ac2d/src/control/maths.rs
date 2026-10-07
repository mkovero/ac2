//! Math channels: their configuration checks, their operands' invariants, and the stored
//! operands' columns they start with (`docs/design/math-channels.md`).
//!
//! A math channel names its operands by id. That holds for as long as it exists: a live
//! operand cannot be deleted, turned into another kind or moved to another grid, and a
//! stored one cannot be deleted, while a math channel names it — the channel would silently
//! stop meaning what its name says. The operator edits the channel first.

use std::sync::Arc;

use ac2_proto::grid::{BinColumns, GridDef};
use ac2_proto::model::{
    AverageMethod, MathConfig, MathDomain, MathExpr, MathOp, MathReference, MeasKind, Measurement,
    Operand, TraceKind, TraceMeta,
};
use ac2_proto::units::{MeasId, SessionEpoch, TraceId};
use ac2_proto::{ErrorCode, ProtoError};
use ac2_traces::columns::frequencies;
use ac2_traces::math::trace_on_grid;

use super::{Control, static_grid};
use crate::jobs::math::{Grids, Held, needs_shared_time_base};
use crate::util::perr;

/// A stored operand's columns, or `None` for a live one.
type StoredOperand = Option<Arc<Held>>;

fn invalid(m: impl Into<String>) -> ProtoError {
    perr(ErrorCode::Invalid, m.into())
}

/// Checks of a math channel's configuration that need no other entity.
pub(super) fn validate(c: &MathConfig) -> Result<(), ProtoError> {
    let operands = c.expr.operands();
    match &c.expr {
        MathExpr::Binary { a, op, b } => {
            if a == b {
                return Err(invalid("A and B are the same operand"));
            }
            if c.domain != MathDomain::Transfer && matches!(op, MathOp::Divide | MathOp::Multiply) {
                return Err(invalid(
                    "spectra and RTA bands are levels: they take − (level difference) and + \
                     (power sum); ÷ and × are for transfer functions",
                ));
            }
        }
        MathExpr::Average { of, method } => {
            if !(MathConfig::MIN_AVERAGE..=MathConfig::MAX_AVERAGE).contains(&of.len()) {
                return Err(invalid(format!(
                    "an average has {} … {} operands",
                    MathConfig::MIN_AVERAGE,
                    MathConfig::MAX_AVERAGE
                )));
            }
            for (i, o) in of.iter().enumerate() {
                if of[..i].contains(o) {
                    return Err(invalid(format!("{} is listed twice", operand_word(*o))));
                }
            }
            if c.domain != MathDomain::Transfer && *method != AverageMethod::Power {
                return Err(invalid("spectra and RTA bands average on power only"));
            }
        }
    }
    match c.reference {
        MathReference::Operand { operand } if !operands.contains(&operand) => {
            return Err(invalid(format!(
                "the phase reference, {}, is not an operand",
                operand_word(operand)
            )));
        }
        MathReference::Fixed { delay } if !delay.0.is_finite() => {
            return Err(invalid("the reference delay must be finite"));
        }
        _ => {}
    }
    if c.smoothing.is_some() && c.domain == MathDomain::Rta {
        return Err(invalid(
            "RTA bands are fractional-octave already: no smoothing",
        ));
    }
    Ok(())
}

/// `measurement 3`, `trace 7`.
fn operand_word(o: Operand) -> String {
    match o {
        Operand::Meas { meas } => format!("measurement {meas}"),
        Operand::Trace { trace } => format!("trace {trace}"),
    }
}

/// The domain a stored trace of `kind` belongs to, if any.
fn trace_domain(kind: TraceKind) -> Option<MathDomain> {
    match kind {
        TraceKind::Transfer | TraceKind::Sweep | TraceKind::Target => Some(MathDomain::Transfer),
        TraceKind::Spectrum { .. } => Some(MathDomain::Spectrum),
        TraceKind::Rta { .. } => Some(MathDomain::Rta),
    }
}

fn domain_word(d: MathDomain) -> &'static str {
    match d {
        MathDomain::Transfer => "transfer functions",
        MathDomain::Spectrum => "spectra",
        MathDomain::Rta => "RTA bands",
    }
}

/// What the math channel's operands are, looked up in the state.
pub(super) struct Lookup<'a> {
    pub(super) meas: &'a dyn Fn(MeasId) -> Option<(&'a str, &'a MeasKind)>,
    pub(super) trace: &'a dyn Fn(TraceId) -> Option<(&'a TraceMeta, GridDef)>,
    /// The current session epoch, the time base of every live operand; `None` skips the
    /// time-base checks (a session being loaded starts a new epoch: its math channels say
    /// per frame which operands share it).
    pub(super) epoch: Option<SessionEpoch>,
}

/// Checks math channel `own`'s operands (`None` while it is being created) against the
/// state and returns the grid its result has without a session (a transfer result's), if
/// one is known yet.
pub(super) fn check_operands(
    own: Option<MeasId>,
    c: &MathConfig,
    l: &Lookup<'_>,
) -> Result<Option<GridDef>, ProtoError> {
    let mut live_grid: Option<(GridDef, String)> = None;
    let mut stored_grid: Option<GridDef> = None;
    let mut time_bases: Vec<(Option<SessionEpoch>, String)> = Vec::new();
    let want = domain_word(c.domain);
    let ratio = matches!(
        c.expr,
        MathExpr::Binary {
            op: MathOp::Divide | MathOp::Multiply,
            ..
        }
    );
    for o in c.expr.operands() {
        match o {
            Operand::Meas { meas } => {
                if Some(meas) == own {
                    return Err(invalid("a math channel cannot be its own operand"));
                }
                let (name, kind) = (l.meas)(meas)
                    .ok_or_else(|| perr(ErrorCode::NotFound, format!("no measurement {meas}")))?;
                let ok = matches!(
                    (kind, c.domain),
                    (MeasKind::Transfer { .. }, MathDomain::Transfer)
                        | (MeasKind::Spectrum { .. }, MathDomain::Spectrum)
                        | (MeasKind::Rta { .. }, MathDomain::Rta)
                );
                if let MeasKind::Math { .. } = kind {
                    return Err(invalid(format!(
                        "{name} is a math channel: capture it (Ctrl+1…9, `ac2 trace capture`) \
                         and use the trace"
                    )));
                }
                if !ok {
                    return Err(invalid(format!(
                        "{name} is not one of the {want} this math channel combines"
                    )));
                }
                if let Some(g) = static_grid(kind) {
                    match &live_grid {
                        None => live_grid = Some((g, name.to_owned())),
                        Some((first, first_name)) if *first != g => {
                            return Err(invalid(format!(
                                "{name} and {first_name} have different grids; live transfer \
                                 operands share one"
                            )));
                        }
                        Some(_) => {}
                    }
                }
                time_bases.push((l.epoch, name.to_owned()));
            }
            Operand::Trace { trace } => {
                let (t, g) = (l.trace)(trace)
                    .ok_or_else(|| perr(ErrorCode::NotFound, format!("no trace {trace}")))?;
                let name = &t.edit.name;
                if trace_domain(t.kind) != Some(c.domain) {
                    return Err(invalid(format!(
                        "{name} is not one of the {want} this math channel combines"
                    )));
                }
                if t.kind == TraceKind::Target && !ratio {
                    return Err(invalid(format!(
                        "{name} is a target curve (magnitude only): compare with it by ÷"
                    )));
                }
                if c.domain != MathDomain::Transfer {
                    // Band powers and FFT bins do not interpolate.
                    match &stored_grid {
                        None => stored_grid = Some(g.clone()),
                        Some(first) if *first != g => {
                            return Err(invalid(format!(
                                "{name} is on another bin grid or band layout than the other \
                                 stored operands"
                            )));
                        }
                        Some(_) => {}
                    }
                } else if stored_grid.is_none() {
                    stored_grid = Some(g);
                }
                time_bases.push((t.source.shared_epoch(), name.clone()));
            }
        }
    }
    if needs_shared_time_base(c) && l.epoch.is_some() {
        let first = time_bases.first().and_then(|t| t.0);
        if let Some((_, name)) = time_bases.iter().find(|(t, _)| t.is_none() || *t != first) {
            return Err(invalid(format!(
                "{name} shares no time base with the other operands (an import, or a capture \
                 from an earlier audio session): their relative arrival is unknown, so they \
                 cannot be summed, differenced or averaged with phase; use ÷, ×, or a power \
                 average"
            )));
        }
    }
    Ok(match c.domain {
        MathDomain::Transfer => live_grid.map(|(g, _)| g).or(stored_grid),
        // A live spectrum's or RTA's grid depends on the session's rate.
        MathDomain::Spectrum | MathDomain::Rta if !has_live(c) => stored_grid,
        MathDomain::Spectrum | MathDomain::Rta => None,
    })
}

fn has_live(c: &MathConfig) -> bool {
    c.expr
        .operands()
        .iter()
        .any(|o| matches!(o, Operand::Meas { .. }))
}

/// The display grid of a spectrum's every-bin grid, with each display column's first bin.
pub(super) fn spectrum_display(g: &GridDef) -> Option<(GridDef, Vec<u32>)> {
    let GridDef::Linear { fs, n } = g else {
        return None;
    };
    let ppo = crate::jobs::spectrum::DISPLAY_PPO;
    let cols = BinColumns::new(fs.0, *n, ppo);
    Some((
        GridDef::LogBins {
            fs: *fs,
            n: *n,
            ppo,
        },
        cols.first_bin,
    ))
}

impl Control {
    /// Name and kind of measurement `id`.
    pub(super) fn lookup(&self, id: MeasId) -> Option<(&str, &MeasKind)> {
        self.store
            .state()
            .measurements
            .iter()
            .find(|m| m.id == id)
            .map(|m| (m.config.name.as_str(), &m.config.kind))
    }

    /// Metadata and grid of stored trace `id`.
    fn trace_lookup(&self, id: TraceId) -> Option<(&TraceMeta, GridDef)> {
        let t = self.store.state().traces.iter().find(|t| t.id == id)?;
        let (g, _) = self.traces.get(id)?;
        Some((t, g.clone()))
    }

    /// The grid of `kind` for measurement `own` without a session: a transfer's own, a
    /// transfer math channel's operands'.
    pub(super) fn grid_of(
        &self,
        own: Option<MeasId>,
        kind: &MeasKind,
    ) -> Result<Option<GridDef>, ProtoError> {
        match kind {
            MeasKind::Math { config } => {
                let meas = |m| self.lookup(m);
                let trace = |t| self.trace_lookup(t);
                check_operands(
                    own,
                    config,
                    &Lookup {
                        meas: &meas,
                        trace: &trace,
                        epoch: Some(self.epoch()),
                    },
                )
            }
            k => Ok(static_grid(k)),
        }
    }

    /// The grids of math channel `own` on the open session at `fs`, and its stored
    /// operands' columns on them (`None` for a live operand), in expression order.
    pub(super) fn math_setup(
        &self,
        own: MeasId,
        c: &MathConfig,
        fs: u32,
    ) -> Result<(Grids, Vec<StoredOperand>), ProtoError> {
        let static_grid = self.grid_of(Some(own), &MeasKind::Math { config: c.clone() })?;
        let mut grid: Option<(GridDef, String)> = static_grid.map(|g| (g, String::new()));
        let operands = c.expr.operands();
        // Live spectra and RTA: their grids at this rate, all equal.
        for o in &operands {
            let Operand::Meas { meas } = o else {
                continue;
            };
            let Some((name, kind)) = self.lookup(*meas) else {
                continue;
            };
            let g = match kind {
                MeasKind::Spectrum { config } => crate::jobs::spectrum::capture_grid(config, fs),
                MeasKind::Rta { config } => {
                    let bank = crate::jobs::rta::bank(config, fs).map_err(invalid)?;
                    crate::jobs::rta::grid(config, &bank)
                }
                _ => continue,
            };
            match &grid {
                None => grid = Some((g, name.to_owned())),
                Some((first, first_name)) if *first != g => {
                    return Err(invalid(format!(
                        "{name} and {first_name} have different {}; level math needs one",
                        if c.domain == MathDomain::Rta {
                            "band layouts"
                        } else {
                            "FFT lengths"
                        }
                    )));
                }
                Some(_) => {}
            }
        }
        let (grid, _) = grid.ok_or_else(|| invalid("a math channel needs operands"))?;
        let freqs = frequencies(&grid);
        let mut stored = Vec::with_capacity(operands.len());
        for o in &operands {
            let Operand::Trace { trace } = o else {
                stored.push(None);
                continue;
            };
            let t = self.stored(*trace)?;
            if c.domain != MathDomain::Transfer && t.grid != grid {
                return Err(invalid(format!(
                    "{} is on another bin grid or band layout than the live operands (band \
                     powers and FFT bins do not interpolate)",
                    t.meta.edit.name
                )));
            }
            let scale = match t.meta.kind {
                TraceKind::Spectrum { scale } | TraceKind::Rta { scale } => Some(scale),
                _ => None,
            };
            let mic_curve = t.meta.mic_curve.is_some()
                || t.meta.mic.as_ref().is_some_and(|m| m.curve.is_some());
            stored.push(Some(Arc::new(Held {
                columns: trace_on_grid(&t, &grid, &freqs),
                delay: t.meta.delay.0,
                time_base: t.meta.source.shared_epoch(),
                scale,
                mic_curve,
            })));
        }
        let display = match c.domain {
            MathDomain::Spectrum => spectrum_display(&grid),
            MathDomain::Transfer | MathDomain::Rta => None,
        };
        Ok((Grids { grid, display }, stored))
    }

    /// The math channels naming `o`.
    pub(super) fn maths_naming(&self, o: Operand) -> Vec<&Measurement> {
        self.store
            .state()
            .measurements
            .iter()
            .filter(|m| match &m.config.kind {
                MeasKind::Math { config } => config.expr.names(o),
                _ => false,
            })
            .collect()
    }

    /// Restarts the running math channels naming `o`, so they take its columns as they are
    /// now (a stored operand's mic curve changed). One that cannot start again is stopped
    /// and says why in the log.
    pub(super) fn restart_maths_naming(&mut self, o: Operand) {
        if self.session.is_none() {
            return;
        }
        let running: Vec<Measurement> = self
            .maths_naming(o)
            .into_iter()
            .filter(|m| m.running)
            .cloned()
            .collect();
        for m in running {
            self.stop_job(m.id);
            if let Err(e) = self.start_job(&m) {
                tracing::warn!("math channel {} not restarted: {}", m.id, e.msg);
            }
        }
    }

    fn named_by(&self, o: Operand) -> Result<(), ProtoError> {
        match self.maths_naming(o).first() {
            None => Ok(()),
            Some(m) => Err(perr(
                ErrorCode::Refused,
                format!(
                    "{} is an operand of the math channel {}; edit it there first",
                    operand_word(o),
                    m.config.name
                ),
            )),
        }
    }

    /// Refuses deleting trace `trace` while a math channel names it.
    pub(super) fn check_trace_operand_delete(&self, trace: TraceId) -> Result<(), ProtoError> {
        self.named_by(Operand::Trace { trace })
    }

    /// Refuses changing measurement `meas` into another kind or onto another grid (another
    /// FFT length, another band layout) while a math channel names it.
    pub(super) fn check_operand_update(
        &self,
        meas: MeasId,
        new: &MeasKind,
    ) -> Result<(), ProtoError> {
        let Some(a) = self
            .maths_naming(Operand::Meas { meas })
            .first()
            .map(|a| a.config.name.clone())
        else {
            return Ok(());
        };
        let same_shape = match (self.lookup(meas).map(|(_, k)| k), new) {
            (Some(old @ MeasKind::Transfer { .. }), MeasKind::Transfer { .. }) => {
                static_grid(old) == static_grid(new)
            }
            (Some(MeasKind::Spectrum { config: a }), MeasKind::Spectrum { config: b }) => {
                a.fft_len == b.fft_len
            }
            (Some(MeasKind::Rta { config: a }), MeasKind::Rta { config: b }) => {
                a.fraction == b.fraction && a.f_lo == b.f_lo && a.f_hi == b.f_hi
            }
            _ => false,
        };
        if same_shape {
            Ok(())
        } else {
            Err(perr(
                ErrorCode::Refused,
                format!(
                    "measurement {meas} is an operand of the math channel {a}: it keeps its \
                     kind and grid; edit the math channel first"
                ),
            ))
        }
    }
}
