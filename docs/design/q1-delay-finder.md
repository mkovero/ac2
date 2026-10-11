# Q1 — Delay finder: target, estimator, confidence, acceptance

Status: implemented (`ac2-core::delay`, phase 2). Answers Q1 in `open-questions.md` within decisions 1a–1f
and 8b. Normative for `ac2-core::delay`.

Evidence:

- `tools/experiments/q1/finder.py` is the reference prototype. It implements this note line
  by line. Where the two disagree, this note wins and the prototype is a bug.
- `tools/experiments/q1/mc.py` runs Monte-Carlo scenario classes, `sweep.sh` runs the parameter
  sweeps behind the defaults, `tracking.py` evaluates the tracking rule, and
  `check_fixtures.py` runs the prototype against the golden fixtures.
- `tools/refgen/sets/delay_finder.py` generates the scenarios and writes the golden sets
  `fixtures/golden/delay_finder_*` (§13).

All numbers below are at fs = 48 kHz unless stated otherwise. Lengths given in samples scale
with fs, rounded to a power of two.

---

## 1. Decisions in this note

| # | Decision |
|---|---|
| Q1-a | **Target.** The answer is the first significant arrival, meaning the earliest candidate within **−12 dB** of the strongest (1a). This is one global threshold that the operator can adjust (1b). It is the same for every band; the experiments found no reason to make it band-dependent. The strongest arrival is always reported alongside it. |
| Q1-b | **Estimator.** Regularised H1 with **two passes** on the raw, unaligned pair. Pass 1 tiles the whole signed search range. Pass 2 is a refinement tile centred on the strongest arrival, plus one at the first-arrival pick when that lies outside it. Both use Hann Welch segments, zero-padded 2×, with H = Gxy/(Gxx + ε·mean_band Gxx), ε = 0.01. The result is band-limited, and an analytic (Hilbert) IR comes out of one inverse FFT. |
| Q1-c | **Candidates** are local maxima of the **Hilbert envelope**, never of \|h\|. A candidate must be above a statistical detection floor and must not be explained by a stronger candidate's pulse skirt. Times and levels come from a joint pulse-model fit that removes neighbouring arrivals ("deblend"). |
| Q1-d | **GCC-PHAT is not an automatic fallback.** It removes the relative levels the −12 dB rule depends on, and on our scenarios it never beat regularised H1 (§11.3). It remains available as a diagnostic estimator option. Poor excitation leads to "no estimate", not a switch of estimator. |
| Q1-e | **Outcomes:** `Accepted`, `Ambiguous` (with ≤ 3 ranked candidates) or `NoEstimate` with typed reasons. Accuracy targets apply to `Accepted` only. |
| Q1-f | **Tracking** moves the held delay only on two `Accepted` results from windows that share no samples and agree within ±1 sample (full, mid) or ±0.1 ms (sub). |
| Q1-g | **Acceptance numbers** are restated per scenario class in §12. The provisional targets hold for full range. For mid and sub they hold only for *resolved* arrivals, with revised tolerances; unresolved clusters get a weaker, measured guarantee. |

---

## 2. What the finder reports

- **Delay** = absolute signed lag D in samples at the input rate. `meas[i] ≈ (h ∗ ref)[i − D]`,
  and positive means the measurement is late. Sample indices from the block headers make it
  absolute: the result never depends on the block offsets, and the held delay is never added
  to it.
- **First significant arrival**: the earliest candidate whose level is ≥ T (default
  −12 dB) relative to the strongest candidate. This is the value that gets proposed, inserted
  and tracked.
- **Strongest arrival**: reported in the same form, so the UI can show both (1c).
- Each arrival has a fractional delay, the integer delay (rounded), its level in dB relative to the
  strongest, its analytic phase in degrees (≈ 0° in phase, ≈ 180° inverted, anything else
  means a dispersive path), a 1-σ timing uncertainty, and a lobe-shape misfit.
- **Ranked list** of at most 3 candidates when the result is ambiguous (§8).
- **Confidence record** (always present, also with `NoEstimate`): PSR, acquisition PSR, band
  SNR, excited fraction, pulse width (actual and nominal), detected excitation period.

"First significant arrival" is a band-dependent quantity. In a band B, arrivals closer than
the pulse width (≈ 1/B) are a single lobe and cannot be separated by any estimator working in
that band (§11.5). The finder states this through `Ambiguous{MergedLobe | CloseArrivals}`.
It never invents a separation.

---

## 3. Conventions

- Band weighting B(f): zero-phase. The −6 dB edges sit at f_lo and f_hi. A raised cosine in
  log₂ f, 1 octave wide, is centred on each edge:
  `B = R(log2(f/f_lo)) · R(log2(f_hi/f))`, with `R(x) = ½ − ½ cos(π·clamp(x + ½, 0, 1))`.
  The upper edge is clipped to `f_hi ≤ (fs/2)·2^(−½)` so the taper ends at or below Nyquist.
  B is the same in the finder and in the fixture SNR definition.
- Windows are periodic Hann (Q4 convention).
- **Band SNR (scenario definition)**: the ratio of B²-weighted power of the clean measurement
  to that of the noise in the measurement block. Acceptance targets are stated against it.

---

## 4. Estimator

### 4.1 Inputs and coverage

The inputs are a ref block `r[0..Lr)` at absolute index `a` and a meas block `m[0..Lm)` at
absolute index `b`, both at fs and on the same clock. Every lag in the search range
`[Dmin, Dmax]` needs the ref to cover `[b − Dmax, b + Lm − Dmin)`. The daemon's ring therefore
hands over the ref block extended by the search range on both sides: Lr = Lm + (Dmax − Dmin).
The finder also accepts shorter coverage. A lag whose ref segment is missing for more than half
the meas segments is **invalid** and is excluded from the search. If every lag is invalid,
the result is `NoEstimate{InsufficientOverlap}`.

Segment grid: `Kmax = ⌊(Lm − N)/hop⌋ + 1` segments, with the grid centred in the meas block.

### 4.2 One tile (regularised H1 at a pre-shift D0)

For tile shift D0 (integer) and segment length N, with meas segment starts s_i:

```
r0_i   = b + s_i − D0 − a                           (segment used only if 0 ≤ r0_i ≤ Lr − N)
X_i    = FFT_2N( w · r[r0_i .. r0_i+N) )            zero-padded to 2N
Y_i    = FFT_2N( w · m[s_i  .. s_i+N)  )
Gxy_k  = Σ_i conj(X_i,k) · Y_i,k        Gxx_k = Σ_i |X_i,k|²
μ      = Σ_k B_k Gxx_k / Σ_k B_k                     (in-band mean, B-weighted)
Ĥ_k    = Gxy_k / (Gxx_k + ε·μ)                       ε = 0.01
A_k    = c_k · B_k · Ĥ_k,  k = 0..N   (c = 1 at DC and Nyquist, 2 otherwise; negative bins 0)
a[n]   = IFFT_2N(A)[n]                               complex analytic IR
g      = (B_0 + 2·Σ_{k=1}^{N−1} B_k + B_N) / 2N      envelope peak of a unit pure delay
ρ(δ)   = Σ_n w[n]·w[n+δ] / Σ_n w[n]²                  Hann overlap at lag δ
h(D0+δ) = a[δ mod 2N] / (g · ρ(δ))
```

Zero-padding to 2N makes lags |δ| < N linear rather than circular. That is the "≥ 2× span"
requirement of PLAN §5.2 applied per tile, since each tile only serves |δ| ≤ N/4. Dividing
by ρ(δ) removes the window-overlap loss for an arrival that is offset from the tile centre.
In expectation, a unit pure delay gives |h| = 1 at its lag in every tile.

### 4.3 Pass 1: acquisition over the whole search range

Segment N = N₁ (per band, §9) with hop N₁/2. The tile centres are
`D0_j = Dmin + N₁/8 + j·N₁/4`, and each tile serves lags `δ ∈ [−N₁/8, N₁/8)`. Stitching
them gives h₁(D) for every D in [Dmin, Dmax]. The envelope is e₁ = |h₁|.

Away from the true arrival, a tile's estimate is a correlation of unaligned segments. Its floor
is set by the time–bandwidth product, not by noise: about −37 dB median in the full band and
−27 dB in the sub band, even without noise (§11.4). Pass 1 therefore only finds the strongest
arrival (`i_s = argmax e₁`), and its confidence gate is weaker: PSR_acq ≥ 10 dB.

### 4.4 Pass 2: refinement tiles

One tile at `D0 = D(i_s)` with N₂ = 2·N₁ and hop N₂/4 (75 %), serving |δ| ≤ N₂/8. Inside this
**refinement window** its values replace h₁. The dominant arrival is now explained, so the
floor falls to the noise: from −37 dB to about −60 dB median in the full band at 40 dB SNR
(§11.4). At 48 kHz the window is ±21 ms (full), ±43 ms (mid) and ±171 ms (sub).

The candidate stage (§5) then runs. If the rule pick (§6) lies outside every refinement
window, a **second refinement tile** is centred on the pick's integer lag, and the candidate
stage runs once more. All tiles share one normalisation (a unit pure delay reads 1), so levels
from different tiles are comparable. Every level, time and confidence figure used for the
decision comes from refinement windows. A direct sound 25–45 ms before a louder reflection
(beyond the first window in the full band) was accepted correctly in 99 % of trials (§11.1,
`full/two_path_far`).

The pulse model, excitation check and detection floor use the Gxx of the first refinement
tile.

### 4.5 Pulse model

```
W_k  = Gxx_k / (Gxx_k + ε·μ)          (regularisation shrink; ≡ 1 for the nominal model)
P_k  = B_k · W_k
p(t) = analytic IR of P, normalised to |p(0)| = 1
w_p  = full width of |p| at −6 dB      (nominal w₀ with W ≡ 1)
```

p is the expected shape of one pure-delay arrival under this excitation. It is used for
sidelobe rejection, the lobe fit and deblending. At 48 kHz, w₀ is 3.8 samples (full),
20 samples (mid) and 535 samples ≈ 11 ms (sub).

### 4.6 GCC-PHAT

The option `Estimator::Phat` replaces Ĥ by `Gxy/|Gxy|` (W ≡ 1). It is never selected
automatically (Q1-d, §11.3).

---

## 5. Candidates

### 5.1 Detection floor

For each region (the refinement window, and the rest of the valid pass-1 range), the floor is
derived as follows:

```
B_eff  = (Σ_k P_k)² / Σ_k P_k² · Δf                       equivalent noise bandwidth
N_ind  = (region length / fs) · B_eff                     independent envelope samples
κ      = sqrt( ln(N_ind / p_fa) / ln 2 ),  p_fa = 1e−3
m      = median(e) over the region
m'     = median(e) over the region minus lags within 3·w_p of any lag with e > κ·m
         (only if at least ¼ of the region remains; otherwise m' = m)
det    = κ · m'           σ_n = m' / 1.1774
```

Under a Rayleigh envelope, the median is σ·√(2 ln 2) and P(e > a) = exp(−a²/2σ²). The level
`det` is therefore exceeded by noise somewhere in the region with probability ≈ p_fa. Removing
lobes before taking the median keeps the wide sub-band lobes from inflating the floor.

### 5.2 Peaks

1. Candidate lags are the local maxima of e (`e[i] ≥ e[i−1]` and `e[i] > e[i+1]`) at valid
   lags with `e ≥ max(det, e_max·10^(−20/20))`. The list depth is −20 dB.
2. Visit them in descending e. Drop a peak j if a stronger kept candidate s lies within w_p,
   or if `e_j ≤ e_s · |p(j − s)| · 10^(6/20)`, meaning it is explained by s's pulse skirt plus a
   6 dB margin.

### 5.3 Times and levels (deblend + lobe fit), refinement window only

- The initial time is the vertex of a parabola through ln e at i−1, i, i+1. The initial amplitude is
  `A = h(i)/p(i − τ)`.
- Two iterations, for each candidate a:
  - residual `r(k) = h(i_a + k) − Σ_{b≠a, |τ_b − τ_a| < 6 w_p} A_b · p(i_a + k − τ_b)` for
    |k| ≤ ⌈1.5 w_p⌉. This subtracts the complex (polarity- and phase-aware) model of the
    neighbours.
  - fit `c·|p(k − τ)|` to |r(k)| over the samples where |p| ≥ 0.1, minimising the relative RMS
    misfit `μ_a = ‖|r| − c|p|‖ / ‖c|p|‖` over τ ∈ i_a ± max(1.5, 0.05 w_p). Use a 25-point grid
    refined twice by 8×, then a parabolic vertex. Evaluate p on a 1/16-sample grid.
  - `A_a = r(k*)/p(k* − τ_a)` at the sample nearest τ_a.
- level_a = 20·log10(|A_a| / max_b |A_b|). phase_a = arg A_a.
- Outside the window, the parabolic vertex and e give time and level, and the misfit is 0.
- **Uncertainty**: `u_a = max(0.35 · w_p · σ_n / |A_a|, 0.1)` samples (≈ 1 σ, calibrated in
  §11.2). The 0.1-sample floor reflects the fractional-interpolation bias that dominates at
  high SNR in the full band.

Without deblending, a neighbour's skirt 3 pulse widths away biased a sub-band pick by 8–13
samples (0.17–0.27 ms). With deblending the bias is under 0.3 samples (§11.5). The lobe fit
uses the whole main lobe. For wide sub-band lobes this is the difference between a 3-sample
vertex and a matched estimate.

---

## 6. First-arrival rule

```
strongest = argmax level      (level 0 dB)
pick      = earliest candidate with level ≥ T          T = −12 dB (operator-adjustable)
```

The threshold is the same in every band. The experiments gave no reason for band-dependent
thresholds: for accepted resolved arrivals, the level estimate of the direct sound is within
0.2 dB of truth in every band (max |error| 0.19 dB full, 0.20 dB mid, 0.13 dB sub, `two_path_resolved`).
What does differ per band is resolution (w_p), and that is handled by the ambiguity rules.

---

## 7. Confidence and refusal (`NoEstimate` reasons)

Every reason is evaluated and reported, not just the first one found. Any reason makes the
result `NoEstimate`. The candidate list and confidence record are still returned for the IR
panel and the CLI.

| reason | test | default |
|---|---|---|
| `NoReference` / `NoSignal` | ref or meas block identically zero; no envelope peak | — |
| `ObservationTooShort` | Lm < N₂ | — |
| `InsufficientOverlap` | no valid lag (§4.1) | — |
| `InsufficientExcitation` | excited fraction < 0.5. **Excited fraction** = octaves of [f_lo, f_hi] where W_k ≥ 0.5 (Gxx ≥ ε·μ), divided by log₂(f_hi/f_lo) | 0.5 |
| `PeriodicExcitation{period}` | excitation period P (from the generator, else detected, §10) with P ≤ (Dmax − Dmin) + T_tail·fs | T_tail = 1 s |
| `LowPsr` | PSR = 20·log10(e_max/det_win) < 15 dB, or PSR_acq = 20·log10(e₁,max/det₁) < 10 dB, or no candidate | 15 / 10 dB |
| `LowPrecision` | 2.5·u_pick > band tolerance (§9) | k = 2.5 |
| `PeakAtSearchEdge` | pick within 2 samples of Dmin, or strongest within 2 samples of Dmax | 2 samples |
| `LowBandSnr` | band SNR estimate < −10 dB | −10 dB |

The PSR threshold of 15 dB is |T| + 3 dB. If an earlier arrival exists exactly at the
threshold, it must stand at least 3 dB above the level that noise maxima reach with
p_fa = 10⁻³. Otherwise the absence of a first arrival cannot be certified, and the finder
refuses rather than reporting a reflection as the first arrival.

**Band SNR estimate** (reported; its refusal threshold is only a sanity guard): the pair is
aligned at the strongest integer delay, and Welch coherence is computed with Hann segments of
N₁ (hop N₁/2):
`SNR_band = 10·log10( Σ B_k Gyy_k γ²_k / Σ B_k Gyy_k (1 − γ²_k) )`. Reflections lower it as
well, so it reads lower than the scenario SNR in reverberant cases. That is why it is not
used as the main confidence measure.

---

## 8. Ambiguity (`Ambiguous` reasons) and the ranked list

These checks run only when there is no refusal reason. Any of the following makes the result
`Ambiguous`:

| reason | test | default |
|---|---|---|
| `BorderlineLevel` | a candidate earlier than `pick_clear` has level in [T − m_b, T + m_b), where pick_clear is the earliest candidate with level ≥ T + m_b | m_b = 2 dB |
| `CloseArrivals` | another candidate with level ≥ −20 dB lies within 2·w_p of the pick | 2 w_p |
| `MergedLobe` | pick misfit μ > max(μ_band, 3·σ_n/\|A_pick\|) | μ_band: 0.10 full, 0.05 mid, 0.03 sub |
| `OutsideRefinement` | the pick still lies outside every refinement window, e.g. when the ref does not cover its refinement tile (only pass-1 evidence) | — |

**Ranked list** (≤ 3, no duplicates): the rule pick, then pick_clear, then the strongest,
then the unsure and close candidates by descending level.

Behaviour (decision 1c):

- The rule pick is candidate 1. The app lists the candidates and the operator picks one
  with 1–3; the IR panel marks all of them. CLI: `AMBIGUOUS · <reasons>` and the ranked
  list; `ac2 delay insert --pick 1…3` (or `first` / `strongest`).
- Tracking never acts on `Ambiguous` (§10). An ambiguous `delay.find` result also pauses
  tracking outright (the measurement's `delay.awaiting_pick`): no window is observed until the
  operator resolves it with `delay.insert` (a candidate) or `delay.set` (a typed value), or runs
  `delay.find` again (a result that is not ambiguous resumes it). Otherwise two later
  `Accepted` windows of one of the candidates could move the delay to a choice the operator
  was just asked to make. A resumed tracker starts over: the audio skipped while paused cannot
  be spliced onto what follows.
- UI (Transfer pane): X finds and inserts the first arrival, Shift+X the strongest, in the
  band and observation chosen in the palette (`Delay finder: auto / full / mid / sub band`,
  `custom band (Hz)…`, `observation length (s)…`); auto band and the band's default
  observation unless set.

**`MergedLobe` with a single candidate.** `MergedLobe` is the only reason that can stand
alone with a one-row list: `BorderlineLevel` and `CloseArrivals` each add a second candidate,
and `OutsideRefinement` alone lists the coarse pick. A merged lobe is a peak whose shape does
not fit one arrival in the band: two arrivals closer than the pulse width, or a dispersive
path. A two-way box with an LR4 crossover inside the full band (2.6 kHz) does exactly that:
the LR4 sum is a flat-level 2nd-order allpass with ≈ 0.17 ms group delay at f0, so the lobe is
smeared (misfit ≈ 0.17 against μ 0.10), its phase is far from 0° and its centre sits ≈ 1.2
samples after the onset — outside the full-band tolerance, so accepting it would break §12.
Decision: the outcome stays `Ambiguous{MergedLobe}` and lists only the peak; the finder never
invents the second arrival (§2). CLI and app say why there is one row ("One peak only: two
arrivals closer than this band resolves (or a crossover's group delay) are merged into it …"),
offer only pick 1, and point to a band without the crossover for the first arrival. Tests:
`ac2-core` `crossover_in_band_is_a_merged_lobe_listed_alone` (the rig case, crossovers modelled
at 430 Hz and 2.6 kHz), `ac2-scene` `finding` (the text).

---

## 9. Parameters per band

| | full range | mid | sub |
|---|---|---|---|
| band (−6 dB edges) | 2 kHz – min(16 kHz, fs/2·2^−½) | 300 Hz – 3 kHz | **20 – 120 Hz** (1e) |
| N₁ (acquisition segment) | 4096 (85 ms) | 8192 (171 ms) | 32768 (683 ms) |
| N₂ (refinement, = 2 N₁) | 8192 | 16384 | 65536 |
| refinement window | ±21 ms | ±43 ms | ±171 ms |
| default observation Lm | 0.25 s | 0.5 s | **4 s** |
| minimum observation | N₂ (0.17 s) | N₂ (0.34 s) | N₂ (1.37 s) |
| nominal pulse width w₀ | 3.8 samples (79 µs) | 20 samples (0.42 ms) | 535 samples (11.1 ms) |
| accuracy tolerance | **1 sample** | **0.05 ms** (2.4 samples) | **0.1 ms** (4.8 samples) |
| merged-lobe misfit μ_band | 0.10 | 0.05 | 0.03 |
| tracking agreement | ±1 sample | ±1 sample | ±0.1 ms |

Shared: ε = 0.01, T = −12 dB, list depth −20 dB, p_fa = 10⁻³, PSR ≥ 15 dB, PSR_acq ≥ 10 dB,
m_b = 2 dB, close spacing 2 w_p, sidelobe margin 6 dB, deblend reach 6 w_p, floor exclusion
3 w_p, uncertainty coefficient 0.35 with k = 2.5, band SNR ≥ −10 dB, periodicity −10 dB,
T_tail = 1 s, search ±1 s (1d).

A custom band takes its segment lengths, tolerance and μ_band from the preset its lower edge
falls in: ≥ 1 kHz full range, ≥ 150 Hz mid, lower sub (`Band::class`).

Implementations may decimate before the sub and mid bands (e.g. by 16 and by 4) if the results
stay within the acceptance table. N and windows then scale with the rate. The reported delay
is always at the input rate.

### 9.1 Auto band

Auto mode evaluates full → mid → sub in that order and reports the first band whose result is
not `NoEstimate`, naming the band. "Picks from measured excitation" (1e) works through the
excitation check and the PSR gate: a sub fed sub-only content refuses the full and mid bands
with `InsufficientExcitation` or `LowPsr`. `auto_band.py` (direct −3 dB + reflection 50 ms
later, 30 dB) selects full for pink and 100 Hz–8 kHz excitation, mid for excitation that stops
at 1.2 kHz (full: `InsufficientExcitation`), and sub for a 25–150 Hz feed (full: `LowPsr`;
mid: `InsufficientExcitation`, `LowPsr`). Every pick was within tolerance.

---

## 10. Periodic excitation and tracking

### 10.1 Periodic excitation (PLAN §5.4)

With a periodic stimulus of period P, the measured pair only identifies h modulo P. A
reflection later than P − (lag of the direct sound) lands at an earlier apparent lag and is
indistinguishable from an arrival. The rule is therefore a refusal, not a heuristic:
`P ≤ (Dmax − Dmin) + T_tail·fs` → `NoEstimate{PeriodicExcitation}`.

- `FinderConfig::excitation_period` takes the generator's period when the stimulus is its own
  periodic pink. The daemon currently leaves it `None`, so P is always detected.
- P is detected from the ref block. Take the whitened autocorrelation in a 40 Hz –
  16 kHz band, `A_k = B_k|R_k|²/(S_k + ε·mean S)` with S being |R|² smoothed over ±(Nfft/4096)
  bins, and the IFFT envelope normalised to 1 at lag 0 and divided by its overlap fraction
  (Lr − τ)/Lr. A peak ≥ −10 dB at τ ∈ [256, Lr − max(2048, Lr/8)] is a period. A repeat
  longer than the ref block cannot be seen, which is another reason to pass the generator's
  period.
- In the fixture `periodic_wrap`, a reflection 7800 samples late with P = 8192 appears at
  +88 samples, 392 samples before the direct sound, at −6 dB. Every Monte-Carlo trial of this
  class was refused (§11).

### 10.2 Tracking

State: `held: Option<i64>`, the last inserted (fractional) delay, and
`pending: Option<(fractional delay, meas_window_end)>`.

1. A `NoEstimate` or `Ambiguous` result clears `pending`. Nothing moves.
2. If an `Accepted` result's meas window starts before `pending.window_end`, it is ignored.
   Overlapping windows are the same audio read twice.
3. If an `Accepted` result agrees with `pending` within the band's agreement tolerance (§9),
   `held` is set to the first arrival's integer delay and `pending` clears. The delay
   inserted is the pair's fractional mean when the two agree within 0.1 sample
   (`FINE_AGREEMENT_SAMPLES`; it moves again only when the next mean is > 0.1 sample away),
   otherwise the integer delay when it changed. A fraction that wanders more than that
   between windows would only turn the phase by noise.
4. Any other `Accepted` result becomes the new `pending`.
5. A change of band, threshold, search range or measurement routing clears `pending`.
   An operator insert or typed delay sets `held` (`Tracker::set_held`).

**Local search while holding.** A tracking window runs every observation (0.25 s full range),
so a full ±1 s search per window is most of the analyzer's CPU on small machines. While the
last accepted result's first arrival is `held`, a window is searched only within one
acquisition segment N₁ of its first and strongest arrivals, on the full search's tile grid
(so those tiles are the full search's), with the full search's false-peak level and the
period found by the last full search. The local result stands only when it is `Accepted`
with first arrival `held` and both arrivals at least N₁/2 from the local edges; anything
else is replaced by the full search of the same window. Every refusal, ambiguity and
delay move therefore comes from a full search. A full search also runs at least every 2 s
and whenever the local span would cover more than half the search range (always for sub).
The cost: an arrival that appears far from the followed ones while the held delay stays
valid is seen at the next forced full search, up to 2 s late.

Tracking is off by default. The operator enables it per measurement. It agrees only between
results of the *same* rule, so a deterministic wrong arrival is tracked just as confidently as a
correct one. This is why wrong-arrival acceptance is scored per window in §12, and why
agreement is never used as evidence of correctness.

---

## 11. Evidence

All results come from `tools/experiments/q1` with fixed seeds; the full tables are in
`tools/experiments/q1/results/*.txt` (`python3 mc.py --n 200` for `baseline.txt`;
`sh sweep.sh DIR` then `python3 report.py DIR` for the sweeps; `tracking.py --n 60`).
Scoring: **ok** = accepted and |error| ≤ band tolerance from an acceptable first arrival (the
true first significant arrival, or a path within ±2 dB of the threshold); **wrong** =
accepted otherwise; ambiguity and refusal are not errors.

Scenario classes (`mc.py`): `single`; `two_path` (direct + copy, separation log-uniform
0.2–20 ms, direct −20…+10 dB re the copy, copy polarity ±1); `room` (direct −8…+3 dB plus 3–6
reflections 0.5–25 ms at −15…0 dB); `exc_*` (two-path under the named excitation); `snr_*`
(fixed band SNR); `periodic_wrap`; `large_delay` (±0.9 s in a ±1 s search); `no_signal`.
Band SNR is 20–40 dB unless the class fixes it.

### 11.1 Results at the defaults

4.9 k trials (`results/baseline.txt`).

- **Full range** meets every provisional target in every class: 0 wrong in ~4 000 trials,
  largest accepted error 0.30 samples (0 dB SNR), nothing refused at ≥ 20 dB except 2.5 % of
  dull programme (`exc_music_hf`, excited fraction just under 0.5). Ambiguity (two_path 14 %)
  is the borderline rule and close arrivals. `two_path_far` (direct 25–45 ms before a louder
  reflection, beyond the first refinement window) is 99 % correct.
- **Mid** meets the targets with the 0.05 ms tolerance. Separations below 2 pulse widths
  (< 0.8 ms) are almost always `Ambiguous`, never wrong (two_path: 64 % ok, 36 % ambiguous).
- **Sub, isolated arrivals** (`single`, `exc_pink`, `exc_subonly`, `snr_+20…+40`) meet 0.1 ms
  with 0 % refusal at ≥ 20 dB and 4 s observation (e95 ≤ 1.96 samples). At 10 dB the
  precision gate refuses 95 %.
- **Sub, 0.2–20 ms sweep** (`sub/two_path`): 39 % wrong — every separation is below one sub
  pulse width (11 ms), see §11.5. **Sub, resolved** (2.5–8 pulse widths): 1.7 % wrong (an
  inverted reflection 2.6 widths away left a 0.6 ms deblending residual), 10 % refused.
- **Programme in the sub band**: 33 % refused at 4 s, 17 % at 8 s, no wrong acceptance at 8 s.
- `periodic_wrap`, `exc_narrow`, `no_signal`, `snr_−10`: 100 % refused. `large_delay`: 100 %
  correct.

### 11.2 Timing uncertainty

u = 0.35·w_p·σ_n/|A| against the actual error of accepted single-arrival trials: |err|/u p50
/ p90 / p99 is 0.57 / 1.71 / 2.52 (mid) and 0.50 / 1.50 / 2.43 (sub), so u is a slightly
conservative 1 σ. k = 2.5 comes from the sub classes: k = 2 let 3 % of 10 dB sub trials through
with |err| > 0.1 ms; 2.5 cost nothing at ≥ 20 dB on pink. In the full band the error is a
fractional-interpolation bias of ≤ 0.3 samples that does not scale with noise (median raw u
0.004 samples), hence the 0.1-sample floor; it cannot matter for a 1-sample tolerance.

### 11.3 GCC-PHAT vs regularised H1

GCC-PHAT flattens |H| to 1, so the relative level of two arrivals does not survive: the
two-path sum `1 + g·e^{−jωΔ}` becomes a phase-only function whose IR has spurious peaks and
distorted levels. In every multipath class PHAT accepted the wrong arrival 10–20 % of the time
(full/two_path 20 %, full/exc_bandlimited 13 %, mid/two_path 13 %) against 0 % for
regularised H1, and it is no better on coloured or band-limited excitation, the case PLAN
proposed it for. Regularised H1 with ε = 0.01 already handles those: excited bandwidth is
checked explicitly and weak bins shrink rather than blow up. Hence Q1-d.

### 11.4 Two passes

Envelope floor (median, re peak), single arrival at 40 dB SNR:

| band | tile holding the arrival | other tiles |
|---|---|---|
| full | −62 dB | −37 dB |
| mid | −53 dB | −33 dB |
| sub | −32 dB (pass 1) / −46 dB (pass 2) | −27 dB |

A tile whose shift does not match the arrival estimates H from unaligned segments; its floor
is set by the time–bandwidth product, not noise. In the sub band acquisition alone leaves a
PSR of about 16 dB, too close to the 15 dB gate: without the refinement tile (`norefine`) sub
single arrivals are 100 % refused, with it 100 % correct. In full and mid, acquisition alone is
already 0 % wrong; refinement buys PSR margin (≈ 26 → 45 dB full band) and cleaner lobes for
deblending.

### 11.5 Resolution: when arrivals merge

- Two arrivals closer than about w_p are one lobe; below about 0.25 w_p the lobe is
  indistinguishable from a single arrival (misfit < 0.01, as for a true single arrival with
  noise). In the sub band 0.25 w_p is 2.8 ms, so a floor bounce at 1–3 ms is *physically* one
  arrival at 20–120 Hz and the finder reports the merged lobe's delay: accepted errors re the
  first arrival reached **1.9 ms** for separations ≤ 0.25 w_p. Between 0.25 and 2 w_p the
  merged-lobe and close-arrival rules make it `Ambiguous` (94 %), 1 wrong in 77 trials.
- Full band: at ≥ 2 w_p every level class is 100 % correct except direct −14…−10 dB (the
  borderline band), which is 100 % ambiguous. Mid: < 2 w_p is mostly ambiguous, ≥ 2 w_p
  correct except the borderline band.
- Deblending matters once arrivals are resolved but their skirts overlap. Sub fixture (direct
  −4 dB, reflection 35 ms = 3.1 w_p later): the plain envelope fit was biased by +8.3 samples
  (−13 at 50 ms spacing, +4 at 70 ms); after deblending, 0.25 samples.
- The full-band 0.2–20 ms sweep never goes below 2.5 w_p (0.2 ms = 9.6 samples), so every case
  is resolved.

### 11.6 Parameter sweeps

- **ε.** 0.01 is the knee. Larger ε shrinks the weakly excited part of a coloured spectrum
  below W = 0.5, so programme fails the excitation check (0.1: 58 % refused on dull programme;
  0.3: up to 100 %). 0.001 admits noisy bins: sub programme refusals rise from 33 % to 50 %. No
  ε produced a wrong acceptance in the full band.
- **m_b** only trades ambiguity for acceptance near the threshold (1 dB: full two_path 9 %
  ambiguous; 3 dB: 22 %), never wrong. 2 dB matches the ±2 dB scoring and leaves margin over
  the measured level error (≤ 0.2 dB) for real responses.
- **Close spacing.** k = 1 lowers mid ambiguity from 36 % to 28 % with no wrong acceptance
  (deblending removes neighbours beyond one pulse width). k = 2 is kept: the sub band was not
  swept and its unresolved cases are where wrong answers live (§11.5).
- **PSR.** 12 vs 15 dB changes nothing measurable; 18 dB refuses 54 % at 0 dB full-band SNR.
  15 dB = |T| + 3 dB is kept for its meaning (§7).
- **Observation length.** Full: 0.2 s refuses 42 % at 0 dB, 0.25 s nothing at ≥ 0 dB. Sub: 2 s
  refuses 30 % at 20 dB (one 0.1 ms miss), 4 s 0 %, and 6 s still refuses 57 % at 10 dB —
  10 dB in the sub band is outside what 0.1 ms can promise at any practical length. Sub
  programme needs about 8 s for ≤ 20 % refusal.

### 11.7 Tracking

Each trial cuts back-to-back windows of the default observation (6 × 0.25 s full, 4 × 4 s
sub) and steps the delay by 5 samples to 30 % of the search range halfway through.

- Full range (single; direct −9…−3 dB with an inverted reflection 1–15 ms later): every move
  correct, locking in the minimum 2 windows (0.5 s) at start and after the step. Borderline
  scenes (direct −13…−11 dB) are 100 % ambiguous, so tracking never moves (1c).
- Sub: single arrivals lock in 2 windows (8 s). Reflections 1–15 ms behind the direct sound are
  below one pulse width: ambiguous, tracking holds. In `borderline` (direct −13…−11 dB, 1–15 ms
  ahead) 4 moves in 15 trials landed on the merged-lobe delay, > 0.1 ms from the direct
  sound — agreement between windows is repeatability, not correctness (§11.5).

---

## 12. Acceptance numbers (replace PLAN §5.2 provisional values)

These numbers are scored per window, over scenario classes like those in §11. Wrong =
accepted and |first.delay − true first significant arrival| > tolerance, where a path within
±2 dB of the threshold also counts as a true answer. Refusal and ambiguity are not errors.
"Resolved" means that every other path above −20 dB re the strongest is at least 2 nominal
pulse widths away (full 0.16 ms, mid 0.8 ms, sub 22 ms).

| class | tolerance when accepted | wrong (of trials) | refusal at ≥ 20 dB band SNR | measured (baseline) |
|---|---|---|---|---|
| full range, any separation ≥ 0.2 ms, white/pink/band-limited/programme | **1 sample** | **≤ 1 %** | **≤ 10 %** | max err 0.30 samples; 0 wrong / ~4 000; refusal ≤ 2.5 % |
| mid, resolved | **0.05 ms** (was: 1 sample) | ≤ 1 % | ≤ 10 % | e95 0.25 samples; 0 wrong / 1 200; 0 % refused |
| mid, unresolved (< 2 w_p) | — (expected `Ambiguous`) | ≤ 1 % | — | 0 wrong; ambiguous except buried-direct scenes, which are accepted correctly |
| sub, isolated, pink/white/sub-only feed, 4 s | **0.1 ms** | ≤ 1 % | ≤ 10 % | e95 ≤ 1.96 samples; 0 wrong / 360; 0 % refused |
| sub, resolved two-path (≥ 2.5 w_p) | 0.1 ms | **≤ 2 %** | **≤ 20 %** | 1.7 % wrong; 10 % refused; 13 % ambiguous |
| sub, unresolved (< 0.25 w_p ≈ 3 ms) | **≤ 2 ms** re first arrival | ≤ 1 % beyond 2 ms | — | max accepted error 1.87 ms |
| sub, unresolved (0.25–2 w_p) | — (expected `Ambiguous`) | ≤ 2 % | — | 94 % ambiguous, 1/77 wrong |
| sub, programme material | 0.1 ms | ≤ 2 % | **≤ 20 % at 8 s** | 33 % refused at 4 s, 17 % at 8 s, 0 wrong at 8 s |
| periodic excitation, P ≤ W + T | — | 0 % | must refuse | 100 % refused |
| excitation not reaching the band | — | 0 % | must refuse | 100 % refused |
| band SNR ≤ −10 dB | — | 0 % | must refuse | 100 % refused |

Where the provisional targets are unreachable, and why:

- **Sub band, arrivals closer than about 3 ms.** At 20–120 Hz the arrivals are one lobe, and no
  estimator in this band can separate them (§11.5). The provisional "≤ 0.1 ms, ≤ 1 % wrong"
  failed on 39 % of the 0.2–20 ms sweep (`sub/two_path`). The realistic statement is: the
  finder reports the merged lobe, its error re the first arrival is ≤ 2 ms, and the IR panel
  shows one wide lobe. Operators align subs by phase at crossover anyway, where this is the
  relevant delay.
- **Sub band below 20 dB SNR, or programme material.** The timing CRLB of a 100 Hz-wide band
  needs the time–bandwidth product. At 10 dB, 0.1 ms needs well over 6 s, so the finder
  refuses (95 % at 4 s) instead of guessing.
- **Mid band, 1 sample.** A 2.7 kHz-wide band has a 20-sample pulse, and 1 sample is 5 % of it.
  Resolved arrivals reach 0.25 samples at e95, but the skirt of a louder neighbour 3–4 widths
  away cost 1.0–1.3 samples before deblending. 0.05 ms keeps the same fraction of the pulse as
  the sub band's 0.1 ms.

---

## 13. Fixture acceptance table (Rust tests)

The golden sets are `fixtures/golden/delay_finder_{full_pink, full_music, full_narrow,
full_periodic, mid_pink, sub_pink}`, written by `tools/refgen/sets/delay_finder.py`. Each set
contains one `ref` array (shared by all its cases) and one `meas.<case>` array per case.
`parameters.cases[]` holds the paths, expectation, acceptable delays, span and reasons. The
scalars `<case>.{meas_start, search_min, search_max, tol_samples, first_delay,
strongest_delay}` are given, and `parameters.ref_start` is the absolute index of `ref[0]`.
The test calls the finder with the band's defaults (§9). Only the search range comes from the
case, and no excitation period is passed: periodicity must be detected.

Expectations (`parameters.expect_semantics`):

- `accepted`: `Accepted` and |first.delay − acceptable| ≤ tol.
- `accepted_or_ambiguous`: `Ambiguous`; or `Accepted` with first.delay within tol of an
  acceptable delay, or inside `span` when one is given.
- `no_estimate`: `NoEstimate` with at least one of `reasons_any`.

| set / case | class | paths (delay samples @ 48 kHz, gain dB, polarity) | SNR | search | expect | tolerance |
|---|---|---|---|---|---|---|
| full_pink / single_frac | single | 137.37 | 30 | ±2400 | accepted, 137.37 | 1 |
| full_pink / negative_frac | single | −611.6 | 30 | ±2400 | accepted, −611.6 | 1 |
| full_pink / refl_louder_inverted | reflection louder | 250.25 −6; 370.25 0 inv | 30 | ±2400 | accepted, 250.25 | 1 |
| full_pink / refl_louder_far | reflection louder | −80.5 −10; 879.5 0 | 30 | ±2400 | accepted, −80.5 | 1 |
| full_pink / direct_buried | direct below threshold | 400 −18; 544 0 | 30 | ±2400 | accepted, **544** (reflection) | 1 |
| full_pink / borderline | borderline | 300 −12.3; 500 0 | 30 | ±2400 | accepted (300 or 500) or ambiguous | 1 |
| full_pink / close_interfering | unresolved | −200.4 −2; −195.4 0 inv | 30 | ±2400 | ambiguous, or accepted in [−201.4, −194.4] | 1 |
| full_pink / room | room | 1020.6 −3; 1164.6 −1 inv; 1404.6 −4; 1692.6 −6; 1980.6 −9 inv; 2700 −12 | 25 | −2400…4800 | accepted, 1020.6 | 1 |
| full_pink / large_negative | single | −35040.3 0; −34740.3 −6 | 30 | **±48000 (±1 s)** | accepted, −35040.3 | 1 |
| full_pink / low_snr | noise | 90 | −15 | ±2400 | no estimate: LowPsr or LowBandSnr | — |
| full_music / music_refl_louder | programme | 55.5 −6; 355.5 0 | 30 | ±2400 | accepted, 55.5 | 1 |
| full_narrow / narrow_excitation | excitation | 100 (excitation ≤ 1.2 kHz) | 30 | ±2400 | no estimate: InsufficientExcitation | — |
| full_periodic / periodic_wrap | periodic | 480 0; 8280 −4 (P = 8192 → image at 88) | 30 | ±2400 | no estimate: PeriodicExcitation | — |
| mid_pink / mid_single_frac | single | −1234.6 | 30 | ±4800 | accepted, −1234.6 | 2.4 |
| mid_pink / mid_refl_louder_inverted | reflection louder | 700.3 −6; 940.3 0 inv | 30 | ±4800 | accepted, 700.3 | 2.4 |
| sub_pink / sub_refl_louder | reflection louder, resolved | −600.3 −4; 1079.7 0 (35 ms) | 40 | ±9600 | accepted, −600.3 | 4.8 |

Observation lengths: full 12 000 samples (0.25 s), mid 24 000 (0.5 s), sub 96 000 (2 s; the
case is at 40 dB, where 2 s is enough, and keeps the fixture small). Total size is 4.9 MB.

`python3 tools/experiments/q1/check_fixtures.py` runs the prototype against these files; the
Rust side is `ac2-core` `tests/it/delay/fixtures.rs` (`fixture_acceptance_table`,
`matches_python_prototype`). `scenes.rs` adds index invariance and sign (shifting both block
starts leaves the result unchanged; swapping ref and meas negates a single-path delay) and the
`NoReference` / `NoSignal` / `ObservationTooShort` / `InsufficientOverlap` paths.

---

## 14. API shape (`ac2-core::delay`)

The types live in `crates/ac2-core/src/delay/mod.rs`: `Block` (absolute start index),
`Band` (`FullRange`, `Mid`, `Sub`, `Custom`), `Estimator` (`RegularisedH1`, `Phat`),
`FinderConfig` (`FinderConfig::new(fs, band)` = §9 defaults, `Tuning::default()` = the shared
constants), `Arrival`, `Confidence`, `NoEstimateReason`, `AmbiguityReason`, `Outcome`
(`Ambiguous` carries ≤ 3 ranked, rule pick first) and `FinderResult` (all candidates by
delay, resolved band, meas window). `find` / `find_auto` (§9.1) return
`Result<FinderResult, ConfigError>`; `find_with` / `find_auto_with` reuse a `FinderScratch`
(FFT plans and buffers). `Tracker` (`track.rs`) implements §10.2; `DelayStream` (`stream.rs`)
cuts back-to-back windows from sample-indexed blocks and runs the local search while tracking.

- The finder runs on a worker thread, never in the audio callback.
- A block that spans a discontinuity is never passed in: `DelayStream` restarts a stream's
  buffer at a discontinuity and drops the window it breaks, so there is no discontinuity
  reason.
- The residual check on the aligned stream (PLAN §5.2) belongs to the MTW pair, not here.

---

## 15. Limits

- All scenarios are synthetic pure delays with frequency-independent path gains. Real
  loudspeakers add band-dependent group delay, so sub-band and full-range delays of one box
  legitimately differ; the finder reports the band and comparison is the operator's call
  (§2). Recorded captures (off-axis, near boundaries, other boxes) are still owed, scored
  against tape-measure geometry, never against `ac`. Nothing here was tuned on real
  loudspeaker responses.
- "Music" is a synthetic programme model (tonal notes, kick, hats, pink bed): it shows the
  effect of colour and tonality, not the full range of real programme.
- Noise is stationary pink. Impulsive noise (claps, speech) lifts the floor estimate and
  should show up as `LowPsr` refusals, not wrong arrivals; not measured.

---

## 16. Decisions D1–D4

Accepted provisionally (`open-questions.md` Q1).

1. **D1 — revised acceptance numbers (§12).** Mid tolerance 0.05 ms instead of 1 sample; sub
   ≤ 0.1 ms only for arrivals ≥ 2 pulse widths (≈ 22 ms) from others; ≤ 2 ms for unresolved
   sub clusters; sub resolved two-path ≤ 2 % wrong, ≤ 20 % refused; sub programme ≤ 20 %
   refused at 8 s. 0.1 ms for every sub scene is not achievable by any estimator in a 100 Hz
   band (§11.5).
2. **D2 — sub observation 4 s default**, operator-selectable 2/4/8 s (`FinderConfig::
   observation_s`), the refusal reason always shown. 2 s updates faster but refuses 30 % at
   20 dB.
3. **D3 — sub tracking agreement ±0.1 ms** (≈ 5 samples) instead of PLAN's ±1 sample: with
   1 σ ≈ 0.5–2 samples at 20 dB, ±1 sample would rarely agree and sub tracking would stall.
   Full and mid stay ±1 sample.
4. **D4 — GCC-PHAT is a diagnostic option, not a fallback** (Q1-d), contrary to PLAN §5.2's
   original wording.

## Known issue

The §7 excited-bandwidth check measures each bin relative to the band's own mean, so a
perfectly flat in-band floor (e.g. a brick-wall low-passed synthetic reference with only f32
quantisation noise in band) passes as fully excited and the finder returns `Ambiguous` instead
of `InsufficientExcitation`. Real reference noise behaves correctly. Fix candidate: also
compare the in-band level against an absolute floor from the capture's noise estimate.
