//! Spatial averages: their members' invariants (`docs/design/spatial-average.md`).
//!
//! An average names transfer measurements on one grid. That holds for as long as the
//! average exists: a member cannot be deleted, turned into another kind or moved to another
//! grid while an average names it — the average would silently stop meaning what its
//! name says. The operator removes it from the average first.

use ac2_proto::grid::GridDef;
use ac2_proto::model::{AverageReference, MeasKind, Measurement, SpatialAverageConfig};
use ac2_proto::units::MeasId;
use ac2_proto::{ErrorCode, ProtoError};

use super::{Control, static_grid};
use crate::util::perr;

fn invalid(m: impl Into<String>) -> ProtoError {
    perr(ErrorCode::Invalid, m.into())
}

/// Checks of an average's configuration that need no other measurement.
pub(super) fn validate(c: &SpatialAverageConfig) -> Result<(), ProtoError> {
    let n = c.members.len();
    if !(SpatialAverageConfig::MIN_MEMBERS..=SpatialAverageConfig::MAX_MEMBERS).contains(&n) {
        return Err(invalid(format!(
            "a spatial average has {} … {} members",
            SpatialAverageConfig::MIN_MEMBERS,
            SpatialAverageConfig::MAX_MEMBERS
        )));
    }
    for (i, m) in c.members.iter().enumerate() {
        if c.members[..i].contains(m) {
            return Err(invalid(format!("measurement {m} is listed twice")));
        }
    }
    match c.reference {
        AverageReference::Member { meas } if !c.members.contains(&meas) => Err(invalid(format!(
            "the phase reference, measurement {meas}, is not a member"
        ))),
        AverageReference::Fixed { delay } if !delay.0.is_finite() => {
            Err(invalid("the reference delay must be finite"))
        }
        _ => Ok(()),
    }
}

/// The grid of average `own` (`None` while it is being created) with members looked up by
/// `kind_of`: every member must exist, be a transfer measurement and share one grid.
pub(super) fn members_grid<'a>(
    own: Option<MeasId>,
    c: &SpatialAverageConfig,
    kind_of: impl Fn(MeasId) -> Option<(&'a str, &'a MeasKind)>,
) -> Result<GridDef, ProtoError> {
    let mut grid: Option<(GridDef, &str)> = None;
    for &m in &c.members {
        if Some(m) == own {
            return Err(invalid("an average cannot be its own member"));
        }
        let (name, kind) = kind_of(m).ok_or_else(|| {
            perr(
                ErrorCode::NotFound,
                format!("no measurement {m} to average"),
            )
        })?;
        let MeasKind::Transfer { .. } = kind else {
            return Err(invalid(format!(
                "{name}: only transfer measurements can be averaged"
            )));
        };
        let g = static_grid(kind).ok_or_else(|| invalid(format!("{name}: no grid")))?;
        match &grid {
            None => grid = Some((g, name)),
            Some((first, first_name)) if *first != g => {
                return Err(invalid(format!(
                    "{name} and {first_name} have different grids; an average needs one grid"
                )));
            }
            Some(_) => {}
        }
    }
    grid.map(|(g, _)| g)
        .ok_or_else(|| invalid("a spatial average needs members"))
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

    /// The grid of `kind` for measurement `own`: a transfer's own, an average's members'.
    pub(super) fn grid_of(
        &self,
        own: Option<MeasId>,
        kind: &MeasKind,
    ) -> Result<Option<GridDef>, ProtoError> {
        match kind {
            MeasKind::SpatialAverage { config } => {
                members_grid(own, config, |m| self.lookup(m)).map(Some)
            }
            k => Ok(static_grid(k)),
        }
    }

    /// The averages naming `member`.
    fn averages_of(&self, member: MeasId) -> Vec<&Measurement> {
        self.store
            .state()
            .measurements
            .iter()
            .filter(|m| match &m.config.kind {
                MeasKind::SpatialAverage { config } => config.members.contains(&member),
                _ => false,
            })
            .collect()
    }

    /// Refuses deleting `member` while an average names it.
    pub(super) fn check_member_delete(&self, member: MeasId) -> Result<(), ProtoError> {
        match self.averages_of(member).first() {
            None => Ok(()),
            Some(a) => Err(perr(
                ErrorCode::Refused,
                format!(
                    "measurement {member} is a member of the average {}; remove it there first",
                    a.config.name
                ),
            )),
        }
    }

    /// Refuses changing `member` into another kind or onto another grid while an average
    /// names it.
    pub(super) fn check_member_update(
        &self,
        member: MeasId,
        new: &MeasKind,
    ) -> Result<(), ProtoError> {
        let Some(a) = self
            .averages_of(member)
            .first()
            .map(|a| a.config.name.clone())
        else {
            return Ok(());
        };
        let old = self.lookup(member).map(|(_, k)| static_grid(k));
        let same_grid = matches!(new, MeasKind::Transfer { .. })
            && old.is_some_and(|g| g.is_some() && g == static_grid(new));
        if same_grid {
            Ok(())
        } else {
            Err(perr(
                ErrorCode::Refused,
                format!(
                    "measurement {member} is a member of the average {a}: it stays a transfer \
                     measurement on the same grid; remove it from the average first"
                ),
            ))
        }
    }
}
