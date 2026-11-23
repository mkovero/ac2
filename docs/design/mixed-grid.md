# Mixed grid: column density that follows the analysis

A transfer curve sits on one uniform log grid (`ppo` from `Resolution`). The MTW ladder
resolves that grid only above the deepest stage's validity edge `κ(ppo) · Δf`: at 1/48 about
68 Hz with the standard ladder (≈ 0.98 Hz bins), about 17 Hz with `Ladder::LongLf`
(≈ 0.24 Hz). Below it the frame marks the range unresolved and the plot shades it. This note
asks whether the grid itself should get coarser there ("mixed grid"), so that no column is
finer than the analysis.

## What happens below the edge today

Facts the options rest on, read from the code at `9a92f8e5`:

- `column_plan` (`ac2-core/src/mtw/mod.rs:478`) gives each column the bins whose centres lie
  in its edges (`bins_in`, :467). Below the edge some columns hold one bin, the rest none.
  A column with no bin is `ColumnSource::None` → `Validity::Thinned`
  (`mtw/engine.rs:501`), sent as `ValidityMask::THINNED` (`ac2d/src/jobs/transfer.rs:527`).
- So the frame already says, per column, which columns carry an own estimate. A column with
  several bins sums their cross-spectra (`column_estimate`): H1 and Gxx-weighted γ² over the
  column.
- The scene draws a live curve with `Fill::Gaps` (`ac2-scene/src/trace.rs:207`): invalid
  columns are NaN and a NaN breaks the polyline (`primitives.rs`, `Polyline`). Where fewer
  than half the columns hold a bin (below ≈ 34 Hz at 1/48, standard ladder) the curve is
  isolated points or two-column dashes.
- Display smoothing runs per run of valid columns (`ac2-core/src/smoothing.rs:249`). In the
  gapped range runs are one to three columns long, so a 1/3-octave setting smooths almost
  nothing there while the legend says 1/3 oct.
- Peak/notch landmarks need finite immediate neighbours (`cursor_jump.rs:139`): none are found
  in the gapped range, so room modes below the edge cannot be jumped to.
- A capture NaNs every non-valid column (`ac2-traces/src/ops.rs:111`) and a stored transfer
  trace keeps no `Unresolved` (`trace.rs:121` doc: "None … stored transfer curves"): the
  shading disappears on capture, the gaps stay.
- `resample` (`ac2-traces/src/columns.rs:149`) gives NaN where either neighbour is NaN. Math
  between a Standard and a LongLf capture at the same `ppo` (same grid id, no resample) is
  NaN wherever either thinned; across grids most of the gapped range goes NaN.
- A precedent exists: `GridDef::LogBins` (`ac2-proto/src/grid.rs:51`) is already a
  non-uniform grid on the wire (one column per bin, then `1/ppo` columns), used by the fixed
  spectrum display.

So the visible problem is less "too many columns" than "columns without an estimate break
the line, the smoothing and the peaks".

## Where the uniform grid is assumed

| Site | What it assumes |
|---|---|
| `ac2-core/src/grid.rs:25` `LogGrid {ppo,k_min,k_max}` | one `ppo`; `frequency(i)`, `edges(i)` closed form |
| `ac2-core/src/smoothing.rs:173-236` `Smoother` | one tap vector for every column ("the grid is uniform in log frequency"); half-width `0.75·ppo/b` columns |
| `ac2-core/src/smoothing.rs:334` linear-bin smoother | uniform linear bins (useful below the edge, see Smoothing) |
| `ac2-core/src/resolution.rs` `unresolved_ranges/columns` | compares cell to a uniform grid's column width |
| `ac2-core/src/mtw/mod.rs:478` `column_plan` | walks `grid.len()` columns; agnostic to spacing if `edges` are right |
| `ac2-core/src/mtw/layout.rs` `LADDER_DESIGN_PPO = 48` | crossovers placed for 1/48 regardless of display grid |
| `ac2-proto/src/grid.rs:26` `GridDef::Log`, `id()` tag byte 1 | grid identity = `(ppo,k_min,k_max)`; mirrored in `tools/protocol/ac2proto.py:153` |
| `ac2-proto/src/model/defaults.rs:25` `LogGridSpec::transfer(ppo)` | one spec per resolution (3 callers: `validate.rs`, `eq.rs`, `ir_wav.rs`) |
| `ac2-proto/src/model/dsp.rs` `Unresolved {resolution, ranges}` | ranges relative to one `Resolution` |
| `ac2d/src/control/validate.rs:428` `static_grid` | a transfer's grid is known from its config alone (used by `eq.rs`, `maths.rs` ×3, `meas_jobs.rs`, `load_check.rs`) |
| `ac2d/src/jobs/transfer.rs:274-289` | builds `GridDef::Log`, the `grid_id`, and `Unresolved` from the layout |
| `ac2d/src/jobs/transfer.rs:534` | smooths the frame with the uniform `Smoother` |
| `ac2d/src/control/eq.rs:31` `eq_grid` | EQ curve evaluated on the transfer's grid (any `GridDef` works: `centres()`) |
| `ac2d/src/jobs/math.rs`, `control/traces.rs` | carry `GridDef`/`grid_id`; no spacing assumption |
| `ac2-traces/src/smooth.rs:49-53` | smooths `GridDef::Log` and `Linear`; any other grid returned unsmoothed, silently |
| `ac2-traces/src/columns.rs:149` `resample` | linear in log f between adjacent columns; spacing-agnostic |
| `ac2-traces/src/math.rs:168`, `ops.rs:166` | same grid id → column-wise, else `resample` |
| `ac2-traces/src/text/table.rs:308` `import_grid` | off-grid imports land on uniform 1/48 or 1/96 |
| `ac2-traces/src/text/export.rs:245,311` | writes the grid JSON and one row per centre; any grid |
| `ac2-traces/src/session.rs:62,113` | stores `GridDef`; `VERSION = 48` |
| `ac2-traces/src/ir_wav.rs:98` | imported IR analysed onto `LogGridSpec::transfer(ppo)` |
| `ac2-traces/src/check.rs:31` | validates each `GridDef` variant |
| `ac2d/src/sweep.rs` | sweeps use the same uniform log grid, `Fill::Interpolated` |
| `ac2-scene/src/grid.rs` | grid-agnostic: `centres()`/`edges()` cached per id |
| `ac2-scene/src/unresolved.rs` | text "coarser than 1/48 oct (gaps)" from one `Resolution` |
| `ac2-scene/src/trace.rs:437` group delay | fit over ±1/(2b) octave by frequency; spacing-agnostic |
| `ac2-scene/src/cursor_jump.rs:110-145` landmarks | adjacent-column comparison; spacing-agnostic, gap-hostile |
| `ac2-scene/src/tf/mark.rs:13` cursor | nearest column in log f; may land on a thinned column |
| `tools/crosscheck/crosscheck/checks_levels.py:29` | infers `ppo` from the median log spacing of a sweep trace |

Counts in `crates/*/src` (tests and samples excluded): `GridDef::Log {` 17 sites in 13 files,
`LogGrid {` 15 in 8, `Smoother::new(` 22 in 4, `grid_id` 91 in 43. Most `grid_id` sites only
pass the id along and do not care what the grid is.

## Option A: a piecewise log grid on the wire and in traces

**What changes.** A new `GridDef` variant, e.g. `LogSegments { segments: [{ppo, k_min,
k_max}] }`, ascending and contiguous at segment joins. The daemon builds it per transfer
from the layout: for each stage below its validity edge, the finest `ppo ∈ {asked, asked/2,
…}` whose column is at least one bin wide. `LogGrid` in core becomes that piecewise type (or
a second type beside it); `column_plan` and `unresolved_ranges` work unchanged on `edges`,
and the latter returns empty by construction. The `Unresolved` marker turns into a
"resolution steps" marker: where the density changes, the scene says `1/24 oct below
68 Hz`.

**What breaks.** `Smoother` (one tap vector) needs taps per segment and a rule at joins;
`ac2-traces/src/smooth.rs` would otherwise skip the new grid silently. `static_grid` stops
being config-only: it depends on the ladder and sample rate (it already depends on nothing
else), so EQ, maths and load checks need the rate. `import_grid` keeps landing on uniform
1/48 or 1/96, so an exported-then-imported mixed trace changes grid unless the ac2 text
format's grid JSON is honoured (it is: `text/ac2.rs`). Python `ac2proto.grid_id` needs the
new tag. Crosscheck's median-spacing inference still reads the top `ppo`.

At 1/96 the ladder leaves the octave above each crossover unresolved too, so the grid would
step 96 → 48 → 96 at every crossover: non-monotonic density, more joins for smoothing.

**Trace math across grids.** Different grids already resample linearly in log f
(`resample`). Two mixed grids from the same ladder and asked `ppo` share an id and need
nothing. Standard against LongLf, or 1/48 against 1/96, resample onto the result grid; the
result grid should be the coarser of the two per frequency (resampling a coarse curve onto
fine columns invents detail). That is a new rule ("union by coarsest") in `ops.rs` and
`math.rs`.

**Wire and files.** New `GridDef` variant: `PROTO_VERSION` bump, `WIRE_LOCK`, Python
fixtures. Session `VERSION` bump (stored traces carry the new grid). `Unresolved` changes
meaning or goes.

**Size.** L: about 20 files across core, proto, ac2d, traces, scene, tools.

## Option B: uniform grid, draw only own estimates

**What changes.** Nothing on the wire: `THINNED` already marks every column without an own
estimate. In the scene, inside an unresolved range, a `Fill::Gaps` curve skips `THINNED`
columns instead of breaking at them, so the bins are drawn as points joined by a line (a
new `Fill::Joined`, text "joined bins" or similar). Other invalidities (settling, out of
band, no reference) still break the line. The shading and its text stay, so the operator
still sees where the curve is coarser than asked.

**Visible difference.** Today below ≈ 34 Hz (1/48, standard) the curve is dots and dashes;
with B it is a continuous line through each bin, with a point marker per bin so the line is
not read as detail between them. Each bin is drawn at its column's centre, at most half a
column (1/96 oct) from the bin's frequency; drawing at `k·Δf` exactly would need the bin
frequency per column, which the frame does not carry (it could be derived in the scene from
the layout, but the scene does not know the layout today). Against A, B shows every bin;
A would average 1–2 bins into each coarse column. Near the edge they are the same.

**Also needed for B to be honest.**
- Landmarks and cursor treat `THINNED` columns as absent: neighbours are the nearest own
  estimates. Cursor readout snaps to the nearest own estimate inside the range.
- Smoothing in the gapped range (see Smoothing).
- Captures keep the information: `StoredTrace` gets the frame's `Unresolved` (the struct
  exists, an imported IR's `IrFile` already has one), and the THINNED columns stay NaN, so a stored curve
  draws the same as the live one. That is a session-format field: `VERSION` bump.
- `resample` interpolates across NaN columns inside an unresolved range only between own
  estimates, so A−B across ladders keeps the bins it can.

**Wire and files.** No `PROTO_VERSION` change for drawing. Smoothing change in the daemon
changes frame values, not the format. Stored `Unresolved` on transfer traces bumps the
session `VERSION`; if traces travel on the wire with it, `PROTO_VERSION` too.

**Size.** M: scene `trace.rs`, `unresolved.rs`, `cursor_jump.rs`, `tf/mark.rs`; core
`smoothing.rs`; ac2d `transfer.rs`, `control/traces.rs`; traces `ops.rs`, `columns.rs`,
session field. About 10 files.

## Option C: resolution quick to change, daemon picks the finest resolved

**What changes.** A key cycles `Resolution` on the selected transfer (a keytip, a table row,
a guide row). Optionally a `Resolution::Auto { band }` that picks the finest `ppo` whose
`unresolved_ranges` is empty over the band (e.g. 20 Hz–20 kHz): standard ladder → 1/12
(resolved from ≈ 17 Hz), LongLf → 1/48 (from ≈ 17 Hz).

**What breaks.** Nothing structural. The stage sums are per bin and the grid only maps them
(`column_plan`), so a regrid need not lose averages; I did not check whether today's
config path rebuilds the job and starts over. `Auto` makes the grid depend on the ladder and
rate, like A; captures at different picks need resampling for math.

**Cost to the operator.** One `ppo` for the whole curve: on the standard ladder, picking
1/12 to make 20 Hz resolved throws away 1/48 where it is resolved (everything above
68 Hz). With LongLf, 1/48 is already resolved down to ≈ 17 Hz, so C plus LongLf covers the
usual band at the cost of a four-second window. That is the
trade the mixed grid wants to avoid.

**Wire and files.** None for the key. `Auto` is a new `Resolution` variant: `PROTO_VERSION`
and session `VERSION`.

**Size.** S for the key (ac2-ui keys, keytips, guide); S–M with `Auto`.

## Interactions

**Smoothing.** On a uniform grid one kernel serves every column. On A each segment needs its
own taps (`0.75·ppo_s/b` columns) and a column near a join takes taps from both sides by
frequency distance, not column distance. On B the own estimates below the edge are exactly
the deepest stage's linear bins, so `smoothing.rs`'s linear-bin smoother (fractional-octave
Hann over bins) is the right tool there, and the log smoother above; they meet at the
edge. Either way the rule is: a `1/b` label means a `1/b`-octave kernel in frequency at
every column, or the curve says it is narrower (a smoothing note where `1/b` is finer than
the resolution: a 1/48 kernel on 1 Hz bins at 30 Hz is no smoothing).

**Peaks and cursor.** Prominence is in dB, so non-uniform spacing is fine; gaps are not.
A and B both remove the gap problem. Peak frequency precision below the edge is the bin
(≈ 1 Hz or 0.24 Hz) whichever option; the readout should not print more digits than that.

**Coherence.** Per column, Gxx-weighted over its bins (`column_estimate`). A coarse A column
averages more bins, so its γ² is smoother and less biased low than a single bin's. B keeps
per-bin γ². Coherence blanking thresholds then mean slightly different things per segment
in A; in B they mean the same everywhere below the edge.

**Captured traces and trace math.** Today: captures lose the unresolved marker; math across
ladders or grids loses most of the gapped range. A: needs the coarsest-wins rule. B: needs
the stored `Unresolved` and gap-aware `resample`. C: as today.

**Text export/import, REW/OSM.** Export writes every centre with its value (NaN rows for
thinned columns today). A exports variable density; REW and OSM import arbitrary
frequency lists as far as I know, and ac2's own format carries the grid JSON. B could skip
NaN rows in the unresolved range on export, which makes the file non-uniform too, honestly
so. Crosscheck reads sweep traces and infers `ppo` from the median spacing; neither option
breaks that, but a mixed sweep grid would need it to read per range.

**EQ.** `eq.rs` evaluates the operator's sections on the transfer's grid centres; there is
no automatic fit in the code. Any option works. The prediction (measurement × EQ) is
column-wise on one grid, so under A the EQ curve takes the mixed grid too, which makes a
narrow LF notch look coarser than the filter is; under B the EQ curve stays on the uniform
grid and is drawn at full density (it is analytic, not measured).

**IR.** The live IR comes from stage 0's linear H1 (`transfer.rs` `ir_frame`,
`stage_h1_into(0, …)`), not from the log grid: no option touches it. An imported IR
(`ir_wav.rs:98`) is analysed onto the uniform transfer grid with its own `Unresolved` and
`Fill::Interpolated`; A would give it a mixed grid by its window length, B and C leave it.

**Trust is a separate question.** "Unresolved" means coarse, not wrong: one bin at 30 Hz is
as valid as any. What makes LF untrustworthy is low γ², too few averages, a window shorter
than the room's decay, or settling. Those already have their own marks (coherence
blanking, validity masks, `eff_avg`, settling range). The resolution marker should say the
resolution plainly (e.g. `below 68 Hz: 1 Hz bins (joined)`), not look like a warning, so it
is not misread as "do not trust".

## Recommendation

**B**, with smoothing and captures fixed in the same change. Reasons:

- The frame already carries exactly the information a mixed grid would encode
  (`THINNED` per column); the problem the operator sees is how the gap is drawn, smoothed
  and searched, not the grid.
- B shows every bin the analysis has; A averages some away and adds a grid type every crate
  must handle, a session and protocol bump, and a coarsest-wins rule for math.
- B keeps one grid per resolution, so captures, imports, EQ and math across measurements
  keep comparing column for column, and the same quantity reads the same in every view.
- The resolution marker stays and can say the bin width in hertz, which is the honest
  statement below the edge.

C's key is cheap and worth having beside B, without `Auto`.

## Open questions for the operator

1. In the unresolved range, joined bins with a point per bin, or joined without points?
2. Marker text: `coarser than 1/48 oct` as today, or the bin width (`1 Hz bins`)?
3. Smoothing below the edge: true `1/b`-octave over the bins (curve smoother than today at
   LF), or leave it and say "not smoothed below 68 Hz"?
4. Should text export skip thinned rows (non-uniform file) or keep NaN rows (uniform file)?
5. A resolution key on the transfer: which key, and does it need to keep averages across a
   change (only matters if today's path restarts the job)?
6. If A is still wanted later: should sweeps get the mixed grid too (their unresolved ranges
   come from the sweep window, not the ladder)?

## Decisions

The operator chose B, then bins on the wire as a further step.

1. In the unresolved range: a point per bin, joined by a line.
2. The marker says the bin width (`1 Hz bins`), not `coarser than 1/48 oct`.
3. Smoothing does what its label says everywhere: a true fractional-octave average over
   the bins below the edge too.
4. Text export skips the rows without an estimate of their own.
5. A resolution change may start the averages over; keeping them is wanted if it is cheap.
6. Sweeps and transfers behave alike: what is drawn, marked, smoothed and exported in a
   range the analysis does not resolve follows the same rules for both.

Bins on the wire: the analysis already holds one set of bins per stage. Each stage's bins
can be a stream of its own, so a viewer zoomed into the low end need not receive the
high-rate stage's bins. The uniform grid stays the form captures, trace math and EQ work
on; the bins are for viewing. Its design is a note of its own (`bin-streams.md`).
