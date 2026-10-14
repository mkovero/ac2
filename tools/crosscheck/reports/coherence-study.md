# Coherence study: ac2 and OSM γ² against finite-average expectations

Run: `python -m crosscheck coherence-study --seeds 8` (ac2 70655493 release, osm-harness OSM v1.5.2; run 2026-10-14),
96 kHz white-noise fixtures, M = R + uncorrelated noise, 20 s each, 8 independent seeds per row
(seeds paired across settings). Means over 1–20 kHz (ac2: its finite 48-ppo columns; OSM: its
FFT bins, squared γ). Residual = observed mean − model; ± sem = seed sd/√8.

Models:
- ac2 forms γ² per bin and averages it over the column weighted by the estimated Gxx. Each
  bin reads Carter's E[γ̂²] at the stage's per-bin model effective averages (port of ac2-core
  `OverlapModel` + ladder depth matching, `coherence_study.py`); equal confidence gives every
  stage about the same count: 7.6–8.1 at --blocks 8, 30.4 at --blocks 32. The Gxx weight
  adds its own bias: a bin whose reference read strong also reads more coherent. Over many
  independent bins the column tends to γ² + (1 − γ²)/n (E[|Gxy|²/Gyy] / E[Gxx], exact), and a
  Monte Carlo of the estimator puts k bins 1 − 1/k of the way from Carter's value to that
  limit (`ac2_column_g2`; k = the model's N_eff over the column's bins ÷ its N_eff per bin).
  Column plan and crossover blend as in ac2.
- OSM: Welch equivalent count of 21 Hann ticks, hop round(0.08·fs) (5.4 at FFT16, 19.4 at FFT14).

| SNR dB | setting | true γ² | ac2 N_eff per bin | ac2 E[γ̂²] | ac2 mean ± sd | ac2 resid ± sem | ac2 resid 1–2.1k / 2.1–20k | OSM N_eff | OSM E[γ̂²] | OSM mean ± sd | OSM resid ± sem |
|---|---|---|---|---|---|---|---|---|---|---|---|
| +20 | A (ac2 --blocks 8, OSM FFT16) | 0.9901 | 7.6–8.1 | 0.9908 | 0.9909 ± 0.0002 | +0.0001 ± 0.0001 | +0.0001 / +0.0000 | 5.4 | 0.9901 | 0.9901 ± 0.0001 | +0.0000 ± 0.0000 |
| +10 | A (ac2 --blocks 8, OSM FFT16) | 0.9091 | 7.6–8.1 | 0.9161 | 0.9172 ± 0.0026 | +0.0011 ± 0.0009 | +0.0017 / +0.0009 | 5.4 | 0.9113 | 0.9112 ± 0.0007 | -0.0002 ± 0.0002 |
| +3 | A (ac2 --blocks 8, OSM FFT16) | 0.6661 | 7.6–8.1 | 0.6974 | 0.6944 ± 0.0087 | -0.0030 ± 0.0031 | -0.0021 / -0.0033 | 5.4 | 0.6926 | 0.6899 ± 0.0021 | -0.0026 ± 0.0008 |
| +0 | A (ac2 --blocks 8, OSM FFT16) | 0.5000 | 7.6–8.1 | 0.5519 | 0.5478 ± 0.0086 | -0.0041 ± 0.0030 | +0.0042 / -0.0069 | 5.4 | 0.5550 | 0.5518 ± 0.0024 | -0.0032 ± 0.0008 |
| -3 | A (ac2 --blocks 8, OSM FFT16) | 0.3339 | 7.6–8.1 | 0.4092 | 0.4089 ± 0.0055 | -0.0003 ± 0.0019 | -0.0070 / +0.0020 | 5.4 | 0.4253 | 0.4193 ± 0.0032 | -0.0060 ± 0.0011 |
| +20 | B (ac2 --blocks 32, OSM FFT14) | 0.9901 | 30.4–30.4 | 0.9903 | 0.9904 ± 0.0002 | +0.0001 ± 0.0001 | +0.0001 / +0.0001 | 19.4 | 0.9901 | 0.9901 ± 0.0001 | +0.0000 ± 0.0000 |
| +10 | B (ac2 --blocks 32, OSM FFT14) | 0.9091 | 30.4–30.4 | 0.9108 | 0.9109 ± 0.0008 | +0.0000 ± 0.0003 | +0.0004 / -0.0001 | 19.4 | 0.9096 | 0.9096 ± 0.0003 | +0.0000 ± 0.0001 |
| +3 | B (ac2 --blocks 32, OSM FFT14) | 0.6661 | 30.4–30.4 | 0.6738 | 0.6730 ± 0.0057 | -0.0008 ± 0.0020 | +0.0012 / -0.0015 | 19.4 | 0.6723 | 0.6711 ± 0.0021 | -0.0012 ± 0.0007 |
| +0 | B (ac2 --blocks 32, OSM FFT14) | 0.5000 | 30.4–30.4 | 0.5128 | 0.5126 ± 0.0043 | -0.0002 ± 0.0015 | -0.0039 / +0.0011 | 19.4 | 0.5136 | 0.5139 ± 0.0029 | +0.0003 ± 0.0010 |
| -3 | B (ac2 --blocks 32, OSM FFT14) | 0.3339 | 30.4–30.4 | 0.3526 | 0.3535 ± 0.0037 | +0.0009 ± 0.0013 | +0.0042 / -0.0001 | 19.4 | 0.3576 | 0.3562 ± 0.0032 | -0.0014 ± 0.0011 |

Against Carter at the per-bin count alone (no weight term) the same runs read high by
+0.0007 / +0.0067 / +0.0110 / +0.0110 / +0.0126 (A, +20 … −3 dB) and +0.0003 / +0.0015 /
+0.0030 / +0.0041 / +0.0047 (B), 3–6 sem at A: the Gxx weight's bias is real and the
weighted-column model accounts for it.

Findings:
- ac2: observed mean agrees with its per-bin, Gxx-weighted column model in every row within
  ~1.4 sem (largest −0.0041 ± 0.0030 at 0 dB, A).
- Against the true γ², ac2 reads +0.048 at 0 dB SNR with --blocks 8: a column does not pool
  its bins into one larger average, so its bias is the per-bin one (n ≈ 7.6, not 7.6–48) plus
  the weight's. In exchange γ² does not drop where the phase turns across a column or at
  coarse resolution.
- OSM FFT14 (19.4): residuals within ~1.4 sem, model holds.
- OSM FFT16 (5.4): residual consistently negative, −0.0026/−0.0032/−0.0060 at +3/0/−3 dB
  (3–5 sem); OSM behaves like ≈5.8 averages, not 5.4.

Open:
1. OSM FFT16 effective count ≈5.8 vs Welch-model 5.4 (consistent across SNR). Candidates: how the
   harness/OSM accumulates its 21 ticks (first ticks partly empty? running-sum reset), or the
   Welch formula's triangular lag weighting vs OSM's actual sum. Current stage tolerance
   (coh_osm_model) still passes; the model is slightly pessimistic.
2. ac2 sub-bands: all within 1.9 sem (largest 2.1–20 kHz at 0 dB, A: −0.0069, −1.9 sem). Watch with more
   seeds; not established.
3. The model is white-noise stationary (as ac2's own); coloured or non-stationary signals (rig
   takes) are not covered by this study.
