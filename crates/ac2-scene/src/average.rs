//! What a live spatial average says about its members: how many positions it averaged,
//! which it left out and why (`docs/design/spatial-average.md`).
//!
//! An average that silently dropped a position would read as the room's response over
//! every position while being the response over fewer: the legend counts them, and a banner
//! names the ones left out. With fewer than two usable positions there is no average, and
//! the banner says so instead of a curve.

use ac2_proto::frame::{MemberStatus, ProtectionFlags, TfAverage};
use ac2_proto::model::{AverageMethod, SpatialAverageConfig};
use ac2_proto::units::MeasId;

/// `power`, `complex`, `coherence-weighted`.
pub fn method_name(m: AverageMethod) -> &'static str {
    match m {
        AverageMethod::Power => "power",
        AverageMethod::Complex => "complex",
        AverageMethod::CoherenceWeighted => "coherence-weighted",
    }
}

/// Why a member was left out, in the words of its own banner: `stopped`, `settling`,
/// `clip`, `no reference`, `check routing`, `no signal`; `None` when it was averaged.
pub fn left_out_reason(s: MemberStatus) -> Option<String> {
    match s {
        MemberStatus::Included => None,
        MemberStatus::Stopped => Some("stopped".into()),
        MemberStatus::Settling => Some("settling".into()),
        MemberStatus::Refused { protection } => {
            let words: Vec<&str> = [
                (ProtectionFlags::CLIP, "clip"),
                (ProtectionFlags::NO_REFERENCE, "no reference"),
                (ProtectionFlags::CHECK_ROUTING, "check routing"),
                (ProtectionFlags::NO_SIGNAL, "no signal"),
            ]
            .into_iter()
            .filter(|(f, _)| protection.contains(*f))
            .map(|(_, w)| w)
            .collect();
            Some(if words.is_empty() {
                "refused".into()
            } else {
                words.join(", ")
            })
        }
    }
}

/// A spatial average's frame, as the display tells it.
#[derive(Clone, Debug, PartialEq)]
pub struct AverageStatus {
    pub method: AverageMethod,
    /// Positions averaged into the frame.
    pub included: usize,
    /// Positions configured.
    pub members: usize,
    /// `Seat 3: no signal`, one per position left out, in configuration order.
    pub left_out: Vec<String>,
}

impl AverageStatus {
    /// From a frame's [`TfAverage`], members named by `name_of`.
    pub fn new(a: &TfAverage, name_of: impl Fn(MeasId) -> String) -> Self {
        Self {
            method: a.method,
            included: a.included(),
            members: a.members.len(),
            left_out: a
                .members
                .iter()
                .filter_map(|m| {
                    left_out_reason(m.status).map(|r| format!("{}: {r}", name_of(m.meas)))
                })
                .collect(),
        }
    }

    /// Whether the frame carries an average at all.
    pub fn has_value(&self) -> bool {
        self.included >= SpatialAverageConfig::MIN_MEMBERS
    }

    /// `4 positions` when every member is in, else `3 of 4 positions`.
    pub fn count(&self) -> String {
        if self.included == self.members {
            format!("{} positions", self.members)
        } else {
            format!("{} of {} positions", self.included, self.members)
        }
    }

    /// The legend tag: `3 of 4 positions · power avg`.
    pub fn tag(&self) -> String {
        format!("{} · {} avg", self.count(), method_name(self.method))
    }

    /// Banner text and detail when a position is left out: `AVERAGE · 3 OF 4 POSITIONS`
    /// (warning) or, below two, `NO AVERAGE · 1 OF 4 POSITIONS` (fault); the detail names
    /// what was left out and why. `None` when every position is in.
    pub fn banner(&self, name: &str) -> Option<(bool, String, String)> {
        if self.left_out.is_empty() {
            return None;
        }
        let fault = !self.has_value();
        let head = if fault { "NO AVERAGE" } else { "AVERAGE" };
        Some((
            fault,
            format!("{head} · {} OF {} POSITIONS", self.included, self.members),
            format!("{name}: left out {}", self.left_out.join(" · ")),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::frame::AverageMemberState;

    fn avg(statuses: &[MemberStatus]) -> TfAverage {
        TfAverage {
            method: AverageMethod::Power,
            members: statuses
                .iter()
                .enumerate()
                .map(|(i, s)| AverageMemberState {
                    meas: MeasId(i as u32 + 1),
                    status: *s,
                })
                .collect(),
        }
    }

    fn name(m: MeasId) -> String {
        format!("Seat {}", m.0)
    }

    #[test]
    fn every_position_in() {
        let s = AverageStatus::new(&avg(&[MemberStatus::Included; 4]), name);
        assert_eq!(s.tag(), "4 positions · power avg");
        assert_eq!(s.banner("Audience"), None);
    }

    #[test]
    fn positions_left_out_are_named_with_their_reason() {
        let s = AverageStatus::new(
            &avg(&[
                MemberStatus::Included,
                MemberStatus::Included,
                MemberStatus::Refused {
                    protection: ProtectionFlags::NO_SIGNAL.with(ProtectionFlags::CLIP),
                },
                MemberStatus::Stopped,
            ]),
            name,
        );
        assert_eq!(s.tag(), "2 of 4 positions · power avg");
        assert_eq!(
            s.banner("Audience"),
            Some((
                false,
                "AVERAGE · 2 OF 4 POSITIONS".into(),
                "Audience: left out Seat 3: clip, no signal · Seat 4: stopped".into()
            ))
        );
    }

    #[test]
    fn one_position_is_no_average() {
        let s = AverageStatus::new(
            &avg(&[MemberStatus::Included, MemberStatus::Settling]),
            name,
        );
        assert!(!s.has_value());
        let (fault, text, detail) = s.banner("Audience").unwrap_or_default();
        assert!(fault);
        assert_eq!(text, "NO AVERAGE · 1 OF 2 POSITIONS");
        assert_eq!(detail, "Audience: left out Seat 2: settling");
    }
}
