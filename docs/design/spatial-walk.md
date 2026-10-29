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
  (`SweepConfig::measurement_input`).
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
            meas: Vec<MeasId>, points: Vec<WalkPoint>, current: Option<u16>,
            average: Option<MeasId> }
WalkPoint { label: "P3", at: [f64; 3], mic: Option<MeasId>, state: PointState }
PointState: pending | captured { traces: Vec<TraceId> } | skipped
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
stages anyway (`set_delay`). The operator never resets by hand. Sweeps have no live averaging or
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
| **K** | walk pane | skip the current point |
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

## 8. One sweep, several mics (G8)

`SweepConfig::measurement_input: u16` becomes `measurement_inputs: Vec<u16>` (one reference). One
play records every listed input; the run path (`ac2d/src/control/sweeps.rs`) reads the reference
plus each input, and each input's run is its own trace under the measurement, with
`TraceSource::Sweep` naming its input. Each input's calibration and mic curve apply (a mic curve
in use on an input applies to every measurement on it). Wire and session-format change.

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

## 11. Room parameters over a walk (G10)

A sweep walk gives ISO 3382-1 what it asks for: T20, EDT, C80 and D50 per band averaged over the
positions (arithmetic mean per band, with the range), shown in the room-metrics table as *mean
of 8 positions · 1 source* (ISO 3382-1 asks for at least two source positions; a PA walk has one,
and the table says so). Stage 3.

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
