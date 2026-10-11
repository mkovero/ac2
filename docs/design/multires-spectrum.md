# Multi-resolution spectrum

Status: implemented (`PROTO_VERSION` 41, session format 24). Decision row S1 in
`open-questions.md`. Code: `ac2_core::multires` (estimator), `ac2d::jobs::spectrum`
(job, `Engine::Multi`), `ac2_scene::stitch` (window-change lines). Wire:
`docs/protocol.md` §3.2 *Spectrum resolution*. Golden vectors: `tools/refgen/sets/multires.py`.

## Problem

One FFT has one window for every frequency. A 65 536-point FFT at 96 kHz resolves 1.46 Hz
(what the low end of a PA needs) but needs 683 ms of signal, so the whole curve reacts to a
change in about a second; a 2048-point FFT reacts within 21 ms but its 47 Hz bins cannot
separate anything below a few hundred hertz. The operator wants both: LF detail at least
that of a 65 536-point FFT at 96 kHz, HF that responds within tens of milliseconds.

## Ladder

Six bands, `N_b = N_max / 2^b`, `b = 0 … 5`. `N_max` is the smallest power of two with
`fs / N_max ≤ 96 000 / 65 536 Hz` (65 536 at 88.2/96 kHz, 32 768 at 44.1/48 kHz, 131 072 at
176.4/192 kHz). The shortest window is therefore 21–23 ms at every common rate.

Every band runs at the full sample rate on the same stream: no decimation, no zoom FFT. A
decimated band would need an anti-alias filter whose group delay shifts the band's time
against the others and whose passband ripple would show at the crossovers; full-rate bands
cost more FFT work (below) but each band's frame is exactly the `N_b` samples ending where it
ends, which keeps the ages exact.

Each band has its own `SpectrumAnalyzer`, its own hop (the daemon's display-hop rule: the
largest power of two ≤ fs/30, clamped to `N/8 … N/2`) and its own averaging state. At 96 kHz
the hops are 8192, 4096, 2048, 2048, 2048, 1024 samples (85, 43, 21, 21, 21, 10.7 ms); at
48 kHz the windows and hops have the same durations at half the samples.

## Placement

The MTW rule (`ac2_core::mtw::layout`): a window with bin `Δf_b` resolves a 1/p-octave column
from `f = κ(p) · Δf_b` up, `κ(p) = 1 / (2^(1/2p) − 2^(−1/2p))` (κ(48) ≈ 69.25: the column
there is exactly one bin wide). Band `b`'s anchor is

`A_b = 2 κ(48) Δf_b`,

where its bin fills half a 1/48-octave column. `A_{b+1} = 2 A_b`, and at `A_b` the next
shorter band (bins twice as wide) just reaches its own validity edge: between two anchors
both bands resolve the 1/48 grid, so blending them there loses no resolution at the design
density. At 96 and 48 kHz the anchors are 202.9, 406, 812, 1625, 3250 and 6492 Hz.

## Level

A column reports **tone level**, as the fixed spectrum does: folded power per bin scaled by
`2 / S₁²` (`S₁` = window sum), so a sine of RMS `a` reads `a²` at its bin in every band
whatever its length. Within a band a column takes the highest bin whose centre it covers,
or the bin under its centre when it covers none (the fixed spectrum's display rule).

A tone-scaled bin reads a stationary noise of one-sided PSD `S(f)` as
`S(f) · ENBW · Δf_b` (ENBW in bins: Hann 1.5, Blackman-Harris 2.0, flat-top 3.77). Halving
the window doubles the noise reading. Switching bands at a point would therefore step a
noise reading by 3 dB at every crossover. Instead, for `A_b < f < A_{b+1}` the column
blends the two bands in power with the shorter band's weight

`w = f / A_b − 1`,  `P(f) = (1 − w) P_b + w P_{b+1}`.

- A sine reads `a²` in both bands, so `P = a²`: the blend keeps its level.
- A noise reads `P_b (1 − w) + 2 P_b w = P_b (1 + w) = P_b · f / A_b`, i.e. a noise
  bandwidth `B(f) = ENBW · Δf_b · f / A_b = ENBW · f / (2κ)`. It is proportional to `f`
  across the whole range `A_0 … A_5` and continuous at every anchor (`w` runs 0 → 1 and the
  next span starts at `P_{b+1}` alone): a constant relative bandwidth of
  `log2(1 + ENBW/(2κ))` octave, ≈ 1/64 octave for Hann (1/48 for Blackman-Harris, 1/25 for
  flat-top). Pink noise reads flat, white noise rises 3 dB per octave, with no steps.
- Below `A_0` band 0 serves alone and above `A_5` band 5 alone: there the noise bandwidth is
  that band's constant `ENBW · Δf_b`, as in a fixed spectrum.

The level is not a band level: use the RTA for dB SPL in bands. The axis says
`dBFS (tone, multi-res)` and its help text says the above.

## Resolution cell and the marker

`cell_hz(f)` is the bin width of the coarsest band contributing to a column. Where it is
wider than the column at the chosen grid, neighbouring columns repeat the same bins: the
frame lists those ranges (`SpecBands.unresolved`, `Unresolved`) and the scene shades them
in the resolution marker's style (fill `steps`). At 1/48 the only such range is below about
`A_0 / 2` ≈ 101 Hz at 96 kHz (band 0 alone, 1.46 Hz bins narrower than a column only above
`κ · Δf_0` ≈ 101 Hz). At 1/96 the octave above each anchor is unresolved too, as for MTW.

## Output grid

`GridDef::Log { ppo, k_min: −7·ppo, k_max: floor(ppo · log2(fs/2000) − ½) }`: 7.8 Hz to
below Nyquist. The capture uses the same grid, so a stored multi-res trace is the
displayed columns. Display smoothing (`SpectrumConfig.smoothing`) runs on these columns with
`ac2_core::smoothing::Smoother` in magnitude mode, as stored log-grid traces are smoothed.

## Time and ages

A column changes only when a band behind it produces a frame: the bottom at band 0's hop,
the top at band 5's. A column is never extrapolated, and it has no value (NaN) until every
band behind it has a frame. Because every window and hop is a power of two and band 5's
hop divides all others, every band's frame ends on a multiple of band 5's hop: the job
stamps a `spec` frame at the newest band frame's end, and `SpecBand.age` is the exact time
from each band's newest frame end to that stamp (nil before its first frame). The UI's
glide moves a curve between two frames; since unchanged columns are identical in both, the
glide moves only the columns whose band made a new frame and invents no motion at the
bottom.

| Frequency (96 kHz) | Windows | Updates every |
|---|---|---|
| < 203 Hz | 683 ms | 85 ms |
| 203 – 406 Hz | 683 + 341 ms | 85 / 43 ms |
| 406 – 812 Hz | 341 + 171 ms | 43 / 21 ms |
| 812 Hz – 1.6 kHz | 171 + 85 ms | 21 ms |
| 1.6 – 3.25 kHz | 85 + 43 ms | 21 ms |
| 3.25 – 6.5 kHz | 43 + 21 ms | 21 / 11 ms |
| > 6.5 kHz | 21 ms | 11 ms |

## Averaging

`SpecAveraging` applies to every band on its own frames: a FIFO of N frames averages band b
over `N · hop_b`. A FIFO stores every frame; the bound is on values per frame
(`SpecResolution::fifo_values_per_frame`: 2¹⁷ for multi, the six bands' bins), so a multi
FIFO holds at most 128 frames (`invalid` beyond).

## Window changes on screen

The operator asked that the change of behaviour across frequency be visible. Each band's
anchor (`SpecBand.sole`) is drawn as a faint grey dashed vertical line in the
resolution-marker style (`theme.unresolved_edge`), labelled with the window length at the
plot foot (`683 ms` … `21 ms`, a label that would collide with the previous one is left
out), on the spectrum and across the spectrograph (labelled there only when the spectrum
is not shown above it). The cursor's frequency line names the windows behind its column
(`1.00 kHz · 171 + 85 ms windows`). The marker's toggle (`ViewState.unresolved`) hides the
lines and the shade together. Positions and text come from the frame (`ac2_scene::stitch`);
`ac2-plot` only draws.

## Accuracy (tests)

- `multires_tests.rs`: flat-top sines within 0.02 dB at any frequency (the curve's peak over
  the neighbouring columns); Hann within its 1.42 dB scalloping (≤ 1.45 dB); white noise
  averaged over 640 frames in sixth-octave groups within ±0.45 dB of
  `10 log10(S · ENBW · f / 2κ)`, steps across anchors < 0.4 dB (measured ≤ 0.32 dB,
  statistical); weights continuous at every anchor; per-band frame ends exact multiples of
  the top hop; NaN before every band has a frame.
- `fixtures/golden/multires_spectrum.*` (numpy, independent implementation): three sines
  plus noise at 12 kHz, 1/24 grid, FIFO 4; Rust matches within 1e-6 dB.
- `ac2d` job test: frame stamps and band ages exact, LF columns unchanged unless band 0 has
  a new frame. End to end: fixed → multi by `meas.update`, `log` grid id, bands on the frame,
  capture on the same grid, FIFO bound refused.

## CPU

`cargo test -p ac2-core --release --test it multires_bench:: -- --nocapture`, 96 kHz, Hann,
FIFO 4, columns formed after every hand-off with a frame: multi-res 1/48 2.05 % of one
core, fixed 65 536 0.49 %, ratio 4.2 (Threadripper PRO 3945WX). The FFT work alone predicts
≈ 3.6× (six full-rate bands, the short ones at shorter hops); the rest is column forming at
up to 94 frames/s. Estimate for pupu (Core i5-2415M, Sandy Bridge 2.3/2.9 GHz, no AVX2): about
2.5–3.5× slower per core, so 5–7 % of one core at 96 kHz.

## Rejected

- **Decimated octave bands / zoom FFT**: anti-alias filters add band-dependent delay and
  ripple at the crossovers; ruled out by the brief.
- **Hard switch at each crossover**: 3 dB noise steps.
- **Short (sub-octave) blend**: steps reduced, not removed; the noise bandwidth would be
  piecewise constant with ramps.
- **Making it the default**: new spectra stay `fixed 65536`; the operator chooses multi-res
  in the dialog or with `--resolution multi`.
