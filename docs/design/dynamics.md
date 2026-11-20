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

## Stages

1. **Daemon, core, CLI.** The programme source (onset placement, release flag, normalisation),
   the envelope, delay, edge and step analysis in `ac2-core` (pure; tested against a synthetic
   processor: a feed-forward limiter with known attack and release time constants, a
   programme-dependent two-stage release, a group delay of a few samples, a CV feedthrough
   step, and the ideal distortion-free, thump-free limiter, which must read "≤ floor" for every
   harmonic and thump column), the measurement kind, `dynamics.run`, trace storage, CSV export and import, session
   save. CLI: `meas new dynamics`, `dynamics run` (prints the step table), `trace export`.
2. **App.** A Dynamics pane: the envelopes (gain reduction vs log time from the edge) per trace
   and input, overlaid, and the step table with two runs side by side and the difference.
3. **Link.** Bursts on a subset of the outputs while the others hold `pilot`, to measure how far
   a linked channel follows. Needs a per-output level in the programme source.
