# Sub-sample arrival on the sweep; group delay that holds below 100 Hz

Status: implemented (`ac2-core::sweep` fine arrival and column reading,
`ac2-core::delay::Tracker` fractional tracking, `ac2-scene::trace` fitted group delay,
`ac2 trace delay-diff`). Inputs: the REW cross-check on pupu (`docs/rigs/pupu.md`, "REW
cross-check, electrical"), `delay-no-resettle.md` (fractional delays on the live transfer
function), `sweep-distortion.md`.

## What the operator gets

- The sweep reports its **arrival to a hundredth of a sample** (≈ 0.1 µs at 96 kHz), not to
  the nearest sample; the IR's time axis and the phase are referred to that arrival.
- The live tracker inserts the fractional mean of two results agreeing within 0.1 sample
  (`FINE_AGREEMENT_SAMPLES`); the daemon moves the delay only when that arrival is more than
  0.05 sample from the applied one (a 600.37-sample path tracks to within 0.02 sample, phase
  flat to 1° at 10 kHz).
- Readouts show the arrival to the µs, and the A − B difference to 0.1 µs in `ac2 trace
  delay-diff` and in the legend of a transfer math channel ÷, − or + on one time base
  (`arrival Δ +3.7 µs · +1.3 mm @ 20 °C`).
- The **group delay pane is right to a few percent from 16 Hz up**, on live and sweep
  traces alike, and says over what bandwidth it was taken (`Group delay ms · 1/12 oct`).

## Why it matters

One sample is 10.4 µs at 96 kHz and 20.8 µs at 48 kHz: 37° and 75° at 10 kHz. Two boxes
aligned to the nearest sample can sit 18°/37° apart at 10 kHz before anything else is
wrong; the summation through a crossover sees that. A delay readout that only moves in whole
samples hides exactly the residual the operator is trimming. Air does set a floor — 1 mm of
mic movement is 2.9 µs, a 1 °C change over 20 m is ≈ 35 µs — so the target is a readout
that is never the limiting term, not nanoseconds for their own sake.

## Measured before the change (pupu, Xone path out 3 → in 5 re loopback in 2, −30 dBFS)

Steady sines (`~/rew-dl/jphase.py` on pupu: least-squares sine fit of in 5 / in 2 at each
frequency, group delay from a ±1/48-octave pair) are the reference: no window, no
deconvolution, no interpolation.

| | steady sine | ac2 sweep | REW 5.40 live |
|---|---|---|---|
| arrival (IR peak re the reference) | — | 0 (whole samples) | 3.69 µs |
| direct cross-spectrum (numpy) | 3.70 µs | | |
| phase at 16 Hz | +10.125° | +10.296° | +10.788° |
| phase at 31.5 Hz | +5.399° | +5.422° | +5.589° |
| phase at 100 Hz | +1.720° | +1.720° | +1.738° |
| phase at 10 kHz | −7.207° | −7.203° | −6.972° |
| magnitude, 16 Hz – 10 kHz | | ≤ 0.04 dB from the sine | |

The sweep's phase and magnitude are right. Its arrival is not reported finer than a sample:
the 3.7 µs stays inside the phase (it is part of the −7.2° at 10 kHz).

Group delay, µs:

| f, Hz | steady sine | ac2 as displayed (central difference, 1/48 oct) | ac2, slope fitted over ±1/12 oct | REW, same fit |
|---|---|---|---|---|
| 16 | 1535 | 1919 (+25 %) | 1530 | 1753 |
| 20 | 1054 | 731 (−31 %) | 1124 | 1174 |
| 25 | 706 | 705 | 763 | 766 |
| 31.5 | 459 | 535 (+17 %) | 449 | 484 |
| 40 | 290 | 259 (−11 %) | 285 | 304 |
| 50 | 187 | 162 (−13 %) | 187 | 197 |
| 80 | 75 | 78 | 74 | 78 |
| 100 | 49 | 47 | 47 | 52 |
| 10 000 | 3.2 | 3.1 | 3.2 | 3.1 |

The data is fine; the derivative was not. The display took `−Δφ/(360·Δf)` between
neighbouring columns. On the 48-ppo grid the two neighbours are
`Δf = f·(2^{1/48} − 2^{−1/48}) ≈ 0.029·f` apart, so a phase error σ_φ becomes

    σ_τ ≈ √2 · σ_φ / (360° · 0.029 · f)

— 0.03° at 50 Hz is 58 µs, a third of the true 187 µs. The point-to-point group delay at
18–23 Hz also swung 670 ↔ 1830 µs with a period of about 1.1 Hz (a ripple `A·sin(2πfT)`
gives `τ = A·T`); §5 found its cause.

## Design

### 1. Fractional sweep arrival (`ac2-core::sweep`)

- `d`, the strongest sample of the deconvolved `h`, stays the whole-sample anchor for every window (linear, harmonic, room): windows
  stay in whole samples, as on the live path (`delay-no-resettle.md`, "The two halves").
- Refine the peak: take `h[d−32 … d+32]` (`FINE_HALF`; ±16 leaves ≈ 0.002 sample of bias
  from the tails it cuts, ±32 ≈ 0.0004), interpolate band-limited by zero-padding its DFT
  ×16 (the sweep is band-limited to `f2`, so the samples define the peak exactly), then a
  parabolic vertex on |h| at the fine grid. A parabola on the raw samples alone is biased
  by up to ≈ 0.05 sample for a sinc-shaped peak, and more when `f2` is well below Nyquist;
  after ×16 the bias is below 0.001 sample.
- Define the arrival as that peak (`D = d + φ`, |φ| ≤ ½), the same definition REW's delay
  uses, so the two read alike. Onset (the room parameters' `onset`) stays separate.
- Refer the phase to `D`: rotate by `e^{j2πfφ}` as the live path does. On the Xone path the
  13.3° that the 3.7 µs adds at 10 kHz leaves the phase; what remains is the path's filters.
- `arrival_s` carries the fraction; `t0` of the IR block and the room times are re `D`; the
  IR's decimated buckets keep their whole-sample edges and state `t0` exactly.

### 2. Live tracking inserts the fraction

Fractions are carried everywhere (`delay-no-resettle.md`). The tracker inserts the fraction
once two results agree within 0.1 sample, and whole samples otherwise: a wandering fraction
would only move the phase by noise.

### 3. Readout

- The measurement list shows µs when the delay has a fine part (`12.502 ms`); the sweep's
  arrival likewise.
- A **difference readout** between two arrivals (A − B, µs: `ac2 trace delay-diff`, the math
  legend), because the operator aligns pairs: that number is what sub-sample precision is
  for.

### 4. Group delay over a stated bandwidth (`ac2-scene::trace`)

Replace the central difference with a phase-slope fit:

- `τ(fᵢ) = −slope / 360` of a least-squares line through the unwrapped phase against f over
  the columns within ±B of fᵢ, each weighted by its coherence `γ²/(1−γ²)` (the inverse of
  the phase variance for an averaged estimate); sweep columns weight 1.
- `B` follows the trace's phase smoothing when it is wider (a 1/6-oct smoothed trace gets
  ±1/12); otherwise ±1/24 oct (`GROUP_DELAY_SPAN_B` = 12, a 1/12-octave span). The pane
  title names it.
- Columns with fewer than three finite neighbours in the span stay NaN, as gaps do now.
- Arithmetic on columns the daemon sent, in `ac2-scene`; no DSP crosses into `ac2-ui`.

Error with ±B fitted over n ≈ 2·48·B + 1 columns: the slope's σ falls as `n^{−3/2}`, so
against the neighbour difference ±1/24 oct (5 columns) gains ≈ 2× and ±1/12 (9 columns)
≈ 5× for uncorrelated phase error (a periodic ripple faster than the span averages out
further); the table above shows ±1/12 within 7 % of the steady sine from 16 Hz. The cost is resolution:
a resonance narrower than the span is smeared, which is what the stated bandwidth tells the
operator.

### 5. The LF phase ripple: columns read off-centre

The ≈ 0.03° ripple, ≈ 1.1 Hz long near 20 Hz, was not a component 0.9 s from the arrival
but the linear spectrum read at the wrong frequencies. With the default gate (0.89 s) the
spectrum's bins are fs/n ≈ 0.37 Hz apart, wider than a 1/48-octave column below ≈ 25 Hz.
A column took the mean of the whole bins between its edges (one bin, or the interpolated
centre when it held none), so its phase belonged to a frequency up to half a bin off its
own; through a path with group delay τ that misreads the phase by 360°·τ·Δf, ≈ 0.07° at
20 Hz with τ ≈ 1 ms, and the column grid beating against the bin grid makes it periodic
(period ≈ bin·column/(bin − column): 0.9 … 1.4 Hz at 18 … 20 Hz). The neighbour difference
turned it into the 670 ↔ 1830 µs swing.

Ruled out on the pupu captures (`e3/r10a.wav`, `r10b.wav`) with a numpy replica: the
deconvolved `h` evaluated as an exact DTFT at each column centre over the same gate leaves
0.007° rms (the noise in the gate, different between the two runs), against 0.020° for the
bin-summed columns — so gate edge, noise window, regularisation and wrap contribute nothing
measurable at this level.

Fix (`ac2-core::sweep`): each column is the mean over its exact edges of the linear
interpolant between bins (power for the magnitude, the arrival-referred complex value for
the phase), centred on the column whatever the bins; a column narrower than a bin reads the
interpolated value at its centre. Test (`low_frequency_phase_is_free_of_bin_ripple`): a 3 Hz second-order high-pass with 3.7 µs through
a 10 Hz – 20 kHz, 5.5 s sweep: phase within 0.002° of analytic at 16–40 Hz after a line,
neighbour-difference group delay within 0.4 % (before: 0.084°, 42 %).

On the captures (ac2 analysis replaying the 10 Hz sweep as played; phase with the arrival
added back, vs steady sines):

| | 16 Hz | 20 | 25 | 31.5 | 40 | 50 | 80 | 100 |
|---|---|---|---|---|---|---|---|---|
| steady-sine phase, ° | 10.125 | 8.288 | | 5.399 | 4.277 | 3.432 | 2.156 | 1.720 |
| r10a phase before / after | 10.219 / 10.209 | 8.374 / 8.369 | | 5.415 / 5.404 | 4.291 / 4.287 | 3.425 / 3.428 | 2.160 / 2.157 | 1.722 / 1.719 |
| steady-sine group delay, µs | 1535 | 1054 | 706 | 459 | 290 | 187 | 75 | 49 |
| r10a neighbour diff before / after | 1801 / 1532 | 730 / 1082 | 709 / 715 | 511 / 456 | 281 / 291 | 242 / 237 | 66 / 69 | 46 / 46 |
| r10b neighbour diff before / after | 1909 / 1611 | 739 / 1100 | 700 / 710 | 508 / 453 | 278 / 289 | 191 / 187 | 63 / 65 | 44 / 45 |
| r10a ±1/12 oct fit before / after | 1528 / 1521 | 1092 / 1061 | 728 / 722 | 455 / 459 | 292 / 291 | 194 / 192 | 72 / 72 | 45 / 45 |

Phase ripple about a smooth fit (14–50 Hz): 0.020 / 0.022° rms before, 0.005 / 0.006° after;
neighbour-difference group delay over 18–23 Hz 515…1695 µs before, 845…1248 µs after. What
is left is the noise inside the 0.89 s gate (uncorrelated between r10a and r10b, and 50 Hz
mains near 50 Hz); the ±1/12-octave fit averages it out.

## Validation

- `ac2-core` `sweep/tests/response.rs`, `slow_a_fractional_delay_reads_as_a_fractional_arrival`:
  sweeps through an exact fractional delay (frequency-domain phase ramp) for
  φ ∈ {0, 0.1, 0.25, 0.5, 0.73} at 48 and 96 kHz, `f2` at 20 and 40 kHz, clean and at 40 dB
  SNR: arrival within 0.005 sample clean, 0.05 sample at 40 dB.
- `ac2-scene` `trace/tests.rs`, `group_delay_fit_holds_at_low_frequencies`: a 3 Hz high-pass
  with 3.7 µs and a 0.03° ripple of period 1.1 Hz.
- Field (pupu, electrical): the tables above, ac2 against steady sines and REW's 3.69 µs.

## Open questions

- Arrival = peak, or the phase slope over the band where the response is flat? The peak
  matches REW and is unambiguous for one path; the phase slope is what crossover summation
  cares about. Report the peak; the difference readout could offer both.
- Default span for an unsmoothed trace: ±1/24 oct is a choice between resolution and noise. The scene's analytic test (high-pass at 3 Hz, 0.03° ripple of
  period 1.1 Hz) reads within 4.6 % from 16 to 25 Hz at ±1/24 (the neighbour difference: 18 %)
  but up to ≈ 10 % at 45–50 Hz, where the ripple's slope is a larger part of a smaller group
  delay and five columns no longer span its period; ±1/12 stays within 2.6 % from 16 to 50 Hz.
  With the ripple gone (§5) the remaining LF error is noise inside the gate, which the fit
  averages.
