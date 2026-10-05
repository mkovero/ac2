# Multiple devices, one clock domain, drift detection

Status: design (PLAN §3.1 "Multiple devices in one clock domain; drift detection", P2,
phase 7; §12 "Clock drift between devices"). Part implemented: drift detection on the
loopback timing monitor (§5, §6). Multi-device *support* (resampling) is out of scope (§8).
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
drift warning. Only **device against device** matters.

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

### What the existing monitor got wrong (found while writing this note)

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

- **Acquisition** chains windows that lie on one line: the second window may differ from
  the first by up to `tolerance + max_drift·Δx` (max 500 ppm: beyond that the stimulus
  smears by more than `W·ε` ≈ 16 samples inside one 0.68 s window and the correlation peak
  degrades), every later one must lie within the agreement tolerance (1 sample) of the line
  through the candidate's first and last windows. On lock, the candidate's points seed the
  regression, so the slope is known from the first locked hop.
- **Tracking** compares each window with the line's prediction at its centre. Within the
  tolerance: followed (drift), point added. Outside: a jump candidate, confirmed over
  non-overlapping windows exactly as in Q3, and then judged against the prediction (the jump
  size is measured re the drifted offset, not re the last value).
- **Steps never enter the slope.** A confirmed jump shifts the held points by the step, so
  the regression continues across it with the same slope; a sub-tolerance step (deviation of
  more than half a sample from the prediction, confirmed by the next window) is absorbed the
  same way. One outlier window off by half a sample to a sample, not confirmed by the next,
  is dropped. A rate ratio does not change when frames drop, so continuing the line is the
  physically right model.
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
  detail `output and input on different clocks (0.5 ms per 10 s) · TF on the loopback
  reference unaffected`; while no stimulus plays the detail ends with the estimate's age.
  It stays up for the session (the clock relation does not heal) and goes when the session
  is reopened. Asserted headless in `ac2-scene::banner`.
- `ac2 status` prints a `clock` line: the session's clock relation in words and, once
  measured, `drift 52.0 ppm over 30 s (WARNING: output and input on different clocks)`.
  `ac2 timing` keeps its drift line.
- The session dialog notes, for an open session whose output plays on another device than
  its input (`ClockRelation::Unknown`): the two devices may run on different clocks, which
  the loopback monitor checks while a stimulus plays.
- **Refused**: the internal reference (generator standing in for the loopback reference)
  whenever drift is detected or not yet ruled out. Nothing else is refused: transfer
  functions, sweeps, RTA and SPL on a measured reference are unaffected (§3).
- **Published**: `TimingStatus.drift` (`Drift {ppm, span, warning, at}` in `state.timing`)
  with `at` the wall time of the newest window in the estimate. Control commits it when the
  warning flips or the shown value (0.1 ppm) changes.

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
