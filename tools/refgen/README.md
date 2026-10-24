# refgen — golden vectors

`generate.py` computes reference results with numpy/scipy and analytic formulas and writes
them to `fixtures/golden/`. `crates/ac2-testkit` loads them; ac2's DSP is tested against
them. Expected values never come from `ac` (PLAN.md §5.6).

## Setup and use

```
cd tools/refgen
python3 -m venv .venv            # gitignored
.venv/bin/pip install -r requirements.txt
.venv/bin/python generate.py     # rewrite fixtures/golden
.venv/bin/python generate.py --check
```

`--check` regenerates everything into a temporary directory and compares it with the
committed files, also flagging missing or stale files. Array data must agree within
rtol = atol = 1e-12: platform libm differences (e.g. `log10`) move derived values by a few
ulps between machines even with the pinned numpy/scipy (`requirements.txt`). JSON metadata
must match exactly except the blob hash. On a mismatch it prints the largest absolute
difference per array. Inputs are deterministic: fixed `numpy.random.default_rng`
seeds, fixed parameters, no timestamps.

After changing a generator: run `generate.py`, then `--check`, then `cargo test -p
ac2-testkit`, and commit the script and the fixtures together.

## Format (`format = "ac2-golden"`, `format_version = 1`)

Each vector set `<name>` is two files:

- `<name>.json` — metadata (UTF-8, pretty-printed).
- `<name>.bin` — one blob: all arrays concatenated, little-endian IEEE-754 `f64`, no header,
  no padding.

JSON fields:

| field | meaning |
|---|---|
| `format`, `format_version` | `"ac2-golden"`, `1` |
| `name`, `description` | set name (= file stem) and what it contains |
| `generator` | `{script, function}` that produced it |
| `versions` | `{numpy, scipy}` used |
| `parameters` | every input parameter (seeds, rates, lengths, formulas as text) |
| `references` | citations: standards, PLAN.md sections, scipy functions |
| `scalars` | named scalar results (numbers) |
| `blob` | `{file, nbytes, sha256}` of the `.bin` |
| `arrays` | list of array descriptors, in blob order |

Array descriptor:

| field | meaning |
|---|---|
| `name` | unique within the set |
| `dtype` | `"f64"` or `"c128"` (complex, stored as interleaved `re, im` f64 pairs) |
| `shape` | dimensions, row-major; element count = product |
| `offset`, `nbytes` | byte range in the blob (`nbytes = count × 8`, ×16 for `c128`) |
| `unit`, `description` | physical unit and meaning |
| `tolerance` (optional) | suggested comparison for an independent implementation |
| `axis` (optional) | name of an earlier array giving the x-axis (frequency) for messages |

Tolerances:

- `{"kind": "linear", "abs": a, "rel": r}` — pass if `|actual − expected| ≤ a + r·|expected|`
  (complex: modulus of the difference, modulus of expected).
- `{"kind": "db", "db": d, "floor_db": f}` — values are dB; both sides are clamped to at
  least `f`, then `|actual − expected| ≤ d`.

Arrays without a tolerance are inputs (signals) or informational (e.g. an analytic `h_true`
that an estimator is not expected to match exactly).

## Vector sets

Each set's JSON `description` and `parameters` say exactly what it holds; module in brackets
(`generate.py` itself where none).

| name | content |
|---|---|
| `spectrum_hann_tone_noise` | one 4096-sample Hann frame at 48 kHz: bin-centred 3 kHz sine (peak 0.5) + white noise; amplitude spectrum (FS RMS, dBFS) and PSD (FS²/Hz, dB) |
| `spectrum_offbin_windows` | full-scale sine half a bin off-centre through Hann, Blackman-Harris 4, HFT95 and rectangular windows: scalloping loss [`levels.py`] |
| `transfer_h1_biquad_delay` | white noise through an RBJ peaking biquad + 24-sample delay + uncorrelated noise; scipy Welch `Pxx`, `Pyy`, `Pxy`, H1, coherence; analytic `h_true` |
| `mtw_stage_pipeline_48k` | MTW stages: integer alignment, Kaiser decimation, Hann Welch auto/cross spectra per stage [`mtw.py`] |
| `weighting_iec61672` | A and C weighting at the exact base-10 1/3-octave frequencies 10 Hz–20 kHz (IEC 61672-1 Annex E) plus the standard's rounded Table 3 |
| `fft_banding_noise`, `rta_butterworth_bands` | 1/1- and 1/3-octave band power from PSD bins; Butterworth band-pass responses on IEC 61260-1 edges [`levels.py`] |
| `spl_toneburst`, `spl_time_weighting` | F / S / I time weighting: 4 kHz toneburst maxima, step rise and fall (IEC 61672-1 Formula 7) [`levels.py`] |
| `calibration_mic_curve` | analog mic model at 1/12-octave points; correction normalised at 1 kHz and its log-f power average per 1/3-octave band [`calibration.py`] |
| `delay_integer_pos_neg` | white-noise reference and copies delayed +137 / −61 samples plus noise; expected lags in `scalars` |
| `delay_finder_{full,mid,sub}_*` | delay finder scenarios per band and excitation (pink, music, narrow, periodic), direct sound below a louder reflection [`delay_finder.py`] |
| `room_schroeder_octaves` | two-slope IR through IEC octave bands; Schroeder curves, EDT / T20 / T30, C50 / C80 [`room.py`] |
| `spectrum_density` | spectrum density `2·c_k·|X_k|²/(fs·Σw²)` (twice the PSD below, so it keeps the dBFS reference: white noise σ reads `4σ²/fs`), 1024-point Hann at 48 kHz, FIFO 32, on noise + sine and pink noise [`density.py`] |
| `multires_density`, `multires_spectrum` | multi-resolution spectrum at 12 kHz on the 1/24 grid: density divided by `B(f) = Σ w_b·ENBW·fs/N_b`, and tone level with bands blended in power [`density.py`, `multires.py`] |
| `display_smoothing_biquad`, `display_ir_etc` | fractional-octave display smoothing (power and complex); live IR by circular IFFT and ETC from the Hilbert envelope [`display.py`] |
| `ppm_quasi_peak`, `ppm_vu_step`, `ppm_true_peak` | IEC 60268-10 Type I / II quasi-peak (two-charge attack fitted to the burst table by `ppm.py --fit`) over a 5 kHz burst sequence; VU step; BS.1770-5 Annex 2 true peak per block [`ppm.py`] |
| `stereo_correlation` | zero-lag correlation over a 0.3 s exponential window, gated per side, on pink-noise pairs (mono, one side, reversed, independent, 1 ms delayed) [`stereo.py`] |

## Conventions

- **Window**: periodic (DFT-even) Hann, `w[i] = 0.5 − 0.5·cos(2πi/N)`.
- **DFT**: `X[k] = Σ w[n]·x[n]·e^{−2πikn/N}`, one-sided bins `k = 0..N/2`.
- **One-sided fold** `c_k`: 2 for interior bins, 1 for DC and Nyquist (they have no mirror
  image, so they are not doubled).
- **Amplitude spectrum** (FS RMS): `√c_k·|X[k]| / Σw`. A bin-centred sine of peak `a`
  reads `a/√2`; DC of value `c` reads `c`. dBFS = `20·log10(amp·√2)`, so a full-scale sine
  is 0 dBFS (decision 4a).
- **PSD** (FS²/Hz, one-sided): `c_k·|X[k]|² / (fs·Σw²)`; dB re 1 FS²/Hz = `10·log10`.
  Matches `scipy.signal.periodogram(..., scaling="density")`, which the generator asserts.
- **Cross-spectrum**: `Pxy = mean(conj(X)·Y)` (scipy convention); `H1 = Pxy/Pxx`;
  `γ² = |Pxy|²/(Pxx·Pyy)`.
- **Delay sign**: positive lag = measurement `y` late relative to reference `x`.
- **Units**: FS = digital full scale (sample value 1.0).

## Adding vector sets

Put a module in `sets/` (see `sets/README.md`); `generate.py` discovers it. No edits to
`generate.py` needed.
