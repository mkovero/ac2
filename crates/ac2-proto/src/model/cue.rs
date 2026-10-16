//! Cues: what the rig does for external gear (lights, a stage display) when an SPL limit's
//! state changes — an OSC message, DMX channel levels in the rig's Art-Net universe, or
//! both — and the state of every cued subject.

use serde::{Deserialize, Serialize};

use super::{AlarmSubject, DmxSet, OscMessage, PeakQuantity, Weighting};
use crate::units::{Hz, MeasId, Seconds};

/// What one state of a cue does.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CueAction {
    /// The OSC message sent on entering the state (and repeated); `None`: none.
    pub osc: Option<OscMessage>,
    /// The levels this state holds in the rig's Art-Net universe while it is current. The
    /// universe is the highest level any current state lists for a channel (HTP), and 0
    /// where none lists it.
    pub dmx: Vec<DmxSet>,
}

impl CueAction {
    /// Neither a message nor a level: the state does nothing.
    pub fn is_empty(&self) -> bool {
        self.osc.is_none() && self.dmx.is_empty()
    }
}

/// What each state of a limit (or of the whole meter) does; `None`: nothing for that state.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cue {
    /// Under the limit, or no limit to judge.
    pub ok: Option<CueAction>,
    /// Within the warn margin below the limit.
    pub near: Option<CueAction>,
    /// Over the limit.
    pub over: Option<CueAction>,
    /// The rig cannot judge: no audio, device lost, or not calibrated.
    pub fault: Option<CueAction>,
}

impl Cue {
    /// The action for `state`.
    pub fn get(&self, state: CueState) -> Option<&CueAction> {
        match state {
            CueState::Ok => self.ok.as_ref(),
            CueState::Near => self.near.as_ref(),
            CueState::Over => self.over.as_ref(),
            CueState::Fault => self.fault.as_ref(),
        }
    }

    /// The action slot for `state`.
    pub fn get_mut(&mut self, state: CueState) -> &mut Option<CueAction> {
        match state {
            CueState::Ok => &mut self.ok,
            CueState::Near => &mut self.near,
            CueState::Over => &mut self.over,
            CueState::Fault => &mut self.fault,
        }
    }

    /// The OSC message for `state`.
    pub fn osc(&self, state: CueState) -> Option<&OscMessage> {
        self.get(state).and_then(|a| a.osc.as_ref())
    }

    /// The DMX levels for `state` (empty: none).
    pub fn dmx(&self, state: CueState) -> &[DmxSet] {
        self.get(state).map_or(&[], |a| a.dmx.as_slice())
    }
}

/// State of a cued limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CueState {
    /// Under the limit, or no limit.
    Ok,
    /// Within the warn margin.
    Near,
    /// Over.
    Over,
    /// Cannot judge.
    Fault,
}

impl CueState {
    /// Every state, in cue field order.
    pub const ALL: [Self; 4] = [Self::Ok, Self::Near, Self::Over, Self::Fault];
}

/// What a cue is about: one limit of an SPL meter, or the meter as a whole (the worst
/// state of its limits).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CueSubject {
    /// The whole meter ([`super::SplConfig::cue`]).
    Meter,
    /// A rolling Leq window.
    Window {
        /// Window length.
        duration: Seconds,
        /// Window weighting.
        weighting: Weighting,
    },
    /// A peak limit.
    Peak {
        /// LCpeak or LAFmax.
        quantity: PeakQuantity,
    },
    /// A band window of the band meter.
    Band {
        /// Window length.
        duration: Seconds,
        /// Window weighting.
        weighting: Weighting,
        /// Nominal centre.
        nominal: Hz,
    },
    /// The band meter's predicted LAeq window.
    Predicted,
}

impl From<AlarmSubject> for CueSubject {
    fn from(s: AlarmSubject) -> Self {
        match s {
            AlarmSubject::Window {
                duration,
                weighting,
            } => Self::Window {
                duration,
                weighting,
            },
            AlarmSubject::Peak { quantity } => Self::Peak { quantity },
            AlarmSubject::Band {
                duration,
                weighting,
                nominal,
            } => Self::Band {
                duration,
                weighting,
                nominal,
            },
            AlarmSubject::Predicted => Self::Predicted,
        }
    }
}

/// The current state of one cued subject.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigCue {
    /// Measurement.
    pub meas: MeasId,
    /// Subject.
    pub subject: CueSubject,
    /// State, as last sent.
    pub state: CueState,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DmxChannel, OscArg, OscPath};

    fn m(p: &str, args: Vec<OscArg>) -> OscMessage {
        OscMessage {
            path: OscPath::new(p).expect("valid"),
            args,
            append_level: false,
        }
    }

    fn set(channel: u16, value: u8) -> DmxSet {
        DmxSet {
            channel: DmxChannel::new(channel).expect("valid"),
            value,
        }
    }

    #[test]
    fn cue_round_trips() {
        let cue = Cue {
            ok: Some(CueAction {
                osc: Some(m("/light", vec![OscArg::Int(0)])),
                dmx: vec![set(12, 0)],
            }),
            near: None,
            over: Some(CueAction {
                osc: Some(OscMessage {
                    append_level: true,
                    ..m(
                        "/light",
                        vec![
                            OscArg::Int(1),
                            OscArg::Float(0.5),
                            OscArg::Str("red".into()),
                        ],
                    )
                }),
                dmx: vec![set(12, 255), set(512, 40)],
            }),
            fault: Some(CueAction {
                osc: None,
                dmx: vec![set(1, 255)],
            }),
        };
        let b = rmp_serde::to_vec_named(&cue).expect("valid");
        assert_eq!(rmp_serde::from_slice::<Cue>(&b).expect("valid"), cue);
        assert_eq!(cue.get(CueState::Near), None);
        assert_eq!(cue.osc(CueState::Fault), None);
        assert_eq!(cue.dmx(CueState::Fault), [set(1, 255)]);
        assert_eq!(cue.dmx(CueState::Near), []);
        assert_eq!(
            cue.osc(CueState::Ok).expect("valid").path.as_str(),
            "/light"
        );
        assert!(CueAction::default().is_empty());
    }
}
