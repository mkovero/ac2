# Q3 — Continuous loopback timing monitor

Status: implemented (`ac2-core::timing`, `ac2 timing`; phase 1). Answers Q3 in `open-questions.md`. Inputs: decisions
3a/3b, `spike-audio-duplex.md` §6–7.

## What is measured and why

The operator wires the reference out through the same converter as the stimulus and loops it
back into an input (`ac` README "Reference wiring"; decision 3b: a reference is required).
Transfer functions compare the **measurement input against the reference input**. Both are
captured on one input clock, so output-side timing never affects TF alignment.

The monitor watches a different relation: **generator output stream → loopback input**. It
exists to:

1. Detect silent output-side problems that would otherwise corrupt measurements without any
   visible sign: dropped or repeated output frames, cpal/ALSA internal xrun recovery, WASAPI
   render glitches (never reported), clock slips between separate input and output devices.
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
fine, because only its stability matters.

The daemon keeps a **generator history ring** of the last 4 s of emitted samples for the
loopback channel, indexed by output index. It is written by the output callback after the
block is rendered, and is lock-free.

## Estimator

- GCC-PHAT between generator history and loopback capture over a window of W = 2^15 samples
  at 48 kHz (≈ 0.68 s; scaled with the rate), hop 0.25 s, zero-padded to avoid wrap.
  GCC-PHAT is chosen because our own stimulus is known and broadband, so the sharpest
  peak wins. Excitation colour doesn't matter here; it does for Q1.
- The capture window is tapered (raised cosine over its first and last eighth). PHAT weights
  every bin alike, so for a narrowband stimulus (the first seconds of a slow sweep, a tone)
  the out-of-band bins hold only the leakage of a rectangular window's ends; that leakage
  lines up with the reference slice's ends and gave confident peaks exactly on an end of the
  search range (offset `min` or `max`: "jumps" of 0 ↔ 1 s). Tapered, those windows are "no
  measurement" (the state may read Lost while a slow sweep is in its lower part) instead of a
  false offset.
- Sub-sample peak by parabolic interpolation on the correlation magnitude, reported with
  the integer offset. Integer stability is the criterion; the fraction is diagnostics.
- **Acquisition** searches the whole configured range (default 0–1 s of output→input
  latency, configurable to several seconds for long digital chains). **Tracking** searches
  ±64 samples around the locked offset, which is cheaper and immune to other far-off peaks.
- Confidence: peak-to-sidelobe ratio and stimulus level in the loopback. Below either floor
  the window is "no measurement", never a value.
- Cost: one 2^16 real FFT pair per 0.25 s, negligible next to MTW jobs.

## States

```
NoStimulus ──stimulus on──► Acquiring ──3 agreeing windows──► Locked
     ▲                          ▲   │                            │
     └──── stimulus off ────────┘   └── confidence lost ─► Lost ◄┘
Locked ── offset change > 1 sample in 2 consecutive non-overlapping windows ──► Jumped ─► Locked(new)
```

- **NoStimulus:** shows the last locked offset with its age (decision 3a). No claims.
- **Acquiring:** a new epoch has started; three non-overlapping windows must agree within
  ±1 sample.
- **Locked:** internal reference is allowed; offset and its age are published on `ka`.
- **Jumped:** emits event `timing.jump {epoch, from, to, at_capture_sample}`, shows a UI banner
  (OUTPUT TIMING JUMP, with size in samples/ms) and records it in the event log. Jobs that
  use internal reference reset. TF jobs on a measured reference are annotated, not reset.
- **Drift:** the slope of the offset over a 30 s regression. Above 2 ppm (configurable) the
  monitor warns "input and output are on different clocks"; internal reference is then
  refused (PLAN §4.3).

## Epochs

A new **offset epoch** starts, and the state returns to Acquiring, on:

- stream open, device change, sample-rate or buffer change (also a new session epoch, 5b);
- any block with a `BREAKS_CONTINUITY` flag on capture, or an output-side gap;
- an `XRUN` event on either side. On JACK a late cycle can look contiguous in the frame
  counter, so the xrun flag alone must trigger it, even when it arrives one block late.

## Where it runs

It runs as a daemon job, auto-started when a session has a `loopback` mapping (output
channel → reference input) and stopped when it doesn't. Its job lifetime does not depend on
subscribers (PLAN §4.3). Results go on topic `d/timing/loopback` and in the keepalive
summary.

## Tests

CI uses the fake backend, with configurable output latency, output gaps, clock drift and noise:

| Case | Expectation |
|---|---|
| fixed latency 0 / 37 / 4 800 / 48 000 samples | Locked at the right offset within 4 hops |
| output drops 17 frames once | one `timing.jump` with Δ = −17, re-locked |
| output repeats 64 frames | one jump with Δ = +64 |
| drift 20 ppm | drift warning within 30 s; internal reference refused |
| stimulus stops | NoStimulus with age; no jump reported |
| loopback SNR 0 dB, white and pink stimulus | Locked; no false jumps over 10 min simulated |
| xrun flag without a counter jump | new epoch, re-acquired |

A local JACK dummy server checks the same cases with real plumbing. Hardware: steps 3–6 of
`spike-audio-duplex.md` §8 on each OS (loopback cable; level −40 dBFS; operator present).

## Not in scope

- Absolute interface latency as a reported quantity. It is a backend estimate, used only for
  plausibility.
- Output-side compensation. ac2 detects and reports; it never shifts audio to hide a jump.
