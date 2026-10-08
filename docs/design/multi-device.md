# Multiple devices, one clock domain, drift detection

Status: PLAN §3.1 "Multiple devices in one clock domain; drift detection", P2, phase 7;
§12 "Clock drift between devices". Implemented: output-vs-input drift detection on the
loopback timing monitor (§5, §6; `ac2-core::timing::drift`, PROTO 16). Designed, not
implemented: methods B and C (§4, §7). Multi-device *support* (resampling) is out of scope (§8).
Builds on `q3-loopback-timing.md` (the monitor), `spike-audio-duplex.md` (cpal has no
duplex API) and decisions 3a/3b in `open-questions.md`.

## 1. What "one clock domain" means

Two sample streams are in one clock domain when their sample clocks are derived from the
same oscillator: N samples on one are exactly N samples on the other, forever. Then every
offset between them is a constant per stream start, and the loopback reference cancels it.
Two free-running crystals are never equal: oscillator tolerance is tens of ppm, so two
independent interfaces typically differ by 5–100 ppm, and the difference wanders slowly
with temperature. PLAN §12's "≈ 600 µs in 6–30 s" is 20–100 ppm.

A single device against the *system* clock is not a clock-domain problem. Measured on pupu
(RME Fireface 400 via JACK, 96 kHz, NTP-disciplined host): over 32 h the interface's sample
clock ran +5.56 ppm against `CLOCK_REALTIME`, stable to ±0.3 ppm across 20+ segments of
1 000–25 000 s, with per-row wall timestamps jittering ±1 period (2.67 ms) and never
accumulating (`docs/rigs/pupu.md`). ac2 never compares audio against wall time for a
measurement (wall time only stamps frames for age), so this must not and does not raise a
drift warning. Only **device against device** matters. Within one device the inputs
share its clock: on pupu at 48 kHz / 128 frames over a 65 min run with 0 xruns, every
transfer delay between inputs of the one FF400 stayed at 0 samples.

## 2. Per OS: what one domain looks like and what ac2 can know

| Platform / backend | How several devices become one stream | Clock relation ac2 can state |
|---|---|---|
| Linux, JACK (JACK2 or PipeWire's JACK) | Only through the server: one driver device; extra devices via `alsa_in`/`zita-a2j` (adaptive resampling to the driver clock), or PipeWire's graph, which resamples every non-driver device to the driver | `SingleCallback`: every port is on the server's clock. Resampled devices are on that clock *on average*; their latency wanders slowly as the resampler servo works |
| macOS, Core Audio (cpal) | An aggregate device made in Audio MIDI Setup. With "drift correction" on for a subdevice, Core Audio resamples it to the clock source; off, it free-runs and its samples slide against the clock source's | cpal opens one device (aggregate or not) for both directions: `SameDeviceSeparateCallbacks`. ac2 cannot see the aggregate's drift-correction flags through cpal |
| Windows, WASAPI (cpal) | None in ac2: capture and render are separate endpoints (two streams, two event threads). Picking the input of one interface and the output of another is two clocks; even one interface's two endpoints cannot be *proven* to share a clock through the API | `Unknown` for a capture endpoint, whose output is necessarily another endpoint. Shared-mode engine SRC runs both at the mix rate but does not lock them |
| fake | One simulated callback; `FakeConfig::drift_ppm` resamples the DAC side to simulate two clocks | `SingleCallback` (the drift is what tests inject) |

`ClockRelation` already says this per opened session (`OpenSession.clock`):
`SingleCallback` and `SameDeviceSeparateCallbacks` are *claims* of one clock;
`Unknown` means "may drift; drift detection must warn". None of them is proof: an aggregate
without drift correction is one cpal device and still two clocks inside. So the relation is
a prior, and the loopback measurement is the truth (decision 3a).

ac2 has one input stream per session. All captured channels travel in one block with one
sample index (PLAN §4.3 sync contract), so ac2 itself can never put the TF reference and the
measurement on two different input *streams*. Two input clocks can only reach ac2 already
merged by the OS or the server: an aggregate subdevice without drift correction (macOS) or a
non-resampled bridge (not something JACK/PipeWire do by default).

## 3. Which drift matters

**Output device vs input device** (stimulus vs capture). The generator writes output
samples on the output clock; the capture runs on the input clock. With a rate ratio
`1 + ε` the loopback offset changes by `ε` samples per sample: at 50 ppm, 2.4 samples/s at
48 kHz, 0.5 ms in 10 s.

- *Transfer function with the measured loopback reference*: unaffected. Reference and
  measurement both hear the same drifting stimulus on one input clock; H = M/R cancels the
  common time warp exactly as it cancels latency. Same for the sweep IR, which divides by
  the recorded reference (`ac2-core::sweep`): the sweep is stretched by `1 + ε` (50 ppm over
  a 10 s sweep: 0.5 ms), which shifts harmonic IR positions by a negligible fraction of
  their spacing.
- *Internal reference* (generator as reference, P1): broken. Over an averaging time T the
  stimulus slides by `ε·T`; the phase error at f is `360°·f·ε·T` — at 10 kHz, 2 ppm over
  1 s is already 7°. Refused whenever drift is detected (`TimingTracker::
  internal_reference_allowed`).
- *Loopback timing monitor itself*: the offset is no longer constant, so "offset changed" is
  not by itself a jump. The monitor must follow the drift and judge jumps against the
  drift-predicted offset (§5), or every drift step reads as OUTPUT TIMING JUMP.

**Input channel vs input channel on different clocks** (TF reference and measurement on two
clock domains, i.e. an aggregate without drift correction): the transfer function itself is
wrong. The delay between reference and measurement grows by `ε·T`; H1 averaging over the
MTW stage lengths smears the phase (at 10 kHz the 360° point is reached after `1/(f·ε)` =
2 s at 50 ppm), coherence collapses from the top down, and the delay finder chases a moving
target. This should be refused or loudly flagged. It cannot be seen by the loopback monitor
(output vs loopback input); it shows as a steady slope of the tracked TF delay. See §7.

## 4. Detection methods and resolution

**A. Loopback offset slope (implemented).** While a stimulus plays, the monitor measures the
generator → loopback offset every hop (0.25 s) with sub-sample resolution (GCC-PHAT with
parabolic peak; σ of a few hundredths of a sample at 10 dB loopback SNR). A least-squares
line over the last 30 s of offsets gives the rate ratio. Resolution over a span S with n
points of noise σ: `σ_slope ≈ σ·√(12/n)/S`; at σ = 0.05 sample, S = 10 s, n = 40 at 48 kHz:
≈ 0.06 ppm. The parabolic interpolator's bias (periodic in the fractional offset, a few
hundredths of a sample) adds a few percent of the slope near threshold. Single-clock rigs
read exactly 0 within that noise. This is the sharp tool, but only while a stimulus plays.

**B. Callback timestamps per stream (designed, not implemented).** Each stream's frame
count against the host monotonic clock gives its rate against the host; the difference of
the input and output rates is the device-vs-device drift, without a stimulus. Timestamps
are period-quantised (pupu: ±1 period, 2.67 ms) and on WASAPI arrive per packet with
scheduling jitter. By endpoints alone, 1 ppm needs ~1 h; by regression over n callbacks
`σ_slope ≈ J·√(12/n)/T` (J = 2.7 ms, 375 callbacks/s at 96 kHz/256: ≈ 1 ppm after ~2 min,
0.1 ppm after ~10 min). Useful only where input and output are separate streams (cpal);
on JACK and the fake both directions share one counter and the method reads 0 by
construction. It is a rough pre-stimulus hint ("these two endpoints look like two clocks"),
not a substitute for A. Left for when a Windows/macOS rig can validate it.

**C. TF delay slope (designed, not implemented).** The input-vs-input case of §3: a tracked
delay moving monotonically at a constant rate over minutes is a clock slip between the
reference and measurement inputs. Delay tracking already re-estimates the delay; a
regression of tracked delay over time with the same threshold logic as A would flag it.

### What the Q3 monitor got wrong (found while writing this note, fixed by §5)

Probing the Q3 tracker with the simulated drift (`timing/tests.rs`):

- **Drift above one sample per hop was read as jumps.** The tracker compared each window
  with the previous offset within ±1 sample. At 48 kHz one sample per 0.25 s hop is 83 ppm
  (42 ppm at 96 kHz). At 100 ppm, 48 kHz, 20 s produced 14 OUTPUT TIMING JUMPs of +5 samples
  and no drift estimate; at 300 ppm it never locked. PLAN §12's own example (100 ppm) was
  undetectable.
- **A one-sample step was read as drift.** An offset change of exactly one sample is within
  the ±1 sample agreement, so it was followed and stayed in the regression: a step `h` in
  the middle of a span S biases the slope by `1.5·h/S`, 3 ppm for one sample over 10 s at
  48 kHz — a false CLOCK DRIFT warning from one dropped frame.

## 5. The drift model in the monitor

The offset within an epoch is `offset(x) = c + ε·x + Σ steps`: a straight line (one rate
ratio) plus integer steps (dropped or repeated frames). The tracker keeps that model
(`ac2-core::timing::drift::DriftLine`) and judges every window against its *prediction*:

Offsets are fractional (integer peak plus the parabolic fraction), and each window's
offset belongs to its centre. A window gives an offset only if its correlation's main lobe
(width at half the peak, about the inverse of the stimulus bandwidth) is at most
`max_lobe_s` = 0.2 ms. PHAT whitens every bin, and where a narrowband window's band ends the
leakage of the window's edges pulls the peak off the true lag by a fraction of the lobe.
On pupu (96 kHz, 1743-sample loopback) the sweep emitted from 4.3 Hz read offset 0 (the edge
of the searched range) with PSR 21–30 dB from 9 to 230 Hz. With a floor relative to the
strongest bin it read 1669…1742 up to 1.4 kHz: −74 samples at a 549-lag lobe, −1 at 20–42,
exact at 14 or less. A bias of tens of samples against a ½-sample jump threshold reads as a
lock elsewhere, a jump back and a tilted line: every suite sweep logged a false OUTPUT TIMING
JUMP, and a −3.6 ppm CLOCK DRIFT that refused the internal reference. Such a window is
`Narrowband`. The lobe is judged before the PSR, since a missing loopback whitens to a narrow
lobe. A narrowband window says nothing about the loopback, so it neither counts towards
Lost nor changes the state.

- **Acquisition** chains windows that lie on one line: the second window may differ from
  the first by up to `1 sample + max_drift·Δx` (bound 500 ppm), every later one must lie
  within 1 sample of the line through the candidate's first and newest windows. On lock,
  the candidate's windows seed the regression, so the slope is known from the first locked
  hop.
- **Tracking** compares each window with the line's prediction at its centre. Within the
  jump threshold — ½ sample, or 3σ of the prediction if larger — it is followed (drift) and
  added to the line. σ is the larger of the regression scatter (widened by extrapolation)
  and the correlation smear `W·|ε|/√12`: while a window is captured the stimulus slides by
  `W·ε` samples (3.3 at 100 ppm in a 0.68 s window) and the peak can sit anywhere along
  that. Past the newest held window σ also grows by `slope_sigma_ppm` (1 ppm) times the
  distance: a sweep is timed at the group delay of the frequency it is at plus the
  estimator's band-dependent bias, so on pupu's one clock the offset still rose 0.5 sample
  within each 2.25 s sweep (2.3 ppm). Carried 20 s to the next sweep that slope missed by
  4–5 samples; every suite sweep after the first logged a JUMP 1748 → 1743, and the shifted
  points added up to a 2.0–2.3 ppm CLOCK DRIFT. Unshifted, the regression across sweeps
  sees the same trend repeat and its slope is the clocks' (5 ppm between sweeps is judged
  as such). Over a 20 s gap a step must exceed about 6 samples to be a jump; within
  continuous stimulus the term is a hundredth of a sample per hop. Outside the threshold a window is a jump candidate, confirmed when its windows lie
  on one line over at least W (as in Q3). The jump is measured re the drifted offset at the
  newest candidate window (the first may straddle the step).
- **Steps never enter the slope.** A confirmed jump shifts the held points by the step, so
  the regression continues across it with the same slope. A rate ratio does not change when
  frames drop, so continuing the line is the physically right model. With a ½-sample
  threshold a single dropped or repeated frame is an OUTPUT TIMING JUMP of one sample (it
  was invisible before), and no 10-minute 0 dB SNR run reads a false one.
- **When no line fits** (off the line for longer than a jump takes to confirm plus the loss
  count, without a new line forming) the state is Lost, never a Locked offset nothing
  confirms. Measured on the simulated loopback (pink, 10 dB SNR, 48 kHz): followed without
  jumps, slope within 0.02 ppm, up to 200 ppm; around 300 ppm the lock comes and goes; at
  1000 ppm Lost.
- **The estimate outlives the stimulus.** Points are kept through stimulus gaps within the
  epoch (the line predicts the re-lock offset across the gap, so a gap of a minute at
  100 ppm — 290 samples at 48 kHz — re-locks without a false jump), and the last *judged*
  estimate (span ≥ 10 s) is kept even across new offset epochs of the same stream: a
  dropped block or an xrun changes the offset, not the clocks. A new session (stream open,
  device or rate change) starts without one.

Threshold: **2 ppm** on a span of at least **10 s**, regression over the last **30 s**.
Physics: 2 ppm moves the offset by one sample in ≈ 10 s at 48 kHz, so at the minimum span
the decision rests on a change the integer tracker itself can see, ≈ 30× the slope noise
of method A; and it is well below the 5–100 ppm that separate independent crystals. A
locked clock reads 0; an adaptive resampler (PipeWire, `alsa_in`, a drift-corrected
aggregate) reads 0 on average with servo wander whose amplitude on real hardware is an
open question (§9).

## 6. What the operator sees, what is refused

- **CLOCK DRIFT banner** (warning, below OUTPUT TIMING JUMP): `CLOCK DRIFT · 52 ppm`,
  detail `output and input clocks differ (0.52 ms per 10 s); loopback TF unaffected`. It
  stays up for the session (the clock relation does not heal, so the judged estimate is
  kept through stimulus gaps and offset epochs) and goes when the session is reopened.
  Asserted headless in `ac2-scene::banner` and from mirrored state in `ac2-ui`.
- `ac2 status` prints a `clock` line: the session's clock relation in words and the drift,
  e.g. `clock        one clock (one callback for input and output); drift +50.0 ppm over
  30 s  WARNING: output and input on different clocks` (the fake rig claims one callback
  and simulates two clocks: the relation is a prior, the measurement decides). `ac2 status
  --json` carries the whole `timing` status; `ac2 timing` keeps its drift line.
- The session dialog notes, for an open session whose output plays on another device than
  its input (`ClockRelation::Unknown`): the two devices may run on different clocks, which
  the loopback monitor checks while a stimulus plays.
- **Refused**: the internal reference (generator standing in for the loopback reference)
  whenever drift is detected. Nothing else is refused: transfer
  functions, sweeps, RTA and SPL on a measured reference are unaffected (§3).
- **Published**: `TimingStatus.drift` (`Drift {ppm, span, warning, at}` in `state.timing`)
  with `at` the wall time of the newest window in the estimate. Control commits it when the
  warning flips, the span first reaches the judged length, or the shown value changes
  (1 ppm; 0.1 ppm below 10 ppm while warning), not on every regression wobble.

### Tests

| Where | Case | Expectation |
|---|---|---|
| `ac2-core::timing::drift` | known slope −120 … 500 ppm, no noise | exact; warning iff \|ppm\| > 2 on ≥ 10 s |
| | jitter σ = 0.05 sample over 10 s | slope within 0.2 ppm, σ_pred 0.04–0.08 sample |
| | unshifted vs shifted one-sample step | 2.6 ppm false drift vs exact slope |
| | new epoch, a minute's gap | judged drift kept; line predicts across the gap |
| `ac2-core::timing` (simulated loopback) | ±100, 200 ppm | no jumps, one warning, slope within 2 % (measured 0.02 ppm) |
| | one-sample step | one jump 500 → 501, no warning, \|slope\| < 0.3 ppm |
| | 50 ppm with a −17 jump | one jump, slope 50 ± 1 ppm, one warning |
| | 80 ppm, 20 s stimulus gap | re-lock without a jump, warning kept |
| | 1000 ppm | Lost, never a stale Locked |
| `ac2d/tests/drift.rs` (empty daemon, fake DAC clock) | 0 ppm | drift shown, no warning |
| | 50 ppm | warning, 50 ± 0.5 ppm published and committed; reopened session forgets it |
| | 17 output frames dropped | OUTPUT TIMING JUMP 2000 → 1983, no drift warning |
| `ac2-cli/tests/drift_rig.rs` | 50 ppm | `ac2 status` clock line with `+50.0 ppm` and the warning |
| `ac2-scene`, `ac2-ui` | banner text, order, detail; from mirrored state; session dialog note | asserted headless |

## 7. Input-vs-input (designed, not implemented)

The TF case of §3 needs method C. Once implemented: a measurement whose tracked delay moves
by more than one sample over the stage length of its top MTW band, at a constant rate over
≥ 10 s, gets a protection flag (`CLOCK_SLIP`) that the scene shows as a fault banner
("reference and measurement on different clocks"), and averaging stops; this is a refusal,
because every averaged value is wrong. It needs a field case (an aggregate without drift
correction) to set the slope floor against normal delay wander (temperature, wind).

## 8. Multi-device support (out of scope)

Running devices that are *not* in one clock domain as if they were needs ac2 to resample
one stream to the other's clock: a drift-tracking asynchronous sample-rate converter per
extra device, servoed on its ring fill level (what `alsa_in`, PipeWire and Core Audio's
drift correction do), with its latency wander reported to the timing monitor and every
measurement stamped with which device a channel came from. Until then: one clock domain is
the operator's job (one interface, a word-clock-locked set, a JACK/PipeWire graph, or a
macOS aggregate with drift correction), and ac2 detects and says when it is not.

## 9. Open questions

- Servo wander of adaptive resamplers (PipeWire non-driver devices, `alsa_in`, Core Audio
  drift correction) on real hardware: does a 30 s regression stay below 2 ppm? Measure on a
  rig with two interfaces before tightening or loosening the threshold.
- Method B on WASAPI: callback jitter of two endpoints, and whether it can say "two clocks"
  before the first stimulus.
- Method C's slope floor (§7).
