# Delay change without a full resettle; sub-sample delay

Status: implemented (`ac2_core::mtw` — `Mtw::set_delay`, `KEEP_MIN_WINDOW_CORRELATION`,
the aligner's splice; `delay.nudge`; `applied_samples` fractional; the app's Ctrl / Alt +
`,` `.` keys; `ac2 delay nudge`). Answers PLAN.md §3.3 "Delay change without full ladder
resettle; sub-sample delay" and the §12 risk "MTW resettle on delay change (~2.4 s)".
Tests: `crates/ac2-core/tests/mtw_delay.rs` (analytic systems), the aligner's splice test,
`crates/ac2d/tests/transfer.rs` (daemon), `crates/ac2-ui/src/state_tests.rs` (keys).

## What the operator gets

Changing a measurement's delay used to restart the whole MTW ladder: every stage went back
to *settling* and the curve was untrustworthy for about 2.4 s (the deepest stage's fill).
Now:

- **Ctrl+,** / **Ctrl+.** move the delay by one sample, **Alt+,** / **Alt+.** by a tenth
  (`delay.nudge`, `ac2 delay nudge main-l -0.25samples`). The phase moves at once — no
  block needs to arrive — and no column goes back to settling.
- Delays carry fractions of a sample everywhere: the finder's fractional estimate is
  inserted exactly, `ac2 delay set main-l 600.25samples` works, the measurement list shows
  the delay to the microsecond when it has a fine part (`12.502 ms`), a captured trace records the exact value.
- A larger correction keeps what it can: each stage keeps its averages while the change is
  small next to its window (at 48 kHz: 2.4 ms at full rate, 9.4 ms on the 12 kHz stage,
  28 ms on the 4 kHz stage); only the stages beyond that settle again. Beyond every stage's
  limit (a fresh insert from 0, a wrong arrival corrected) the ladder restarts as before.

The per-trace display nudge (`,` `.`, decision 8a) is unchanged: it shifts how a trace is
drawn; the new keys change what the measurement aligns to — and are seen the same way (next
section).

## What the keys mean on the view

The transfer panes draw every curve of the session's shared time base as if measured with
the reference's delay (decision 8a, `ac2_scene::trace`). Referred to the *applied* delay, a
change of the compensation cancels: a curve that is not the reference does not move at all,
and when the stepped curve is the reference every other curve is redrawn against the new
reference delay and turns with it, so the whole picture moves together and looks unchanged
(seen on the rig: `Δt` of every stored trace changed by the step, nothing appeared to move).

So the daemon keeps the operator's steps apart: `DelayState::nudged` is what `delay.nudge`
added to the arrival (`applied = arrival + nudged`), and every TF frame states it
(`TfMeta::nudged`). The view's time base of a live curve is the arrival, `applied − nudged`.
Its columns carry `e^{+jω·applied}`, so the curve is drawn `e^{+jω(τ_ref + nudged)}`: a
step Δ moves it by `e^{+jωΔ}`, exactly as a display nudge `ν = Δ` moves a trace (Ctrl+. and
`.` both lead the phase; Ctrl+, and `,` both lag it — no key is flipped), and since the
reference is an arrival too, no other curve moves, whichever curve is the reference.

- **Insert** sets a new arrival: `nudged` = 0 (nothing on the view moves).
- **Nudge** adds its step to `nudged` (at most ±10 s).
- **Typed value** keeps the arrival: `nudged` = typed − arrival, so the live curve alone moves
  to it, as the same run of steps would.
- **Tracking** moves the arrival to the tracker's result: whole samples while two windows
  agree only to a sample, their fractional mean once they agree within 0.1 sample. The
  daemon leaves the delay alone while that arrival is within 0.05 sample of the applied
  arrival (applied − nudged), so tracking a steady path does not keep turning the phase by
  its own scatter; a move goes in as any other (whole samples in time, the fraction as
  phase), keeping the operator's nudge on top.
- **Finder** runs on the raw, unaligned pair: the applied delay never enters it.
- **IR view**: built from the full-rate stage's averaged H1, whose time origin is now the
  exact applied delay including the fraction (`IrMeta::inserted_delay`).
- **Traces** record `TfMeta::delay` = the exact applied delay in seconds; phase comparison
  (decision 8a) therefore refers overlays to the exact value.
- **Protocol** (PROTO_VERSION 15): `DelayState::applied_samples` is a float (samples at the
  session rate, fractions included); `delay.nudge {meas, by: Seconds}` moves the delay by a
  step (an operator action that keeps the last finding: it refines it). The daemon snaps the
  delay in samples to 10⁻⁶ sample so whole-sample values stay exactly whole (no rotation is
  applied to them at all).
- **Sessions**: the saved form (`applied: Seconds`, and since format 12 `nudged`); a loaded
  delay is not rounded to whole samples.
- **TF frame shape** is unchanged (the live spatial average and other consumers see the same
  frames; the curve just stops going back to settling).

## Validation (`crates/ac2-core/tests/mtw_delay.rs`)

Inputs are periodic and band-limited (period 2¹⁹, nothing at Nyquist), built in the
frequency domain as `Y_k = X_k·H(f_k)·e^{−j2πf_kτ/fs}`: every sample is exactly the
analytic system's output for any fractional τ, so expected values are analytic. 48 kHz,
FIFO 8 (and exponential where marked).

| case | result |
|---|---|
| pure delay τ ∈ {37.3, −12.7, 480.5, 0.25}, aligned exactly | per bin of every stage to its served band (0.45·fs at full rate): ≤ 0.028° and 0.0034 dB; γ² ≥ 0.99999 |
| same, aligned to the nearest whole sample | the fraction's linear phase stays: 48.6° at 0.45·fs for 0.3 samples |
| nudges +1, −1, −1, +0.3, −0.3, −0.3, +1.3 samples (FIFO and exponential) | every stage kept; each bin's H1 equals the old one × `e^{j2πfΔ/fs}` to 10⁻¹²; per bin vs analytic ≤ 0.06° and 0.009 dB at once, no column settling |
| peaking-EQ system, nudge +1 and −0.3 | per-bin RMS phase error at once 0.077° / 0.074° / 0.047° (stages 0/1/2) vs a fresh settled measurement's 0.099° / 0.070° / 0.044°; mean magnitude error < 0.0003 dB |
| whole-sample change | exactly 14 blocks dropped (2 + 4 + 8); fraction-only change drops none |
| corrected by 300 samples | full-rate stage resets (its columns *settling*), 12 kHz and 4 kHz stages kept: mean bias −0.023 dB and −0.004 dB vs model −0.019 / −0.002 dB |
| corrected by 113 / 114 samples | 113 keeps the full-rate stage: mean −0.050 dB (model −0.044), mean γ² 0.990; 114 resets it |
| corrected by 2000 samples | ladder restarts |
| two quick 100-sample steps, then one after the average turned over (FIFO, exponential) | reset of the full-rate stage on the second, kept on the third |

Unchanged: `partial_coherence_matches_theory_and_large_delay_does_not_bias_it` (bit for
bit across ±0.2 s), the golden and loopback suites in `tests/mtw.rs`.

## Cost

`crates/ac2-core/tests/mtw_timing.rs` (opt-in, release; this machine was shared with other
builds, so runs vary by ±30 %): MTW per second of input at 48 kHz — main 2.7–3.9 ms,
branch 3.9 ms with a whole-sample delay and 3.9 ms with a fraction; at 96 kHz main 4.5–5.9
ms, branch 5.9 / 5.4 ms: no difference beyond the noise. One kept delay change (rotating
every held block, FIFO 16 at full rate, all stages) costs ≈ 0.5 ms on the job thread, never
in the audio callback.

## Open questions

- The 0.995 window-correlation bound is a choice: 0.999 would shrink the keep limits to
  45 % (50 samples at full rate) and cut the inherited scatter; field use will say
  whether medium corrections (2–30 ms) should keep the low stages more or less eagerly.
