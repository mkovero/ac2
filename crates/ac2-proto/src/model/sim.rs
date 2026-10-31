//! Room simulation (`docs/design/spatial-walk.md` §7): geometric acoustics on a plan's prism,
//! run by the daemon on a worker (`sim.run`, `sim.fit`). Everything here is *simulated*: a
//! [`SimState`] is not a trace, has no colour and is never a math-channel operand (those take
//! [`TraceId`](crate::units::TraceId)s), so a prediction cannot be mistaken for a measurement.

use serde::{Deserialize, Serialize};

use super::{RoomAcoustics, Surface, WalkPointSpec, WalkRoom};
use crate::units::{Hz, Seconds, SimId, WalkId};

/// The material of a wall or the ceiling a plan leaves untagged (a name in the simulator's
/// table): plastered masonry, the common hard wall of a venue.
pub const DEFAULT_WALL_MATERIAL: &str = "plaster_on_masonry";
/// The material of an untagged floor: concrete, the common floor of a venue.
pub const DEFAULT_FLOOR_MATERIAL: &str = "concrete";

/// What `sim.run` simulates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimRequest {
    /// The plan, by name: the session's copy when it holds one, else the library's.
    pub plan: String,
    /// Speaker markers of the plan, by name, each simulated as an omnidirectional source;
    /// at least one.
    pub sources: Vec<String>,
    /// Where it is heard.
    pub receivers: SimReceivers,
    /// How.
    pub settings: SimSettings,
}

/// The receivers of a simulation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SimReceivers {
    /// Every point of a walk (whose plan is the request's).
    Walk {
        /// The walk.
        walk: WalkId,
    },
    /// Points given here.
    Points {
        /// The points (at most [`SimSettings::MAX_RECEIVERS`]).
        points: Vec<WalkPointSpec>,
    },
}

/// How a simulation runs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimSettings {
    /// Highest image-source order (reflections per specular path), at most
    /// [`Self::MAX_IMAGE_ORDER`].
    pub image_order: u8,
    /// Rays traced per octave band for the late tail (at most [`Self::MAX_RAYS`]).
    pub rays_per_band: u32,
    /// Wall-clock budget for the ray tracing of one pass (a fit runs several): when it runs
    /// out the pass keeps the rays it has traced and says so.
    pub budget: Seconds,
    /// Seed: the same plan, request and seed give the same result.
    pub seed: u64,
}

impl SimSettings {
    /// The highest image order accepted: the image count grows as (surfaces − 1)^order.
    pub const MAX_IMAGE_ORDER: u8 = 4;
    /// The most rays per band accepted.
    pub const MAX_RAYS: u32 = 1_000_000;
    /// The longest budget accepted, seconds.
    pub const MAX_BUDGET_S: f64 = 600.0;
    /// The most receivers of one run.
    pub const MAX_RECEIVERS: usize = 64;
    /// The most receiver results of one run, over every source (sources × receivers): what
    /// one state event holds.
    pub const MAX_RESULTS: usize = 64;
}

impl Default for SimSettings {
    fn default() -> Self {
        SimSettings {
            image_order: 3,
            rays_per_band: 20_000,
            budget: Seconds(10.0),
            seed: 0,
        }
    }
}

/// What `sim.fit` fits: the absorption per band so the simulated T20 at a sweep walk's
/// captured positions matches the T20 measured there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimFitRequest {
    /// The sweep walk (its plan is the session's copy).
    pub walk: WalkId,
    /// The speaker marker the walk was measured from.
    pub source: String,
    /// How each pass runs.
    pub settings: SimSettings,
}

/// The latest simulation (`sim.run` / `sim.fit`), mirrored. Simulated, valid above
/// [`Self::valid_above`] only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimState {
    /// Id.
    pub id: SimId,
    /// The plan simulated.
    pub plan: String,
    /// A run, a fit or a coverage map.
    pub kind: SimKind,
    /// The walk whose points are the receivers (a run at a walk, a fit); `None` for given
    /// points and maps.
    pub walk: Option<WalkId>,
    /// How it runs.
    pub settings: SimSettings,
    /// Where it is.
    pub status: SimStatus,
    /// The octave bands every per-band value is given in, nominal mid-band frequencies.
    pub bands: Vec<Hz>,
    /// Where the absorption comes from.
    pub absorption: SimAbsorption,
    /// Per source, in the request's order; empty until done.
    pub sources: Vec<SimSource>,
    /// Schroeder frequency 2000·√(T/V) of the room as simulated; `None` until done.
    pub schroeder: Option<Hz>,
    /// Below this nothing simulated may be shown as valid; `None` until done.
    pub valid_above: Option<Hz>,
    /// Eyring's reverberation time per band for the room as simulated (materials, scale,
    /// air); empty until done.
    pub eyring: Vec<Seconds>,
    /// Rays traced per band (fewer than asked when the budget ran out).
    pub rays_per_band: u64,
    /// The budget ran out before every ray was traced.
    pub budget_exhausted: bool,
    /// Rays that slipped out of the room through rounding at an edge, over all bands.
    pub lost_rays: u64,
    /// A coverage map's grid (`sim.map`), once done; `None` for runs and fits.
    pub map: Option<SimMap>,
}

impl super::State {
    /// Takes `s` as the latest simulation of its kind: a map is [`Self::sim_map`], a run
    /// or a fit [`Self::sim`].
    pub fn set_sim(&mut self, s: SimState) {
        if s.kind == SimKind::Map {
            self.sim_map = Some(s);
        } else {
            self.sim = Some(s);
        }
    }
}

impl SimState {
    /// Still running.
    pub fn active(&self) -> bool {
        matches!(self.status, SimStatus::Running { .. })
    }
}

/// A run or a fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SimKind {
    /// `sim.run`.
    Run,
    /// `sim.fit` to a sweep walk.
    Fit {
        /// The walk.
        walk: WalkId,
    },
    /// `sim.map`: direct and early energy over the plan's zones.
    Map,
}

/// Where a simulation is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SimStatus {
    /// Running: `done` of `total` steps of `stage`, for source `source` (index into the
    /// request's sources), in pass `pass` of a fit (1-based; 0 for a run).
    Running {
        /// The stage.
        stage: SimStage,
        /// Steps done.
        done: u64,
        /// Steps in the stage.
        total: u64,
        /// Source index.
        source: u16,
        /// Fit pass, 1-based; 0 for a run.
        pass: u8,
    },
    /// Done; the results are in.
    Done,
    /// Stopped before it was done.
    Cancelled {
        /// Why.
        why: SimCancel,
    },
    /// Failed while running.
    Failed {
        /// Detail for the operator.
        msg: String,
    },
}

/// A stage of a simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimStage {
    /// Building and checking image sources.
    ImageSources,
    /// Tracing rays.
    RayTracing,
    /// Evaluating the room parameters on the simulated impulse responses.
    Analysis,
}

/// Why a simulation stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimCancel {
    /// `sim.cancel`.
    Operator,
    /// Its plan changed or was deleted.
    PlanChanged,
    /// Its walk was deleted.
    WalkDeleted,
    /// The audio session closed.
    SessionClosed,
}

/// Where a simulation's absorption comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SimAbsorption {
    /// The materials' table values: *absorption assumed*.
    Assumed,
    /// Scaled per band so the simulated T20 matched the T20 measured over a sweep walk's
    /// positions. Only the level of the absorption is fitted, not which surface absorbs.
    Fitted {
        /// The walk.
        walk: WalkId,
        /// Its positions carrying room parameters.
        positions: u16,
        /// Per band of [`SimState::bands`].
        bands: Vec<SimFitBand>,
    },
}

/// One band of a fit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimFitBand {
    /// The multiplier on every surface's absorption; `None`: the walk measured no T20 in
    /// this band, which keeps the table's absorption (assumed).
    pub scale: Option<f64>,
    /// The T20 measured: the mean over the walk's positions.
    pub measured: Option<Seconds>,
    /// The simulated T20 at the same positions with the scale (mean).
    pub simulated: Option<Seconds>,
    /// The simulated T20 came within the fit's tolerance of the measured one.
    pub converged: bool,
}

/// What one source gives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimSource {
    /// The speaker marker's name.
    pub name: String,
    /// Where it is, m.
    pub at: [f64; 3],
    /// One per receiver, in the request's order.
    pub receivers: Vec<SimReceiver>,
    /// The room parameters over the receivers (mean, least, greatest per band).
    pub room: Option<WalkRoom>,
}

/// What one receiver hears from one source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimReceiver {
    /// The point's label.
    pub label: String,
    /// Where it is, m.
    pub at: [f64; 3],
    /// Direct sound and specular reflections up to the image order, by time (the first
    /// [`Self::MAX_ARRIVALS`]).
    pub arrivals: Vec<SimArrival>,
    /// The rest of the energy, from ray tracing, in at most [`SimEchogram::MAX_BINS`] bins.
    pub late: SimEchogram,
    /// The room parameters of the simulated impulse response, octave bands, evaluated as a
    /// measured one's are.
    pub room: RoomAcoustics,
}

impl SimReceiver {
    /// The most arrivals kept per receiver, the earliest: later ones crowd together and the
    /// echogram has them, and a state of [`SimSettings::MAX_RESULTS`] receivers stays inside
    /// an event.
    pub const MAX_ARRIVALS: usize = 48;
}

/// A specular arrival.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimArrival {
    /// Arrival time after the source emits.
    pub time: Seconds,
    /// Energy per band of [`SimState::bands`], relative to the direct sound at 1 m in free
    /// field (the direct sound at d metres carries 1/d², less the air).
    pub energy: Vec<f32>,
    /// The surfaces it reflected from, in order; empty for the direct sound.
    pub path: Vec<Surface>,
}

/// An echogram: energy per band per time bin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimEchogram {
    /// Bin width.
    pub bin: Seconds,
    /// Per band of [`SimState::bands`], the energy in each bin (as [`SimArrival::energy`]).
    pub energy: Vec<Vec<f32>>,
}

impl SimEchogram {
    /// The most bins a state carries: the simulator's 1 ms bins are summed into whole
    /// milliseconds until the tail fits, so a long tail at many receivers stays inside an
    /// event.
    pub const MAX_BINS: usize = 128;
}

/// Why `sim.run` / `sim.fit` refused (`invalid`, with [`crate::ErrorDetail::Sim`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SimRefusal {
    /// The plan has no ceiling: outdoors there is no reverberant field to simulate.
    Outdoor,
    /// A surface names a material the simulator does not know.
    UnknownMaterial {
        /// The surface.
        surface: Surface,
        /// The name given.
        name: String,
    },
    /// The plan does not validate, or the simulator does not take its outline.
    Plan {
        /// Why, in a sentence.
        reason: String,
    },
    /// The plan has no speaker marker of this name.
    NoSpeaker {
        /// The name.
        name: String,
    },
    /// A source or receiver is not strictly inside the room.
    Outside {
        /// The marker's name or the point's label.
        name: String,
    },
    /// No source or no receiver given.
    Empty,
    /// The walk is a live walk: it measured no decay.
    NotASweepWalk,
    /// The walk measured no T20 in any band.
    NoMeasuredT20,
    /// A simulation is running (`sim.cancel` it first).
    Busy,
    /// A coverage map needs a zone, and a cell centre inside one at the spacing asked.
    NoZones,
    /// A coverage map at this spacing has more cells inside the zones than one map holds:
    /// ask a coarser spacing.
    MapTooFine {
        /// Cells the spacing gives.
        cells: u32,
        /// The most a map holds ([`SimMapRequest::MAX_CELLS`]).
        max: u32,
    },
}

/// What `sim.map` maps: the direct and early energy of the plan's speakers over its zones,
/// image sources only (no ray tracing: the early window is specular reflections and the
/// late field is the same everywhere to a first approximation).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimMapRequest {
    /// The plan, by name: the session's copy when it holds one, else the library's.
    pub plan: String,
    /// Speaker markers of the plan, by name, each an omnidirectional source; their
    /// energies add (incoherently).
    pub sources: Vec<String>,
    /// Grid spacing, metres: cells are squares this wide, each read at its centre at its
    /// zone's ear height.
    pub spacing: f64,
    /// The octave band mapped (a nominal centre of [`SimState::bands`]); `None`: every band
    /// summed (broadband).
    pub band: Option<Hz>,
    /// Energy counted from the direct sound's arrival for this long: the direct sound and
    /// the early reflections that add to it.
    pub window: Seconds,
    /// Highest image-source order, at most [`SimSettings::MAX_IMAGE_ORDER`].
    pub image_order: u8,
}

impl SimMapRequest {
    /// The spacing a map is asked at unless the operator says otherwise, metres: about a
    /// seat's row, coarse enough for a hall to fit [`Self::MAX_CELLS`].
    pub const DEFAULT_SPACING_M: f64 = 1.0;
    /// The default window, seconds: the direct sound and the early reflections the ear
    /// fuses with it.
    pub const DEFAULT_WINDOW_S: f64 = 0.05;
    /// The longest window accepted, seconds.
    pub const MAX_WINDOW_S: f64 = 0.5;
    /// The default image order: second-order paths are most of the energy within 50 ms in
    /// a hall, and a map has thousands of receivers.
    pub const DEFAULT_IMAGE_ORDER: u8 = 2;
    /// The most cells of a map: what one state event carries with room to spare.
    pub const MAX_CELLS: usize = 4096;
}

/// A coverage map: level per grid cell inside the plan's zones, relative to the map's
/// loudest cell. Simulated: omnidirectional sources, the absorption of its state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimMap {
    /// Cell width, metres.
    pub spacing: f64,
    /// Centre of cell column 0, row 0, plan metres; column `c`, row `r` is centred at
    /// `origin + (c, r) · spacing`.
    pub origin: [f64; 2],
    /// The band mapped; `None`: broadband.
    pub band: Option<Hz>,
    /// The energy window after the direct sound.
    pub window: Seconds,
    /// The cells inside a zone, row by row.
    pub cells: Vec<SimMapCell>,
}

impl SimMap {
    /// The lowest level a cell reads, dB re the loudest: the map's colour scale ends there.
    pub const FLOOR_DB: f32 = -40.0;
}

/// One cell of a coverage map.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimMapCell {
    /// Column.
    pub col: u16,
    /// Row.
    pub row: u16,
    /// Level, dB re the map's loudest cell (≤ 0, at least [`SimMap::FLOOR_DB`]).
    pub db: f32,
}

/// The absorption fit the daemon keeps for later runs on the same plan (the latest
/// `sim.fit` done), mirrored so a client shows it after a reconnect; dropped when its plan
/// changes or its walk is deleted. Not saved with the session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimFitKept {
    /// The plan it was fitted on.
    pub plan: String,
    /// The walk it was fitted to.
    pub walk: WalkId,
    /// The fit (always [`SimAbsorption::Fitted`]).
    pub absorption: SimAbsorption,
}
