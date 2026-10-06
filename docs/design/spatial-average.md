# Live spatial average of transfer functions

Status: implemented (`MeasKind::SpatialAverage`, the daemon's average job
`crates/ac2d/src/jobs/average.rs`, `ac2 meas new avg`, the app's "New spatial average"
dialog). Answers PLAN.md §3.3 "Live spatial average of N transfer functions".

## What the operator gets

A loudspeaker's response changes from seat to seat, so a system is tuned to the average of
several mic positions rather than to one spot. With one mic per position (one transfer
measurement each: same reference, its own mic input), a **spatial average** is a
measurement that names those transfer measurements by name and publishes their average as
its own `tf` stream, updating with every frame. Every client draws it like any transfer
function, the CLI watches it (`ac2 meas list --watch`), and it captures to a stored trace
(`trace.capture`) that names the positions averaged and the method.

One mic moved from seat to seat is the stored-trace average (`trace.average`, `M` in the
app): capture each position, then average the captures. Its mathematics is the same, so
the two agree.

## Definition

Members: 2 … 16 distinct transfer measurements on one log grid. Every live member runs in
the current session epoch, so all members share one time base (decision 8a) and their
phases can be combined. What is averaged, per frame:

1. Each member's **current result before display smoothing**, formed on request (the same
   request `trace.capture` makes), with its mic curve already taken off the magnitude. The
   members' *published* frames are not used: they exist only while someone subscribes, and
   averaging whichever frames happened to be sent would average results of different ages.
2. Each member `k` was measured with its own inserted delay `τₖ` removed
   (`hₖ = Hₖ·e^{+j2πfτₖ}`). Before combining, every member is **re-referred** to one
   reference delay `τ_ref`: `hₖ·e^{−j2πf(τₖ − τ_ref)}` (`ac2_core::average`). `τ_ref` is a
   member's inserted delay (default: the first member's) or an explicit delay; every frame
   states the delay used (`TfMeta.delay`). The phase of the average therefore shows each
   position's arrival relative to that reference — position-to-position timing is not
   silently removed.
3. The combination is `trace.average`'s (`ac2_traces::ops::average_on_time_base`, shared
   by both): **power** (default) `|H| = sqrt(mean |hₖ|²)`, the level over the positions
   without phase cancellation, phase the argument of the complex mean; **complex**
   `mean hₖ`, what one point summing the arrivals would see (arrivals that differ in time
   cancel); **coherence-weighted**, the complex mean weighted by `γ²/(1 − γ²)` (the inverse
   variance of an H1 estimate, γ² capped at 0.999).
4. A column has a value only where **every included member** has one; elsewhere the
   column's validity mask is the union of the members' reasons. Averaging a varying subset
   per column would draw steps where positions drop in and out, which read as response
   features.
5. Display smoothing, if the average has one, is applied **after** averaging, to the
   averaged columns (the members' own smoothing is not used), the same way a stored trace is
   re-smoothed. A capture holds the unsmoothed average and shows it smoothed.

**Coherence of the average:** not estimated. The published `coh` is the plain mean of the
included members' γ² — a display mask (alpha, blanking), not a coherence of the averaged
transfer function, exactly as for `trace.average`. A coherence of a spatial average would
need cross-spectra between positions that no member has.

**Calibration:** a transfer function is a ratio; the average is uncalibrated like its
members. `mic_curve` is true only when every included member had its mic curve applied.

## Members left out (principle 8)

Whenever a frame is due, each member is asked for its result. A member is **left out** of
that frame, and the frame says so per member (`TfAverage.members[k].status`), when it is:

| status | when |
|---|---|
| `stopped` | not running (or no job) |
| `settling` | running without a usable result: no valid column yet, nothing formed, another epoch or grid, or no answer within 250 ms |
| `refused` {protection} | its own frame raises a fault banner: `CLIP`, `NO_REFERENCE`, `CHECK_ROUTING`, `NO_SIGNAL` |

`WEAK_REFERENCE` and `DISCONTINUITY` do not leave a member out: they hold or restart the
member's averaging, which its validity mask already reports per column.

Fewer than two usable members is **no average**: every column NaN with the validity bit
`FEW_MEMBERS`. One position is not an average, and drawing it as one would mislead. The app
shows `NO AVERAGE · 1 OF 4 POSITIONS` (fault) naming each position left out and why; with
some positions left out it shows `AVERAGE · 3 OF 4 POSITIONS` (warning) and the legend
says `3 of 4 positions · power avg`. A capture refuses when fewer than two positions are in.

When the reference member is left out, its newest seen inserted delay stays the
reference, so the averaged phase does not jump to another member's arrival.

## Members' invariants

While an average names a measurement, the daemon refuses to delete it, to change it into
another kind, or to move it to another grid (`refused`, naming the average): the average
would otherwise stop meaning what its name says. `meas.reset` of an average is `invalid`
(it holds no averaging of its own); the app's reset on an average resets its members.

## Daemon

The average is a job like the others, fed by the capture fan-out although it analyses no
audio: it publishes on its members' clock, its frames carry the session's sample index, and
it goes STALE exactly when audio stops. Control keeps a registry of running transfer jobs'
probes (`jobs::Probes`); when a frame is due (only while someone subscribes, or for a
capture), the average sends every member a capture request first and then collects the
answers, so the wait is the slowest member's. The members never wait on the average.

## Wire and files

- `MeasKind::spatial_average {config: SpatialAverageConfig}` = {`members`, `method`,
  `reference`: `member` {meas} \| `fixed` {delay}, `smoothing`}.
- `TfMeta.average`: `TfAverage` {`method`, `members`: [{`meas`, `status`}]} (nil for a
  transfer measurement). Validity bit `FEW_MEMBERS` 512.
- `TraceSource::spatial_average` {`meas`, `meas_name`, `epoch`, `at_sample`, `method`,
  `members`: [{`meas`, `name`}]}: shares its epoch's time base like a capture.
- `PROTO_VERSION` 19, session format 9 (sessions hold averages; a load checks their
  members against the loaded measurements).

## Tests

- `ac2d` unit tests (`jobs/average/tests.rs`): ±3 dB members → power average
  `10·lg((10^0.3 + 10^−0.3)/2)`; two members of one path 0.5 ms apart, each aligned by its
  own delay → complex average `|cos(πfτ)|` at every column; equal coherence → weighted =
  complex; members left out by status; a member's gap stays a gap with its reason; the
  reference member left out keeps its delay; one member → `FEW_MEMBERS`.
- `ac2-traces`: the shared time-base average against the same analytic values.
- `ac2d/tests/spatial_average.rs`: a fake rig with +3 dB, −3 dB and silent mic inputs: the
  live average reads the analytic power average within 0.3 dB, the silent position is
  `refused` (NO SIGNAL), the capture names the two positions, member delete / grid change /
  average reset are refused, a stopped member gives `FEW_MEMBERS`.
- `ac2-cli/tests/avg_rig.rs`: `meas new avg` by member names; refusals reach the operator.
- `ac2-scene`: legend tags and banners (`average.rs`, `banner.rs`).
- `ac2-ui/tests/embedded.rs`: from an empty daemon, two positions, the dialog by name, the
  live average at −6 dB, its legend, a position stopped → the NO AVERAGE banner naming it.

## Open questions

- A complex average with every member aligned to its **own** arrival (the speaker's
  response without position-to-position timing) is not offered: the stored-trace average
  has no such reference either. If wanted, it is a third `AverageReference` (each member's
  own delay), on both averages.
- Positions are unweighted (each counts once); per-member weights (e.g. by audience area)
  are a later option.
- The app's "capture position → add to average" flow for one moved mic is not built; the
  stored-trace average covers it today.
