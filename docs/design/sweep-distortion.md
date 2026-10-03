# Sweep (Farina) measurement with harmonic distortion

Status: implemented (`ac2_core::sweep`, `ir.capture`, `ac2 ir capture`, the app's sweep dialog
and Distortion pane). Answers PLAN.md §3.7 "IR capture by ESS, deconvolution, harmonic split,
gating" and §5.5; the ISO 3382 room parameters are separate work.

## Stimulus

One synchronised exponential sine sweep per repeat (`ac2_core::generator::EssPlan`):

- instantaneous frequency `f(t) = f1·e^(t/L)` from `f1` to `f2`; the rate constant `L` is
  rounded so that `f1·L` is an integer (Novak et al. 2015). Then `sin(k·φ(t)) = sin φ(t + L·ln k)`
  exactly: the k-th harmonic of the sweep **is the sweep itself advanced by `L·ln k`**, so after
  deconvolution the k-th harmonic's impulse response lands at `t_k = −L·ln k` relative to the
  linear one, in phase, with no fractional-cycle error.
- defaults `f1 = 20 Hz`, `f2 = 20 kHz` (≤ Nyquist), requested duration 3 s (the actual one is
  `L·ln(f2/f1)` after rounding). Half-cosine fades: in over the first 1/6 octave of the sweep,
  out over the last 1/24 octave (`EssSpec::with_fades`), so it starts and stops without a step.
  Distortion is not reported for fundamentals inside the fades: their level is lower there.
- **level** is typed by the operator, dBFS RMS of the constant-envelope part (0 dBFS RMS = a
  full-scale sine, so the sweep peaks at the typed level + 3 dB re 1/√2…, i.e. peak = 10^(L/20)
  FS). No default; refused above the daemon's ceiling (§5.4).
- each repeat is followed by `post_roll` of silence (≥ 1 s, longer for long sweeps) that
  carries the room's tail and the noise estimate; `repeats` (1…8) sweeps are averaged
  (complex mean of the deconvolved spectra: noise falls 3 dB per doubling, distortion stays).

## Reference: deconvolve the measured loopback

`H(f) = M(f)·R*(f) / (|R(f)|² + ε)` per repeat, `M` = mic record, `R` = the loopback reference
record of the same capture, both zero-padded to `2·next_pow2(len)`; `ε = 10⁻⁶·max|R|²`.

Dividing by the **measured** reference (instead of convolving with the emitted sweep's inverse
and a loopback-derived latency):

- cancels every latency of the chain (DAC/ADC, driver buffering, a cpal stream pair's start
  offset) sample-exactly, without estimating it; t = 0 is "when the loopback heard it", the
  same time base as the transfer function, so the sweep's arrival equals the delay finder's;
- cancels the converters' and the loopback's own response and level (the transfer-function
  convention: mic re reference);
- needs no assumption that the emitted samples arrived unaltered (sample-rate converters,
  dropped output frames during a capture show up as a bad reference instead of a wrong IR).

The reference is a clean copy of the sweep (its own distortion ≪ the speaker's), so dividing by
it is dividing by the sweep: the harmonic separation is the same as Farina's inverse filter.
`ε` keeps out-of-band bins (no excitation) from amplifying noise; in band `|R|²` spans 30 dB
(pink sweep) and the bias at the top is < 0.01 dB. The emitted sweep's inverse is still used
once: a matched filter on the reference locates each repeat's onset (to cut the repeats apart
and to check the reference actually carries the sweep: refused below −40 dB).

## Separating the harmonics

The arrival `d` is the largest |h| in [−20 ms, ½·post_roll]. Harmonic k sits at
`t_k = d − L·ln k`; consecutive orders are `g_k = L·ln((k+1)/k)` apart, which shrinks with k,
so the highest order analysed (`K = 5`, H2…H5) sets one window used for **every** order and the
fundamental's distortion reference:

- pre = 0.1·g_K (before `t_k`, half-Hann rise), post = 0.9·g_(K−1) (after `t_k`, half-Hann fall
  over its last 20 %). For 20 Hz–20 kHz in 3 s: L = 0.434 s, window 8 ms + 87 ms.
- one length for all orders makes the windows' noise and time resolution equal, so ratios and
  the noise floor compare like with like. A harmonic IR longer than the window (a long room
  tail at that frequency) is truncated, as in any gated measurement.
- low-frequency limit: a 95 ms window resolves ≈ 2/W = 21 Hz; fundamentals below
  `max(f1·2^(1/6), 2/W)` are not reported. Longer sweeps (L grows) lengthen the window.
- high-frequency limit: harmonic k exists for fundamentals up to `f2/k` (the sweep stops at
  f2) and below `0.45·fs/k` (anti-alias filters).

## Distortion per harmonic, THD

For the windowed spectra `S_k`, power is averaged over 1/24 octave around a frequency
(`P_k(F)`), so a room's comb does not turn ratios into noise. Harmonic k at fundamental f:

```
HD_k(f) = 10·log10( P_k(k·f) / P_1(f) )        dB re fundamental (k·f → plotted at f)
THD(f)  = 10·log10( Σ_k P_k(k·f) / P_1(f) )    power sum of the orders in band (THD_F)
```

`P_1` uses the same window as the harmonics. The reported fundamental *response* (magnitude and
phase on the 48 ppo log grid, phase referred to `d`) uses the linear gate below. Percent is
`100·10^(dB/20)`.

For a memoryless `y = x + a2·x² + a3·x³` at sweep amplitude A: fundamental `A + ¾a3A³`,
H2 `½a2A²`, H3 `¼a3A³` — the analytic values the tests compare against.

## Noise floor and validity

A window of the same shape and length is cut from the deconvolved silence after the linear
response (ending 10 ms before the end of the post-roll, which every repeat's record covers
fully). Its spectrum gives `floor_k(f) = 10·log10(N(k·f)/P_1(f))` per order and the THD floor
(power sum). A point is **valid** when `HD_k ≥ floor_k + 6 dB`: the noise inside the window then
adds at most 1.25 dB. Otherwise it is shown and printed as `< floor` (the floor's value), never
as a distortion figure. Repeats lower the floor by 10·log10(repeats).

## Linear response gating

The linear IR is windowed from `d − 0.1·L·ln 2` (just after H2's window) to `d + gate`:

- `gate` = operator choice (`--gate 5ms`: free-field response without reflections, valid above
  ≈ 2/gate); default: up to the start of the noise window (the whole room response).
- the stored IR spans from H5's window start to the end of the linear window, so the IR view
  shows the harmonic impulses at `−L·ln k`; ≥ 16 384 points are decimated peak-preserving
  (signed extreme and Hilbert-envelope maximum per bucket).

## Accuracy (tests in `crates/ac2-core/src/sweep/tests.rs`)

Synthetic system through the same path the daemon uses (one continuous recording, onsets
located, repeats cut and averaged), 48 kHz:

| system | quantity | target | achieved |
|---|---|---|---|
| memoryless a2, a3 (H2 −40 dB, H3 −50 dB), 50 Hz–6 kHz, 3 s | HD2, HD3 | ±0.5 dB | see test output |
| same | THD | ±0.5 dB | |
| same | fundamental magnitude 100 Hz–5 kHz | ±0.1 dB | |
| Hammerstein (polynomial → 2nd-order low-pass at 2 kHz) | HD2, HD3 vs `|G(kf)|/|G(f)|` | ±1 dB | |
| linear + noise | every harmonic invalid ("< floor"); floor within 3 dB of analytic | | |

## Protocol and storage

`ir.capture` (lease, generator armed, typed level) runs the repeats as one generator source,
records reference and mic on a job thread, analyses there and stores a `sweep` trace
(fundamental magnitude/phase on the log grid + per-order distortion and floors + IR). Progress
and outcome are the mirrored `sweep` entity. Lease expiry, `gen.stop`/`gen.release`, a forced
takeover or a closed session abort the capture and discard its audio. Sessions (format 4) save
the curves in the trace's CSV and the IR in a JSON sidecar.

## Running it on a speaker (pupu, Genelec 1083 at −50 dBFS)

`ac2 ir capture --ref 2 --mic 1 --out 1,2 --level -50dbfs --duration 6s --repeats 2`: 6 s gives
L = 0.87 s (H5 window ≈ 190 ms, LF limit ≈ 11 Hz), two repeats lower the floor by 3 dB. At
−50 dBFS with room noise ≈ −72 dBFS the fundamental SNR after deconvolution is ≈ 60 dB in band,
so harmonics down to roughly −55…−60 dB re fundamental are measurable; below that the curves
read `< floor`. Raise the level only within the speaker ceiling (−50 dBFS on pupu).
