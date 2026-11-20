# Dynamics measurement: attack, release and their programme dependence

A measurement that plays a tone-burst programme through a dynamics processor (limiter, compressor,
VCA card) and its reference loop, and stores how the processor's gain moves: attack, overshoot
and release per step size and burst length, the distortion while the gain moves, and the
low-frequency thump a VCA leaks when its control voltage moves fast. One run is one trace, so
two runs (before and after a VCA swap, one channel against the other) overlay and compare.

The ordinary generator cannot do this. It ramps every level change over 20 ms (`RAMP_SECONDS`),
which hides any attack faster than 20 ms, and a level change sent over the wire lands at no known
sample. The first bench run of a Studer VCA limiter card (2026-11-20) bounded attack at "faster
than the ramp" and measured release once, after one 3 s hold; programme-dependent release (a
short burst recovers faster than a long one: the card's detector has an RC ladder of roughly
18 ms, 70 ms and 2 s sections, spec "attack 1 ms, release 50 ms–5 s programme dependent") was
untestable.

## Stimulus

One sine carrier, played on every output of the measurement (the processor's inputs and the
reference loop's output alike). The level follows a programme:

1. **Linearity check.** `pilot − 6 dB` for 2 s, then `pilot` for 2 s.
2. **Steps.** Each `DynamicsStep` is a burst at `burst` level for `d`, then a rest at `pilot`.
   The steps are the product of the `bursts` level list and the `durations` list, durations
   inner (every duration at one level, then the next level), in a fixed order: a repeatable
   programme is what makes two runs comparable.
3. The run ends with the generator's fade-out.

- **No ramp inside the programme.** A level change is a step, the stimulus a dynamics test
  needs. `onset` (typed: `zero` or `peak`) places it at a positive-going zero crossing (the
  waveform stays continuous; the default) or at a positive peak (the hardest case for a peak
  detector: from a zero-crossing onset the signal needs a quarter period to reach its peak, so
  an attack shorter than a quarter period cannot show, and overshoot depends on onset phase).
- **Pilot.** The rest level is a pilot tone below threshold (a Studer card at +4.9 dBu threshold:
  pilot about −20 dBu). Gain can only be read while a carrier is present, and a pilot well below
  threshold does not feed the detector. No defaults for `pilot` or `bursts`: the operator types
  every emitting level.
- **Adaptive rest.** A rest ends when the processor has recovered: at least `rest_min` (2 s)
  passed, and the gain is flat (the medians of the last two 1 s windows differ by less than
  0.05 dB; single samples would send every rest to the maximum on noise), up to `rest_max`
  (default 30 s; at τ ≈ 2.7 s exponential in dB, 20 dB of reduction takes 12 s to reach
  0.2 dB). The source counts `rest_max` itself, so a stalled job thread cannot leave the carrier
  playing and the run time shown at arm is a bound. The job thread can only shorten a rest: it
  publishes a monotonic "released through step n" counter that the source reads (an atomic load)
  in `fill`; the step lands at the next onset point. A rest that reaches `rest_max` marks the
  next step "not recovered". Edges are found in the recording (below), so a step need not land
  at a precomputed sample. This is the one source whose output depends on when blocks are cut
  (through the release time); the module doc says so.
- **Replayed rests.** A detector's slow section can hold charge with no visible gain reduction,
  so different rest lengths can change the next step. The actual rest lengths are stored with
  the run, and a run can replay the rests of a previous trace (`rests_from`) instead of adapting,
  so an A/B pair plays an identical programme.
- **Carrier.** Typed as a frequency, snapped at run time to the nearest frequency whose period
  is a whole number of samples at the session's rate, even for `zero` onset and a multiple of 4
  for `peak` onset (so the peak lands on a sample and edge midpoints are exact): 1 kHz at
  44.1 kHz plays as 1002.27 Hz, 44 samples; the actual frequency and period are stored with the run. Burst lengths are whole
  periods; the actual lengths are stored. Highest carrier `fs / 12` (8 kHz at 96 kHz: 0.125 ms
  periods, for attack). One carrier per run: a 100 Hz run, a 1 kHz run and an 8 kHz run of the
  same measurement are three traces.
- **Shortest burst: 3 carrier periods.** Edges are found in the reference's centred
  one-period envelope (*Edges* below), which ramps over the period around each edge; each
  edge's midpoint is refined against the levels two periods either side of it, so two periods
  after the start must still be on the burst's plateau, half a period before its end: 2.5
  periods, 3 whole ones. A shorter burst's envelope is a ramp that never plateaus (one period:
  a triangle), so neither its edges nor any time inside it can be read. A burst played as
  fewer than 3 periods is refused, naming the shortest at that carrier: at `meas new` on the
  typed carrier, and again at every run on the carrier as snapped to the session's rate. Three
  periods leave the attack searches a period and a half after the skipped first period;
  `settled` needs a longer burst (its last 10 % must hold a period of envelope wholly inside
  the burst: 15 periods), and the 4-period harmonic windows and the thump gate read "can't
  tell" on short bursts by their own rules.
- **Defaults.** `durations`: those of 1, 3, 10, 30, 100, 300 ms, 1, 3, 10 s (the release knee
  of a programme-dependent detector sits between 10 ms and 1 s) that last at least 3 periods
  of the carrier, chosen at `meas new`: 3 ms … 10 s at 1 kHz, all nine at 8 kHz, 30 ms … 10 s
  at 100 Hz. One fixed list cannot suit every carrier (3 periods are 0.375 ms at 8 kHz and
  30 ms at 100 Hz), and a default the run then refuses would read as a fault; an explicit
  list is the operator's and is refused, not trimmed. `onset` zero.

### Generator contract

The programme is a new `Signal` variant in `ac2-core::generator`. Its unit-RMS normalisation is
taken at the highest burst level, so the generator's level is that highest level: the crest
factor √2 holds for `validate_level` and the clip count, and the ceiling is checked once, at
every run. The step table is computed in `Generator::new`; `fill` reads it and the release flag
only, with no allocation. `is_finished` covers the source. A level change during a run is
refused, as it is during a sweep ("a sweep is playing"); stop, lease expiry and a closed session
abort with the ordinary fade-out. The module's doc ("level changes never step") gains the
exception: the programme's own steps, inside one constant generator level. At arm, the CLI shows
the highest level and the longest run time (all rests at `rest_max`) for the typed-level
confirmation.

## Analysis

### Envelopes

Reference `r(t)` (the loop input) and each measured input `y(t)` are mixed down by the carrier
and by its harmonics 2–5 (`x(t)·e^{−j2πk f_c t}`), each averaged over exactly one carrier period.
That gives complex envelopes `R_k(t)` and `Y_k(t)` at the full sample rate; the one-period
average cancels the carrier, its harmonics and the 2k·f_c mixing products without ripple, but
only for a steady fundamental. The
gain is `g(t) = 20·log10(|Y_1(t)| / |R_1(t)|)`, with the baseline `g₀` (median over the last
second of the linearity check at `pilot`) subtracted, so gain reduction reads 0 dB below
threshold. Dividing by the reference cancels the interface's response and any level ripple of
the generator.

- **Harmonics while the gain moves.** A fundamental whose gain is moving has sidebands, and a
  one-period boxcar leaks them into the k·f_c mixers: an ideal, distortion-free limiter reads
  fake h2 of −37 dB (1 kHz carrier, 18 ms attack) to −25 dB (1 ms attack), more than a real
  VCA's distortion, and changing with attack speed. So `Y_k` for k ≥ 2 uses a Hann window of
  4 periods, and each window gets a leakage floor: the reconstructed fundamental
  `Re(Y_1(t)·e^{j2πf_c t})` pushed through the same mixers. Harmonic `k` reads
  `20·log10(|Y_k| / |Y_1|)`, printed as "≤ floor" where it is not at least 6 dB above that floor.
  Every sample whose 4-period window contains an edge is excluded (the boxcar-reconstructed
  fundamental smears a step differently from the real one, and an ideal limiter then reads
  20–26 dB above the floor), so harmonics are blind for 4 periods after each edge: 4 ms at 1 kHz,
  0.5 ms at 8 kHz.

- **Delay.** The processor's own group delay `δ` would make `R` step before `Y` inside the
  averaging window and fake a gain dip at every burst start (2 samples read about −1.5 dB on a
  20 dB step at 1 kHz / 96 kHz). `δ` is measured on the linearity check's 6 dB edge, after its
  gate passes: the centroids of the rises of `|Y_1|` and `|R_1|` (envelope delay; a midpoint crossing moved by about 1.5 samples with even-order distortion's DC shift; the phase of
  `Y_1/R_1` is phase delay and reads half a period on an inverting device). `Y` is shifted by
  the whole samples; the threshold searches below skip the first period plus one sample after
  each edge.
- **Edges.** Each step's start and end are found in the reference envelope `|R_1|` (the midpoint
  of its one-period rise or fall, exact for a step) and checked against the programme (step
  count, burst lengths within `max(1, P/64)` samples, against the 6 dB edge's midpoint). The
  midpoint is read on `R_1`'s component in phase with the louder plateau, not on `|R_1|`: an
  AC-coupled reference loop adds a decaying offset to a zero-onset step whose carrier
  content is partly in quadrature, and `|R_1|` weighs that by the step's size over the level
  beside it, so the 6 dB edge and a 28 dB burst edge moved differently (2.7 samples at
  100 Hz through a 5 Hz high-pass at 96 kHz); in phase, the shift is the same fraction of a
  period on every edge and the grid cancels it (0.24 samples left there). Any real mismatch
  is a whole period off. A mismatch or a capture gap aborts the run as a
  dropout, as a sweep capture does (`crates/ac2d/src/jobs/sweep.rs`).
- **Resolution** is one carrier period. A crossing inside the skipped window after an edge reads
  "≤ one period"; one that never happens reads "can't tell"; neither is printed as a number.
- **Reference required.** A dynamics measurement without a loopback reference is refused at
  create (settled decision 3b).
- **Memory.** The analysis runs on the job thread as the audio arrives, step by step; raw audio
  is kept only from the step being analysed on (burst + rest ≤ 40 s × 3 inputs at 96 kHz, about
  46 MB, plus the audio that arrives while it is analysed).
- **Pacing.** A step is analysed once its rest is over, but not in one go: its passes (sliding
  sums and filters whose state is kept between blocks) run 4 samples per input for every
  sample fed, so each block carries a few blocks' worth of work and a 40 s step is done about
  20 s later, while the next steps play. Analysing it at once held the job thread for about a
  second at 96 kHz on a desktop (more on a Pi), and behind the other jobs past the fan-out's
  2 s queue the run lost audio. Only the run's last step is analysed in the block that completes
  the run: no audio is needed after it, and the result is due.

### Gates

- **Pilot below threshold.** On the linearity check, the gain at `pilot` must equal the gain at
  `pilot − 6 dB` within 0.1 dB. Otherwise the run aborts with "pilot is not below threshold":
  every GR number would be measured from a limiting baseline.
- **Settled.** A burst counts as settled when its gain changed less than 0.05 dB over its last
  10 %. A level's settled GR `GR_s` comes from its longest settled burst and is used for every
  row at that level. A level with no settled burst reports attack and settle times as "can't
  tell". A level whose `GR_s` is under 1 dB reports "no gain reduction" and no times; the 2 dB
  columns need `GR_s` of at least 3 dB.
- **Recovered.** As in *Adaptive rest*; the drift of each rest's final gain from `g₀` is stored
  per step, since VCA temperature drift over a long run is itself worth seeing.

### What a step reports

Per step and measured input, `t = 0` at the burst's first sample, `t_e` at its last:

| column | meaning |
|---|---|
| `gr_end` | gain reduction over the last period of the burst; for a burst shorter than the attack, how far the processor got |
| `attack_tau` | first time GR reaches 63 % of `GR_s` (in dB) |
| `attack_2db` | first time after which the output stays within 2 dB of its settled level (the datasheet attack time) |
| `settle_0_5db` | the same within 0.5 dB |
| `overshoot` | max \|y\| in the burst over the settled peak (max \|y\| over the last 10 % of the longest settled burst at that level), dB, and its time; from the raw waveform, since an overshoot shorter than a period is averaged away in the envelope. The reference's own overshoot (converter reconstruction ringing, largest at `peak` onset) is reported beside it as the floor |
| `release_tau` | time after `t_e` for GR to fall to 37 % of `gr_end` (in dB) |
| `recovery_2db`, `recovery_0_5db` | time after `t_e` until GR stays under 2 dB / 0.5 dB |
| `h2`…`h5`, THD | per window: attack (first 50 ms of the burst), settled (last 10 %), release (first 1 s after `t_e`), rest (last second) |
| `thump_peak`, `thump_rms` | in the 200 ms after each edge: the output low-passed at 200 Hz (8th-order Butterworth), dB re burst peak; carrier ≥ 950 Hz only, otherwise "can't tell". Floor: what an ideal gain change alone would put through the same filter, `r(t − δ)·|Y_1|/|R_1|` low-passed the same way (an enveloped sine has low-frequency content of its own: a 1 kHz zero-onset step reads −24 dB re burst peak through the filter with no thump at all); "≤ floor" where not 6 dB above it. The one-period gain envelope cannot follow a gain change shorter than about a period, so the floor under-reads there (an ideal limiter with a 1 ms attack on a 1 kHz carrier reads 9 dB over it): thump at a burst's start is printed only where `attack_tau` is at least 4 carrier periods, and at its end only where `release_tau` is (an ideal limiter at 3 periods, peak onset, reads 3.6 dB over the floor; at 4 periods 2.8 dB), otherwise "can't tell" |
| `drift` | final gain of the following rest re `g₀` |
| flags | not settled, not recovered |

Times are named as what they are: a 63 % / 37 % time is a time constant (`τ`); a 2 dB time is
a common datasheet convention for attack and recovery time.

Thump runs use an 8 kHz carrier with `peak` onset: there the stimulus edge's own low-frequency
content is lowest (−53 dB re burst peak through the filter, against −24 dB for 1 kHz with zero
onset).

**Thump through the interface.** The FF400's inputs are AC coupled (about −7.7° at 31 Hz on the
loop: a first-order corner near 4 Hz) and the card's output has a 100 µF coupling capacitor, so
the thump is measured high-passed: its slow part is lost and its peak reads lower than at the
VCA. The chain is the same before and after a module swap, so the comparison holds; the
absolute value is of this chain.

## Storage

A `dynamics` trace under the measurement, named `Run <n>` (`TraceSource::Dynamics { number }`),
holds the run's settings (actual carrier, onset, levels, actual burst lengths, `δ`, `g₀`) and,
per measured input:

- the step table above;
- per step, the gain-reduction envelope and `h2`…`h5` from 50 ms before the burst to the end of
  the rest after it, stored at quarter-period steps for the first 50 ms after each edge and
  log-spaced at 1/48 decade after that.

The CSV export holds the step table as one table and the envelopes as a second, long-format
table (`input, step, edge, t_s, gr_db, h2_db … h5_db`), with the settings in a
`# dynamics_info:` JSON header line, so `trace import` restores the trace; sessions save that
one CSV per trace (as for sweep traces). The kind is `MeasKind::Dynamics` with `stream() = None`;
the run, its progress (a mirrored `dynamics` entity: step n of m, current phase) and its outcome
follow the sweep measurement's model (`measurement-tree.md`, `sweep-distortion.md` § Protocol
and storage): `dynamics.run` under the stimulus lease, armed, one shot, disarmed after. Wire
changes bump `PROTO_VERSION`.

## Comparing

Before and after a VCA swap, both channels captured every run (the untouched channel shows
drift between sessions): the same programme gives the same rows, so the step tables diff row by
row; the envelopes overlay on a shared time axis from each edge.

## App: the Dynamics pane

What the first card bench (2026-11-20) needed to read, in the order it needed them: whether each
burst left the processor where it started; where in the level × length grid one channel departs
from the other; the release's programme dependence; single envelopes. The pane follows every
pane rule: curves by drag from any rig, the shared legend and readout plates, `T` for plot
chrome, every key with its keytip, key-table row and guide row, no Ctrl chord, no second y axis.
`ac2_scene::dynamics` computes every number and string; `ac2-plot` places pixels.

### Curves, A and B

A pane entry stays an `EntryRef` (a measurement's group or one trace); a dynamics trace holds
several measured inputs, and **every input of every entry is a curve**, named
`<input name> · Run <n>` (with the measurement's name when the pane holds more than one), in the
pane's per-curve colours. Envelopes and Release draw every curve.

Steps and Table compare two curves, **A** and **B**, stored in the pane's layout as
`{rig, trace, input}` (trace ids are per daemon, so a curve names its rig as a pane entry
does; `ui.toml` keeps the view, metric, step and toggles but not A and B, whose trace ids
belong to the session they were picked in). Default A: the first input of the first trace; default B: the trace's
second input if it has one (one channel against the other, the bench case), else the first
input of the second trace, else none. `B` / `Shift+B` cycle A / B through the pane's curves.
Other curves are not in Steps or Table. The Global `C` (Compare) and `V` (next trace) keep
their meaning in this pane, acting on its entries as in any graph pane.

Two curves compare row by row when their programmes match: same carrier, onset, pilot, levels,
burst lengths and sample rate. Two inputs of one trace always match. Rests may differ (adaptive
rests end where each run's processor recovered; two runs' rests differ when any step's stored
`rest` differs by more than one sample): then the comparison carries the caption `rests differ`, since a different rest changes the next step's start. Otherwise Steps and Table
say "programmes differ" and show A alone; Envelopes still overlays any curves.

### The picked step

One level and one burst length, shared by all views and stored in the layout as values (dBFS,
seconds), not indices: a curve from another run picks its nearest step, and a curve without a
step within 0.5 dB / 10 % reads "no such step". Default: the middle level, the longest burst.
`Z` / `Shift+Z` step the level down / up and `X` / `Shift+X` the length shorter / longer (the
arrows are taken: ↑ ↓ reserved for the stimulus level, ← → for pan, Alt+arrows for pane size
and offset); a click on a Steps cell picks it, a double click also switches to Envelopes. The
pick is printed as caption text in the title strip, `−8 dBFS · 1 s` (P2: the title is text, no
controls), and in full in the readout.

### Views (`G` / `Shift+G`)

`PaneKind::Dynamics`, `DynamicsView` in `PaneLayout`, `Scope::Dynamics` in the key table, both
`draws_curves` copies updated.

- **Steps** (default). The level × length grid. `K` / `Shift+K` cycle the metric (`Shift+M` is Global new math): GR end,
  attack τ, attack 2 dB, settle 0.5 dB, overshoot, release τ, recovery 2 dB, recovery 0.5 dB,
  THD attack / settled / release, thump start / end, drift. A cell shows A's value, or A − B
  when B is set; the readout plate gives A, B and A − B for every column of the picked step
  (the scene also takes a hovered step; the app passes the picked one). Non-numbers print as words ("≤ 1 period", "can't tell", "≤ floor", "no gain
  reduction") and never enter a difference ("can't compare"). A − B beyond the metric's
  tolerance gets the gentle hint (faint shade, dashed edge). Starting tolerances: 0.3 dB for
  GR end, overshoot and drift; for times the larger of 25 % and one carrier period; 3 dB for THD and
  thump, and only where both values are numbers above their floors. A step baseline-flagged in
  either curve reads "can't compare" and is never shaded. The tolerances are constants in
  `ac2_scene::dynamics` until a bench repeat run (one channel, rests replayed with
  `rests_from`) measures run-to-run spread per metric; that run sets them.
- **Envelopes.** Two stacked plots for the picked step: onset (−5 ms to the burst's end) above,
  release (burst's end to the rest's end) below; time from the edge, linear before it and log
  from one carrier period after it; one GR scale shared by both plots and matched across all
  curves. `D` toggles the y quantity between GR and THD (H2–H5 summed, dB re carrier); where
  the stored harmonics are blind (NaN) the THD line has a gap, never a zero. Readout: time and
  the value per curve.
- **Release.** For the picked level: release τ against burst length (log x, a mark per measured
  length) above, recovery to 0.5 dB on the same x below; "≤ 1 period" and "can't tell" points
  are hollow at their bound. `E` toggles to every level of curve A, drawn in one sequential
  ramp (levels are an ordered magnitude) with a level legend.
- **Table.** The step table for A and B and the difference, one row per step; the rows shown
  follow the picked step (`Z` / `X`);
  exported as a file with the trace's existing CSV export, as today.

### Baseline check

`drift` (the gain at the end of the step's rest re `g₀`) is the pilot gain the next burst
starts from. A rest ends when the gain moves under `FLAT_DB` over `FLAT_WINDOW_SECONDS`, which
on an exponential-in-dB tail leaves a residual of about 0.05 dB/s × τ: a healthy processor's
drift grows with its release (Studer card ch 2, 2026-11-20: −0.03 dB at 3 dB GR up to −0.12 dB
at 13 dB, τ ≈ 3 s). So per curve and level the expected residual is `r = 0.05 dB/s × τ_tail`, where the tail
constant is `τ_tail = max(release_tau, recovery_0_5db / ln(gr_end / 0.5 dB))` over the
level's steps, the second term only where `gr_end` is at least 1 dB (near 0.5 dB the log goes
to zero): `release_tau` alone reads the fast stage of a two-stage release (100 ms then a
5 s tail would give r ≈ 0.005 dB against a real residual of about 0.25 dB), and a step's baseline **moved** when
`|drift| > 2r + 0.1 dB` (bench: 0.41 dB at τ = 3.1 s; the card's ch 1 was at −0.85 dB). A step
that hit `rest_max` reads "not recovered" instead; where neither term of `τ_tail` is a
number the check reads "can't tell". A moved step and the step after it carry a hatched corner in Steps and
Table, "baseline moved −0.85 dB" in the readout (a measured number, no cause), and Envelopes
draws the following step's curve dashed. The drift metric in Steps shows the whole picture,
including a slow thermal ramp, which crosses the threshold only once it is as large as a moved
baseline. No wire change: `drift` and `release_tau` are in the step table.

### Making and running one

- **New measurement**: a form beside the sweep form, opening on a screen of its own with one
  Dynamics pane. Fields: reference input, measured inputs, outputs, carrier, onset, pilot,
  burst levels (dBFS), burst lengths (1 ms … 10 s by default; a length under 3 carrier
  periods is refused by the daemon at create, naming the shortest), longest rest. A loudest
  burst above the daemon's ceiling is kept, as a sweep's level is: the form makes it with a
  note to raise the max level before running, and create, update, salvage and session load
  accept it; the run is refused against the ceiling in force when it plays (the operator
  raises the ceiling for an approved run only).
- **Edit**: the same form through `meas.update`, which already takes any kind that validates;
  the daemon refuses it while that measurement's run plays. Traces keep the settings they ran
  with.
- **Run**: the stimulus cluster, as for a sweep: `Space` arms the dynamics measurement of the focused
  pane's lead entry on the rig shown (refused, with the reason, when the lead is another rig's) (showing the run-time bound and its loudest level), `Enter` fires, `Esc` stops
  (fade-out as any stop). The palette and the measurement's context menu reach the same path.
  An arm offers "replay rests of Run n" (`rests_from`) when the measurement has a run. During
  the run the title shows step n of m and the phase (`Waiting`, `Check`, `Burst`, `Rest`). A
  failed run (`DynamicsStatus::Failed`: pilot not below threshold, no reference, dropout,
  programme mismatch, stopped) shows a U1 banner with its message. The finished trace joins
  the pane that started it.
- **Data**: the step tables come from `dynamics.table` (settings and step table, no envelopes;
  added with the stage 1 fixes for oversize traces, which also make `trace.get` of a large
  dynamics trace a typed refusal instead of a dropped reply);
  the Envelopes view fetches `dynamics.envelope {trace, input, step}` on demand: both edges'
  points as parallel `f32` arrays (`t`, `gr_db`, `h_db[4]`), a few tens of kB per step even at
  an 8 kHz carrier. A test builds the largest legal programme (8 inputs × the most levels and
  lengths) and checks every reply against `MAX_CTRL_BYTES`. Session save and load keep the CSV.

### What changes on the wire and on disk

`PaneKind::Dynamics`, `DynamicsView`, the A/B pair, the picked step, metric and toggles in
`PaneLayout` (a layout change: `PROTO_VERSION`, session `VERSION`, `rig.json` meter layouts,
`ui.toml`); `dynamics.envelope`; the run-plays guard on `meas.update`.

## Stages

1. **Daemon, core, CLI.** The programme source (onset placement, release flag, normalisation),
   the envelope, delay, edge and step analysis in `ac2-core` (pure; tested against a synthetic
   processor: a feed-forward limiter with known attack and release time constants, a
   programme-dependent two-stage release, a group delay of a few samples, a CV feedthrough
   step, and the ideal distortion-free, thump-free limiter, which must read "≤ floor" for every
   harmonic and thump column), the measurement kind, `dynamics.run`, trace storage, CSV export and import, session
   save. CLI: `meas new dynamics`, `dynamics run` (prints the step table), `trace export`.
2. **App.** The Dynamics pane (*App: the Dynamics pane*): curves and A/B, the picked step,
   the Steps, Envelopes, Release and Table views, the baseline check, the new/edit form,
   running from the stimulus cluster, `dynamics.envelope`, and the daemon's level and run guards.
   Builds on the stage 1 fixes that add `dynamics.table`.
3. **Link.** Bursts on a subset of the outputs while the others hold `pilot`, to measure how far
   a linked channel follows. Needs a per-output level in the programme source.
