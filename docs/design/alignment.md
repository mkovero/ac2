# Alignment: delay and polarity of two systems in their overlap

Status: design (2026-10-20). Operator's request: a sub/main alignment aid, generalised to any
two systems that overlap in a band — sub/main, main/front-fill, main/delay tower. The user
picks two transfer traces and names the band to look at; the analyzer says which one to
delay, by how much (ms, m), whether to flip polarity, how well that fits, and what the two
then sum to.

## What exists

- Transfer traces: a measurement's live curve, its captures, imports, sweep runs — each a
  math operand (`Operand::Meas` = the live curve, `Operand::Trace` = a stored trace).
- Math channels (`math-channels.md`): `A ÷ B` already *is* the phase difference
  (`H_a / H_b`, phase keeps A's arrival relative to B); `A + B` is the summation prediction
  on a shared time base. Nothing fits a delay to that phase or applies one to a sum.
- The delay finder (`q1-delay-finder.md`) answers another question: the broadband first
  arrival of one measurement in its impulse response. Two systems crossing over are aligned
  where they overlap, by phase, and the filters' own phase makes that differ from the
  arrival difference; so the alignment is a fit over the band, not two finder results
  subtracted.

## Decision: an `align` math expression

`MathExpr::Align {a, b, band: {lo, hi}, max_delay}` (domain transfer). It reuses what a math
channel already has: operands by name (live curves or stored traces, two under one
measurement too), the result following a live operand, operands left out with a reason,
operand invariants (an operand cannot be deleted under it), an owner in the tree, sessions,
capture. A measurement kind of its own would duplicate all of that.

- Needs phase and a shared time base (like `A + B`): without one the relative arrival is
  unknown and any suggested delay would be invented. Refused at create for an operand
  without phase; an operand on another time base is left out as `mismatch` at run time.
- Its curve (stream `tf`) is the **aligned sum** `A + B` with the suggested delay and
  polarity applied to the delayed one — drawn on the magnitude and phase views like any
  sum, its phase referred to A's delay. Coherence: none (as a sum).
- The fit rides in the frame's meta, `MathState.align: AlignState` (`AlignOutcome`: a fit
  or the reason there is none); the alignment view's four per-column curves ride as i16
  frame arrays on the frame's grid, `align_phase_before/after` (degrees, wrapped) and
  `align_sum_before/after` (dB) — all four or none.
- The UI does no maths (`tests/it/no_dsp.rs`): the fit is `ac2_core::align` (pure, tested
  analytically), the per-column combination `ac2_traces::math::align`, run by the daemon's
  math job; the scene only formats and draws.

## The maths (`ac2_core::align`)

Inputs per column `k` in `lo ≤ f ≤ hi`: `A_k`, `B_k` on one time base (each operand's
`h·e^{−j2πfτ}`), coherences. `Δφ_k = arg(A_k / B_k)`; model `Δφ(f) = −2πfτ + pπ (mod 2π)`:
A arrives `τ` after B, `p = 1` for opposite polarity.

- **Weight** `w_k = |A_k|·|B_k|·γ²_k` (γ² the lower of the two; 1 without coherence):
  `|A||B|` is the size of the interaction term in `|A + B|² = |A|² + |B|² + 2|A||B|cos Δφ`,
  so a column where one system is far below the other cannot cancel anything and barely
  counts; low coherence counts less.
- **Score** `S(τ) = Σ w_k cos(Δφ_k + 2πf_kτ) / Σ w_k` ∈ [−1, 1] — the weighted mean cosine
  of the residual. Maximising `S` (normal polarity) or `−S` (inverted) is maximising the
  band's summed power, which is what the user wants; it needs no unwrapping.
- **Candidates (2π ambiguity).** `S` is scanned over `|τ| ≤ max_delay` in steps of
  `1/(16·hi)` (≤ 22.5° at the top of the band); every local maximum of `S` and of `−S` is a
  candidate — neighbours differ by one wrap, about `1/f_centre` apart. Each candidate is
  refined by weighted least squares on its wrapped residual
  `r_k = wrap(Δφ_k + 2πf_kτ − pπ)`, Gauss–Newton `δτ = −Σw r 2πf / Σw (2πf)²`, i.e. the
  slope fit of the unwrapped phase difference around that wrap.
- **Reported:** the best (highest score), its residual (weighted RMS of `r`, degrees), the
  best of the other polarity, and the runner-up of the same polarity when its score is
  within 0.1 of the best (a narrow band does not decide the wrap; the user picks by
  geometry). Positive `τ`: delay B by `τ`; negative: delay A by `|τ|`. Polarity is the
  delayed one's.
- **Summation** per column `σ_k = 20·lg(|A + B| / (|A| + |B|))` ≤ 0 dB (0: they add fully,
  −∞: they cancel), before and after; reported in the band as weighted mean and worst.
- **No fit** (`AlignNoFit`): fewer than 3 usable columns in the band (band outside the
  grid, gaps), or no weight (coherence 0 everywhere). The curve is then the plain sum.

Metres are `|τ|·c` with `c = 331.3·√(1 + T/273.15)`, formatted where every other delay is
(`ac2_scene::readout::delay_readout`, the CLI's `--temp`), so the alignment and the legend's
`@ 20 °C` read the same.

Accuracy targets (core tests): two analytic TFs differing by a known delay and polarity
(pure delay; LR4 low/high pair, which are in phase at any frequency) return the delay within
1 % and the polarity exactly; noise at γ² 0.5 within 5 %; a narrow band yields a runner-up
one period away; a band outside the data is `no fit`.

## Wire and files

- `MathExpr::Align`, `AlignBand {lo, hi}` (Hz, `0 < lo < hi`), `max_delay` (0 < … ≤ 1 s,
  default 50 ms: a delay tower 17 m away is 50 ms).
- `MathState.align: Option<AlignState>`, `AlignFit {best, other_polarity, runner_up,
  before, after, columns}`, `AlignCandidate {delay, delayed: A | B, inverted, residual,
  score}`, `Summation {mean, worst}`.
- `TransferView::Alignment` in pane layouts.
- `PROTO_VERSION` 58 → 59, session format 28 → 29 (sessions hold math channels and layouts).

## App

- **Alt+M** (palette *New alignment…*; Shift+M is the math channel, Alt+M the same dialog
  with the operator `align`): **A** and **B** — every transfer trace by name, live curves
  `(live)` and stored ones, two under one measurement too; **Overlap from / to** (Hz, typed);
  **Max delay**; **Name** (`Sub ↔ Main`, following the picks). The band is asked, never
  assumed: it is prefilled with where both curves are within 6 dB of each other and within
  20 dB of their own peak (`ac2_scene::align::overlap`, over the curves the app already
  draws), and empty when there is no such region. Enter creates and starts it; *Edit
  measurement…* reopens it.
- A new alignment opens its own pane: the focused pane is split and the new half shows the
  owner's **alignment view** (never taking over the focused pane).
- **Alignment view** (a transfer-pane view; **G** steps to it only when the pane's group
  has an alignment, so other panes cycle as before): top, the
  phase difference A − B before (faint) and after the suggested alignment, ±180°; bottom,
  the summation before and after (dB, 0 = full addition). The band is a gentle hint (faint
  grey shade + dashed edges) in both. Readout plate:
  `Delay Main by 4.21 ms · 1.44 m @ 20 °C · polarity normal`,
  `residual ±12° over 60–120 Hz · summation −7.9 dB → −0.3 dB (worst −21.0 dB → −1.2 dB)`,
  `other polarity: Main +9.80 ms inverted, ±41°`, and when close
  `also fits: Main +15.30 ms, ±14° — the band is too narrow to tell: pick by distance`. No fit:
  `NO FIT · fewer than 3 columns in 60–120 Hz` (warning banner). The view shows the selected
  alignment of the pane's group, else its first; with none, it says how to make one.
- Legend of the aligned sum: `Sub ↔ Main aligned · Main +4.21 ms`.

## CLI

`ac2 align new A B --band 60..120 [--max-delay 50ms] [--name N] [--under M] [--temp 20]` —
creates and starts the alignment (operands as `ac2 math`: name, id, `trace:NAME`,
`meas:NAME`), waits for its first frame and prints the readout; `ac2 align show <name>
[--temp]` prints the current one (`--json` for both). `ac2 math set` edits it like any math
channel's name and smoothing.

## Tests

- core: analytic cases above, the candidate rules, no-fit reasons.
- traces: `align` on columns (delay + polarity recovered through dB/degree columns and
  operand delays, the after-sum ≥ before-sum in the band).
- daemon: the math job's align frame; `tests/it` fake rig: live A + stored B, refusal of an
  operand without phase, mismatch across epochs.
- scene: readout strings, overlap prefill, no-fit text; GPU snapshot of the view.
- UI e2e from an empty daemon: two captures, Alt+M, Enter, the view's readout.
- CLI: `align new` / `show` on the fake rig.

## Open

- Applying the suggested delay to an output (a DSP's) is the user's; ac2 drives no DSP.
- The fit treats the band as one region; two crossovers (sub/main/top) are two alignments.
