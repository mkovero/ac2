# Q3 — Continuous loopback timing monitor

Status: implemented (`ac2-core::timing`, daemon job `jobs/timing.rs`, `ac2 timing`). Answers
Q3 in `open-questions.md`. Inputs: decisions 3a/3b, `spike-audio-duplex.md` §6–7.

## What is measured and why

The operator wires the reference out through the same converter as the stimulus and loops it
back into an input (`ac` README "Reference wiring"; decision 3b: a reference is required).
Transfer functions compare the **measurement input against the reference input**. Both are
captured on one input clock, so output-side timing never affects TF alignment.

The monitor watches a different relation: **generator output stream → loopback input**. It
exists to:

1. Detect silent output-side problems that would otherwise corrupt measurements without any
   visible sign: dropped or repeated output frames, a host's internal xrun recovery, render
   glitches a host never reports, clock slips between separate input and output devices.
2. Provide the validated mapping that the optional internal-reference mode (P1) needs: using
   the generator's own samples as the reference only when this monitor is **Locked**.

The monitor never touches the TF delay finder (Q1) and never changes a measurement's delay.

## Mapping

Each side has its own sample counter: the output index from `OutputTick.start_sample` and
the capture index from block headers. They are only shared on JACK, ASIO and fake.

```
capture_index = output_index + offset          (offset in samples, signed, per epoch)
```

On cpal, `offset` includes the arbitrary start difference between the two streams; that is
fine, because only its stability matters. On the exact backends (JACK, ASIO, and Core
Audio's `coreaudio`, the macOS default) input and output share one index, so `offset` is the
loop's absolute latency: the stated `input + output` plus the converters' remainder
(`coreaudio.md` §7).

The daemon keeps a **generator history ring** of the last 4 s of emitted samples for the
loopback channel, indexed by output index. It is written by the output callback after the
block is rendered, and is lock-free (`ac2-audio` `history.rs`).

## Estimator

- GCC-PHAT between generator history and loopback capture over a window of W = 2^15 samples
  at 48 kHz (≈ 0.68 s; scaled with the rate), hop 0.25 s, zero-padded to avoid wrap.
  GCC-PHAT is chosen because our own stimulus is known and broadband, so the sharpest
  peak wins. Excitation colour doesn't matter here; it does for Q1.
- The capture window is tapered (raised cosine over its first and last eighth). PHAT weights
  every bin alike, so for a narrowband stimulus (a slow sweep's first seconds, a tone) the
  out-of-band bins hold only the leakage of a rectangular window's ends, which lines up with
  the reference slice's ends and gives confident false peaks on an end of the search range.
  Tapered, such windows are "no measurement" (Lost while a slow sweep is low) instead.
- A window whose main lobe (at half the peak) is wider than `max_lobe_s` gives no offset:
  where a narrowband stimulus's whitened band ends, edge leakage pulls the peak off the true
  lag by a fraction of the lobe width.
- Sub-sample peak by parabolic interpolation on the correlation magnitude, reported with
  the integer offset. Integer stability is the criterion; the fraction is diagnostics.
- **Acquisition** searches the whole configured range (`TimingConfig::acquisition`, default
  0–1 s of output→input latency). **Tracking** searches ±64 samples (`track_radius`) around the locked offset, which is cheaper and immune to other far-off peaks.
- Confidence: peak-to-sidelobe ratio and stimulus level in the loopback. Below either floor
  the window is "no measurement", never a value.
- Cost: one 2^16 real FFT pair per 0.25 s, negligible next to MTW jobs.

## States

```
NoStimulus ──stimulus on──► Acquiring ──3 agreeing windows──► Locked
     ▲                          ▲   │                            │
     └──── stimulus off ────────┘   └── confidence lost ─► Lost ◄┘
Locked ── off the drift line (> ½ sample or 3σ) in windows spanning ≥ W ──► Jumped ─► Locked(new)
```

- **NoStimulus:** shows the last locked offset with its age (decision 3a). No claims.
- **Acquiring:** a new epoch has started; three non-overlapping windows must agree within
  ±1 sample.
- **Locked:** internal reference is allowed; the state is in every keepalive (`KaMeta.timing`).
- **Jumped `{from, to}`:** published in the `timing` state event (`TimingStatus`: epoch,
  state, last lock, drift, internal-reference flag); the UI shows the OUTPUT TIMING JUMP banner
  (`ac2-scene` `banner.rs`). Jobs that use internal reference reset. TF jobs on a measured
  reference are not affected.
- **Drift:** the slope of the offset over a 30 s regression. Above 2 ppm (`drift_threshold_ppm`) the
  monitor warns "input and output are on different clocks"; internal reference is then
  refused (PLAN §4.3). Windows are judged against the drift line's prediction, not the
  previous offset, so a drifting offset is followed and a jump (more than half a sample, or
  three standard deviations of the prediction, off the line) is measured against it and
  taken out of the slope: `multi-device.md` §5.

## Epochs

A new **offset epoch** starts, and the state returns to Acquiring, on:

- stream open, device change, sample-rate or buffer change (also a new session epoch, 5b);
- any block with a `BREAKS_CONTINUITY` flag on capture, or an output-side gap;
- an `XRUN` event on either side. On JACK a late cycle can look contiguous in the frame
  counter, so the xrun flag alone must trigger it, even when it arrives one block late.

## Where it runs

It runs as a daemon job, auto-started when a session has a `loopback` mapping (output
channel → reference input) and stopped when it doesn't. Its job lifetime does not depend on
subscribers (PLAN §4.3). Results go on topic `timing`, in `State.timing` and in the keepalive.

## Tests

CI uses the fake backend, with configurable output latency, output gaps, clock drift and noise:

| Case | Expectation |
|---|---|
| fixed latency 0 / 37 / 4 800 / 48 000 samples | Locked at the right offset within 4 hops |
| output drops 17 frames once | one jump with Δ = −17, re-locked |
| output repeats 64 frames | one jump with Δ = +64 |
| drift 20 ppm | drift warning within 30 s; internal reference refused |
| stimulus stops | NoStimulus with age; no jump reported |
| loopback SNR 0 dB, white and pink stimulus | Locked; no false jumps over 10 min simulated |
| xrun flag without a counter jump | new epoch, re-acquired |

A local JACK dummy server checks the same cases with real plumbing. Hardware: `ac2 selftest duplex --emit …`
on each OS, `spike-audio-duplex.md` §8 (loopback cable; level −40 dBFS; operator present).

## Not in scope

- Absolute interface latency as a reported quantity. It is a backend estimate, used only for
  plausibility.
- Output-side compensation. ac2 detects and reports; it never shifts audio to hide a jump.
