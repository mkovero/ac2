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

| name | content |
|---|---|
| `spectrum_hann_tone_noise` | one 4096-sample frame at 48 kHz: bin-centred 3 kHz sine (peak 0.5) + white noise; periodic Hann; one-sided amplitude spectrum (FS RMS, and dBFS) and PSD (FS²/Hz, and dB) |
| `transfer_h1_biquad_delay` | white noise through an RBJ peaking biquad + 24-sample delay + uncorrelated noise; scipy Welch `Pxx`, `Pyy`, `Pxy`, H1 and coherence; analytic `h_true` |
| `weighting_iec61672` | A and C weighting in dB at the exact base-10 one-third-octave frequencies 10 Hz–20 kHz from IEC 61672-1 Annex E, plus the standard's rounded Table 3 values |
| `calibration_mic_curve` | analog mic model (HP 18 Hz, +3 dB at 9 kHz, LP 30 kHz) at 1/12-octave points; exact correction normalised at 1 kHz at IEC 1/3-octave centres, and its log-f power average per band (`sets/calibration.py`) |
| `delay_integer_pos_neg` | white-noise reference `x` and `y_pos` / `y_neg` delayed by +137 / −61 samples plus noise; expected lags in `scalars` |
| `room_schroeder_octaves` | an impulse response (two exponential decays, direct sound, background noise) through IEC octave bands run backwards in time; Schroeder curves from each band's −20 dB trigger to a fixed truncation point, EDT / T20 / T30 by `numpy.polyfit`, C50 / C80 by window-before-filtering (`sets/room.py`) |

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
