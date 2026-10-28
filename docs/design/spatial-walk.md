# Spatial walk: room plans, suggested mic positions, a guided average

Status: design (2026-10-28). The operator wants three things: a spatial average of N mics made in
one step; a walkthrough that suggests mic positions inside a room of any shape and follows the
measurements through them; and a room model of that shape that can later be ray traced. The room
model stays a separate crate that can grow without slowing the rest of ac2's development (§2).

## What exists

- **Average of live transfers.** A math channel `expr: average` over transfer measurements
  (one per mic, each with its own reference): power, complex or coherence-weighted, every
  operand re-referred to one delay, left-out operands named (*AVERAGE · 3 OF 4 POSITIONS*)
  ([math-channels.md](math-channels.md), [spatial-average.md](spatial-average.md)). It is made
  with **Shift+M**, where the operator row starts on a ratio.
- **Average of stored traces.** The same maths over captures (`Ctrl+1 … 9`) or sweep runs: a
  math channel of traces or the one-shot `M` (`trace.average`).
- **Sweep gate** (`--gate`), and ISO 3382-1 parameters from a sweep IR
  ([room-metrics.md](room-metrics.md)). They are measured at one position; positions are the
  operator's job (§ *Not offered* there).
- **The New measurement chooser** has Alignment and Programme meter rows.

What is missing: the average needs too many keystrokes; nothing knows where a mic stood; nothing
suggests where it should stand; and nothing has a room shape.

## 1. Spatial average in one step (G1)

A **Spatial average** row in the New measurement chooser. It needs no new key, because the chooser
is already where a measurement is made. It opens the math dialog filled in:
- operator *average of several*, method **power**;
- every **running** transfer measurement of the current session included (**←/→** leaves one
  out, as now);
- the name `Spatial average` (then `Spatial average 2`, …).

**Enter** creates and starts it. With fewer than two running transfers the row is shown dimmed,
with its reason (*needs two running transfer measurements*), and does not open. The change is
UI only: it makes the same `meas.new` math request that Shift+M makes, so nothing on the wire
changes.

## 2. `ac2-room`: its own repository (G2)

The plan geometry, the position patterns and later the acoustic simulation live in **`ac2-room`**.
It has its **own git repository** (github.com/mkovero/ac2-room), is published to crates.io, and
ac2 depends on it by version, as with `cahal`.
- **Why a separate repository, not a workspace member:** in-tree, every `cargo nextest run
  --workspace`, clippy pass and CI push would build and test the simulator; out of tree, ac2 builds
  only what it depends on, and the simulator's growth (rayon, big tests, benchmarks) costs ac2 one
  pinned version. Its CI, test tiers and release pace are its own.
- **Features.** `default = []` is the plan and patterns module (`ac2_room::plan`,
  `ac2_room::pattern`): pure, no I/O, `forbid(unsafe_code)`, no dependencies beyond `serde`.
  `sim` adds the simulator (§7) and its dependencies. ac2 uses `default-features = false` and turns
  on `sim` only in the stage that shows simulation results, in the crates that need it (`ac2d`).
- **The boundary is data.** The plan types are plain serde structs that ac2-proto embeds. ac2 never
  re-exports `ac2-room` internals. A plan in ac2's session files is ac2's own format, written from
  those structs, so an `ac2-room` release that reorganises its internals does not move ac2's wire
  format unless the plan structs change. A change to the plan structs is a `PROTO_VERSION` bump in
  ac2, as for any wire change.
- **While stages 2–3 are being built**, ac2 points at a git revision of `ac2-room` (a
  `[patch.crates-io]` entry or a `git = …, rev = …` dependency). Each ac2 release pins a published
  version.

## 3. Room plans (G3)

A **plan** is a floor polygon plus heights; the room is that polygon extruded (a prism). This is
enough for halls, fan shapes, L-shaped bars and wedges, and it is what the ray tracer needs (§7).
Raked floors and balconies are zones, not geometry.

```
Plan {
  name, units: metres,
  outline: Polygon,                 // simple polygon, CCW, ≥3 vertices, no self-intersection
  ceiling: Height,                  // one height for the prism (a sloped ceiling: later, §9)
  zones: Vec<Zone>,                 // where people are: stalls, balcony, VIP pit …
  markers: Vec<Marker>,             // speakers, FOH, any named point
  surfaces: Vec<SurfaceTag>,        // per outline edge + floor + ceiling: a material name (sim only)
}
Zone { name, outline: Polygon, ear_height: Height, weight: f32 }  // inside `outline` in plan view
Marker { name, kind: Speaker | Foh | Point, at: (x, y, z), heading_deg: Option<f32> }
```

- **Coordinates** are metres in the plan's own frame. The walkthrough states every position
  relative to a marker the operator picks (default the FOH marker, else the first speaker) and
  that marker's heading: *P3 · 2.1 m left, 4.0 m back from FOH*. Without a heading it gives x/y in
  plan coordinates and says so.
- **Validation** (in `ac2-room`): the outline is simple and has non-zero area; every zone is a
  simple polygon inside the outline in plan view (a balcony is a zone with a higher ear height);
  every marker lies inside the outline and below the ceiling. A plan that fails is refused with
  the reason (*edge 3 crosses edge 6*).
- **Editing in the app.** A vertex table (type `x y` per row) and a plan view where a click adds a
  vertex, snapped to 0.1 m, and a drag moves one. Both edit the same list. The plan view is an
  `ac2-plot` primitive (lines, filled polygons, points, labels). Its text and the scale bar are
  `ac2-scene` truth, tested headless.
- **Where plans live.** On the daemon, like the mic library: a **plan library** in the daemon's
  data directory (`plans/<name>.json`, atomic writes through `ac2-paths`), and a copy inside every
  session that uses a plan, so a session opened on another rig carries its room. CLI:
  `ac2 plan new|list|show|import|export`.

## 4. Suggested positions (G4)

`ac2_room::pattern::suggest(plan, zones, n, rules, seed) -> Vec<Point>`. It is deterministic for a
seed, so the same plan and settings give the same points; **R** in the walk setup reshuffles (new
seed).

**Rules**, defaults after ISO 3382-1's guidance for measurement positions (all editable):
- **minimum spacing** between positions: 2 m (about half a wavelength at the lowest band the
  average is meant for), reduced automatically, and stated, when the zone cannot hold `n` points
  at that spacing;
- **distance from any wall** of the outline: ≥ 1 m;
- **distance from every speaker marker**: ≥ `d_min = 2·√(V / (c·T))` when the operator gives a
  reverberation time T (from a room-metrics measurement, or typed), otherwise ≥ 1 m. Closer
  positions are dominated by the direct sound and under-represent the room;
- **no two positions at the same distance from a speaker** (within 0.3 m) and **none on a mirror
  axis** of the outline through a speaker (within 0.3 m): positions in the same symmetric spot see
  the same modes and reflections, so they bias the average instead of sampling it;
- **spread over the zones by weight** (default: weight = area).

**Method.** Candidate points from a Poisson-disk sample of the zones at the spacing; candidates that
break a rule are removed; then a farthest-point selection picks `n`, starting from the point
nearest the zone's centroid. Rules that cannot all be met are relaxed in a fixed order (spacing
first, then wall distance), and the walk setup names each one relaxed and by how much.

**Two kinds of pattern:**
- **Area** (default): the rules above over the chosen zones.
- **Seat cluster**: `n` points within a sphere of radius r (default 0.3 m) around one marker. This
  is the room-correction habit of measuring around one listening position. The spacing and
  symmetry rules do not apply here, so the setup says the average is local to that seat.

The operator may drag a point, delete one or add one. Points that break a rule after an edit are
marked on the plan; they are not refused, because the operator can see the room and the plan
cannot (a pillar, a lighting tower).

## 5. The walkthrough (G5)

A **walk** belongs to one measurement setup: a list of mic inputs (one or more, §6), each with its
transfer or sweep measurement, a plan, and a list of points.

```
Walk { id, plan, anchor: MarkerName, kind: LiveTransfer | Sweep,
       meas: Vec<MeasId>,                       // one per mic, same kind
       points: Vec<WalkPoint>, average: MeasId }  // the math channel it feeds
WalkPoint { label: "P3", at: (x, y, z), mic: Option<MeasId>,
            state: Pending | Captured { trace: TraceId } | Skipped }
```

**Flow** (a dialog with its own key scope; Space/Enter stay stimulus keys everywhere):
1. **Setup:** plan, zones, pattern, `n`, kind, mics. The plan view shows the suggested points.
2. **Step:** *Move mic to P3* with the plan, the point highlighted, its coordinates from the
   anchor, and the next point faint.
   - **Live transfer:** the capture control enables when the measurement is **settled**: running,
     no fault banner, average depth full, and coherence above its usual floor. Settled is the
     daemon's existing state; the walk only reads it. It is a hint, not a gate: capture is
     allowed earlier, and the trace is then marked *captured before settled*.
   - **Sweep:** Enter fires it as now; the run, when complete and not refused, is the capture.
   - A capture stores a trace under the measurement, with `TraceMeta.position: Option<Position>`
     (label, x/y/z, plan name and walk id). The walk advances to the next pending point.
3. **Redo** a captured point (its old trace is kept, marked superseded, and leaves the average),
   **skip** one, or go **back**. Esc closes the dialog; the walk keeps its state and reopens
   where it stopped.
4. **Running average:** the walk's math channel averages its captured traces. It is created at
   setup and gains each trace as it is captured, so the average builds as you walk. With fewer
   than two captures it shows *NO AVERAGE · 1 OF 8 POSITIONS*, as any average does.

**Wire.** A walk is daemon state (it must survive the app restarting and be visible to a second
client): `walk.new|update|capture|skip|redo|delete`, `WalkState` in the session state, and a math
operand kind `Operand::Walk(WalkId)` that expands to the walk's current captured traces. That is
a `PROTO_VERSION` bump and a session-format bump.

## 6. Several mics (G6)

With M mics, each step places all M at once. The pattern's points are grouped into steps of M by
a nearest-neighbour assignment from each mic's previous point, so cables move as little as
possible. The operator may re-assign which mic stands where. A step's capture takes every mic's
trace in the same block (live transfers: one `trace.capture` per measurement on the same request;
sweeps: one sweep play recorded on every mic input, §8).

## 7. Simulation (`ac2-room` feature `sim`, a later stage) (G7)

Geometric acoustics on the prism: **image sources** for the early reflections (up to order 3–4),
**ray tracing** (stochastic, energy-based, per octave band) for the late tail.

What it can claim, and what it must refuse:
- **Valid only above the Schroeder frequency** `f_S ≈ 2000·√(T/V)` and where surfaces are large
  compared with the wavelength. Below `f_S` the room is modal and geometric acoustics is wrong, so
  results below it are not drawn. The plot says *simulation not valid below 180 Hz (Schroeder)*,
  in the faint-shade style of other validity hints.
- **Absorption is unknown on site.** Materials come from a small built-in table (concrete, glass,
  plaster, wood, curtain, audience) per octave band. The simulation can be **fitted**: scale the
  absorption per band so the simulated T20 at the walk's positions matches the measured T20
  (room-metrics). The fitted result says it was fitted and how far the fit misses. An unfitted
  result says *absorption assumed*.
- **Output** is always labelled *simulated*, is never a trace that a math channel averages with
  measurements, and is never shown in a measurement's colour. Uses: predicted early-reflection
  arrivals at a point overlaid on the measured ETC (which wall is that 7 ms reflection?), and a
  direct-plus-early-energy map over the zones for a speaker marker. Speaker directivity starts as
  omnidirectional, stated on the map; directivity data (a polar table import) comes later.

The simulator runs in `ac2d` on a worker thread outside the audio path, with a time budget and a
progress report. A plan change cancels a run.

## 8. One sweep, several mics (G8)

A sweep measurement today has one `input`. Stage 3 makes it `inputs: Vec<InputId>` (one
reference): one play records every listed input, giving one run per input, each its own trace
under the measurement. Every input's own calibration and mic curve apply (a mic curve in use on an input applies to
every measurement on it). The walk's sweep kind with M mics needs this. A wire and
session-format change.

## 9. Not offered

- Arbitrary 3D geometry (sloped ceilings, balconies as solids, imported CAD). The prism is stage 4
  at the earliest; a sloped ceiling is the first extension if the operator asks.
- Wave-based simulation (FEM/BEM) for the modal range.
- Choosing positions from measurements (adaptive sampling). The pattern is suggested before
  measuring and the operator moves points.

## 10. Spread across positions (G9)

With ≥ 3 captured positions, the average's plot can show the **spread per column**: the standard
deviation of the positions' levels in dB, drawn as a faint band around the average (gentle plot
hint style, toggle). It shows where the positions agree, which is where EQ helps every seat, and
where they scatter, where EQ helps only some seats. With fewer than 3 positions the band is not
drawn and the legend says *spread: too few positions*. The spread uses the same operands and
columns as the average (left-out operands do not count).

## Stages

1. **G1** Spatial average chooser row (UI only, no wire change).
2. **`ac2-room` 0.1** (new repository): plan, validation, zones, markers, patterns, with tests.
   **G3–G5** in ac2: plan library, plan editor, walk with one mic and live transfer.
   PROTO and session-format bump.
3. **G6, G8, G9**: several mics, multi-input sweeps, sweep walks, the spread band.
4. **G7** `ac2-room` feature `sim`: image sources and ray tracing, T20 fit, reflection overlay,
   coverage map.

## Tests

- `ac2-room`: polygon validation (self-intersection, orientation, degenerate edges); pattern
  determinism per seed; every rule holds on the suggested points, or the relaxation is reported;
  a symmetric rectangle gets no mirrored pair; L-shapes and concave polygons keep every point
  inside; rule relaxation order. `sim`: image sources in a shoebox against the analytic image
  lattice; ray-traced T for a shoebox against Sabine/Eyring within a stated tolerance; energy
  conservation for zero absorption.
- `ac2d`: a walk over the fake backend: captures carry positions; redo removes a trace from the
  average and keeps it stored; the math operand expands to the walk's captures; a session
  round-trip keeps the walk and the plan.
- `ac2-scene`: step text, coordinates from the anchor, relaxed-rule notices, *NO AVERAGE · 1 OF 8
  POSITIONS*, the dimmed chooser row and its reason.
- `ac2-ui`: an end-to-end walk from an empty daemon (fake backend), per the UX rule.

## Open (for the operator)

1. Creating the `ac2-room` repository on GitHub and publishing it to crates.io: yes, under
   `mkovero`?
2. The anchor's coordinates: left/right and back from FOH facing the stage, as proposed, or plan
   x/y always?
3. Seat cluster's default radius (0.3 m) and whether it should be offered at all in stage 2.
