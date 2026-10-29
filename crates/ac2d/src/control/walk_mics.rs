//! A walk's mics (`docs/design/spatial-walk.md` §6, §8): which measurement captures with each
//! and on which input, and what keeps them so while the walk names them.
//!
//! A live walk's mics are its transfer measurements, one per mic; a sweep walk's are the
//! measurement inputs of its one sweep measurement, which records every one in one play.
//! Either way two mics never share an input: one input is one mic, and one mic stands at one
//! point.

use ac2_proto::model::{MeasKind, MeasScope, WalkKind, WalkMic, WalkState};
use ac2_proto::units::MeasId;
use ac2_proto::{ErrorCode, ProtoError};

use super::Control;
use crate::util::perr;

impl Control {
    /// The mics of a new walk of kind `kind` capturing with `meas`, checked.
    pub(super) fn walk_mics(
        &self,
        kind: WalkKind,
        meas: &[MeasId],
    ) -> Result<Vec<WalkMic>, ProtoError> {
        let bad = |m: String| perr(ErrorCode::Invalid, m);
        match kind {
            WalkKind::LiveTransfer if meas.is_empty() || meas.len() > WalkState::MAX_MICS => {
                return Err(bad(format!(
                    "a live walk captures with 1 … {} transfer measurements, one per mic",
                    WalkState::MAX_MICS
                )));
            }
            WalkKind::Sweep if meas.len() != 1 => {
                return Err(bad(
                    "a sweep walk records with one sweep measurement: its measurement inputs \
                     are the mics"
                        .into(),
                ));
            }
            WalkKind::LiveTransfer | WalkKind::Sweep => {}
        }
        if (1..meas.len()).any(|k| meas[..k].contains(&meas[k])) {
            return Err(bad("a walk names each measurement once: one per mic".into()));
        }
        let mut mics: Vec<WalkMic> = Vec::with_capacity(meas.len());
        for &id in meas {
            let m = self.meas(id)?;
            let name = &m.config.name;
            if m.config.scope != MeasScope::Session {
                return Err(bad(format!("{name} is a rig meter")));
            }
            match (kind, &m.config.kind) {
                (WalkKind::LiveTransfer, MeasKind::Transfer { config }) => {
                    // Two measurements of one input would capture the same mic at two places.
                    if mics.iter().any(|o| o.input == config.measurement_input) {
                        return Err(bad(format!(
                            "{name} measures an input another of the walk's measurements does: \
                             each mic needs its own input"
                        )));
                    }
                    mics.push(WalkMic {
                        meas: id,
                        input: config.measurement_input,
                    });
                }
                (WalkKind::Sweep, MeasKind::Sweep { config }) => {
                    config
                        .check_inputs()
                        .map_err(|e| bad(format!("{name}: {e}")))?;
                    if config.measurement_inputs.len() > WalkState::MAX_MICS {
                        return Err(bad(format!(
                            "{name} records {} inputs; a walk places at most {} mics",
                            config.measurement_inputs.len(),
                            WalkState::MAX_MICS
                        )));
                    }
                    mics.extend(
                        config
                            .measurement_inputs
                            .iter()
                            .map(|&input| WalkMic { meas: id, input }),
                    );
                }
                (WalkKind::LiveTransfer, _) => {
                    return Err(bad(format!(
                        "{name} is not a transfer measurement: a live walk captures transfers"
                    )));
                }
                (WalkKind::Sweep, _) => {
                    return Err(bad(format!(
                        "{name} is not a sweep measurement: a sweep walk records sweeps"
                    )));
                }
            }
            if let Some(w) = self.walk_of(id) {
                return Err(perr(
                    ErrorCode::Refused,
                    format!("{name} captures for the walk {} already", w.name),
                ));
            }
        }
        Ok(mics)
    }

    /// Refuses a change to a walk's measurement that would change its mics: another kind,
    /// or other measurement inputs. The walk's points name the mics that stand at them, and
    /// its average averages what they captured.
    pub(super) fn check_walk_meas_update(
        &self,
        meas: MeasId,
        new: &MeasKind,
    ) -> Result<(), ProtoError> {
        let Some(w) = self.walk_naming_meas(meas) else {
            return Ok(());
        };
        let refuse = |what: &str| {
            Err(perr(
                ErrorCode::Refused,
                format!(
                    "measurement {meas} belongs to the walk {}: it keeps its {what} while the \
                     walk names it",
                    w.name
                ),
            ))
        };
        if w.average == Some(meas) {
            return match new {
                MeasKind::Math { .. } => Ok(()),
                _ => refuse("kind"),
            };
        }
        let now: Vec<WalkMic> = w.mics.iter().copied().filter(|m| m.meas == meas).collect();
        let after: Vec<WalkMic> = match (w.kind, new) {
            (WalkKind::LiveTransfer, MeasKind::Transfer { config }) => vec![WalkMic {
                meas,
                input: config.measurement_input,
            }],
            (WalkKind::Sweep, MeasKind::Sweep { config }) => config
                .measurement_inputs
                .iter()
                .map(|&input| WalkMic { meas, input })
                .collect(),
            _ => return refuse("kind"),
        };
        if after == now {
            Ok(())
        } else {
            refuse("measurement inputs (the walk's mics)")
        }
    }

    /// Refuses moving the mics of walk `w` while its sweep plays or is analysed: the run
    /// records them where they stood when it started.
    pub(super) fn check_walk_idle(&self, w: &WalkState) -> Result<(), ProtoError> {
        if self.walk_sweeping() == Some(w.id) {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "the walk {}'s sweep is running: it records the mics where they stand; \
                     wait for it, or stop it",
                    w.name
                ),
            ));
        }
        Ok(())
    }
}
