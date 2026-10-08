# Math channels

Status: implemented (`MeasKind::Math`, `ac2_traces::math`, the daemon's math job
`crates/ac2d/src/jobs/math.rs`, `ac2 math new / set`, the app's math channel dialog,
Shift+M). Supersedes the spatial-average measurement kind (`spatial-average.md`, whose
averaging is this note's *Average*) and the stored-trace `trace.math` command. Answers PLAN.md
§3.3 "Live spatial average" and §3.5 "Trace math".

## What the operator gets

A math channel is a curve made from other curves by name: **A ÷ B**, **A × B**, **A + B**,
**A − B**, or the **average** of several. Its operands are live measurements or stored traces
of one kind; the result is a measurement of that kind — drawn where the kind is drawn
(transfer math on the magnitude / phase / coherence panes, spectrum and RTA math on the
spectrum pane), updating live while any operand is live, listed by `ac2 meas list`, edited
afterwards (operands, operator, method, smoothing) and kept as a stored trace by a
capture (Ctrl+1 … 9, `ac2 trace capture`) whose metadata names the expression and the
operands that went in.

It replaces two things that were each half of it: the palette's "A − B" / "A / B" on stored
traces (an implicit "selected and next shown" pair, magnitude difference or complex division,
and a spectrum difference drawn on the magnitude pane) and the spatial-average measurement
kind (a live average of transfer measurements only). Both are one concept: an expression the
daemon evaluates, with operands chosen by name.

## Model

`MeasKind::Math { config: MathConfig }`:

- `owner`: the measurement it is listed under (`measurement-tree.md`) — the one selected
  when it was made (Shift+M), or `imported`; its captures are filed there too. Moving it is
  a `meas.update` of the owner alone, applied without restarting the channel.
- `domain`: `transfer` | `spectrum` | `rta` — what the operands are, so what the result is and
  which stream it publishes (`tf`, `spec`, `rta`). The daemon checks every operand against
  it; clients read the pane and stream from it without looking the operands up.
- `expr`: `binary {a, op, b}` or `average {of: 2 … 16, method}`.
- `op`: `divide`, `multiply`, `add`, `subtract`.
- operands: `meas {meas}` (a live measurement's current result) or `trace {trace}` (a stored
  trace's columns). A math channel is not an operand: capture it and use the trace (an
  operand chain would wait on another channel's operands within one frame).
- `reference`: `operand {operand}` | `fixed {delay}` — the delay a transfer sum, difference
  or average is referred to (default: the first operand).
- `smoothing`: display smoothing of the result (transfer and spectrum), applied after the
  operands are combined unsmoothed.

## Transfer functions: magnitude and phase together

Each operand `k` was measured with its own inserted delay `τₖ` removed
(`hₖ = Hₖ·e^{+j2πfτₖ}`). Operands of one session epoch (live measurements, and captures of
that epoch) share one time base (decision 8a), so `Hₖ` itself is known and operands combine as
the complex values they are:

| op | result | stated delay | needs a shared time base |
|---|---|---|---|
| A ÷ B | `H_a / H_b` | 0 — the phase keeps A's arrival relative to B | no: without one, `h_a / h_b` of each operand's own alignment, marked `own_alignments` |
| A × B | `h_a · h_b` (the cascade) | `τa + τb` (a cascade's delays add) | no (marked `own_alignments` without one) |
| A + B | `h_a·e^{−j2πf(τa−τref)} + h_b·e^{−j2πf(τb−τref)}` | `τref` | **yes** — refused (create) / operand left out (run time) |
| A − B | the same with − | `τref` | **yes** |
| average | `ac2_traces::ops::average_on_time_base` (*Average* below) | `τref` | complex and coherence-weighted: yes; power: no (magnitude only without one) |

A + B is the summation prediction: what A and B add up to at the mic, e.g. a captured
"sub alone" plus the main live, before both play together. Their relative arrival decides the
sum, so an operand without one (an import, a capture from an earlier epoch) is never summed
from its own alignment — that would invent an arrival nobody measured. The result states its
`PhaseBasis` per frame and per capture: `shared_time_base`, `own_alignments` (the legend says
`phase: own alignments`, the trace is drawn as of an independent time base) or `no_phase`.

An operand without phase (a target curve) takes part in ÷ and × only: the result is
magnitude only (`no_phase`) — "how far is this from the target" is Main ÷ Target.

**Coherence.** A ratio or a cascade is only as trustworthy as its less coherent operand, so
÷ and × carry the lower γ² per column (a display mask: the coherence mask blanks where either
operand is unreliable; nothing is estimated). A sum or difference carries none: its coherence
would need the operands' cross-spectrum, which no operand has — the legend says `no
coherence`. An average carries the plain mean of its operands' γ², as `trace.average` does.

## Levels: spectra and RTA bands

Levels have no phase. `A − B` is the level difference in dB, `A + B` the power sum
`10·lg(10^{a/10} + 10^{b/10})` (incoherent sources adding), the average the power mean. ÷ and
× of levels are refused (a level ratio is a difference in dB: `A − B`). Operands share one
grid: band powers and FFT bins are not interpolated — one FFT length (at the session's rate),
one band layout, one level scale (dBFS and dB SPL do not mix; an operand in another scale is
left out as `mismatch`).

A spectrum is combined on every bin; its live frame gathers the bins into the display
columns a spectrum publishes, each the highest value among its bins (for a level difference,
the largest difference in the column), and its capture keeps every bin. A level difference is
drawn on the level axis in the first operand's scale; the caption says `level difference`.

## Average (the spatial average)

Unchanged from the spatial average (`spatial-average.md`): 2 … 16 operands; **power**
(default) `|H| = sqrt(mean |hₖ|²)`, phase the argument of the complex mean; **complex**
`mean hₖ` (what one point summing the arrivals would see); **coherence-weighted**, the complex
mean weighted by `γ²/(1 − γ²)` (γ² capped at 0.999). Every operand re-referred to `τ_ref`
first; a column has a value only where every included operand has one; display smoothing
after averaging. Spectra and RTA: power only. With live operands it is the live spatial
average of mic positions; with stored traces the one-mic-moved-around average (the same
mathematics as `trace.average`, which stays as the one-shot `M` key).

## Operands left out (principle 8)

Whenever a frame is due, the daemon asks every live operand for its current unsmoothed result
(the request `trace.capture` makes; published frames are not used — they exist only while
someone subscribes, and combining whichever frames happened to be sent would combine results
of different ages). An operand is left out of that frame, and the frame says so per operand
(`MathState.operands[k].status`), when it is:

| status | when |
|---|---|
| `stopped` | a live operand not running |
| `settling` | running without a usable result: no valid column yet, nothing formed, another epoch, or no answer within 250 ms |
| `refused` {protection} | its own frame raises a fault banner: `CLIP`, `NO_REFERENCE`, `CHECK_ROUTING`, `NO_SIGNAL` |
| `mismatch` | it does not combine with the others: another level scale, grid, or (sum, difference, phase average) time base |

Without the operands the expression needs — both of a binary operator, two of an average —
every column is NaN with `FEW_OPERANDS`: one position is not an average, half a ratio no
ratio. The app shows `NO AVERAGE · 1 OF 4 POSITIONS` / `NO RESULT · 1 OF 2 OPERANDS` (fault)
naming what was left out and why, `AVERAGE · 3 OF 4 POSITIONS` (warning) for an average with
positions missing, and the legend counts them. A capture is refused when the expression lacks
its operands. When the reference operand is left out, its newest seen delay stays the
reference, so the phase does not jump to another operand's arrival.

## Operands' invariants

While a math channel names an operand, the daemon refuses to delete it (measurement or
trace), to change a measurement into another kind or onto another grid (transfer grid, FFT
length, band layout). A stored operand's columns are read when the channel starts; a mic
curve applied to that trace afterwards restarts the channel with the corrected columns.
`meas.reset` of a math channel is `invalid` (it holds no averaging; the app's reset resets its
live operands).

## Daemon

The math channel is a job fed by the capture fan-out although it analyses no audio: it
publishes on its live operands' clock, its frames carry the session's sample index, and it
goes STALE exactly when audio stops. Control keeps a registry of probes of running transfer,
spectrum and RTA jobs; when a frame is due (only while someone subscribes, or for a capture)
the job sends every live operand a capture request first and then collects the answers, so
the wait is the slowest operand's. Stored operands are held as their columns on the result's
grid (mic curve baked in, resampled onto a transfer result's grid). The combination is
`ac2_traces::math` (transfer and levels), shared with the tests.

A math channel of stored traces only still needs a running audio session to publish (its
clock is the fan-out's); its result does not change.

## App

**Shift+M** (palette *New math channel…*) opens the dialog: **A** (every live measurement
and stored trace, by name, `(live)` / `(stored, S2)`), **Operator** (by A's kind: ÷ × + − and
average for transfer functions; − + and average for levels), **B** (the curves of A's kind),
or, for the average, one row per curve of A's kind (all in to start with; ←/→ leaves one out),
**Method**, **Phase reference** (the operand whose delay a sum, difference or average is
referred to), **Smoothing**, **Name** (follows the expression — `Main L ÷ Sub`, `Average of 4`
— until the operator types one). A starts as the selected trace, else what the focused pane
shows. *Edit the selected math channel…* opens the same dialog on an existing one and sends
`meas.update`. The legend shows the expression and what it means (`Main L + Sub · no
coherence`), and for a transfer ÷, − or + on one time base how far apart the operands arrive by
the delays their phases are referred to (`· arrival Δ +3.7 µs · +1.3 mm @ 20 °C`, A − B); a spectrum math channel's caption says `level difference` / `power sum`. The UI
does no maths: the dialog builds a configuration, the daemon combines and refuses with a
reason.

## CLI

`ac2 math new "Main L / Sub"` (operator spaced: `/ ÷ * × + - −`), `--op div|mul|add|sub --a
A --b B`, `--op avg --of A,B,C [--method …] [--phase-ref X | --ref-delay T] [--smooth N]
[--name …] [--start]`; `ac2 math set <channel> …` changes what is given. Operands by name or
id; `trace:NAME` / `meas:NAME` when a measurement and a trace share a name. Replaces `ac2 meas
new avg` and `ac2 trace math`.

## Wire and files

- `MeasKind::math {config: MathConfig}`; `MathState` {`operands`: [{`operand`, `status`:
  `OperandStatus`}], `phase`: `PhaseBasis`} in `TfMeta.math`, `SpecMeta.math`,
  `RtaMeta.math`; validity bit `FEW_OPERANDS` 512.
- `TraceSource::math` {`meas`, `meas_name`, `epoch`, `at_sample`, `expr`, `operands`:
  [`NamedOperand` {`operand`, `name`}], `phase`}: a capture shares its epoch's time base when
  its phase is `shared_time_base` and it is a sum, difference or average (a ratio or cascade
  is relative).
- `trace.math`, `MeasKind::spatial_average`, `TraceSource::spatial_average` / `math {a, b,
  op}` removed. `PROTO_VERSION` 22, session format 10 (sessions hold math channels; a load
  checks their operands against the loaded measurements and traces and starts them after
  every measurement and trace is in).

## Tests

- `ac2-traces` (`math/tests.rs`, analytic): ÷ of two first-order filters is their ratio in
  magnitude and phase with the relative arrival; × their product with the delays added; + of
  two delayed copies the comb `|1 + e^{−j2πfτ}|` (− the `|1 − e^{−j2πfτ}|`), referred to a
  fixed delay too; sums across time bases or without phase refused; ratio across time bases
  marked; ±3 dB power average, `|cos(πfτ)|` complex average; gaps stay gaps; level
  difference, power sum and power mean on known levels, ÷ × of levels refused.
- `ac2d` (`jobs/math/tests.rs`): the spatial-average cases (dropouts, reference left out,
  gaps, weighting); a live ÷ stored ratio (6 dB, relative arrival, lower coherence); a sum
  with a stored operand of another epoch left out as `mismatch`; a capture + live sum is the
  comb. `tests/it/math_channels.rs` on the fake rig: the live average of three positions
  (analytic power average, NO SIGNAL position refused, capture naming two positions,
  invariants, FEW_OPERANDS); Seat 1 ÷ Seat 2 = +6 dB with the later arrival in the phase,
  edited to × = 0 dB, capture kept at +6 dB; capture + live = the two-arrival sum; an
  import refused in a sum, allowed in ÷; spectrum − spectrum = 6 dB on the `spec` stream,
  its capture a spectrum trace, operand FFT length locked.
- `ac2-cli` (`tests/it/math_rig.rs`): expressions typed and by `--op`, refusals by message, an
  edit keeping the name, `meas list`.
- `ac2-ui`: the dialog (`math_dialog.rs` tests, `state_tests`), `tests/embedded.rs` from an
  empty daemon (dialog by name, the average at −6 dB with its legend, capture with Ctrl+1,
  edit into ÷ at 0 dB, NO RESULT banner; spectrum math on the spectrum pane and not on the
  transfer pane), GPU snapshots `math_dialog`, `spectrum_math`, `transfer_math_average`.

## Open questions

- A level difference is drawn on the spectrum pane's absolute level axis (in A's scale). A
  relative axis for differences (0 dB centred) would read better; not built.
- Math channels as operands (a chain) are refused; capture first. If wanted, the job would
  need an evaluation order and the wait budget split across levels.
- `trace.average` (M, one-shot on the shown stored traces) stays beside the math average of
  stored traces; one could go.
- A complex average aligned to each operand's own arrival (`spatial-average.md`) and
  per-operand weights are still not offered.
