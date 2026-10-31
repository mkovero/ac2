# Spatial walk: room plans, suggested mic positions, a guided average

Status: design (2026-10-28), reviewed in three rounds; stage 1 (G1) in progress. The operator wants three things: a spatial average of
N mics made in one step; a walkthrough that suggests mic positions inside a room of any shape and
follows the measurements through them; and, later, ray tracing that room. The simulator stays out
of ac2's way while it grows (§2).

## What exists

- **Average of live transfers.** A math channel `expr: average` over transfer measurements
  (one per mic, each with its own reference): power, complex or coherence-weighted, every
  operand re-referred to one delay, left-out operands named (*AVERAGE · 3 OF 4 POSITIONS*)
  ([math-channels.md](math-channels.md), [spatial-average.md](spatial-average.md)). Made with
  **Shift+M**, whose operator row starts on ÷. At most `MathConfig::MAX_AVERAGE` = 16 operands.
- **Average of stored traces.** The same maths over captures (`Ctrl+1 … 9`) or sweep runs (sweep
  runs are transfer-domain operands): a math channel of traces, or the one-shot `M`
  (`trace.average`).
- **Sweep gate** (`--gate`), and ISO 3382-1 parameters from a sweep IR
  ([room-metrics.md](room-metrics.md)), one position at a time. A sweep reads one mic
  (`SweepConfig::measurement_input`); several since G8 (§8).
- **The New measurement chooser** has Alignment and Programme meter rows
  (`ac2-scene/src/new_meas.rs`, `ac2-ui/src/state/new_meas.rs`).
- **What the wire says about averaging depth:** only per-column `ValidityMask::SETTLING` (no
  averaged block yet). How full the MTW stages are (`Mtw::fill_s`) stays inside `ac2-core`.

Missing: the average takes too many keystrokes; nothing knows where a mic stood or suggests where
it should stand; nothing has a room shape; nothing tells a client that a transfer has refilled
since a reset.

## 1. Spatial average in one step (G1)

A **Spatial average** row in the New measurement chooser opens the math form filled in:
- operator *average of several*, method **power**;
- every transfer measurement of the current session, running or stopped (a stopped one is left
  out as `stopped` until it is started, as in any average); **←/→** leaves one out, as now;
- the name `Spatial average` (then `Spatial average 2`, …).

**Enter** sends the same `meas.create` the math form sends. The average opens in its own pane
(a new measurement never takes over the focused one). With fewer than two transfer measurements
the row is dimmed with its reason (*needs two transfer measurements*) and does not open. The
change is UI only: no new key (the chooser is where measurements are made), no wire change.

## 2. Where the code lives (G2)

Two parts with different futures:

- **`crates/ac2-room`**, a workspace member: plans, validation, position patterns (§3–§4).
  Small, pure (no I/O, `forbid(unsafe_code)`, `serde` only), tested in the quick tier, and tied to
  ac2's product. Changes to it go with the ac2 change that needs them.
- **`ac2-roomsim`**, its own repository (github.com/mkovero/ac2-roomsim) and crates.io crate: the
  simulator (§7). It has its own geometry input types and no dependency on any ac2 crate; `ac2d`
  converts an ac2 plan into its input. Its growth (rayon, long tests, benchmarks, its own CI and
  release pace) costs ac2 one pinned version, and its commits never move ac2's `main`, so a
  parallel ac2 worker never re-tests because the simulator changed. It is created when stage 4
  starts.

**The wire is ac2's.** `ac2-proto` owns the plan's wire types (`PlanSpec`, `ZoneSpec`, `Marker`),
under `deny_unknown_fields`, `WIRE_LOCK` and the Python fixtures, like every other wire type.
`ac2-room` takes and returns them (it depends on `ac2-proto`'s model types only, as `ac2-traces`
does; `ac2-proto` depends on no ac2 crate, so there is no cycle). `ac2-scene` uses `ac2-room` for
the plan editor's validation and the FOH-relative readout (it is geometry, not DSP; the ac2-ui
no-DSP test does not forbid it). The crate map gains an `ac2-room` row. A plan change is a `PROTO_VERSION` bump like any wire change.

## 3. Room plans (G3)

A **plan** is a floor polygon plus an optional ceiling; the room is that polygon extruded (a
prism). It covers halls, fan shapes, L-shaped bars and wedges, and is what the simulator takes.

```
PlanSpec {
  name,                             // metres throughout
  outline: Vec<[f64; 2]>,           // simple polygon, CCW, ≥ 3 vertices
  ceiling: Option<f64>,             // None: outdoors (no ceiling, no simulation)
  zones: Vec<ZoneSpec>,             // where people are
  markers: Vec<Marker>,
  surfaces: Vec<SurfaceTag>,        // per outline edge + floor + ceiling: a material (simulation only)
}
ZoneSpec { name, outline: Vec<[f64; 2]>, ear_height: f64, weight: Option<f64>, seats: Option<u32> }  // weight None = area
Marker   { name, kind: MarkerKind (speaker | foh | point), at: [f64; 3], heading_deg: Option<f64> }
```

- **Coordinates for people are metres from FOH, facing the stage.** The FOH marker's heading is
  the direction the operator faces at the desk; every position reads *2.1 m left, 4.0 m back from
  FOH* as seen facing that way (back = away from the stage). Without a FOH marker with a heading,
  positions read from the first speaker marker; without that, plan x/y, and the readout says so.
  Pixels exist only inside the plan view.
- **Validation:** the outline is simple with non-zero area; every zone is a simple polygon inside
  the outline in plan view (a balcony is a zone with a higher ear height; what is under it is not
  modelled, §9); every speaker marker lies inside the outline and below the ceiling; a FOH or
  point marker may lie outside (a truck, an open side). z ≥ 0 for every marker. A plan that fails
  is refused with the reason (*edge 3 crosses edge 6*).
- **Editing in the app:** a vertex table (type `x y` per row) and a plan view where a click adds a
  vertex, snapped to 0.1 m, and a drag moves one; both edit the same list. The plan view is an
  `ac2-plot` primitive (lines, filled polygons, points, labels); its text and scale bar are
  `ac2-scene` truth, tested headless. Keys: §5.
- **Where plans live:** a **plan library** on the daemon, like the mic library
  (`plans/<name>.json` in its data directory, atomic writes through `ac2-paths`), and a copy in
  every session that uses one, so a session opened elsewhere carries its room. CLI:
  `ac2 plan new|list|show|import|export`.

## 4. Suggested positions (G4)

`ac2_room::pattern::suggest(plan, zones, n, preset, seed) -> Suggestion { points, relaxed }`.
Deterministic for a seed; the walk setup's reshuffle makes a new seed.

**Presets** (each rule editable):

| rule | **Tuning** (default) | **ISO 3382-1** (room parameters) |
|---|---|---|
| spacing between positions | ≥ 2 m (λ/2 at 86 Hz) | ≥ 2 m |
| distance from walls | ≥ 1 m (λ/4 at 86 Hz) | ≥ 1 m from every reflecting surface |
| distance from speakers | none: the front rows are seats too | ≥ `d_min = 2·√(V/(c·T))` with T given or measured |
| mirrored pairs | none across a mirror axis of room + speakers | same |
| `n` default | 8 | 6 / 8 / 10 for ≤ 500 / 1000 / 2000 seats (the zones' `seats`; asked when unset) |

- **Why no mirrored pairs:** in a room symmetric about an axis through the speakers, two positions
  mirrored across it see the same modes and reflections, so the second adds nothing to the average.
  A position on the axis is fine. The test is pairs: no two points within half the spacing (1 m at the default 2 m) of each other's
  mirror image, for each mirror axis the outline and speaker markers share.
- **The floor** is kept at the distance by ear height. A balcony soffit over a zone cannot be
  represented in a prism (§9); the setup says so when a zone lies under a higher one.
- **Why `d_min` only for room parameters:** it is the ISO rule for measuring the reverberant field
  with an omnidirectional source, about twice the critical distance (roughly 11 m in a
  10 000 m³ hall with T = 1 s, more for a directional PA). For tuning it would delete the front
  half of the stalls.

**Method:** split `n` over the zones by weight (default area), at least one per zone with a
non-zero quota. In each zone, place the points by a centroidal Voronoi relaxation (Lloyd) over
sample points of the zone, those closer to a wall than the margin rejected (the polygon is never
offset inward, which self-intersects for concave shapes), seeded from the seed. This spreads them evenly instead of
pushing them to the edge, as a farthest-point pick would. Then check the rules and nudge points
that break one. Rules that still fail are relaxed in a fixed order (spacing, then wall distance),
and `relaxed` names each and by how much (*spacing 1.6 m: 8 positions do not fit at 2 m*).

The operator may drag, delete or add points. A point that breaks a rule after an edit is marked on
the plan, not refused: the operator sees the pillar or lighting tower that the plan does not.

(A **seat cluster** pattern, points within about head size around one seat as in home room
correction, is not offered here. Points that close are correlated below about 570 Hz, and the
result holds for one seat only. It can be added later for control rooms.)

## 5. The walkthrough (G5)

A **walk** is daemon state (a second client sees it; it survives an app restart). A session may
hold several walks (one per system tuned, say), each with its own pane. A walk reads the
session's copy of its plan: a library edit mid-walk changes nothing, and points keep their `at`.
It names: a plan, the transfer or sweep measurements (one per mic), its points, and the
math channel it feeds.

```
WalkState { id, name, plan: PlanName, kind: WalkKind (live_transfer | sweep),
            mics: Vec<WalkMic>, points: Vec<WalkPoint>, step: Option<u16>,
            average: Option<MeasId>, room: Option<WalkRoom> }
WalkMic   { meas: MeasId, input: u16 }
WalkPoint { label: "P3", at: [f64; 3], step: u16, mic: WalkMic, state: PointState }
PointState: pending | captured { trace: TraceId, context: CaptureContext } | skipped
```

**A walk is a pane, not a window.** It shows the plan, the current point highlighted with its
readout (*Move mic to P3 · 2.1 m left, 4.0 m back from FOH*), the next point faint, and a state
line. The keyboard stays with the panes, so the stimulus keys (**Space**, **Enter**, **Esc**) and
a transfer pane's **R** work during the walk (an open window would own them, K9). Setup (plan,
zones, preset, `n`, kind, mics) is a window that closes before the walk starts.

**Moving the mic restarts the measurement, at placement, not at the key.** The operator presses
next, picks up the mic, walks and places it; a reset at the key press would average the carry.
So on a move (next, back, redo) the daemon resets every walk measurement's averaging
(`meas.reset`) and turns on **delay tracking** for it (decision 3a, `DelayState.tracking`). While
the mic travels, tracking follows the changing arrival; every tracking move restarts the MTW
stages anyway (`set_delay`). The operator never resets by hand. A loaded walk gets the same on
its measurements' first run after the load: the saved delay and average belong to the old
placement (operator 2026-10-29). Sweeps have no live averaging or
tracking: the run at the point is the capture.

**Ready, a new wire fact.** `TfMeta.filled: f32` (0…1), from a small `ac2-core` addition
`Mtw::fill() -> f64`: over the stages, the least of *blocks accumulated since the stage last
restarted ÷ blocks it needs to be full*. It counts blocks, not time, so protection-rejected blocks
add nothing and a measurement that never averaged never reads full. A stage is full at its
`fill_s` after a restart (discontinuity or delay change) or `settle_s` + one hop after an averaging
reset, exponential stages at their `settle_blocks()`, and *FastLf*-capped stages at their capped
length; the slowest stage is the one with the most blocks to go, not necessarily the deepest. A
math channel's frame carries the least `filled` of its included live operands (1 for traces).

*Ready* at a point = tracking has not moved the delay since the last restart, `filled` = 1, and no
fault banner. The walk pane shows *settling · 60 %* until then, and *arrival: pick one* when the
finder holds two near-equal arrivals (`awaiting_pick`, decision 1c), which waits for the operator's
pick. Capture is allowed earlier (the operator decides); the trace then records
`captured_unsettled`.

**The settling bar** (operator 2026-10-29). `filled` counts blocks, so a percentage jumps in the
stages' coarse steps and flickers. The pane shows the word (*settling*, *ready*) and a short bar
beside it that glides between frames exactly as the Leq bars do (`ac2_scene::bar_ease`, the same
*Leq bars* setting turns it off): app-side display easing of a wire fact, no new DSP. No standard
governs the indicator; it is a plain meter.

**Capture.** Live transfer: the capture key (below) sends `walk.capture`; the daemon captures
every walk measurement of the step in one step (one `trace.capture` per measurement, milliseconds
apart, each a settled average, which is harmless for a spatial average) and advances. Sweep: the
sweep fires with **Enter** as always; a complete, unrefused run at the current point is that
point's capture.

New `TraceMeta` facts (facts, not `TraceEdit`): `position: Option<TracePosition>` (label, x/y/z,
plan, walk id), `captured_unsettled: bool`, `superseded: bool` (a redone point's old trace, kept,
listed dimmed, out of the average).

**The running average** is an ordinary math channel, owned by the walk's first measurement (a math
channel's owner is never another math channel). The daemon creates it at the walk's second
capture (an average needs two operands) and updates its `of` (`meas.update`) on each capture,
redo or skip, as it already restarts maths whose operands change. Once made it stays when redos
and skips leave fewer than two captures (operator 2026-10-29): the panes showing it keep it, and it
reads *NO AVERAGE* until there are two again. A new average still needs two operands (`meas.create`). Before the second capture the
walk pane shows *NO AVERAGE · 1 OF 8 POSITIONS* from walk state. `MAX_AVERAGE` rises from 16 to
64 (positions × mics: ISO 3382-1 alone asks for 10 positions in a 2000-seat hall; the cost is one
pass per column per operand).

**Captures that differ** (operator 2026-10-29). A walk resumed from a saved session another day,
or after the chain changed, averages captures that differ in more than position. Each captured
point records a `CaptureContext`: when (the first trace's `created_at`) and, per walk
measurement, the mic and its correction curve and the EQ marked "in the console" (`EqMark` set +
rev), the same facts the traces record. Calibration and generator level are not recorded: a
transfer is a ratio of two inputs and both cancel out of it; ac2 knows no input gain. The pane
then shows a warning line, never a refusal: the captured points grouped by local day, oldest
first, then each recorded fact that differs (*P1–P4 yesterday, P5–P8 today · EQ changed*;
*captures differ · mic curve changed* on one day). Only recorded facts are named.

Traces captured before ready count in the average and the legend says so (*8 positions · 2
unsettled*), as it counts left-out ones. Captures are named by point and mic name (*P3 · Vilppu*),
never by number.

**Invariants:** a measurement, trace or math channel a walk names cannot be deleted while it does
(the owners check). Deleting the walk keeps its traces, and its average stays as an ordinary
average of them.

**Wire:** `walk.new|update|goto|capture|skip|redo|delete`, `WalkState` in the session state,
`PaneKind::Walk` / `PaneContent::Walk`, `TfMeta.filled`, the three `TraceMeta` facts, the plan types and plan-library commands. A
`PROTO_VERSION` bump and a session-format bump (`ac2-traces` `session::VERSION`).

**Keys** (each ships with its hint-bar tip, key-table row and user-guide row; a new `Scope::Walk`
in the key table and the keymap-doc generator). Pane-scope keys shadow global ones while the walk
pane is focused, so the walk avoids keys whose meaning differs in a transfer pane on screen beside
it (`.`/`,` nudge delay there) and global keys the operator expects (Delete):

| key | scope | does |
|---|---|---|
| **Shift+W** | everywhere | *Spatial walk…*: the window listing the session's walks, **New walk** first (as Shift+M / Shift+S open *New …*) |
| **X** | walk pane | capture here (mark the point) and go to the next |
| **PageDown** / **PageUp** | walk pane | next / previous point (moving there resets, above) |
| **Shift+R** | walk pane | redo the current point (its tip says it resets the walk's measurements) |
| **K** | walk pane | skip the current point (several mics: pick one, or the whole step) |
| **M** | walk pane | the mics trade places at this step (§6) |
| **Space** / **Enter** | walk pane, kind `sweep` | arm / fire the walk's sweep measurement (the walk pane's measurement, as a sweep pane's) |
| click / drag | plan view | add / move a vertex or a point |

`keys.toml` chord parsing (`ac2-ui` `keys/chord.rs`) learns the names `PageUp` / `PageDown`.

The setup window has no letter keys (K9: ↑/↓ rows, ←/→ choices, typed fields): the pattern is a
row (*pattern 3*, **→** next seed).

**Pane wire:** `PaneKind::Walk` and `PaneContent::Walk { walk }`, travelling in `meas.layout` and
sessions like any pane; `check()` refuses a walk pane without its walk.

## 6. Several mics (G6)

With M mics each step places all M. Points are grouped into steps of M by a nearest-neighbour
assignment from each mic's previous point, so cables move as little as possible; the operator may
swap which mic stands where. One `walk.capture` captures all M (§5).

As built (2026-10-29):

- **Grouping** (`ac2_room::steps::group`, pure): one mic keeps the walking order, a step per
  point. With M ≥ 2 the first step puts mic *k* at P*k* (the walk reads from P1); each later
  step is the exact least-total-distance pairing (Hungarian, `steps::assign`) of the mics'
  last points with the points left, on the floor plan. With fewer points left than mics, the
  points are paired with the nearest mics and the others sit the step out (N = 5, M = 2: the
  last step places one mic). Deterministic for the same points and M.
- **State.** Each point carries its `step` and its `mic` (the measurement that stands there:
  the grouping's choice while pending, the one that captured once captured); `WalkState.step`
  is the step the mics are at, each at its pending point of it; `Captured` holds the one
  trace of that mic and its `CaptureContext` (`at`, `mic`, `eq_in`). *Captures differ*
  compares captures of the same measurement only: two mics are two mics by design.
- **Mics** are 1 … 8 transfer measurements, each on its own measurement input (two
  measurements of one input would be one mic at two points). The setup window adds them
  with a *mic 2* … row (**→** adds the next transfer on an input of its own, *none* drops it).
- **Swap** (`walk.swap {points}`, key **M**): the mics at two or more pending points of the
  current step move round by one (two trade places; the key rotates every mic standing).
  The same exchange applies to the pending points of later steps, since those were planned
  from where each mic stood: whoever stands there now walks on. A swap is a move (§5 reset).
- **Skip and redo are per point.** A point that cannot be measured (a pillar, a seat taken)
  is one point, not a step: `walk.skip {point}` leaves it out and the other mics stay; the
  walk advances only when the step has nothing pending. `walk.redo {point}` makes one point
  pending for the mic that captured it and goes to its step, where only that mic stands (the
  others' captures there stay). With several candidates the keys open a chooser: the whole
  step first (a skip or redo per point), then each point by name (*P3 · Vilppu*). The CLI's
  `walk skip` without a point skips every point a mic stands at.
- **PageDown / PageUp** step through steps (`walk.goto {step}`), not points.
- **The walk pane**: the step line names each mic (*Move Vilppu to P3, MM1 to P4*), a line
  per mic in its colour gives its readout from FOH, and on the plan each standing point is
  ringed in its mic's colour (`Theme::trace_color` by the mic's place), the next step's
  faintly. One readiness bar, the **slowest** standing mic's (*settling · MM1*): a step is
  capturable when its slowest mic is, so one bar answers the one question; a bar per mic
  would make the operator find the minimum.

## 7. Simulation (`ac2-roomsim`, stage 4) (G7)

Geometric acoustics on the prism, per octave band: **image sources** for early reflections (to
order 3–4), with a visibility test per image (Borish 1984), since an L-shaped or concave outline
hides some images; **stochastic ray tracing** with scattering (Lambert) for the late tail; air
absorption after ISO 9613-1, which dominates T at 2–8 kHz in large halls.

What it can claim, and what it refuses:
- **Above the Schroeder frequency only,** `f_S ≈ 2000·√(T/V)` Hz. Below it the room is modal and
  geometric acoustics is wrong; the plot shades that region (faint grey, dashed edge) with
  *simulation not valid below 180 Hz*.
- **Absorption is unknown on site.** Materials come from a small built-in table per octave band.
  The simulation can be **fitted**: one absorption scale per band, chosen so the simulated T20 at
  the walk's positions matches the measured T20. The simulated T20 is computed by ac2-core's own
  room-metrics evaluation on the simulated IR, so like is compared with like. One scale per band
  is identifiable; per-surface absorption is not, so after a fit *which wall* a reflection comes
  from is geometry and holds, but *how loud* it is stays an assumption, and the overlay says so.
  An unfitted result says *absorption assumed*.
- **Output** is labelled *simulated*, is never a math-channel operand, and never takes a
  measurement's colour. Uses: predicted reflection arrivals at a point over the measured ETC (which
  wall made that 7 ms reflection), and a direct-plus-early-energy map over the zones per speaker
  marker. Speakers are omnidirectional at first, stated on the map; directivity tables later.
- **Later, as ideas (operator 2026-10-28, from a game-engine / light ray-tracing angle):** GPU ray
  casting (Vulkan ray queries through wgpu), since casting rays against triangles is the same loop
  as light; a 3D view where walls are lit by the energy they reflect at a chosen band; an
  animation of the wavefront and its reflections over time; and **auralization**, the simulated
  IR convolved with programme to hear a change (curtains on the back wall). Auralization emits
  sound only on the operator's explicit request at a typed level, under the global maximum and
  with a fade-out, never from tests. Where sound is not light: every ray carries an arrival time,
  energy is per octave band, and below the Schroeder frequency sound bends around objects instead
  of casting shadows, so the GPU view has the same validity limit as the plots.
- Runs in `ac2d` on a worker thread outside the audio path, with a time budget and progress; a plan
  change cancels the run. Outdoor plans (no ceiling) are not simulated.

**As built (stage 4b part 1: daemon, wire, text).** `ac2d` links `ac2-roomsim` (pinned git rev);
`ac2-ui` does not link it itself (its default `embedded` feature hosts a daemon, which does).
`sim.run` / `sim.fit` / `sim.cancel` (`docs/protocol.md` *Room simulation*), CLI `ac2 sim
run|fit|status|cancel`, texts in `ac2_scene::room::sim`.
- **Plan → prism.** The outline extruded to the ceiling; a surface's material by the simulator's
  name, an untagged wall or ceiling *plaster on masonry*, an untagged floor *concrete*; an
  unknown name is refused with its surface. Air at 20 °C, 50 % RH. Sources are the plan's
  speaker markers, omnidirectional; receivers a walk's points or given points, each strictly
  inside the room.
- **Worker.** One run at a time on its own thread with its own rayon pool (half the cores, 1 … 8);
  progress at most five times a second; the cancel flag is raised by `sim.cancel`, a change or
  deletion of the plan (saving it unchanged does not), deletion of the fit's walk, and
  `session.close`. Results are not saved with the session: they are re-made in seconds from the
  plan, and a saved result would outlive the plan edit that invalidated it.
- **Room parameters.** Per receiver, ac2-core's ISO 3382-1 evaluation of an impulse response
  synthesised from the arrivals and the late echogram (48 kHz). Each octave band is evaluated on
  a response synthesised from that band's energy alone: the synthesis band-limits its noise with
  gentle filters, so in one response the slow 125 Hz decay leaks into the 1 kHz filter and
  outlasts the 1 kHz decay (it read 4.3 s against Eyring's 0.72 s). Broadband from the whole
  response; no third-octave bands (the simulation has no finer detail than an octave). A
  source's `room` is the receivers' mean, least and greatest, as a walk's.
- **Fit rule.** One scale per octave band on every surface's absorption. Start where Eyring's T
  (with air) equals the measured T20 (fixed-point iterations, no simulation). Then simulation
  passes with the request's fixed seed; after each, per band, a Newton step on log T against
  log scale, `scale ← scale · (T_sim / T_meas)^(1/p)`: p = 1 first (T ∝ 1/absorption where the
  surfaces dominate), then p from the last two passes, clamped to 0.2 … 1.5, because absorption
  clipped at 1, the air's share and a field that is not diffuse make T follow the scale less
  than inversely. Scale clamped to 0.05 … 20. Stop when every fitted band is within 3 % of the
  measured T20 (about the spread between seats of one room) or after 6 passes; the last pass
  is the result, a band outside the tolerance flagged `converged: false`. A band the walk gives
  no T20 keeps the table's absorption (*assumed at … Hz*). The fit's scale is kept by the daemon
  and applied to later `sim.run`s of an identical plan.
- **Defaults.** Image order 3, 20 000 rays per band, 10 s budget, seed 0; limits order 4, 10⁶
  rays, 600 s, 64 receivers, 64 source × receiver results, 48 arrivals and 128 late-energy bins
  per result (the largest state fits the 1 MiB event).
- **Labels** (`ac2_scene::room::sim`): *simulated*; *absorption assumed* or *absorption fitted to
  4 positions*; *simulation not valid below 180 Hz* (Schroeder, two digits); *omnidirectional
  source*. The simulated room table brackets bands reaching below the limit. A simulation is a
  `SimState`, a type of its own, never a trace: no math channel can name it.
- **Part 2 (not yet):** the reflection overlay on the measured ETC, the coverage map, and the UI.

## 8. One sweep, several mics (G8)

`SweepConfig::measurement_input: u16` becomes `measurement_inputs: Vec<u16>` (one reference). One
play records every listed input; the run path (`ac2d/src/control/sweeps.rs`) reads the reference
plus each input, and each input's run is its own trace under the measurement, with
`TraceSource::Sweep` naming its input. Each input's calibration and mic curve apply (a mic curve
in use on an input applies to every measurement on it). Wire and session-format change.

*As built (PROTO 76, session format 39):* `SweepConfig::check_inputs` refuses with a typed
`SweepInputsError` (none, more than `MAX_INPUTS` = 16, one listed twice, one equal to the
reference) at `meas.create`/`meas.update`, at every run, in the CLI and in the dialog. The
recorder takes the reference and every input from one fan-out; the analysis thread runs each
input against the shared reference and `SweepStatus::Done` carries `traces` in input order.
Traces of a several-mic run are named `Run N · <input name>`; one input's refused analysis
fails the whole run, naming the input, so a run never stands for fewer positions than it
recorded. The dialog lists *Measurement 2, 3 …* rows below the first, one empty (*none*) row
after the last pick; the CLI takes `--mic 1,3` / `--meas 1,3`.

**Sweep walks** (*as built, PROTO 77, session format 40*):

- **A mic is a `WalkMic {meas, input}`**, a measurement and the input it records the mic on,
  for both kinds. A live walk has one per transfer measurement; a sweep walk names exactly one
  sweep measurement and its `measurement_inputs` are the mics (`walk.new {kind: sweep, meas:
  [m]}`; the daemon derives the mics, ≤ 8). One input is one mic in both kinds, so grouping,
  swap, skip, redo and *captures differ* compare mics by equality and work unchanged. The other
  model (a sweep measurement per mic) was not taken: it would play the stimulus once per mic
  at a step, against G8's one play. While the walk names it, the measurement keeps its kind
  and its inputs (`meas.update` refused).
- **A capture is a run.** `walk.capture` refuses a sweep walk; its capture is an ordinary
  `sweep.run` of its measurement, under the stimulus lease, armed, at the measurement's level
  and the global maximum, faded on stop: the walk adds no play path. When a sweep walk's mics
  stand at a step, the run records only the standing mics' inputs, names each trace
  `<point> · <mic>` and gives it its `TracePosition`. When the run is `Done` the standing
  points are captured by its traces in input order and the walk goes to the next step with a
  pending point. A refused, failed or stopped run leaves the points pending; the pane shows
  *sweep failed: … · P1, P2 stay pending* until the next run. While the walk's run plays or
  analyses, the walk does not move (`walk.goto`/`skip`/`redo`/`swap`/`update`/`delete`
  refused). A run with every point done records the measurement's inputs as an ordinary run.
- **No reset, no tracking** at a move: a sweep's run starts from nothing, and its delay is
  the reference's.
- **The average** is the walk's power-average math channel over the sweep traces (transfer
  domain), as for a live walk; the G9 band applies to it.
- **UI.** The setup's first mic row cycles the session's transfers, then its sweeps (*Sweep 1
  · sweep, 2 mics in one play*); choosing a sweep makes a sweep walk with no further mic rows.
  On a sweep walk's pane **X** (the walk's capture key) arms the walk's sweep when nothing is
  armed and plays it when it is armed, through the same arm/fire path as **Space**/**Enter**,
  which also work there. The ready line reads the run: *sweep plays at P1, P2*, *sweep
  playing · 1 of 2*, *analysing the sweep*.

## 9. Not offered

- 3D geometry beyond the prism: sloped ceilings, balcony soffits as surfaces, imported CAD. A
  sloped ceiling is the first extension if asked.
- Wave-based simulation (FEM/BEM) of the modal range.
- Adaptive positions chosen from measurements.

## 10. Spread across positions (G9)

With ≥ 3 captured positions, the average's plot can show the **range across positions** per
column, as a faint band (gentle plot hint style, toggle): below 6 positions the min–max envelope
of the positions' levels in dB; from 6 on, the 10th–90th percentile. The legend says which
(*range of 4 positions* / *10–90 % of 8 positions*). An envelope needs no centre, so it does not
pretend to be spread around the power average. A column is left out of the band where any
operand is below its coherence floor (the band would show noise, not seats). It shows where seats
agree (EQ helps all of them) and where they scatter (EQ helps some). With fewer than 3 positions
there is no band and the legend says *range: too few positions*.

As built (PROTO 76): the math job computes the band for every transfer average, any method,
live or stored operands (`ac2_traces::math::range_band`, quantiles linear at rank
`p·(n − 1)`); `MathState.range` says what it spans, the frame's `range_lo` / `range_hi` /
`range_coh` arrays carry it, display-smoothed as the curve. The coherence floor is the view's
coherence mask (**B**), the threshold that blanks a single curve: `range_coh` is the lowest γ²
among the positions and `ac2-scene` (`range_band`) drops a column below the mask (with the
mask off, none). Drawn in the magnitude pane with the resolution marker's shade and dashed
edge; T and Settings › Display › Range across positions hide it. Live only: captures of the
average and level (spectrum, RTA) averages carry no band.

## 11. Room parameters over a walk (G10)

A sweep walk gives ISO 3382-1 what it asks for: T20, EDT, C80 and D50 per band averaged over the
positions (arithmetic mean per band, with the range), shown in the room-metrics table as *mean
of 8 positions · 1 source* (ISO 3382-1 asks for at least two source positions; a PA walk has one,
and the table says so).

*As built (PROTO 77):* `WalkState.room: Option<WalkRoom>` is recomputed by the daemon after
every capture, skip and redo and on load, from the room analysis of each captured sweep trace
(`ac2d/src/control/walk_sweep.rs`; the mean and range are `ac2_core::room::positions::spread`).
`WalkRoom {positions, sources: 1, broadband, octave, third}`; each `WalkRoomBand` holds, for
EDT, T20, C80 and D50, a `PositionMean {mean, min, max, n}`. Bands are the union of the
positions' band centres (agreeing within 1 %). A position whose decay does not give a
parameter in a band (refused: too little range, too short) is left out of that band's mean,
and `n` says how many it is of; no position giving it is no value. `ac2-scene`
(`room::walk_room_table`) makes the table: a mean row per parameter with a *min–max* row under
it, *1.40 (3)* where fewer positions than the walk's give it, caption *Room (ISO 3382-1) ·
mean of 4 positions · 1 source · octave bands*, and the legend line *ISO 3382-1 asks for at
least 2 source positions; this walk measured from 1*. A sweep pane's *Room* view of the
walk's measurement shows it while no run of the measurement is selected (a selected run shows
its own); `ac2 walk status` prints it. Only the four parameters ISO 3382-1 reports per position
are averaged; T30 and C50 stay per run.

## Stages

1. **G1** Spatial average chooser row (UI only).
2. **G2–G5**: `crates/ac2-room` (plans, validation, presets, patterns), plan library and editor,
   walk pane with one mic and live transfer, `TfMeta.filled`, reset on move. PROTO + session bump.
3. **G6, G8, G9, G10**: several mics, multi-input sweeps, sweep walks, the range band, room
   parameters over a walk.
4. **G7**: `ac2-roomsim` repository; image sources, ray tracing, T20 fit, reflection overlay,
   coverage map.

## Tests

- `ac2-room`: polygon validation (self-intersection, orientation, degenerate edges, outdoor plan);
  determinism per seed; every rule holds on the suggestion or `relaxed` names it; a symmetric
  rectangle with a centred speaker gets no mirrored pair; concave and L-shaped zones keep every
  point inside; weights split `n` as stated; the relaxation order.
- `ac2-core`: `Mtw::fill()` after a restart, an averaging reset and with rejected blocks (stays
  below 1); FastLf capping.
- `ac2d` (fake backend): moving to a point resets each walk measurement and turns tracking on;
  a delay step mid-walk restarts `filled`; `awaiting_pick` reaches the walk pane; captures carry positions;
  redo supersedes and the average drops the old trace; the average appears at the second capture;
  a walk's measurements and traces cannot be deleted; session round-trip keeps walk and plan.
  A two-mic sweep walk over two steps: a run per step records the standing mics' inputs at
  their points (4 traces), a stopped run leaves the points pending, the average is the power
  average of the four, the room means lie inside their range; a skip takes a position out.
- `ac2-scene`: step text and readout from FOH, *settling · 60 %*, relaxed-rule notices,
  *NO AVERAGE · 1 OF 8 POSITIONS*, the dimmed chooser row and its reason, range-band legend.
- `ac2-ui`: an end-to-end walk from an empty daemon (fake backend).
- `ac2-roomsim` (its own repository): image sources in a shoebox against the analytic image
  lattice; visibility in an L-shape against hand-placed cases; T with diffuse reflections and
  uniform absorption against Eyring within 5 %; air absorption against ISO 9613-1 tables.

## Decided

- 2026-10-28 operator: positions read in metres from FOH facing the stage; pixels stay in the
  plan view.
- 2026-10-28 operator: the simulator (ray tracing) in its own repository, `ac2-roomsim`; plans and
  patterns in-tree as `crates/ac2-room`, since ac2's wire owns the plan types.
- 2026-10-28 operator: the area pattern first; a seat cluster later.
