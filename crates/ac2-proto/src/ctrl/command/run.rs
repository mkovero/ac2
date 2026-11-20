//! Arguments of the commands that play a stored measurement's stimulus and store its result.

use serde::{Deserialize, Serialize};

use crate::units::{LeaseToken, MeasId, TraceId};

/// Run a sweep measurement with its settings: plays its `repeats` synchronised sweeps,
/// records the reference and measurement inputs and stores a `sweep` trace it owns
/// (response, harmonic distortion, IR, room parameters). Like firing, it needs the
/// stimulus lease and the generator armed; the reply is the started run, whose progress
/// and outcome follow as `sweep` events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepRunArgs {
    /// Lease.
    pub lease_token: LeaseToken,
    /// The sweep measurement.
    pub meas: MeasId,
    /// Name of the resulting trace; `None` = `Run <number>`.
    pub name: Option<String>,
}

/// Run a dynamics measurement with its settings: plays its tone-burst programme,
/// records the reference and measured inputs and stores one `dynamics` trace it owns
/// (step table and envelopes per input). Like a sweep it needs the stimulus lease and
/// the generator armed; the reply is the started run, whose progress and outcome follow
/// as `dynamics` events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsRunArgs {
    /// Lease.
    pub lease_token: LeaseToken,
    /// The dynamics measurement.
    pub meas: MeasId,
    /// Name of the resulting trace; `None` = `Run <number>`.
    pub name: Option<String>,
    /// A dynamics trace whose rests to replay instead of adapting them, so an A/B pair
    /// plays an identical programme.
    pub rests_from: Option<TraceId>,
}
