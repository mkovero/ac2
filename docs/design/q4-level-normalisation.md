# Q4 — Level normalisation

Status: implemented (`ac2-core` `window`, `spectrum`, `generator`). Answers Q4 in
`open-questions.md`. The golden vectors `fixtures/golden/spectrum_*` (from `tools/refgen`)
implement these formulas and cross-check them against `scipy.signal.periodogram`.

## Conventions

- Samples are in FS units: full scale = ±1.0.
- Windows are **periodic** (DFT-even), length N. Coherent gain is S₁ = Σw and energy is S₂ = Σw².
- One-sided spectra with N/2 + 1 bins. The weight c_k is 2 for 0 < k < N/2 and 1 at DC and at Nyquist (even N).

## Quantities

| Quantity | Formula | Unit / reference | Reads correctly |
|---|---|---|---|
| Amplitude (RMS per bin) | A_k = √c_k · \|X_k\| / S₁ | FS RMS | a bin-centred sine reads its RMS; DC reads its value |
| Amplitude in dBFS | 20·log10(A_k · √2) | dBFS, 0 dBFS = full-scale sine RMS (4a) | full-scale bin-centred sine = 0 dBFS |
| PSD | P_k = c_k · \|X_k\|² / (fs · S₂) | FS²/Hz, shown as dB re 1 FS²/Hz | white noise of variance σ² reads σ²/(fs/2) |
| Band power | Σ_{k∈band} P_k · Δf, Δf = fs/N | FS², shown in dBFS the same way as amplitude | integrated noise power is independent of N |

dBFS for band power uses 10·log10(power · 2), so a full-scale sine's band reads 0 dBFS. This
matches the amplitude convention.

## Views (decision 4b)

- **Narrowband spectrum:** amplitude in dBFS, with the axis labelled "dBFS per 1.46 Hz bin
  (tone)" — the bin spacing fs/N from the frame's grid (not the window's ENBW: the spacing is
  what the FFT length sets and every grid carries; with Hann, broadband reads 1.76 dB above
  `S · fs/N`). A tone reads its level and noise reads lower as N grows (3 dB per doubling);
  the axis's tooltip says so and points to the RTA for band levels
  (`ac2-scene` `spectrum/units.rs`). A density quantity (PSD per hertz) is offered beside it.
- **RTA:** band power in dBFS (or dB SPL when calibrated), with the axis labelled "dBFS (band)".
  FFT-banded and IEC-filterbank RTAs both use this unit. Which method was used is part of
  the label.
- **Transfer function:** a ratio of spectra, in dB re reference. The normalisation cancels.

## Scalloping and windows (decision 4c)

| Window | Max scalloping loss | Use |
|---|---|---|
| Hann (default) | 1.42 dB | general |
| Blackman-Harris 4-term | 0.83 dB | high dynamic range |
| Flat-top (HFT95) | < 0.01 dB | reading tone levels |
| Rectangular | 3.92 dB | periodic stimuli whose period equals the FFT length |

Scalloping is never applied as a correction. (Showing the active window's scalloping bound in
the axis tooltip was decided but not built.)

## FFT banding (RTA without the filterbank)

- Band edges come from the base-10 IEC centre frequencies (G = 10^(3/10)), at 1/1 … 1/24.
- A bin that straddles an edge contributes in proportion to its overlap with the band, so
  total power is conserved across bands.
- A band narrower than 2 bins is drawn as "insufficient resolution" (NaN with a reason bit),
  never interpolated.

## Generator (decision 4a)

- The level is typed in dBFS RMS. For any signal, 0 dBFS RMS = the RMS of a full-scale sine
  (0.7071 FS).
- Noise is scaled to the requested RMS over its whole period (periodic) or over a long-term
  estimate (free-running). If the requested RMS plus the crest factor would clip, the daemon
  refuses and reports the maximum achievable RMS. It never clips silently.
- The system max level (`gen.ceiling`, capped by daemon config `max_level_dbfs`) is checked
  against the requested RMS.

## Tests

- Golden: tone + noise amplitude and PSD (exists), DC and Nyquist not doubled (exists).
- Off-bin tone at half-bin offset reads exactly the window's scalloping loss.
- White and pink noise band power is independent of N (1024 … 65536) within statistical
  tolerance.
- Band power summed over all FFT bands equals total power (Parseval) within 1e-9 relative.
- Generator: RMS accuracy per signal type; requests that would clip are refused.
