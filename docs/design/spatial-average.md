# Live spatial average of transfer functions

Status: implemented as a math channel's average (`MeasKind::Math`, `expr: average`; created
with Shift+M or `ac2 math new --op avg --of …`). The former `spatial_average` measurement kind
is removed. Model, wire, statuses, daemon and tests: [`math-channels.md`](math-channels.md)
(*Average*, *Operands left out*). This note keeps why the average is formed the way it is.

## Why

A loudspeaker's response changes from seat to seat, so a system is tuned to the average of
several mic positions rather than to one spot. With one mic per position (one transfer
measurement each: same reference, its own mic input) the average of those measurements is a
live `tf` stream like any transfer function. One mic moved from seat to seat is the same
mathematics over stored captures (`trace.average`, `M`, or a math channel of traces), so the
two agree.

## Definition and its reasons

Live operands run in the current session epoch, so they share one time base (decision 8a)
and their phases can be combined.

1. **Each operand's current result before display smoothing**, formed on request (the
   request `trace.capture` makes), mic curve already applied. Published frames are not used:
   they exist only while someone subscribes, and averaging whichever frames happened to be
   sent would average results of different ages.
2. **Re-referred to one delay.** Operand `k` was measured with its own inserted delay `τₖ`
   removed (`hₖ = Hₖ·e^{+j2πfτₖ}`); before combining, each is re-referred to `τ_ref`:
   `hₖ·e^{−j2πf(τₖ − τ_ref)}`. `τ_ref` is an operand's delay (default the first) or an
   explicit one, stated in every frame (`TfMeta.delay`). The averaged phase thus shows each
   position's arrival relative to that reference; position-to-position timing is not
   silently removed. When the reference operand is left out its newest seen delay stays the
   reference, so the phase does not jump to another arrival.
3. **Methods** (`ac2_traces::ops::average_on_time_base`, shared with `trace.average`):
   **power** (default) `|H| = sqrt(mean |hₖ|²)`, the level over the positions without phase
   cancellation, phase the argument of the complex mean; **complex** `mean hₖ`, what one
   point summing the arrivals would see (arrivals that differ in time cancel);
   **coherence-weighted**, the complex mean weighted by `γ²/(1 − γ²)` — the inverse variance
   of an H1 estimate, γ² capped at 0.999.
4. **A column has a value only where every included operand has one**; elsewhere its
   validity mask is the union of the operands' reasons. Averaging a varying subset per column
   would draw steps where positions drop in and out, which read as response features.
5. **Display smoothing after averaging**, on the averaged columns (operands' own smoothing
   unused), as a stored trace is re-smoothed. A capture holds the unsmoothed average.

**Coherence of the average is not estimated.** The published `coh` is the plain mean of the
included operands' γ² — a display mask, not a coherence of the averaged transfer function,
which would need cross-spectra between positions that no operand has.

**Calibration:** a transfer function is a ratio, so the average is uncalibrated like its
operands; `mic_curve` is true only when every included operand had its mic curve applied.

**Left out, not averaged in:** an operand that is stopped, settling, or raising a fault banner
(`CLIP`, `NO_REFERENCE`, `CHECK_ROUTING`, `NO_SIGNAL`) is left out of the frame and named;
`WEAK_REFERENCE` and `DISCONTINUITY` only hold or restart its own averaging, which its
validity mask already reports. Fewer than two usable operands is no average (`FEW_OPERANDS`,
`NO AVERAGE · 1 OF 4 POSITIONS`): one position drawn as an average would mislead.

## Not offered

- A complex average with every operand aligned to its **own** arrival (the speaker's response
  without position-to-position timing); it would be a third phase reference, on the
  stored-trace average too.
- Per-position weights (e.g. by audience area); positions count once.
- An app flow "capture position → add to average" for one moved mic; the stored-trace
  average covers it.
