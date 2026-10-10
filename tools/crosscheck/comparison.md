# ac2 against REW and Open Sound Meter

How far ac2's numbers can be trusted next to the two established tools the suite runs: REW
and Open Sound Meter (OSM). The suite judges every analyser against a truth first, and only
then against the other tools:

- **steady sines** analysed in numpy (one tone at a time, no deconvolution or window),
- **direct** cross-spectra of the very recording an app analysed (so a difference to them is
  the app's analysis, not the take),
- **closed forms** for synthetic WAV pairs (OSM stage).

A tool's number is a second opinion. Where it disagrees with ac2 and the truth sides with ac2,
the row below says so; where the truth cannot decide, it says that too.

Everything here comes from a run report, a committed baseline or the suite's README; each table
names its run (UTC time), ac2 build and level. The tables between the marker comments are
regenerated from `baselines/` (see [Refreshing](#refreshing)); the rest is written by hand.

## The reference tools

### REW 5.40 (API build)

**What it is.** Room EQ Wizard 5.40 beta 135 with its REST API (API 0.9.8), headless on the rig.
The stable release has no API. Measuring through the API needs a Pro licence, so the suite
never lets REW measure live.

**How the suite drives it** (`crosscheck/rew.py`, README "The REW stimulus"):
- REW's sweep file (made in REW's generator) is played by the suite's own JACK client, and the
  recording is imported (`/import/sweep-recordings`): REW deconvolves each input by its own
  stimulus. REW plays nothing.
- The suite reads REW's exports unsmoothed: frequency response (dBFS, and SPL on the speaker
  path), impulse response, group delay, distortion (dBr, 48 per octave), and RT60 on request.
- Ambient stage: REW's SPL meter and RTA, calibrated from ac2's calibration store, against
  numpy on the same mic recording.

**What it can judge**: sweep magnitude and phase per band, magnitude and phase at the sine
tones, absolute level (meas − ref), group delay, harmonics H2–H5, absolute SPL, room
parameters, LAeq.

**What it cannot judge**:
- Live transfer and coherence. REW live needs Pro, and an imported sweep carries no coherence;
  ac2's live TF is judged against the direct estimate instead.
- Arrival time from the offline import. The import refers each channel to its own timing
  marker, which removes the path delay (−3.42 µs reported on the Xone path at −30 dBFS,
  `pupu/xone-30dbfs.json`). The suite puts the delay back from the direct estimate.
- Not exercised on the rig yet (README, "Known gaps"): REW's SPL-meter `levels` and
  `rta/captured-data` formats as the suite parses them, its RTA mode and averaging names, and
  its absolute FR convention for an imported response. Rows resting on these are context.

### Open Sound Meter v1.5.2

**What it is.** An open-source (GPLv3) dual-channel FFT analyser. The suite runs OSM's own
`Measurement` class through the external
[osm-harness](https://github.com/mkovero/osm-harness), which ticks it deterministically every
round(0.08·fs) samples on a WAV pair (OSM itself ticks on an 80 ms wall-clock timer, so it
cannot replay faster than real time). No OSM code or table is in this repository; the stage
reads the JSON the harness writes.

**How the suite drives it** (`crosscheck/osm.py`, README "OSM stage"): offline, no rig,
nothing emitted. Each case is a 96 kHz meas/ref WAV pair. The same pair goes to OSM (FFT 2^16,
Hann, FIFO 16 ticks) and to a private ac2d through `ac2 rec import` and `ac2 session replay
--fast` (capture-only replay backend), where an ac2 transfer measurement, a spectrum and
`ac2 delay find` run on it. Synthetic cases have a closed-form truth; two rig recordings are
ac2 against OSM only.

**What it can judge**: transfer magnitude and phase on a common grid, coherence (through a
model of OSM's estimator), the delay finders, phase-slope delay, the FFT spectrum level of a
tone and of white noise.

**What it cannot judge** (README, "Deliberately not compared"):
- SPL, Leq and RTA band meters. OSM's are not IEC 61672 / 61260 (see the definitions below).
- Display smoothing (OSM's never reaches its exported data) and OSM's LTW mode.
- Sweeps and harmonic distortion: OSM has no sweep analysis.
- Magnitude at low coherence. OSM's |M|/|R| mean is biased there by construction, so the two
  are compared only where both γ² ≥ 0.95; the bias itself is tested against its model.

## Definition differences

How each analyser defines a quantity, and how the suite makes them comparable. ac2's
definitions are in `docs/user-guide.md` and `crates/ac2-core`; OSM's are described from its
source, not copied.

| quantity | ac2 | REW (as the suite uses it) | OSM v1.5.2 | how the suite converts |
|---|---|---|---|---|
| transfer estimator | live: H1 = G_rm/G_rr over a multi-time-window ladder (full rate, ~12 kHz and ~4 kHz stages, NFFT 4096); sweep: meas ÷ measured loopback reference | imported sweep: each input deconvolved by REW's own stimulus, meas ÷ ref | per tick, the plain ratio \|M\|/\|R\| (not H1), averaged linearly in amplitude; phase is a mean of unit phasors | magnitudes compared only where both γ² ≥ 0.95; in noise, OSM is judged against E\|1 + N/R\| and ac2's H1 against 0 dB |
| coherence | γ² (magnitude-squared), from the accumulated cross- and auto-spectra | none for an imported sweep | γ = \|G_rm\|/√(G_rr·G_mm) over a fixed 21 ticks of heavily overlapping frames, whatever the averaging setting | OSM's γ is squared; with Welch's overlap correction its 21 ticks are about 5.4 independent averages at FFT16 / 96 kHz, so it is judged against the estimator's expected value E[γ̂²] at that count, ac2 against the true γ² |
| frequency grid, smoothing | 1/48-octave columns by default (a measurement's Resolution); smoothing is applied after coherence | exports fetched unsmoothed, native bins | native FFT bins; smoothing is display only and never reaches the exported data | common grid = ac2's columns: REW's and OSM's bins power-averaged into each column (phase: complex mean with the delay taken out and put back at the column centre); a column narrower than a bin takes the nearest bin |
| delay finder | sub-sample: regularised H1 with a Hilbert IR (`ac2 delay find`); the sweep reports a fractional arrival | offline import: removed per channel by the timing markers; live with a loopback timing reference REW keeps it (3.69 µs on the Xone path, hand session) | integer argmax of IFFT(M/R) on one unaveraged 65536-point frame every 25 ticks; never applied automatically | ac2 judged within 0.05 sample of the truth, OSM within ±0.5 (its rounding), ac2 vs OSM within 0.6; phase slopes within 0.02 sample |
| phase reference | meas ÷ ref with ac2's reported arrival taken out | each channel referred to its own timing marker | wrapped, no delay removal; the operator sets an integer delay | one reference for all: meas ÷ ref with the path's delay in it; each analyser's own delay is put back into its phase |
| group delay | as displayed: central difference of neighbouring 1/48-octave columns | REW's own GD export (context only) | UI plot only, not in the data the harness exports | both phases also get a ±1/12-octave slope fit; both judged against steady-sine pairs (relative below 1 kHz, absolute µs above) |
| spectrum level | dBFS, peak convention: a sine of peak a reads 20·log10 a; power mean | not compared | RMS module (a full-scale sine reads −3.01 dB); noise averaged in linear amplitude | +3.01 dB added to OSM; for noise its Rayleigh mean reads √(π/4) (−1.05 dB) under a power mean; white-noise truth 4σ²·ENBW/N per bin (Hann ENBW 1.5 bins) |
| RTA bands | IEC 61260-1 class 1 1/3-octave filter bank, FIFO-averaged over the stage's window | RTA export, against numpy (context: averaging names not verified) | power sum of FFT bins without ENBW correction (+1.76 dB for Hann) on a 11.72 Hz·2^(k/n) grid, not IEC base-10 centres | ac2 and REW against numpy's summed bins between ideal band edges; OSM not compared |
| SPL Fast / Slow / Leq | IEC 61672-1: exponential F (125 ms) and S (1 s); Leq the time average of the weighted signal | SPL meter Leq, calibrated from ac2's store | Fast / Slow are rectangular moving averages over 125 ms / 1 s; Leq samples the Fast level once a second (about an eighth of the signal integrated) | ac2 and REW against a numpy Leq of the mic recording; OSM not compared |
| harmonic distortion | exponential sweep, H2–H5 separated before the linear IR, divided by the measured loopback reference | distortion export of the imported sweep (each input by its own stimulus) | THD+N meter with a 1 kHz notch only | ac2 judged against the steady sine net of the reference input's own harmonics (D_m − T(k·f)·D_r), REW against the meas input alone; a reading below floor + 10 dB is a bound (INCONCLUSIVE) |

## Results

How to read a cell: the worst judged value of the checks the row names (signed, with where it
was: band, tone or sweep), then the verdict counts. INCONCLUSIVE means the data cannot tell a
pass from a fail (a reading near its floor, or noise wider than the pass limit); INFO is
context and not judged. Limits are in `tolerances.toml`. Tone keys use the frequency the sine
plan asked for (a 1000 Hz row may have played at 1001.5 Hz to stay off the mains family).

<!-- BEGIN generated by `python -m crosscheck comparison` from baselines/: do not edit by hand -->

#### Baselines

| baseline | stage | level | run | ac2 build | reference | suite commit | FAIL | WARN | PASS | INCONCLUSIVE | INFO |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `host/osm.json` | osm | silent | osm-20261010T035316Z | befb61d | OSM v1.5.2 | 26518d6a08d7 | 0 | 0 | 86 | 0 | 12 |
| `pupu/ambient.json` | ambient | silent | 20261007T100854Z | badbfc1 | REW 5.40 Beta 135 API 0.9.8 | — | 0 | 0 | 5 | 1 | 1 |
| `pupu/genelec-50dbfs.json` | genelec | −50 dBFS | 20261007T104217Z | 3309fff | REW 5.40 Beta 135 API 0.9.8 | — | 3 | 13 | 75 | 161 | 2 |
| `pupu/xone-10dbfs.json` | xone | −10 dBFS | 20261008T174954Z | 35f6ca4 | REW 5.40 Beta 135 API 0.9.8 | 9a0e91b | 0 | 0 | 261 | 145 | 15 |
| `pupu/xone-20dbfs.json` | xone | −20 dBFS | 20261007T165353Z | 2638893 | REW 5.40 Beta 135 API 0.9.8 | 56fb15b | 1 | 0 | 192 | 139 | 11 |
| `pupu/xone-30dbfs.json` | xone | −30 dBFS | 20261007T114727Z | 2638893 | REW 5.40 Beta 135 API 0.9.8 | — | 0 | 1 | 186 | 143 | 11 |
| `pupu/xone-50dbfs.json` | xone | −50 dBFS | 20261007T113432Z | 2638893 | REW 5.40 Beta 135 API 0.9.8 | — | 2 | 1 | 175 | 156 | 11 |

#### pupu ambient

| check | silent (20261007T100854Z, badbfc1) |
|---|---|
| LZeq ac2 vs numpy | −0.014 dB (PASS 1) |
| LAeq ac2 vs numpy | −0.019 dB (PASS 1) |
| LCeq ac2 vs numpy | −0.010 dB (PASS 1) |
| LAeq REW vs numpy | −0.001 dB (PASS 1) |
| LAeq ac2 vs REW | −0.018 dB (PASS 1) |
| 1/3-oct bands: ac2 RTA vs numpy, worst band | INCONCLUSIVE 1 |
| 1/3-oct bands: REW RTA vs numpy, worst band | +3.910 dB (INFO 1) |

#### pupu genelec

| check | −50 dBFS (20261007T104217Z, 3309fff) |
|---|---|
| ac2 sweep vs REW offline import, magnitude spread per band | +1.182 dB @ 1000–20000 Hz (FAIL 1, WARN 1, INCONCLUSIVE 1) |
| ac2 sweep vs REW offline import, phase spread per band | +1.347° @ 1000–20000 Hz (PASS 2, INCONCLUSIVE 1) |
| REW offline vs direct (same recording), magnitude | +1.535 dB @ 1000–20000 Hz (FAIL 1, WARN 1, INCONCLUSIVE 1) |
| REW offline vs direct (same recording), phase | +1.108° @ 1000–20000 Hz (PASS 2, INCONCLUSIVE 1) |
| ac2 sweep vs direct (its own capture), magnitude | +0.149 dB @ 100–1000 Hz (PASS 2, INCONCLUSIVE 1) |
| ac2 sweep vs direct (its own capture), phase | +0.773° @ 100–1000 Hz (PASS 2, INCONCLUSIVE 1) |
| ac2 sweep vs steady sine, magnitude | −0.207 dB @ 5000 Hz (PASS 2, INCONCLUSIVE 6) |
| REW offline vs steady sine, magnitude | −0.346 dB @ 5000 Hz (WARN 1, PASS 1, INCONCLUSIVE 6) |
| ac2 sweep vs steady sine, phase | −4.117° @ 10000 Hz (WARN 1, PASS 3, INCONCLUSIVE 4) |
| REW offline vs steady sine, phase | −4.040° @ 10000 Hz (WARN 1, PASS 3, INCONCLUSIVE 4) |
| ac2 meas÷ref vs REW (meas − ref), absolute level | −0.225 dB (WARN 1) |
| ac2 reference level vs REW's | −0.019 dB (PASS 1) |
| ac2 reported arrival vs direct, every sweep | +1.74 µs @ dist-probe (PASS 3) |
| REW offline reported delay (removed by its timing markers) | −3644.80 µs (INFO 1) |
| H2–H5 vs steady sine: ac2 sweep | INCONCLUSIVE 26 |
| H2–H5 vs steady sine: REW offline import | INCONCLUSIVE 26 |
| absolute SPL vs steady sine: ac2 sweep | +0.843 dB @ 100 Hz (WARN 3, PASS 5) |
| absolute SPL vs steady sine: REW (cal from ac2) | +0.539 dB @ 50 Hz (WARN 1, PASS 7) |

#### pupu xone

| check | −10 dBFS (20261008T174954Z, 35f6ca4) | −20 dBFS (20261007T165353Z, 2638893) | −30 dBFS (20261007T114727Z, 2638893) | −50 dBFS (20261007T113432Z, 2638893) |
|---|---|---|---|---|
| ac2 sweep vs REW offline import, magnitude spread per band | +0.009 dB @ 1000–20000 Hz (PASS 3) | +0.009 dB @ 1000–20000 Hz (PASS 3) | +0.009 dB @ 1000–20000 Hz (PASS 3) | +0.025 dB @ 20–100 Hz (PASS 3) |
| ac2 sweep vs REW offline import, phase spread per band | +0.104° @ 20–100 Hz (PASS 3) | +0.081° @ 20–100 Hz (PASS 3) | +0.093° @ 20–100 Hz (PASS 3) | +0.434° @ 20–100 Hz (PASS 3) |
| REW offline vs direct (same recording), magnitude | +0.004 dB @ 20–100 Hz (PASS 3) | +0.003 dB @ 20–100 Hz (PASS 3) | +0.003 dB @ 20–100 Hz (PASS 3) | +0.021 dB @ 20–100 Hz (PASS 3) |
| REW offline vs direct (same recording), phase | +0.086° @ 20–100 Hz (PASS 3) | +0.062° @ 20–100 Hz (PASS 3) | +0.053° @ 20–100 Hz (PASS 3) | +0.146° @ 20–100 Hz (PASS 3) |
| ac2 sweep vs direct (its own capture), magnitude | +0.009 dB @ 1000–20000 Hz (PASS 3) | +0.009 dB @ 1000–20000 Hz (PASS 3) | +0.009 dB @ 1000–20000 Hz (PASS 3) | +0.032 dB @ 20–100 Hz (PASS 3) |
| ac2 sweep vs direct (its own capture), phase | +0.015° @ 20–100 Hz (PASS 3) | +0.026° @ 20–100 Hz (PASS 3) | +0.041° @ 20–100 Hz (PASS 3) | +0.348° @ 20–100 Hz (PASS 3) |
| ac2 live TF vs direct (γ² ≥ 0.99), magnitude | +0.019 dB @ 20–100 Hz (PASS 3) | +0.004 dB @ 1000–20000 Hz (PASS 3) | +0.013 dB @ 20–100 Hz (PASS 3) | +0.074 dB @ 1000–20000 Hz (PASS 3) |
| ac2 live TF vs direct (γ² ≥ 0.99), phase | +0.111° @ 20–100 Hz (PASS 3) | +0.055° @ 20–100 Hz (PASS 3) | +0.142° @ 20–100 Hz (PASS 3) | +0.494° @ 1000–20000 Hz (PASS 3) |
| ac2 sweep vs steady sine, magnitude | +0.008 dB @ 15 Hz (PASS 9) | +0.008 dB @ 16 Hz (PASS 7) | +0.013 dB @ 16 Hz (PASS 7) | +0.093 dB @ 16 Hz (WARN 1, PASS 6) |
| REW offline vs steady sine, magnitude | +0.004 dB @ 15 Hz (PASS 9) | −0.002 dB @ 20 Hz (PASS 7) | −0.002 dB @ 20 Hz (PASS 7) | −0.007 dB @ 1000 Hz (PASS 7) |
| ac2 live TF vs steady sine, magnitude | +0.001 dB @ 60.5 Hz (PASS 7) | −0.002 dB @ 10000 Hz (PASS 5) | −0.005 dB @ 31.5 Hz (PASS 5) | +0.011 dB @ 50 Hz (PASS 5) |
| ac2 sweep vs steady sine, phase | +0.018° @ 15 Hz (PASS 9) | +0.028° @ 20 Hz (PASS 7) | +0.048° @ 16 Hz (PASS 7) | +0.608° @ 16 Hz (PASS 7) |
| REW offline vs steady sine, phase | −0.069° @ 15 Hz (PASS 9) | +0.023° @ 20 Hz (PASS 7) | +0.024° @ 20 Hz (PASS 7) | −0.046° @ 31.5 Hz (PASS 7) |
| ac2 live TF vs steady sine, phase | +0.052° @ 10000 Hz (PASS 7) | +0.017° @ 31.5 Hz (PASS 5) | +0.046° @ 31.5 Hz (PASS 5) | −0.100° @ 10000 Hz (PASS 5) |
| ac2 meas÷ref vs REW (meas − ref), absolute level | −0.001 dB (PASS 1) | −0.001 dB (PASS 1) | −0.001 dB (PASS 1) | +0.042 dB (PASS 1) |
| ac2 reference level vs REW's | −0.006 dB (PASS 1) | −0.006 dB (PASS 1) | −0.006 dB (PASS 1) | −0.006 dB (PASS 1) |
| ac2 reported arrival vs direct, every sweep | +0.06 µs @ 10Hz-11s (PASS 5) | −0.15 µs @ 10Hz-11s (PASS 5) | −0.15 µs @ 10Hz-11s (PASS 5) | −0.15 µs @ dist-probe (PASS 5) |
| ac2 arrival + phase slope vs direct, every sweep | 0.00 µs @ 10Hz-11s (PASS 5) | 0.00 µs @ 10Hz-11s (PASS 5) | 0.00 µs @ 10Hz-11s (PASS 5) | 0.00 µs @ 3Hz-5.5s (PASS 5) |
| REW offline reported delay (removed by its timing markers) | −6.39 µs (INFO 1) | −3.42 µs (INFO 1) | −3.42 µs (INFO 1) | −3.42 µs (INFO 1) |
| group delay vs sine pairs: ac2, ±1/12-oct fit | +0.01 µs @ 1040 Hz; +1.8 % @ 93.9 Hz (PASS 8) | −0.03 µs @ 1000 Hz; +6.8 % @ 50 Hz (PASS 6) | −0.09 µs @ 1000 Hz; +7.6 % @ 50 Hz (PASS 5, INCONCLUSIVE 1) | −0.02 µs @ 10000 Hz (PASS 1, INCONCLUSIVE 5) |
| group delay vs sine pairs: ac2 as displayed | 0.00 µs @ 10000 Hz; +1.0 % @ 26.25 Hz (PASS 7, INCONCLUSIVE 1) | 0.00 µs @ 10000 Hz; −0.2 % @ 20 Hz (PASS 3, INCONCLUSIVE 3) | +0.06 µs @ 10000 Hz; +2.8 % @ 31.5 Hz (PASS 3, INCONCLUSIVE 3) | INCONCLUSIVE 6 |
| group delay vs sine pairs: REW, ±1/12-oct fit | 0.00 µs @ 1040 Hz; +2.1 % @ 93.9 Hz (PASS 8) | −0.05 µs @ 1000 Hz; +8.4 % @ 100 Hz (PASS 6) | +0.02 µs @ 1000 Hz; +10.6 % @ 100 Hz (WARN 1, PASS 5) | −5.23 µs @ 1000 Hz; +87.8 % @ 100 Hz (FAIL 2, PASS 1, INCONCLUSIVE 3) |
| group delay vs sine pairs: REW's own GD export | +0.01 µs @ 1040 Hz; +0.9 % @ 26.25 Hz (INFO 8) | +0.10 µs @ 1000 Hz; +1.1 % @ 50 Hz (INFO 6) | +0.14 µs @ 1000 Hz; −8.9 % @ 50 Hz (INFO 6) | −2.84 µs @ 1000 Hz; −34.1 % @ 50 Hz (INFO 6) |
| H2–H5 vs steady sine: ac2 sweep | −1.913 dB @ H2 10000 Hz (PASS 5, INCONCLUSIVE 25) | INCONCLUSIVE 22 | INCONCLUSIVE 22 | INCONCLUSIVE 22 |
| H2–H5 vs steady sine: REW offline import | +0.636 dB @ H3 60.5 Hz (PASS 9, INCONCLUSIVE 21) | +6.688 dB @ H2 10000 Hz (FAIL 1, INCONCLUSIVE 21) | INCONCLUSIVE 22 | INCONCLUSIVE 22 |
| live TF: share of columns at γ² ≥ 0.99 | +1.000 (PASS 1) | +1.000 (PASS 1) | +1.000 (PASS 1) | +1.000 (PASS 1) |

#### OSM stage (osm-20261010T035316Z, ac2 befb61d, v1.5.2)

**Transfer (worst column, or the median on a rig take)**

| case | ac2 \|H\| − truth, dB | OSM \|H\| − truth, dB | ac2 ∠ − truth, ° | OSM ∠ − truth, ° | ac2 − OSM \|H\|, dB | ac2 − OSM ∠, ° | ac2 γ² − true γ² | OSM γ² − E[γ̂²] |
|---|---|---|---|---|---|---|---|---|
| identity | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 |
| biquad | +0.031 | +0.024 | +0.273 | +0.215 | +0.030 | +0.299 | 0.000 | 0.000 |
| delay48 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 |
| delay10_5 | +0.002 | +0.001 | +0.014 | +0.005 | +0.002 | +0.014 | 0.000 | 0.000 |
| polarity | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 |
| snr20 |  |  |  |  |  |  | 0.000 | 0.000 |
| snr10 |  |  |  |  |  |  | +0.002 | +0.001 |
| snr0 |  |  |  |  |  |  | +0.013 | −0.005 |
| genelec-rig |  |  |  |  | +0.119 | +0.723 |  |  |
| xone-rig |  |  |  |  | 0.000 | +0.004 |  |  |

**Delay (samples), noise and spectrum**

| case | ac2 finder − truth | OSM finder − truth | ac2 slope − truth | OSM slope − truth | ac2 − OSM finder | ac2 H1 in noise − 0 dB, dB | OSM mean ratio − E\|1+N/R\|, dB | ac2 spectrum − truth, dB | OSM spectrum (converted) − truth, dB |
|---|---|---|---|---|---|---|---|---|---|
| identity | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 |  |  | +0.019 | +0.016 |
| biquad |  |  |  |  | −0.122 |  |  |  |  |
| delay48 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 |  |  |  |  |
| delay10_5 | 0.000 | −0.500 | 0.000 | 0.000 | +0.500 |  |  |  |  |
| polarity | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 |  |  |  |  |
| snr20 |  |  |  |  | 0.000 | +0.014 | −0.009 |  |  |
| snr10 |  |  |  |  | +0.004 | +0.043 | −0.004 |  |  |
| snr0 |  |  |  |  | −0.001 | −0.048 | +0.009 |  |  |
| sine1k |  |  |  |  |  |  |  | 0.000 | 0.000 |
| genelec-rig |  |  |  |  | −0.551 |  |  |  |  |
| xone-rig |  |  |  |  | +0.502 |  |  |  |  |

**Uncompensated (INFO: the path's delay left in)**

| case | check | value |
|---|---|---|
| delay48 | ac2 \|H\| − truth, max over all columns, dB | +0.417 |
| delay48 | ac2 phase slope − truth, samples | −0.322 |
| delay48 | ac2 ∠ − truth, max, ° | +4.282 |
| delay48 | OSM \|M\|/\|R\| − truth, max, dB | +0.316 |
| delay48 | OSM ∠ − truth, max, ° | +1.562 |
| delay10_5 | ac2 \|H\| − truth, max over all columns, dB | +0.035 |
| delay10_5 | ac2 phase slope − truth, samples | −0.001 |
| delay10_5 | ac2 ∠ − truth, max, ° | +1.034 |
| delay10_5 | OSM \|M\|/\|R\| − truth, max, dB | +0.008 |
| delay10_5 | OSM ∠ − truth, max, ° | +0.099 |
| genelec-rig | ac2 − OSM \|H\|, max over all columns, dB | +23.482 |
| genelec-rig | ac2 − OSM ∠, max, ° | +158.565 |

<!-- END generated -->

### Runs without a baseline yet

Same row selection, copied from the runs' `results.json` (not regenerated: these stages have no
committed baseline at this level).

**Genelec at −30 dBFS** (run 20261009T232923Z, ac2 8b87f49, REW 5.40 b135, operator
present): FAIL 0, WARN 8, PASS 156, INCONCLUSIVE 111, INFO 5.

| check | −30 dBFS (20261009T232923Z, 8b87f49) |
|---|---|
| ac2 sweep vs REW offline import, magnitude spread per band | +0.225 dB @ 20–100 Hz (PASS 3) |
| ac2 sweep vs REW offline import, phase spread per band | +2.578° @ 1000–20000 Hz (PASS 3) |
| REW offline vs direct (same recording), magnitude | +0.365 dB @ 20–100 Hz (WARN 1, PASS 2) |
| REW offline vs direct (same recording), phase | +2.366° @ 20–100 Hz (PASS 3) |
| ac2 sweep vs direct (its own capture), magnitude | +0.250 dB @ 20–100 Hz (PASS 3) |
| ac2 sweep vs direct (its own capture), phase | +0.517° @ 20–100 Hz (PASS 3) |
| ac2 live TF vs direct (γ² ≥ 0.99), magnitude | +0.133 dB @ 100–1000 Hz (WARN 1, INCONCLUSIVE 2) |
| ac2 live TF vs direct (γ² ≥ 0.99), phase | +0.705° @ 100–1000 Hz (PASS 1, INCONCLUSIVE 2) |
| ac2 sweep vs steady sine, magnitude | −0.306 dB @ 1000 Hz (WARN 1, PASS 4, INCONCLUSIVE 3) |
| REW offline vs steady sine, magnitude | −0.289 dB @ 1000 Hz (PASS 5, INCONCLUSIVE 3) |
| ac2 live TF vs steady sine, magnitude | −0.446 dB @ 1000 Hz (WARN 1, PASS 3, INCONCLUSIVE 1) |
| ac2 sweep vs steady sine, phase | −1.145° @ 500 Hz (PASS 5, INCONCLUSIVE 3) |
| REW offline vs steady sine, phase | −1.185° @ 500 Hz (PASS 5, INCONCLUSIVE 3) |
| ac2 live TF vs steady sine, phase | +0.825° @ 100 Hz (PASS 4, INCONCLUSIVE 1) |
| ac2 meas÷ref vs REW (meas − ref), absolute level | +0.005 dB (PASS 1) |
| ac2 reference level vs REW's | −0.004 dB (PASS 1) |
| ac2 reported arrival vs direct, every sweep | +2.06 µs @ dist-probe (WARN 1, PASS 2) |
| REW offline reported delay (removed by its timing markers) | −3660.20 µs (INFO 1) |
| H2–H5 vs steady sine: ac2 sweep | −2.868 dB @ H2 5000 Hz (PASS 6, INCONCLUSIVE 20) |
| H2–H5 vs steady sine: REW offline import | −2.616 dB @ H2 5000 Hz (PASS 4, INCONCLUSIVE 22) |
| absolute SPL vs steady sine: ac2 sweep | −0.309 dB @ 1000 Hz (PASS 5, INCONCLUSIVE 3) |
| absolute SPL vs steady sine: REW (cal from ac2) | +0.047 dB @ 2000 Hz (PASS 8) |
| live TF: share of columns at γ² ≥ 0.99 | +0.355 (INFO 1) |

Harmonics where both apps read above their floors (runs 20261009T160212Z–161810Z, ac2 d6cc773,
`docs/rigs/pupu.md`): H3 at 1 kHz ac2 −52.0 / REW −51.9 dBr, H2 at 5 kHz −57.2 / −57.1,
H2 at 2 kHz −64.7 / −61.5 (steady sine −63.1).

**Ambient** (run 20261009T180151Z, ac2 8b87f49, RTA FIFO-averaged over the Leq window): FAIL 0,
WARN 1, PASS 5, INFO 1.

| check | silent (20261009T180151Z, 8b87f49) |
|---|---|
| LZeq ac2 vs numpy | +0.005 dB (PASS 1) |
| LAeq ac2 vs numpy | +0.004 dB (PASS 1) |
| LCeq ac2 vs numpy | −0.002 dB (PASS 1) |
| LAeq REW vs numpy | +0.012 dB (PASS 1) |
| LAeq ac2 vs REW | −0.009 dB (PASS 1) |
| 1/3-oct bands: ac2 RTA vs numpy, worst band | +1.685 dB (WARN 1) |
| 1/3-oct bands: REW RTA vs numpy, worst band | +3.754 dB (INFO 1) |

The ac2 RTA's median band difference is +0.19 dB, REW's −0.11 dB (same report).

### Before the suite: the hand session of 2026-10-07

REW measured live through a PipeWire bridge in a GUI session (`docs/rigs/pupu.md`, "REW
cross-check, electrical"; Xone path, 1/48 octave, 31 Hz – 20 kHz):

| comparison | magnitude | phase |
|---|---|---|
| ac2 sweep −50 dBFS vs REW offline import (same recording) | ≤ 0.02 dB mean, ±0.08 dB below 100 Hz, ±0.02 above 1 kHz | — |
| ac2 sweep −30 dBFS vs REW live −30 dBFS (constant offset removed) | ±0.017 dB below 100 Hz, ±0.005 dB above 1 kHz | ±0.7° |

The constant offset is a convention, not an error: ac2 reports meas ÷ ref (−17.33 dB at 1 kHz,
the loopback's own gain included), REW's "loopback as cal" keeps the measurement channel re the
digital stimulus (−44.87 dBFS for the −30 dBFS sweep).

Group delay against steady-sine pairs on the same path (`docs/design/subsample-arrival-group-delay.md`,
µs): at 20 Hz sine 1054, ac2 ±1/12-oct fit 1124, REW same fit 1174; at 50 Hz 187 / 187 / 197;
at 10 kHz 3.2 / 3.2 / 3.1.

## Known disagreements

| disagreement | numbers | status |
|---|---|---|
| REW's harmonics vs steady sines (electrical) | hand session, REW live at −30 dBFS: REW 8–15 dB below the steady sines throughout (e.g. H2 at 1 kHz: sine −79, REW −88 dBr). Suite, REW offline import: worst +0.636 dB (H3 at 60.5 Hz, 9 judged) at −10 dBFS; one FAIL at −20 dBFS, H2 at 10 kHz +6.688 dB | **open** for REW live (unexplained in `docs/rigs/pupu.md`); the offline import agrees at −10 dBFS. At −30 and −50 dBFS the main sweep's and REW's harmonic rows are all INCONCLUSIVE (floor-limited) |
| ac2 sweep LF H2 excess (electrical) | first run: H2 at 20 Hz −53 dBr vs sine −75. After 8c3a658 (harmonic windows read the response high-passed below the lowest fundamental): +22.4 → +1.1 dB at 22 Hz; after 501330a (minimum-phase high-pass): −0.2 … +1.6 dB on all five sweeps | **explained**. The deconvolution's edge at the loopback's DC-blocking roll-off rang into H2's window. What remains on sweeps starting at 20 Hz (+3.1 dB, 35f6ca4) is the Xone's own short-lived H2 rise after a fade-in, measured with a held 22 Hz tone; the suite reports a sweep reaching 22 Hz within 0.3 s of full level as INFO |
| REW's ±1/12-oct GD fit at −50 dBFS | +87.8 % at 100 Hz, −5.23 µs at 1 kHz (2 FAIL, `pupu/xone-50dbfs.json`); at −30 dBFS one WARN (+10.6 % at 100 Hz) | **explained**: at −50 dBFS the electrical comparisons below 1 kHz are noise-limited; all FAILs of the first baselines were REW's analysis or scatter, none ac2's |
| REW at −20 dBFS: H2 at 10 kHz | +6.688 dB vs steady sine (FAIL, `pupu/xone-20dbfs.json`) | **open**: REW's reading; ac2's rows on the same run pass or are bounds |
| Speaker at −50 dBFS, 1–20 kHz | REW vs direct (same recording) +1.535 dB, ac2 vs REW +1.182 dB, ac2 vs direct (REW's recording) +1.317 dB (3 FAIL, `pupu/genelec-50dbfs.json`, 3309fff) | **open at −50 dBFS**, not reproduced at −30 on 8b87f49, where these rows pass (table above). At −50 dBFS most of the speaker path below 1 kHz is below the noise limit |
| Genelec −30: magnitude at 1 kHz vs steady sine | ac2 sweep −0.306, direct on REW's recording −0.303, ac2 TF −0.446 dB (WARN) | **explained**: the room's fine structure; a 1/48-oct column and the sine's frequency differ by 0.25 dB in the direct estimate itself |
| Genelec −30: live TF spread 100–1000 Hz | +0.133 dB (WARN) | **explained**: the estimate's variance at γ² ≈ 0.99 with 8 blocks, √((1 − γ²)/(2γ²n)) ≥ 0.2 dB, not a bias |
| Genelec −30: dist-probe arrival | +2.06 µs vs direct (WARN; pass 2 µs, warn one sample = 10.4 µs) | **explained**: a fifth of a sample on an acoustic path |
| REW RTA vs numpy | worst band +3.754 dB, median −0.11 dB (INFO, 20261009T180151Z) | **open**: REW's RTA averaging names are not verified on the rig |
| ac2 RTA vs numpy | worst band +1.685 dB at 79 Hz, median +0.19 dB (WARN) | **explained**: filter skirts against ideal band edges in a falling LF ambient, and the averaging span not matching the numpy window to the sample |
| OSM magnitude bias at low SNR | OSM mean ratio +0.171 / +1.216 / +5.630 dB at 20 / 10 / 0 dB SNR; model E\|1 + N/R\| +0.180 / +1.220 / +5.621; ac2 H1 +0.014 / +0.043 / −0.048 dB | **explained**: a mean of \|M\|/\|R\| is not H1; OSM within 0.009 dB of its model |
| OSM NaN phase bins | snr20 1, xone-rig 1, genelec-rig 34 bins | **explained**: OSM's polar form divides imag by real, so an exactly-zero bin is NaN and its FIFO keeps it; masked |
| OSM float32 floor | bins with the reference > 70 dB below its peak: xone-rig 17, genelec-rig 17 | **explained**: OSM subtracts the windowed block's sum in float32, a rounding floor near −80 dB; masked |
| ac2 γ² at low SNR | ac2 0.9110 / 0.5131 against the true 0.9091 / 0.5000 at 10 / 0 dB SNR (+0.002 / +0.013) | **open**, within the 0.02 pass limit. A finite-average γ² estimate reads high in noise; the suite models that for OSM (E[γ̂²] 0.555 at 0 dB) but not ac2's effective average count, so ac2 is held to the truth |
| OSM γ² vs truth | 0.5498 at 0 dB SNR against a true 0.5 | **explained**: about 5.4 independent averages; E[γ̂²] = 0.555 |
| Delay rounding | delay10_5: ac2 10.500, OSM 10; xone-rig: ac2 0.502, OSM 0 (both phase slopes 0.502 / 0.504); genelec-rig: ac2 348.449, OSM 349 | **explained**: OSM's finder is an integer argmax |
| ac2 TF with a flight time left in | delay48 uncompensated: ac2 up to 0.417 dB and 4.28°, slope 0.322 sample short; OSM's ratio scatters 0.316 dB, phase 1.56° (INFO) | **explained**: the MTW's short HF windows decorrelate at their ends; inserting the finder's delay brings ac2 back to 0.000. An operator sets the delay, as with OSM |

OSM numbers in this table are from the run the `host/osm.json` baseline was taken from
(tables "Masked bins and columns", "Delay (samples)" and "Noise" of its report).

## Refreshing

| table | regenerated by |
|---|---|
| everything between the marker comments | `python -m crosscheck comparison` after a baseline changes (`--check` only reports; `tests/test_comparison.py` fails when it is stale) |
| rig baselines (`baselines/pupu/*.json`) | `./rig-run.sh …` (README, "Running it"), review the run, then `python -m crosscheck baseline runs/<UTC time>` |
| OSM baseline (`baselines/host/osm.json`) | `OSM_HARNESS=… AC2_BIN_DIR=… python -m crosscheck osm`, review, `python -m crosscheck baseline runs/osm-<UTC time>` |
| Runs without a baseline yet; the known-disagreements numbers | by hand from the run's `report/report.md` (and its `results.json`) |
| Hand session of 2026-10-07 | fixed history; `docs/rigs/pupu.md` |
