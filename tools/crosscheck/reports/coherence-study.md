# Coherence study: ac2 and OSM γ² against finite-average expectations

Run: `python -m crosscheck coherence-study --seeds 8` (ac2 bdc1bec release, osm-harness OSM v1.5.2),
96 kHz white-noise fixtures, M = R + uncorrelated noise, 20 s each, 8 independent seeds per row
(seeds paired across settings). Means over 1–20 kHz (ac2: its finite 48-ppo columns; OSM: its
FFT bins, squared γ). Residual = observed mean − model; ± sem = seed sd/√8.

Models:
- ac2: E[γ̂²] (Carter) per column at ac2's own model effective averages (port of ac2-core
  `OverlapModel` + ladder depth matching + column plan + crossover blend, `coherence_study.py`),
  then averaged over the same columns. N_eff varies 7.6 (one-bin full-rate columns just above the
  1.62–2.04 kHz crossover) to 48 (16–20 kHz, many bins) at --blocks 8; 30–191 at --blocks 32.
- OSM: Welch equivalent count of 21 Hann ticks, hop round(0.08·fs) (5.4 at FFT16, 19.4 at FFT14).

| SNR dB | setting | true γ² | ac2 N_eff (cols) | ac2 E[γ̂²] | ac2 mean ± sd | ac2 resid ± sem | ac2 resid 1–2.1k / 2.1–20k | OSM N_eff | OSM E[γ̂²] | OSM mean ± sd | OSM resid ± sem |
|---|---|---|---|---|---|---|---|---|---|---|---|
| +20 | A (ac2 --blocks 8, OSM FFT16) | 0.9901 | 7.6–48 | 0.9901 | 0.9901 ± 0.0002 | +0.0000 ± 0.0001 | +0.0001 / +0.0000 | 5.4 | 0.9901 | 0.9901 ± 0.0001 | +0.0000 ± 0.0000 |
| +10 | A (ac2 --blocks 8, OSM FFT16) | 0.9091 | 7.6–48 | 0.9097 | 0.9107 ± 0.0027 | +0.0010 ± 0.0009 | +0.0019 / +0.0008 | 5.4 | 0.9113 | 0.9112 ± 0.0007 | -0.0002 ± 0.0002 |
| +3 | A (ac2 --blocks 8, OSM FFT16) | 0.6661 | 7.6–48 | 0.6736 | 0.6704 ± 0.0090 | -0.0032 ± 0.0032 | -0.0011 / -0.0039 | 5.4 | 0.6926 | 0.6899 ± 0.0021 | -0.0026 ± 0.0008 |
| +0 | A (ac2 --blocks 8, OSM FFT16) | 0.5000 | 7.6–48 | 0.5162 | 0.5124 ± 0.0090 | -0.0038 ± 0.0032 | +0.0056 / -0.0070 | 5.4 | 0.5550 | 0.5518 ± 0.0024 | -0.0032 ± 0.0008 |
| -3 | A (ac2 --blocks 8, OSM FFT16) | 0.3339 | 7.6–48 | 0.3618 | 0.3619 ± 0.0044 | +0.0001 ± 0.0016 | -0.0041 / +0.0015 | 5.4 | 0.4253 | 0.4193 ± 0.0032 | -0.0060 ± 0.0011 |
| +20 | B (ac2 --blocks 32, OSM FFT14) | 0.9901 | 30.4–191 | 0.9901 | 0.9902 ± 0.0002 | +0.0001 ± 0.0001 | +0.0001 / +0.0001 | 19.4 | 0.9901 | 0.9901 ± 0.0001 | +0.0000 ± 0.0000 |
| +10 | B (ac2 --blocks 32, OSM FFT14) | 0.9091 | 30.4–191 | 0.9092 | 0.9093 ± 0.0008 | +0.0000 ± 0.0003 | +0.0004 / -0.0001 | 19.4 | 0.9096 | 0.9096 ± 0.0003 | +0.0000 ± 0.0001 |
| +3 | B (ac2 --blocks 32, OSM FFT14) | 0.6661 | 30.4–191 | 0.6679 | 0.6671 ± 0.0058 | -0.0008 ± 0.0021 | +0.0012 / -0.0015 | 19.4 | 0.6723 | 0.6711 ± 0.0021 | -0.0012 ± 0.0007 |
| +0 | B (ac2 --blocks 32, OSM FFT14) | 0.5000 | 30.4–191 | 0.5039 | 0.5037 ± 0.0047 | -0.0002 ± 0.0017 | -0.0041 / +0.0011 | 19.4 | 0.5136 | 0.5139 ± 0.0029 | +0.0003 ± 0.0010 |
| -3 | B (ac2 --blocks 32, OSM FFT14) | 0.3339 | 30.4–191 | 0.3407 | 0.3415 ± 0.0036 | +0.0008 ± 0.0013 | +0.0038 / -0.0002 | 19.4 | 0.3576 | 0.3562 ± 0.0032 | -0.0014 ± 0.0011 |

Implied single effective count (solve E[γ̂²](γ², n) = observed mean), SNR ≤ 3 dB:

| setting | SNR | OSM model n | OSM implied n | ac2 implied single n (column mix) |
|---|---|---|---|---|
| A | +3 | 5.4 | 5.92 | 27.6 |
| A | 0 | 5.4 | 5.73 | 21.2 |
| A | −3 | 5.4 | 5.78 | 16.5 |
| B | +3 | 19.4 | 23.8 | 124 |
| B | 0 | 19.4 | 18.9 | 69 |
| B | −3 | 19.4 | 20.5 | 59 |

Findings:
- ac2: observed mean agrees with its per-column model in every row within ~1.2 sem (largest
  −0.0038 ± 0.0032 at 0 dB, A). The single-seed +0.013 "over true" at 0 dB in the stage is the
  estimator bias its model predicts (+0.016 at --blocks 8), not an ac2 error. No single N_eff
  describes ac2 over 1–20 kHz: the band mean is a mix of 7.6- to 48-average columns, so the
  implied single n drifts with SNR (16→28) as expected for a mixture.
- OSM FFT14 (19.4): residuals within ~1.4 sem, model holds.
- OSM FFT16 (5.4): residual consistently negative, −0.0026/−0.0032/−0.0060 at +3/0/−3 dB
  (3–5 sem); OSM behaves like ≈5.8 averages, not 5.4.

Open:
1. OSM FFT16 effective count ≈5.8 vs Welch-model 5.4 (consistent across SNR). Candidates: how the
   harness/OSM accumulates its 21 ticks (first ticks partly empty? running-sum reset), or the
   Welch formula's triangular lag weighting vs OSM's actual sum. Current stage tolerance
   (coh_osm_model) still passes; the model is slightly pessimistic.
2. ac2 sub-band 2.1–20 kHz at 0 dB, setting A: −0.0070 ± 0.0037 (1.9 sem); other sub-band rows
   are within ~1.5 sem. Watch with more seeds; not established.
3. The model is white-noise stationary (as ac2's own); coloured or non-stationary signals (rig
   takes) are not covered by this study.
