//! Dynamics measurements (`docs/design/dynamics.md`): a tone-burst programme through a
//! limiter, compressor or VCA, and the attack, release, distortion and thump it reads.

use serde::{Deserialize, Serialize};

use crate::units::{ClientId, Db, Dbfs, DynamicsId, Hz, MeasId, Seconds, TraceId, WallNs};

/// Where in the carrier's cycle a level change lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicsOnset {
    /// At a positive-going zero crossing: the waveform stays continuous.
    #[default]
    Zero,
    /// At a positive peak: the hardest case for a peak detector.
    Peak,
}

/// A dynamics measurement's settings: what each `dynamics.run` of it plays and records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsConfig {
    /// Reference input (the loopback), zero-based. A dynamics measurement needs one.
    pub reference_input: u16,
    /// Measured inputs (the processor's outputs), zero-based: 1 …
    /// [`DynamicsConfig::MAX_INPUTS`], none twice, none the reference.
    pub measurement_inputs: Vec<u16>,
    /// Output channels carrying the programme (zero-based): the processor's inputs and the
    /// reference loop's.
    pub outputs: Vec<u16>,
    /// Requested carrier; each run snaps it to a whole-sample period.
    pub carrier: Hz,
    /// Onset point of every level change.
    pub onset: DynamicsOnset,
    /// Rest level, below the processor's threshold; typed by the operator (no default).
    pub pilot: Dbfs,
    /// Burst levels in play order, each above the pilot; typed by the operator (no
    /// default). The highest is the generator's level, checked against the global maximum
    /// at every run.
    pub bursts: Vec<Dbfs>,
    /// Burst durations in play order (inner loop: every duration at one level, then the
    /// next level).
    pub durations: Vec<Seconds>,
    /// Longest rest: a rest that has not recovered by then ends anyway.
    pub rest_max: Seconds,
}

impl DynamicsConfig {
    /// Most measured inputs of one run.
    pub const MAX_INPUTS: usize = 8;

    /// Default longest rest, s.
    pub const DEFAULT_REST_MAX: f64 = 30.0;

    /// The reference and measurement inputs make a run: at least one measured input, at
    /// most [`DynamicsConfig::MAX_INPUTS`], none twice and none the reference.
    pub fn check_inputs(&self) -> Result<(), String> {
        let m = &self.measurement_inputs;
        if m.is_empty() {
            return Err("a dynamics run needs at least one measured input".into());
        }
        if m.len() > Self::MAX_INPUTS {
            return Err(format!(
                "{} measured inputs: a dynamics run records at most {}",
                m.len(),
                Self::MAX_INPUTS
            ));
        }
        for (i, x) in m.iter().enumerate() {
            if *x == self.reference_input {
                return Err(format!(
                    "input {} is the reference: it cannot also be measured",
                    x + 1
                ));
            }
            if m[..i].contains(x) {
                return Err(format!("measured input {} is listed twice", x + 1));
            }
        }
        Ok(())
    }
}

/// Why a dynamics run ended without a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicsFailure {
    /// `gen.stop`, `gen.release` or a forced takeover stopped it.
    Stopped,
    /// The stimulus lease expired.
    LeaseExpired,
    /// The session closed or reopened.
    SessionClosed,
    /// Audio was lost while recording (an xrun or overflow).
    Dropout,
    /// The reference input carries no programme.
    NoReference,
    /// The gain at the pilot is not the gain 6 dB below it: every gain-reduction number
    /// would be measured from a limiting baseline.
    PilotNotBelowThreshold,
    /// The edges in the reference do not match the programme.
    Mismatch,
}

/// The part of the programme playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicsPhase {
    /// Waiting for the programme to reach the reference.
    Waiting,
    /// The linearity check (`pilot − 6 dB`, then `pilot`).
    Check,
    /// A burst.
    Burst,
    /// A rest.
    Rest,
}

/// Where a dynamics run is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DynamicsStatus {
    /// Playing and analysing step `step` (1-based; 0 during the linearity check) of
    /// `steps`.
    Playing {
        /// Step.
        step: u32,
        /// Steps in the programme.
        steps: u32,
        /// Part of it.
        phase: DynamicsPhase,
    },
    /// Stored as one trace.
    Done {
        /// The trace.
        trace: TraceId,
    },
    /// Ended without a result; its audio is discarded.
    Failed {
        /// Why.
        reason: DynamicsFailure,
        /// Detail for the operator.
        msg: String,
    },
}

/// The latest dynamics run (`dynamics.run`), mirrored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsRun {
    /// Id.
    pub id: DynamicsId,
    /// The dynamics measurement it runs.
    pub meas: MeasId,
    /// Client that started it.
    pub owner: ClientId,
    /// Name of the trace it makes.
    pub name: String,
    /// Reference input.
    pub reference_input: u16,
    /// Measured inputs.
    pub measurement_inputs: Vec<u16>,
    /// Outputs playing it.
    pub outputs: Vec<u16>,
    /// Carrier played (snapped to a whole-sample period).
    pub carrier: Hz,
    /// Onset point.
    pub onset: DynamicsOnset,
    /// Rest level.
    pub pilot: Dbfs,
    /// Burst levels.
    pub bursts: Vec<Dbfs>,
    /// Burst lengths played (whole periods).
    pub durations: Vec<Seconds>,
    /// Longest rest.
    pub rest_max: Seconds,
    /// The trace whose rests it replays; `None`: adaptive rests.
    pub rests_from: Option<TraceId>,
    /// Longest the run can play: every rest at its longest (or replayed length).
    pub longest: Seconds,
    /// Status.
    pub status: DynamicsStatus,
    /// When it started.
    pub started_at: WallNs,
}

impl DynamicsRun {
    /// Still playing.
    pub fn active(&self) -> bool {
        matches!(self.status, DynamicsStatus::Playing { .. })
    }
}

/// A time read from a gain envelope.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DynamicsTime {
    /// From the edge.
    Seconds {
        /// Time.
        value: Seconds,
    },
    /// Inside the window skipped after the edge: one carrier period or less.
    WithinPeriod,
    /// Never happened, or nothing settled to refer to.
    CantTell,
    /// The level's settled gain reduction is under 1 dB.
    NoGainReduction,
}

/// A level that may be lost under its floor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DynamicsLevel {
    /// A level.
    Db {
        /// dB.
        value: Db,
    },
    /// Not 6 dB above its floor.
    BelowFloor,
    /// Not measurable here.
    CantTell,
}

/// Which edge of a burst.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicsEdge {
    /// The burst's first sample.
    Start,
    /// The burst's last sample.
    End,
}

/// `h2 … h5` and THD over one window, re the fundamental.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsHarmonics {
    /// `h2 … h5`.
    pub h: [DynamicsLevel; 4],
    /// THD over `h2 … h5`.
    pub thd: DynamicsLevel,
}

/// The harmonic windows of a step.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsWindows {
    /// The first 50 ms of the burst.
    pub attack: DynamicsHarmonics,
    /// The last 10 % of the burst.
    pub settled: DynamicsHarmonics,
    /// The first second after the burst.
    pub release: DynamicsHarmonics,
    /// The last second of the rest.
    pub rest: DynamicsHarmonics,
}

/// Thump after one edge: the output low-passed at 200 Hz, re the burst peak.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsThump {
    /// Peak.
    pub peak: DynamicsLevel,
    /// RMS.
    pub rms: DynamicsLevel,
}

/// Overshoot in a burst.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsOvershoot {
    /// Largest |y| in the burst over the level's settled peak.
    pub level: Db,
    /// When, from the burst's first sample.
    pub at: Seconds,
    /// The reference's own overshoot the same way: the floor.
    pub reference: Db,
}

/// One step of one measured input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsRow {
    /// Gain reduction over the burst's last period.
    pub gr_end: Db,
    /// The level's settled gain reduction; `None` when no burst at the level settled.
    pub gr_settled: Option<Db>,
    /// The burst's gain changed less than 0.05 dB over its last 10 %.
    pub settled: bool,
    /// First time GR reaches 63 % of the settled GR (a time constant).
    pub attack_tau: DynamicsTime,
    /// First time after which the output stays within 2 dB of its settled level.
    pub attack_2db: DynamicsTime,
    /// The same within 0.5 dB.
    pub settle_0_5db: DynamicsTime,
    /// `None` when the level has no settled burst.
    pub overshoot: Option<DynamicsOvershoot>,
    /// Time after the burst for GR to fall to 37 % of `gr_end` (a time constant).
    pub release_tau: DynamicsTime,
    /// Time after the burst until GR stays under 2 dB.
    pub recovery_2db: DynamicsTime,
    /// The same under 0.5 dB.
    pub recovery_0_5db: DynamicsTime,
    /// Harmonics per window.
    pub harmonics: DynamicsWindows,
    /// Thump after the burst's start.
    pub thump_start: DynamicsThump,
    /// Thump after the burst's end.
    pub thump_end: DynamicsThump,
    /// Final gain of the rest after the burst re `g₀`.
    pub drift: Db,
}

/// One point of a step's envelope.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsPoint {
    /// The edge `t` counts from.
    pub edge: DynamicsEdge,
    /// From the edge, s (negative before the burst).
    pub t: f32,
    /// Gain reduction, dB.
    pub gr_db: f32,
    /// `h2 … h5` re the fundamental, dB; NaN where blind or not above the floor.
    pub h_db: [f32; 4],
}

/// One measured input of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsInput {
    /// The input, zero-based.
    pub input: u16,
    /// Its name at the run.
    pub name: String,
    /// The processor's envelope delay.
    pub delay: Seconds,
    /// Baseline gain at the pilot (output re reference).
    pub g0: Db,
    /// One row per step.
    pub rows: Vec<DynamicsRow>,
}

/// What every input shares about a step.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsStep {
    /// Burst level.
    pub level: Dbfs,
    /// Burst length as found in the reference.
    pub burst: Seconds,
    /// Rest after it as played.
    pub rest: Seconds,
    /// The rest recovered (else it ended at the longest rest).
    pub recovered: bool,
}

/// A run's settings and facts (the CSV export's `# dynamics_info:` header line).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsInfo {
    /// Sample rate.
    pub sample_rate: Hz,
    /// Carrier played.
    pub carrier: Hz,
    /// Its period, samples.
    pub period: u32,
    /// Onset point.
    pub onset: DynamicsOnset,
    /// Rest level.
    pub pilot: Dbfs,
    /// Burst levels.
    pub bursts: Vec<Dbfs>,
    /// Burst lengths played.
    pub durations: Vec<Seconds>,
    /// Longest rest.
    pub rest_max: Seconds,
    /// The rests were replayed from another run.
    pub replayed: bool,
    /// Reference input.
    pub reference_input: u16,
    /// Per step.
    pub steps: Vec<DynamicsStep>,
}

/// The data of a dynamics trace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsData {
    /// Settings and facts.
    pub info: DynamicsInfo,
    /// Per measured input.
    pub inputs: Vec<DynamicsInput>,
    /// Per measured input (as `inputs`), one envelope per step.
    pub envelopes: Vec<Vec<Vec<DynamicsPoint>>>,
}

impl DynamicsData {
    /// Its step table.
    pub fn table(&self) -> DynamicsTable {
        DynamicsTable {
            info: self.info.clone(),
            inputs: self.inputs.clone(),
        }
    }

    /// Step `step`'s envelopes of measured input `input` (a device input, zero-based), or
    /// why there are none: no such input or step.
    pub fn envelope(&self, input: u16, step: u32) -> Result<DynamicsEnvelope, String> {
        let i = self
            .inputs
            .iter()
            .position(|x| x.input == input)
            .ok_or_else(|| format!("input {} is not measured in this run", input + 1))?;
        let points = self
            .envelopes
            .get(i)
            .and_then(|e| e.get(step as usize))
            .ok_or_else(|| {
                format!(
                    "step {step} is not in this run of {} steps",
                    self.info.steps.len()
                )
            })?;
        let edge = |edge| {
            let mut c = DynamicsEdgeCurve::default();
            for p in points.iter().filter(|p| p.edge == edge) {
                c.t.push(p.t);
                c.gr_db.push(p.gr_db);
                for (h, v) in c.h_db.iter_mut().zip(p.h_db) {
                    h.push(v);
                }
            }
            c
        };
        Ok(DynamicsEnvelope {
            input,
            step,
            start: edge(DynamicsEdge::Start),
            end: edge(DynamicsEdge::End),
        })
    }
}

/// One edge's envelope as parallel arrays: a reply of a few tens of kB where the same points
/// as tagged structs would be several times that.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsEdgeCurve {
    /// From the edge, s (negative before it).
    pub t: Vec<f32>,
    /// Gain reduction, dB.
    pub gr_db: Vec<f32>,
    /// `h2 … h5` re the fundamental, dB; NaN where blind or not above the floor.
    pub h_db: [Vec<f32>; 4],
}

/// One step's envelopes of one measured input (`dynamics.envelope`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsEnvelope {
    /// The measured input, zero-based.
    pub input: u16,
    /// The step, zero-based.
    pub step: u32,
    /// From the burst's first sample.
    pub start: DynamicsEdgeCurve,
    /// From the burst's last sample.
    pub end: DynamicsEdgeCurve,
}

/// A dynamics trace's step table without its envelopes (`dynamics.table`): a few kB where
/// the envelopes of a long high-carrier run are tens of MB.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicsTable {
    /// Settings and facts.
    pub info: DynamicsInfo,
    /// Per measured input.
    pub inputs: Vec<DynamicsInput>,
}
