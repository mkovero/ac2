# Sweep (Farina) measurement with harmonic distortion

Status: implemented (`ac2_core::sweep`, the sweep measurement kind and `sweep.run`
(`measurement-tree.md`), `ac2 meas new sweep` / `ac2 sweep run` / `ac2 ir capture`, the app's
sweep dialog and Distortion pane). Answers PLAN.md §3.7 "IR capture by ESS, deconvolution, harmonic split,
gating" and §5.5; the ISO 3382-1 room parameters of the same IR are in
`docs/design/room-metrics.md`.

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
- **onset extension**: the emitted sweep starts about two octaves below the requested `f1`, at
  the same `L`, at the lowest whole number of cycles per `L` that is ≤ `f1/4` (≥ 1 Hz), and
  fades in over those octaves (a half-cosine in time, i.e. raised-cosine in log-frequency),
  replacing the requested fade-in; it reaches full level at `f1` (`SweepTiming::emitted`). The
  requested duration still covers `f1…f2`; the extension adds `L·ln(f1/f_start)`. Why: a path
  answers a sweep's switch-on with a transient over roughly its first two octaves that does not
  follow the sweep's phase, so the deconvolution books it as harmonics of the lowest
  fundamentals (on an electrical path H2 up to 22 dB above the steady-sine value, gone when the
  sweep started 1.7 octaves lower). The rising level keeps the added octaves, where a
  loudspeaker's excursion grows fastest, below full level. A requested start at or below 1 Hz
  is emitted as requested.
- **level** is typed by the operator, dBFS RMS of the constant-envelope part (0 dBFS RMS = a
  full-scale sine, so a sweep at `L` dBFS peaks at `10^(L/20)` FS, crest factor √2). No
  default; refused above the daemon's ceiling (§5.4), and the output path's peak limit holds.
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
  over its last 20 %), both scaled down so the window is at most 100 ms. For 20 Hz–20 kHz in
  3 s: L = 0.45 s (rounded), window 8 ms + 90 ms.
- once capped, the rise is at least a third of the window (W/3 before `t_k`) whenever the window
  still fits in `g_K`: the k-th harmonic IR is band-limited from `k·f_lo ≥ 4/W` up and that band
  edge rings about W/4 on each side of `t_k`, so a short rise cuts it and the lowest columns
  read ~2 dB low; with `W ≤ g_K` the windows still do not overlap.
- why the cap: the sweep's energy per hertz grows with L, the noise inside a window with its
  length. Uncapped (W ∝ L) a longer sweep would only lengthen the window; capped, a sweep of
  twice the duration lowers the floor by 3 dB, as repeats do.
- one length for all orders makes the windows' noise and time resolution equal, so ratios and
  the noise floor compare like with like. A harmonic IR longer than the window (a long room
  tail at that frequency) is truncated, as in any gated measurement.
- low-frequency limit: a 100 ms window resolves ≈ 2/W = 20 Hz; fundamentals below
  `max(f1, 2/W)` are not reported (`f1·2^(1/6)` when the start is not extended).
- high-frequency limit: harmonic k exists for fundamentals up to `f2/k` (the sweep stops at
  f2) and below `0.45·fs/k` (anti-alias filters).

## Distortion per harmonic, THD

For the windowed spectra `S_k`, power is averaged over 1/24 octave around a frequency, but over
at least three resolution cells (3/W Hz) of the window (`P_k(F)`): one cell of noise is
exponentially distributed and exceeds four times its mean 2 % of the time, three averaged almost
never, and a room's comb does not turn ratios into noise. Harmonic k at fundamental f:

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

Windows of the same shape and length are cut from the deconvolved silence after the linear
response, tiled back from 10 ms before the end of the post-roll (which every repeat's record
covers fully) over its second half (≥ 4 windows: W ≤ 100 ms, post-roll ≥ 1 s), and their power
spectra averaged: one window's power per resolution cell is exponentially distributed and 1/f
noise leaves few cells per band at low frequencies, so a single window's floor swings by
several dB between runs; K windows cut the spread by about √K. The averaged spectrum, averaged over 1/3 octave (noise is smooth in frequency, and a steady floor
keeps one noisy estimate from being compared with another), gives
`floor_k(f) = 10·log10(N(k·f)/P_1(f))` per order and the THD floor (power sum). A point is
**valid** when `HD_k ≥ floor_k + 6 dB`: the noise inside the window then adds at most 1.25 dB.
Otherwise it is shown and printed as `< floor` (the floor's value), never as a distortion
figure. The floor falls 3 dB per doubling of repeats, of the sweep duration (with the window
capped) and per 3 dB more level; for white noise of variance σ² at the mic and a sweep of
amplitude A there, `floor(F) ≈ 4·W·F·σ² / (fs·A²·L)` (for the noise test: −44.8 dB predicted,
−44.2 dB measured).

## Display (Sweep / distortion pane)

`ac2_scene::distortion` draws the fundamental's response above the distortion of each order
and THD, and every number the pane shows comes from there.

- **Valid points** are solid lines in the order's colour. A valid column whose neighbours are
  both within the noise is drawn across its own cell (half-way to each neighbour) at its level:
  the value stands for the band around the column, and a lone point would be a dot, or nothing,
  in a small pane where columns are closer than pixels.
- **Within the noise**, an order is drawn at its own floor (the value its readout gives as
  `< …`): a thin dashed line in the order's colour, faded. Each harmonic has its own floor
  (`N(k·f)/P_1(f)`), so one shared floor would be wrong for every order but one; drawing each
  where it applies shows which order is limited by noise, and where, without stacking five
  translucent bands into an unreadable grey. Where an order is valid its floor is not drawn:
  the curve already says it is above it.
- **Shading** fills the plot from the bottom up to the lowest order's floor at each frequency:
  below it every order is within the noise. Legend: the order names in their colours, THD, a
  dashed sample `< floor`, a shaded sample `noise`.
- **dB / %**: the toggle in the pane's title (`dB | %`, key **U**) switches the axis and every
  readout, including `< floor` values. Percent is drawn on a log axis over the same ratios as
  the dB view (−100…0 dB = 0.001…100 %), labelled per decade (1-2-5 when tall enough, every
  second decade when very short): equal ratios keep equal distances, so a curve does not
  change shape when the unit does, and 0.01 % stays as readable as 10 %. The fundamental's
  response stays in dB.
- **Caption** (`name · arrival · repeats × duration · window`) goes right of the fundamental's
  axis title and is shortened to fit (without the name, without the repeats, the arrival
  alone, nothing); `CLIPPED` is kept on every form. The legend wraps under the distortion
  axis title in a narrow pane, and the cursor readout starts below the legend.

## Linear response gating

The linear IR is windowed from `d − 0.1·L·ln 2` (just after H2's window) to `d + gate`:

- `gate` = operator choice (`--gate 5ms`: free-field response without reflections, valid above
  ≈ 2/gate); default: up to the start of the noise window (the whole room response).
- the stored IR spans from H5's window start to the end of the linear window, so the IR view
  shows the harmonic impulses at `−L·ln k`; ≥ 16 384 points are decimated peak-preserving
  (signed extreme and Hilbert-envelope maximum per bucket).

## Accuracy

Synthetic systems through the same path the daemon uses (one continuous recording, onsets
located, repeats cut and averaged), 48 kHz, worst case over the fundamentals 100 Hz …
f2/k/1.15 (`crates/ac2-core/src/sweep/tests.rs`), and the whole chain on the simulated rig
(`crates/ac2d/tests/sweep.rs`: generator → fake converters with a distorting acoustic path →
recorder → analysis, f32 audio):

| system | quantity | target | achieved |
|---|---|---|---|
| memoryless a2, a3 (H2 −40 dB, H3 −50 dB), 50 Hz–6 kHz, 3 s | HD2 / HD3 | ±0.5 dB | 0.16 / 0.21 dB |
| same | THD | ±0.5 dB | 0.16 dB |
| same | fundamental magnitude 100 Hz–5 kHz | ±0.1 dB | 0.001 dB (phase < 2°) |
| Hammerstein (same polynomial → 2nd-order low-pass at 2 kHz) | HD2 / HD3 vs `HD·|G(kf)|/|G(f)|` | ±1 dB | 0.16 / 0.21 dB |
| linear system + noise | harmonics valid | ≤ 5 % of points | passes; level − floor median within 1.5 dB |
| same, 4 repeats vs 1 | floor | −6 ± 1.5 dB | −5.3 dB |
| memoryless, 10 Hz–20 kHz, 5.5 s (capped window) | HD2 / HD3, 20–40 Hz | ±0.5 dB | 0.05 / 0.05 dB (2.1 dB H2 with an 8 ms rise) |
| memoryless, 50 Hz–6 kHz, 3 s, emitted from 11.5 Hz | HD2 / HD3, 50–100 Hz | ±0.5 dB | 0.13 / 0.07 dB |
| 1/f noise, 32 seeds, capped window, 1 s post-roll | floor spread, 4 windows vs 1 | < 0.7× | 0.86 vs 1.89 dB |
| daemon + fake rig (H2 −40 dB, H3 −50 dB at −20 dBFS), 100 Hz–5 kHz, 1 s | HD2 / HD3, 200 Hz–1.4 kHz | ±0.5 dB | 0.10 / 0.15 dB |

## Protocol and storage

`sweep.run` of a sweep measurement (lease, generator armed, the measurement's typed level
checked against the ceiling at every run; `measurement-tree.md`) runs the repeats as one
generator source,
records reference and mic on a job thread, analyses there and stores a `sweep` trace
(fundamental magnitude/phase on the log grid + per-order distortion and floors + IR) under the
measurement, named `Run <n>`. Progress
and outcome are the mirrored `sweep` entity. Once the recording is in, the generator is
disarmed (the lease stays with its holder): a sweep is one shot, and nothing is left armed for
a stray Enter. Lease expiry, `gen.stop`/`gen.release`, a forced takeover or a closed session
abort the capture and discard its audio. The ac2 CSV export (v2) holds the whole sweep: the
curves as columns, the analysis facts (`SweepInfo`) as a `# sweep_info:` JSON header line and
the decimated IR as a second table (`t_s,linear,etc_db`, announced by `# sweep_ir:`), so
`trace import` of an export restores the sweep trace with its delay; sessions (format 5) save
that one CSV per trace. A v1 export (no analysis facts) imports as its transfer function, the
distortion dropped with a note. The sweep's columns are never mic-corrected (the analysis
uses the raw recordings); a curve can be applied afterwards (`q7-calibration.md` §9).

## Running it on a speaker (pupu, Genelec 1083 at −50 dBFS)

`ac2 ir capture --ref 2 --mic 1 --out 1,2 --level -50dbfs --duration 6s --repeats 2` (about
15 s: two 6 s sweeps, each with 1 s of silence after it). Out 2 must play the sweep too: it is
the loopback the analysis divides by. 6 s and two repeats put the floor 6 dB under a single 3 s
sweep (window 100 ms, fundamentals from 20 Hz). The level stays at the speaker ceiling
(−50 dBFS); a 1083 at that level is expected to show H2/H3 well under 1 % (−40 dB) in the
mid band, so where the summary prints `< −…` (below the floor) the next step is more repeats
(−3 dB per doubling), never more level. Room rumble (60–200 Hz bursts up to −53 dBFS on the
mic) raises the floor of low fundamentals' harmonics; repeat the sweep when a burst coincides.
