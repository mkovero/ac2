# ac2 against truth, REW and Open Sound Meter

ac2's measurements are checked in two steps. First, each analysis is compared with a truth that
does not depend on ac2: a steady sine measured with numpy on the same path, numpy cross-spectra
of the same recording, closed-form test signals, and a software device whose distortion is known
exactly. Then the same quantity is compared with REW 5.40 and Open Sound Meter (OSM) v1.5.2 on
the same signals.

This does not validate the physical calibration chain (the microphone's sensitivity and a
calibrator; REW's SPL here is calibrated from ac2, so it checks consistency only), class
conformance of ac2's level meter and RTA to IEC 61672-1 and IEC 61260-1, or any other ac2
build. Every number below is generated from the committed baselines, reports and
`tolerances.toml`, or checked against them by `python -m crosscheck comparison --check`.

<!-- BEGIN generated build: `python -m crosscheck comparison` from baselines/; do not edit by hand -->

Every result below is from **ac2 build 2c07efe**, the build of the newest rig baseline (pupu). Only baselines on that build are shown, at all their levels; results of different builds are never combined. Not run on this build (their baselines are from earlier builds and not shown): pupu: ambient, xone −10 dBFS, xone −20 dBFS, xone −50 dBFS.

<!-- END generated build -->

## Summary

One row per quantity and reference, over every baseline on the build named above. The verdict
counts checks (bands, tones and sweeps, which overlap), not independent trials. "Worst" is the value furthest outside its pass limit,
as a share of that limit. Terms are defined under [Definitions](#definitions).

<!-- BEGIN generated summary: `python -m crosscheck comparison` from baselines/; do not edit by hand -->

| quantity | measured against | on | limit (pass / warn) | verdict | worst (largest share of its limit) | from run |
|---|---|---|---|---|---|---|
| Level: electrical gain | REW (meas − ref) | xone −30 dBFS, genelec −30/−50 dBFS | 0.1 / 0.3 dB | PASS (3 judged) | +0.014 dB @ genelec −30 dBFS | 20261011T191033Z |
| Level: absolute SPL | truth: steady sine | genelec −30/−50 dBFS | 0.5 / 1 dB | **WARN** (WARN 2 of 16 judged, 7 inconclusive) | +0.505 dB @ 50 Hz, genelec −30 dBFS | 20261011T191033Z |
| Magnitude | truth: steady sine | xone −30 dBFS, genelec −30/−50 dBFS | 0.05–0.3 / 0.2–1 dB | **WARN** (WARN 1 of 28 judged, 11 inconclusive) | −0.498 dB @ 1000 Hz, genelec −30 dBFS | 20261011T191033Z |
| Magnitude | truth: direct estimate of the same capture | xone −30 dBFS, genelec −30/−50 dBFS, dut −10/−30 dBFS | 0.03–0.3 / 0.1–1 dB | **WARN** (WARN 8 of 83 judged, 6 inconclusive) | 0.115 dB @ dut-loud, 20–100 Hz, dut −30 dBFS | 20261013T043410Z |
| Magnitude | REW (offline import) | xone −30 dBFS, genelec −30/−50 dBFS | 0.03–0.3 / 0.1–1 dB | PASS (8 judged, 1 inconclusive) | 0.284 dB @ 1000–20000 Hz, genelec −30 dBFS | 20261011T191033Z |
| Magnitude | truth: closed form | osm | 0.05–0.1 / 0.2–0.3 dB | PASS (8 judged) | 0.031 dB @ biquad | osm-2c07efe |
| Phase | truth: steady sine | xone −30 dBFS, genelec −30/−50 dBFS | 1–3 / 3–10° | PASS (29 judged, 10 inconclusive) | +1.522° @ 1000 Hz, genelec −30 dBFS | 20261011T191033Z |
| Phase | truth: direct estimate of the same capture | xone −30 dBFS, genelec −30/−50 dBFS, dut −10/−30 dBFS | 1–3 / 3–10° | PASS (83 judged, 6 inconclusive) | 0.843° @ dut-loud, 20–100 Hz, dut −30 dBFS | 20261013T043410Z |
| Phase | REW (offline import) | xone −30 dBFS, genelec −30/−50 dBFS | 1–3 / 3–10° | PASS (8 judged, 1 inconclusive) | 2.193° @ 1000–20000 Hz, genelec −50 dBFS | 20261011T185816Z |
| Phase | truth: closed form | osm | 0.5 / 2° | PASS (5 judged) | 0.273° @ biquad | osm-2c07efe |
| Magnitude and phase: repeatability | another ac2 sweep of the same path | xone −30 dBFS, genelec −30/−50 dBFS | 0.03–0.3 / 0.1–1 dB; 1–3 / 3–10° | **WARN** (WARN 2 of 44 judged, 4 inconclusive) | 4.060° @ 1000–20000 Hz, genelec −50 dBFS | 20261011T185816Z |
| Group delay (±1/12-oct fit) | truth: steady-sine pairs | xone −30 dBFS | 0.5 / 2 µs; 10 / 25 % | PASS (6 judged, 2 inconclusive) | −8.0 % @ 60.5 Hz | 20261011T052811Z |
| Arrival | truth: IR peak of its own capture | xone −30 dBFS, genelec −30/−50 dBFS, dut −10/−30 dBFS | 0.3–2 / 1–10.4 µs | PASS (27 judged) | −1.04 µs @ dist-probe, genelec −50 dBFS | 20261011T185816Z |
| Delay finder and phase slope | truth: closed form | osm | 0.02–0.05 / 0.1–0.5 samples | PASS (8 judged) | 0.000 samples @ delay10_5 | osm-2c07efe |
| Coherence γ² | truth: closed form, in noise its estimator's expected value | osm | 0.02 / 0.05 | PASS (8 judged) | −0.003 @ snr0 | osm-2c07efe |
| Harmonics H2–H5 | truth: steady sine | xone −30 dBFS, genelec −30/−50 dBFS | 3 / 6 dB | **WARN** (WARN 4 of 19 judged, 287 inconclusive) | +5.760 dB @ H4 500 Hz, genelec −30 dBFS | 20261011T191033Z |
| Harmonics H2–H5 | REW (offline import) | xone −30 dBFS, genelec −30/−50 dBFS | 3 / 6 dB | PASS (7 judged, 299 inconclusive) | −1.284 dB @ H2 5000 Hz, genelec −30 dBFS | 20261011T191033Z |
| Harmonics H2–H5 | truth: exact values of a digital device | dut −10/−30 dBFS | 0.1–0.5 / 0.3–1 dB | PASS (672 judged) | +0.476 dB @ dut-floor-hp, H3 50 Hz, ac2 sweep 10Hz-11s, dut −30 dBFS | 20261013T043410Z |
| Leq Z, A, C | truth: numpy on the same recording | ambient | — | not run on this build | — | — |
| LAeq | REW | ambient | — | not run on this build | — | — |
| 1/3-octave RTA | its own band filters on the same recording | ambient | — | not run on this build | — | — |
| Spectrum (tone, noise) | truth: closed form | osm | 0.02–0.1 / 0.1–0.3 dB | PASS (2 judged) | +0.019 dB @ identity | osm-2c07efe |
| Transfer: ac2 vs OSM | OSM on the same fixtures (max) | osm | 0.05 / 0.2 dB; 0.5 / 2° | PASS (10 judged) | 0.299° @ biquad | osm-2c07efe |
| Transfer: ac2 vs OSM | OSM on the same rig recording (median) | osm | 0.3 / 1 dB; 3 / 10° | PASS (4 judged) | 0.144 dB @ genelec-rig | osm-2c07efe |
| Delay finder: ac2 vs OSM | OSM's whole-sample finder | osm | 0.6 / 1 samples | PASS (10 judged) | −0.551 samples @ genelec-rig | osm-2c07efe |

<!-- END generated summary -->

Every WARN and FAIL in those baselines belongs to one item under [Open items](#open-items):

<!-- BEGIN generated misses: `python -m crosscheck comparison` from baselines/; do not edit by hand -->

| item | status | on | checks | values | limit (pass / warn) | from run |
|---|---|---|---|---|---|---|
| [Genelec H2 at 5 kHz](#genelec-h2-at-5-khz) | open; ac2 and REW alike | genelec −30 dBFS | WARN 4 | +3.521 dB … +4.807 dB | 3 / 6 dB | 20261011T191033Z |
| [Genelec H4 at 500 Hz on the dist sweep](#genelec-h4-at-500-hz-on-the-dist-sweep) | open | genelec −30 dBFS | WARN 1 | +5.760 dB | 3 / 6 dB | 20261011T191033Z |
| [Genelec live TF at 1 kHz](#genelec-live-tf-at-1-khz) | open | genelec −30 dBFS | WARN 2 | −0.501 dB … −0.498 dB | 0.3–0.5 / 1 dB | 20261011T191033Z |
| [Genelec live TF SPL at 51 Hz](#genelec-live-tf-spl-at-51-hz) | open | genelec −30 dBFS | WARN 1 | +0.505 dB | 0.5 / 1 dB | 20261011T191033Z |
| [Genelec live TF against its direct estimate](#genelec-live-tf-against-its-direct-estimate) | open | genelec −30 dBFS | WARN 1 | 0.143 dB | 0.1 / 0.3 dB | 20261011T191033Z |
| [REW's deconvolution against numpy](#rews-deconvolution-against-numpy) | open | genelec −30 dBFS | WARN 1 | 0.317 dB | 0.3 / 1 dB | 20261011T191033Z |
| [The truth chain at the 100 Hz sine](#the-truth-chain-at-the-100-hz-sine) | open | genelec −30 dBFS | WARN 1 | −3.722° | 3 / 10° | 20261011T191033Z |
| [Genelec at −50 dBFS: ac2 sweep repeatability](#genelec-at-50-dbfs-ac2-sweep-repeatability) | open | genelec −50 dBFS | WARN 3 | 0.356 dB … 0.663 dB; 4.060° | 0.3 / 1 dB; 3 / 10° | 20261011T185816Z |
| [Genelec at −50 dBFS: REW's SPL at 51 Hz](#genelec-at-50-dbfs-rews-spl-at-51-hz) | open | genelec −50 dBFS | FAIL 1 | +1.172 dB | 0.5 / 1 dB | 20261011T185816Z |
| [Digital DUT: dut-loud sweep against its capture](#digital-dut-dut-loud-sweep-against-its-capture) | open | dut −10 dBFS | WARN 3 | 0.034 dB … 0.111 dB | 0.03–0.05 / 0.1–0.2 dB | 20261013T044420Z |
| [Digital DUT: dut-loud sweep against its capture](#digital-dut-dut-loud-sweep-against-its-capture) | open | dut −30 dBFS | WARN 3 | 0.035 dB … 0.115 dB | 0.03–0.05 / 0.1–0.2 dB | 20261013T043410Z |

<!-- END generated misses -->

## Results

The tables below hold the evidence behind the summary, per stage. A cell is the worst judged
value of its checks, with the band, tone or sweep where it sits, then the verdict counts.
`[mean …, n … of …]` beside a band value is the band's signed mean difference, and the number
of columns it stands on after every mask out of the band's columns.

<!-- BEGIN generated results: `python -m crosscheck comparison` from baselines/; do not edit by hand -->

### pupu: ac2 against REW, per quantity

| quantity | xone −30 dBFS (20261011T052811Z) | genelec −30 dBFS (20261011T191033Z) | genelec −50 dBFS (20261011T185816Z) |
|---|---|---|---|
| response magnitude | as REW: 9/9 judged; ac2 − REW 0.013 dB @ 20–100 Hz [mean 0.000 dB, n 99 of 111] (PASS 3) | as REW: 5/8 judged; ac2 − REW 0.284 dB @ 1000–20000 Hz [mean +0.034 dB, n 188 of 208] (PASS 3) | as REW: 3/8 judged; ac2 − REW 0.212 dB @ 1000–20000 Hz [mean −0.050 dB, n 191 of 208] (PASS 2, INCONCLUSIVE 1) |
| response phase | as REW: 9/9 judged; ac2 − REW 0.102° @ 20–100 Hz [mean +0.010°, n 99 of 111] (PASS 3) | as REW: 5/8 judged; ac2 − REW 2.078° @ 20–100 Hz [mean −0.378°, n 19 of 111] (PASS 3) | as REW: 2/8 judged; ac2 − REW 2.193° @ 1000–20000 Hz [mean −0.082°, n 191 of 208] (PASS 2, INCONCLUSIVE 1) |
| absolute level | — | as REW: 7/8 judged; ac2 − REW +0.014 dB [n 96] (PASS 1) | as REW: 3/8 judged; ac2 − REW −0.002 dB [n 96] (PASS 1) |
| group delay (±1/12-oct fit) | as REW: 6/8 judged | — | — |
| harmonics H2–H5 | not judged (floor): 0/30 judged; ac2 − REW INCONCLUSIVE 150 | ac2 WARN (REW cannot tell): 9/26 judged; ac2 alone misses at H4 500 Hz; ac2 and REW miss the truth alike (ac2 − REW within tolerance) at H2 5000 Hz; ac2 − REW −1.284 dB @ H2 5000 Hz (PASS 7, INCONCLUSIVE 71) | not judged (floor): 0/26 judged; ac2 − REW INCONCLUSIVE 78 |
| LF H2 at 22 Hz (10 Hz sweep) | not judged (floor): 0/1 judged | — | — |
| arrival (vs its own capture) | PASS: 5/5 judged | PASS: 3/3 judged | PASS: 3/3 judged |

### pupu: electrical (xone) and speaker (genelec) paths, every row

| check | xone −30 dBFS (20261011T052811Z) | genelec −30 dBFS (20261011T191033Z) | genelec −50 dBFS (20261011T185816Z) |
|---|---|---|---|
| verdicts | PASS 211, INCONCLUSIVE 350, INFO 90 | WARN 11, PASS 158, INCONCLUSIVE 183, INFO 72 | FAIL 1, WARN 3, PASS 83, INCONCLUSIVE 249, INFO 72 |
| ac2 sweep vs REW offline import, magnitude spread per band | 0.013 dB @ 20–100 Hz [mean 0.000 dB, n 99 of 111] (PASS 3) | 0.284 dB @ 1000–20000 Hz [mean +0.034 dB, n 188 of 208] (PASS 3) | 0.212 dB @ 1000–20000 Hz [mean −0.050 dB, n 191 of 208] (PASS 2, INCONCLUSIVE 1) |
| ac2 sweep vs REW offline import, phase, max(\|mean\|, spread) per band | 0.102° @ 20–100 Hz [mean +0.010°, n 99 of 111] (PASS 3) | 2.078° @ 20–100 Hz [mean −0.378°, n 19 of 111] (PASS 3) | 2.193° @ 1000–20000 Hz [mean −0.082°, n 191 of 208] (PASS 2, INCONCLUSIVE 1) |
| REW offline vs direct (same recording), magnitude, max(\|mean\|, spread) per band | 0.010 dB @ 20–100 Hz [mean 0.000 dB, n 99 of 111] (PASS 3) | 0.317 dB @ 20–100 Hz [mean −0.036 dB, n 21 of 111] (WARN 1, PASS 2) | 0.119 dB @ 100–1000 Hz [mean −0.043 dB, n 44 of 159] (PASS 2, INCONCLUSIVE 1) |
| REW offline vs direct (same recording), phase, max(\|mean\|, spread) per band | 0.104° @ 20–100 Hz [mean −0.003°, n 99 of 111] (PASS 3) | 1.934° @ 20–100 Hz [mean +0.284°, n 21 of 111] (PASS 3) | 0.722° @ 1000–20000 Hz [mean 0.000°, n 191 of 208] (PASS 2, INCONCLUSIVE 1) |
| ac2 sweep vs direct (its own capture), magnitude, max(\|mean\|, spread) per band | 0.009 dB @ 1000–20000 Hz [mean −0.005 dB, n 197 of 208] (PASS 3) | 0.109 dB @ 20–100 Hz [mean −0.002 dB, n 22 of 111] (PASS 3) | 0.113 dB @ 100–1000 Hz [mean −0.038 dB, n 52 of 159] (PASS 2, INCONCLUSIVE 1) |
| ac2 sweep vs direct (its own capture), phase, max(\|mean\|, spread) per band | 0.061° @ 20–100 Hz [mean +0.004°, n 99 of 111] (PASS 3) | 0.849° @ 20–100 Hz [mean −0.036°, n 22 of 111] (PASS 3) | 0.484° @ 100–1000 Hz [mean −0.013°, n 52 of 159] (PASS 2, INCONCLUSIVE 1) |
| ac2 live TF vs direct (γ² ≥ 0.99), magnitude, max(\|mean\|, spread) per band | 0.017 dB @ 20–100 Hz [mean −0.001 dB, n 73 of 111] (PASS 3) | 0.143 dB @ 100–1000 Hz [mean +0.001 dB, n 30 of 159] (WARN 1, INCONCLUSIVE 2) | INCONCLUSIVE 2 |
| ac2 live TF vs direct (γ² ≥ 0.99), phase, max(\|mean\|, spread) per band | 0.085° @ 20–100 Hz [mean −0.005°, n 73 of 111] (PASS 3) | 1.133° @ 100–1000 Hz [mean +0.014°, n 30 of 159] (PASS 1, INCONCLUSIVE 2) | INCONCLUSIVE 2 |
| ac2 sweep vs steady sine, magnitude | +0.010 dB @ 15 Hz (PASS 9) | −0.176 dB @ 2000 Hz (PASS 5, INCONCLUSIVE 3) | −0.291 dB @ 2000 Hz (PASS 3, INCONCLUSIVE 5) |
| REW offline vs steady sine, magnitude | +0.004 dB @ 56.2 Hz (PASS 9) | −0.185 dB @ 2000 Hz (PASS 5, INCONCLUSIVE 3) | −0.277 dB @ 2000 Hz (PASS 3, INCONCLUSIVE 5) |
| ac2 live TF vs steady sine, magnitude | −0.001 dB @ 93.9 Hz (PASS 7) | −0.498 dB @ 1000 Hz (WARN 1, PASS 2, INCONCLUSIVE 2) | −0.140 dB @ 1000 Hz (PASS 1, INCONCLUSIVE 1) |
| ac2 sweep vs steady sine, phase | +0.037° @ 40.5 Hz (PASS 9) | +1.187° @ 2000 Hz (PASS 5, INCONCLUSIVE 3) | +0.794° @ 2000 Hz (PASS 2, INCONCLUSIVE 6) |
| REW offline vs steady sine, phase | −0.066° @ 15 Hz (PASS 9) | +1.218° @ 2000 Hz (PASS 5, INCONCLUSIVE 3) | +0.740° @ 2000 Hz (PASS 2, INCONCLUSIVE 6) |
| ac2 live TF vs steady sine, phase | −0.056° @ 40.5 Hz (PASS 7) | +1.522° @ 1000 Hz (PASS 5) | −0.734° @ 1000 Hz (PASS 1, INCONCLUSIVE 1) |
| ac2 meas÷ref vs REW (meas − ref), absolute level | −0.001 dB [n 96] (PASS 1) | +0.014 dB [n 96] (PASS 1) | −0.002 dB [n 96] (PASS 1) |
| ac2 reference level vs REW's | −0.006 dB (PASS 1) | −0.004 dB (PASS 1) | −0.006 dB (PASS 1) |
| ac2 reported arrival vs direct, every sweep | −0.01 µs @ 3Hz-5.5s (PASS 5) | +0.53 µs @ 20Hz-5.5s (PASS 3) | −1.04 µs @ dist-probe (PASS 3) |
| ac2 arrival + phase slope vs direct, every sweep | 0.00 µs @ dist-probe (PASS 5) | — | — |
| group delay vs sine pairs: ac2, ±1/12-oct fit | −0.01 µs @ 1040 Hz; −8.0 % @ 60.5 Hz (PASS 6, INCONCLUSIVE 2) | — | — |
| group delay vs sine pairs: ac2 as displayed | 0.00 µs @ 10000 Hz; −5.9 % @ 60.5 Hz (PASS 3, INCONCLUSIVE 5) | — | — |
| group delay vs sine pairs: REW, ±1/12-oct fit | +0.03 µs @ 1040 Hz; +4.6 % @ 40.5 Hz (PASS 7, INCONCLUSIVE 1) | — | — |
| group delay vs sine pairs: REW's own GD export | −0.03 µs @ 10000 Hz; +6.6 % @ 40.5 Hz (INFO 8) | — | — |
| H2–H5 vs steady sine: ac2 sweep | INCONCLUSIVE 30 | +3.521 dB @ H2 5000 Hz (WARN 1, PASS 4, INCONCLUSIVE 21) | INCONCLUSIVE 26 |
| H2–H5 vs steady sine: REW offline import | INCONCLUSIVE 30 | +4.807 dB @ H2 5000 Hz (WARN 1, PASS 2, INCONCLUSIVE 23) | INCONCLUSIVE 26 |
| absolute SPL vs steady sine: ac2 sweep | — | −0.183 dB @ 2000 Hz (PASS 7, INCONCLUSIVE 1) | −0.289 dB @ 2000 Hz (PASS 3, INCONCLUSIVE 5) |
| absolute SPL vs steady sine: REW (cal from ac2) | — | +0.135 dB @ 10000 Hz (PASS 8) | +1.172 dB @ 50 Hz (FAIL 1, PASS 7) |
| live TF: share of columns at γ² ≥ 0.99, 31 Hz–20 kHz | 100.0 % (PASS 1) | 44.6 % (INFO 1) | 20.8 % (INFO 1) |

### pupu: distortion coverage per harmonic order (steady-sine tones judged)

| check | xone −30 dBFS (20261011T052811Z) | genelec −30 dBFS (20261011T191033Z) | genelec −50 dBFS (20261011T185816Z) |
|---|---|---|---|
| H2 ac2 sweep | INCONCLUSIVE 8 | +3.521 dB @ 5000 Hz (WARN 1, PASS 1, INCONCLUSIVE 5) | INCONCLUSIVE 7 |
| H3 ac2 sweep | INCONCLUSIVE 8 | +0.420 dB @ 1000 Hz (PASS 2, INCONCLUSIVE 5) | INCONCLUSIVE 7 |
| H4 ac2 sweep | INCONCLUSIVE 7 | INCONCLUSIVE 6 | INCONCLUSIVE 6 |
| H5 ac2 sweep | INCONCLUSIVE 7 | +0.043 dB @ 500 Hz (PASS 1, INCONCLUSIVE 5) | INCONCLUSIVE 6 |
| H2 REW offline import | INCONCLUSIVE 8 | +4.807 dB @ 5000 Hz (WARN 1, PASS 1, INCONCLUSIVE 5) | INCONCLUSIVE 7 |
| H3 REW offline import | INCONCLUSIVE 8 | +0.372 dB @ 1000 Hz (PASS 1, INCONCLUSIVE 6) | INCONCLUSIVE 7 |
| H4 REW offline import | INCONCLUSIVE 7 | INCONCLUSIVE 6 | INCONCLUSIVE 6 |
| H5 REW offline import | INCONCLUSIVE 7 | INCONCLUSIVE 6 | INCONCLUSIVE 6 |

### OSM stage (osm-2c07efe, v1.5.2: PASS 86, INFO 77)

**Transfer: max over the gated columns; a rig take (not a planned fixture) shows the median over its columns**

| case | \|ac2 \|H\| − truth\|, dB | \|OSM \|H\| − truth\|, dB | \|ac2 ∠ − truth\|, ° | \|OSM ∠ − truth\|, ° | \|ac2 − OSM\| on \|H\|, dB | \|ac2 − OSM\| on ∠, ° | ac2 γ² − true γ² (mean) | ac2 γ² − E[γ̂²] (mean) | OSM γ² − E[γ̂²] (mean) |
|---|---|---|---|---|---|---|---|---|---|
| identity | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 |  | 0.000 |
| biquad | 0.031 | 0.024 | 0.273 | 0.215 | 0.030 | 0.299 | 0.000 |  | 0.000 |
| delay48 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 |  | 0.000 |
| delay10_5 | 0.002 | 0.001 | 0.014 | 0.005 | 0.002 | 0.014 | 0.000 |  | 0.000 |
| polarity | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 |  | 0.000 |
| snr20 |  |  |  |  |  |  | +0.001 | 0.000 | 0.000 |
| snr10 |  |  |  |  |  |  | +0.008 | +0.001 | +0.001 |
| snr0 |  |  |  |  |  |  | +0.049 | −0.003 | −0.005 |
| genelec-rig (median) |  |  |  |  | 0.144 | 0.796 |  |  |  |
| xone-rig (median) |  |  |  |  | 0.000 | 0.004 |  |  |  |

**Delay (samples), noise and spectrum: signed**

| case | ac2 finder − truth | OSM finder − truth | ac2 slope − truth | OSM slope − truth | ac2 − OSM finder | ac2 H1 in noise − 0 dB (median), dB | OSM mean ratio − E\|1+N/R\|, dB | ac2 spectrum − truth, dB | OSM spectrum (converted) − truth, dB |
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

**Columns behind each comparison, and a rig take's tails**

| case | columns judged | below the γ² gate | ac2 non-finite | OSM bins under its float32 floor | OSM NaN-phase bins | \|ac2 − OSM\| on \|H\|: 95th pct, dB | \|ac2 − OSM\| on \|H\|: max, dB | \|ac2 − OSM\| on ∠: 95th pct, ° | \|ac2 − OSM\| on ∠: max, ° |
|---|---|---|---|---|---|---|---|---|---|
| identity | 412 | 0 | 3 | 0 | 0 |  |  |  |  |
| biquad | 412 | 0 | 3 | 0 | 0 |  |  |  |  |
| delay48 | 412 | 0 | 3 | 0 | 0 |  |  |  |  |
| delay10_5 | 412 | 0 | 3 | 0 | 0 |  |  |  |  |
| polarity | 412 | 0 | 3 | 0 | 0 |  |  |  |  |
| snr20 | 412 | 0 | 3 | 0 | 1 |  |  |  |  |
| snr10 | 11 | 401 | 3 | 0 | 0 |  |  |  |  |
| snr0 | 0 | 412 | 3 | 0 | 0 |  |  |  |  |
| genelec-rig | 307 | 105 | 3 | 17 | 34 | 1.054 | 3.587 @ 69.1 Hz | 5.709 | 20.399 @ 54.9 Hz |
| xone-rig | 412 | 0 | 3 | 17 | 1 | 0.002 | 0.005 @ 59.9 Hz | 0.033 | 0.134 @ 59.9 Hz |

### Digital DUT: harmonics H2–H5 against their exact values (dummy JACK)

| order, analyser | dut −10 dBFS (20261013T044420Z) | dut −30 dBFS (20261013T043410Z) |
|---|---|---|
| verdicts | WARN 3, PASS 761, INFO 116 | WARN 3, PASS 761, INFO 116 |
| H2 steady sine | +0.024 dB (PASS 28) | −0.032 dB (PASS 28) |
| H2 ac2 sweep | +0.137 dB (PASS 56) | +0.212 dB (PASS 56) |
| H3 steady sine | +0.034 dB (PASS 28) | −0.023 dB (PASS 28) |
| H3 ac2 sweep | +0.086 dB (PASS 56) | −0.105 dB (PASS 56) |
| H4 steady sine | −0.021 dB (PASS 28) | +0.013 dB (PASS 28) |
| H4 ac2 sweep | +0.144 dB (PASS 56) | −0.196 dB (PASS 56) |
| H5 steady sine | +0.012 dB (PASS 28) | +0.017 dB (PASS 28) |
| H5 ac2 sweep | −0.193 dB (PASS 56) | +0.145 dB (PASS 56) |

<!-- END generated results -->

## Definitions

### Terms

- **Truth**: a reference that does not depend on the analyser judged. The steady sine is a
  numpy measurement of a sine played on the same path. The **direct estimate** is a numpy
  cross-spectrum of the same recording an analyser read, so a difference to it is that
  analyser's processing, not the take. It does not by itself validate the measurement chain.
- **Signed** value: a − b; its sign is the direction. **Unsigned** value: a size with no
  direction. A band row shows the larger of |mean difference| and the spread about that mean;
  the ac2-vs-REW magnitude row shows the spread alone (its level offset is the gain row). The
  OSM transfer columns show the maximum |difference| over the gated columns, and on a rig
  recording the median.
- **PASS / WARN / FAIL**: |value| against the (pass, warn) limits in `tolerances.toml`.
- **INCONCLUSIVE**: the data cannot tell a pass from a fail, for example a reading near its
  noise floor (then it is a bound, not a value) or noise wider than the limit.
- **INFO**: context, not judged.
- **Shared miss** ("ac2 and REW miss the truth alike"): an annotation, not a verdict. Both
  analysers miss the truth with one sign, ac2 − REW is within its own limit, and the two targets
  compare. It says the two read the same number, not why the truth differs. Both checks keep
  their verdict, count in the worst values and are held to their baselines. Two misses without
  it are each analyser's own ("not alike").
- **Coverage**: how many checks could be judged at all. For the live TF it is the share of
  columns at γ² ≥ 0.99; for distortion, the coverage table per harmonic order.
- **Model-limited** (digital DUT): a cell where the sweep's model cannot follow the DUT's
  pre-filter. The DUT table leaves its value out of a row's worst value but counts it in the
  verdicts; the summary counts it like any other check.
  [reports/dut-coverage.md](reports/dut-coverage.md) marks such cells †.

### How the tools are run

- **REW**: the suite plays REW's sweep file itself and imports the recording, which REW
  deconvolves offline. Live REW needs a Pro licence, so REW gives no live TF, coherence or
  arrival time here.
- **OSM**: OSM's own `Measurement` class runs offline on 96 kHz WAV pairs through
  [osm-harness](https://github.com/mkovero/osm-harness) (GPL, outside this repo). OSM has no
  sweeps or IEC level meters, and its only distortion reading is THD+N at 1 kHz, so sweep
  H2–H5, SPL, Leq and RTA are outside the OSM comparison.
- **Digital DUT**: a software device with exactly known harmonics, on a dummy JACK server.
- **Repeats**: [reports/](reports/) holds repeated Genelec takes, the coherence model study and
  the DUT coverage grid, each re-analysed with this suite.

### How the definitions differ

| quantity | ac2 | REW (imported sweep) | OSM v1.5.2 | how the suite compares |
|---|---|---|---|---|
| transfer | live H1 = G_rm/G_rr over a multi-time-window ladder; sweep meas ÷ measured loopback ref | each input deconvolved by REW's stimulus, meas ÷ ref | mean of \|M\|/\|R\| per 80 ms tick (not H1); phase a mean of unit phasors | magnitudes only where both γ² ≥ 0.95; in noise OSM against E\|1 + N/R\|, ac2 against 0 dB |
| coherence | γ² per bin from accumulated cross and auto spectra (7.6<!--= r coherence-study.md // Coherence study // SNR dB=+0; setting=A (ac2 --blocks 8, OSM FFT16) // ac2 N_eff per bin#0 -->–8.1<!--= r coherence-study.md // Coherence study // SNR dB=+0; setting=A (ac2 --blocks 8, OSM FFT16) // ac2 N_eff per bin#1 --> effective averages per bin at 8 blocks), averaged over the column weighted by Gxx. The weight biases γ² upward beyond each bin's own finite-average bias, towards γ² + (1 − γ²)/n over many bins; ac2 keeps that bias by design | none | γ over a fixed 21 overlapping ticks (5.4<!--= r coherence-study.md // Coherence study // SNR dB=+0; setting=A (ac2 --blocks 8, OSM FFT16) // OSM N_eff --> independent averages) | in noise both against their own E[γ̂²]: OSM Carter's at its Welch count (γ squared), ac2 Carter's per bin plus the weight's bias (`coherence_study.ac2_column_g2`); ac2 against the true γ² where that is 1, and beside it (INFO) elsewhere |
| grid | 1/48-octave columns | native bins, unsmoothed | native bins; smoothing is display only | both power-averaged into ac2's columns |
| delay finder | sub-sample (regularised H1, Hilbert IR) | removed by its timing markers | integer argmax of IFFT(M/R), not applied automatically | ac2 within 0.05<!--= t osm.delay_ac2_samples.pass --> sample of truth, OSM within 0.5<!--= t osm.delay_osm_samples.pass -->; in the delay cases and the speaker recording the harness inserts each finder's result (OSM's whole sample as `--delay`) and replays, as an operator would |
| spectrum | dBFS peak convention, power mean of noise | not compared | RMS (a sine reads 3.01 dB lower); a linear mean of amplitudes | +3.01 dB added to OSM; on noise also +1.05 dB, the Rayleigh amplitude mean's √(π/4) under the power mean |
| SPL, Leq, RTA | filters designed to IEC 61672-1 / 61260-1; tested here on one ambient take: Z/A/C Leq against numpy, and the 1/3-oct RTA against its own order-3 Butterworth bands applied to the recording; not class conformance | SPL meter, RTA | rectangular F/S averages, Leq sampled once a second, FFT-bin bands | Leq: ac2 and REW against numpy. RTA: ac2 against its own band filters (largest \|residual\| over 29 bands, 0.25<!--= t level.ambient_rta_model_db.pass --> / 0.5 dB<!--= t level.ambient_rta_model_db.warn -->); ac2 and REW against ideal band edges as context (INFO). OSM not compared |
| phase | meas ÷ ref, its reported arrival put back | REW's timing-marker delay restored from the direct estimate of the same recording | phase of the complex mean | against the steady sine on the absolute reference; between two sources a pure delay difference is fitted (1–20 kHz electrical, 200 Hz–5 kHz speaker) and taken out, so the phase rows compare the aligned phase shape; arrival is judged separately against each sweep's own capture |
| distortion | exponential sweep, H2–H5, ÷ measured ref | distortion export of the meas input alone | THD+N at 1 kHz only; sweep H2–H5 not compared | ac2 against the steady sine net of the ref's own harmonics carried through the path; REW against the meas input's steady sine alone (the two truths differ where the reference carries harmonics) |
| level and gain | meas ÷ ref × reference level; SPL with the mic sensitivity | meas − ref; SPL with the calibration taken from ac2 | not compared | electrical gain (ac2 meas÷ref against REW meas − ref, both paths) apart from acoustic SPL against the steady sine (speaker path only); the Xone has no SPL, so its parity "absolute level" is a dash |

### Differences by design

These pass against their own model; they are differences of definition, not errors.

- **OSM's magnitude in noise** reads high: it is a mean of ratios, not H1. It stays within
  0.009 dB<!--= b host/osm // osm.snr0.osm_mean_m_r_vs_its_expected_bias --> of its model
  E|1 + N/R| at 0 dB SNR, while ac2's H1 stays within
  0.05 dB<!--= b host/osm // osm.snr0.ac2_h1_h_vs_analytic_0_db --> of the true 0 dB.
- **ac2's γ² in noise** reads above the true γ²:
  +0.008<!--= b host/osm // osm.snr10.ac2_γ²_vs_true_γ² --> and
  +0.049<!--= b host/osm // osm.snr0.ac2_γ²_vs_true_γ² --> at 10 and 0 dB SNR on the OSM stage.
  In the coherence study it reads
  0.548<!--= r coherence-study.md // Coherence study // SNR dB=+0; setting=A (ac2 --blocks 8, OSM FFT16) // ac2 mean ± sd#0 --> ±
  0.009<!--= r coherence-study.md // Coherence study // SNR dB=+0; setting=A (ac2 --blocks 8, OSM FFT16) // ac2 mean ± sd#1 -->
  (mean ± sd over 8 seeds) at a true 0.5, 0 dB SNR, 8 blocks, against its model's
  0.552<!--= r coherence-study.md // Coherence study // SNR dB=+0; setting=A (ac2 --blocks 8, OSM FFT16) // ac2 E[γ̂²] -->:
  the finite-average bias of the estimator plus the Gxx weight's own bias, kept by design.
  [reports/coherence-study.md](reports/coherence-study.md) has every SNR and both settings.
- **OSM's γ² in noise** reads
  0.552<!--= r coherence-study.md // Coherence study // SNR dB=+0; setting=A (ac2 --blocks 8, OSM FFT16) // OSM mean ± sd#0 -->
  at a true 0.5 (0 dB SNR): its 21 overlapped ticks are few averages.
- **OSM's delay finder** is an integer argmax, so it rounds a half-sample delay (case
  `delay10_5`) to a whole sample; the OSM delay table shows it.
- **OSM masks**: NaN phase on exactly-zero bins, and a float32 floor
  70 dB under the reference peak, are OSM bugs, masked; the OSM coverage table counts them.

## Open items

An explanation stays **open** until a report, plot or controlled test shows that it accounts for
the residual. Each item below is a row of the miss table under [Summary](#summary); its values
are there.

### Genelec H2 at 5 kHz

All three ac2 sweeps and REW's import read H2 at 5 kHz above the steady sine on the Genelec, by
+3.52 dB<!--= b pupu/genelec-30dbfs // genelec.h2.ac2 sweep 20Hz-5.5s@5000Hz --> to
+4.21 dB<!--= b pupu/genelec-30dbfs // genelec.h2.ac2 sweep dist-probe@5000Hz --> (ac2) and
+4.81 dB<!--= b pupu/genelec-30dbfs // genelec.h2.REW offline import@5000Hz --> (REW). ac2 − REW
passes (from −1.29 dB<!--= b pupu/genelec-30dbfs // genelec.h2.ac2 sweep 20Hz-5.5s@5000Hz // shared.diff -->
to −0.60 dB<!--= b pupu/genelec-30dbfs // genelec.h2.ac2 sweep dist-probe@5000Hz // shared.diff -->),
so this is a shared miss: the two analysers read the same harmonic. Why the sweeps differ from
the steady sine is not shown. An earlier take on another build (run 20261010T173215Z) passed this
cell on both analysers. Next: repeat takes, and check the steady sine's own harmonic floor at
10 kHz.

### Genelec H4 at 500 Hz on the dist sweep

Only the long `dist` sweep reads H4 of 500 Hz above its floor, and reads it
+5.76 dB<!--= b pupu/genelec-30dbfs // genelec.h4.ac2 sweep dist@500Hz --> over the steady sine.
The two shorter ac2 sweeps and REW give only bounds there, so they neither confirm nor refute
it. One take. Next: a repeat with two long sweeps.

### Genelec live TF at 1 kHz

The live TF reads
−0.50 dB<!--= b pupu/genelec-30dbfs // genelec.sine_mag.ac2 TF@1000Hz --> against the steady
sine at 1 kHz; its SPL row carries the same miss. The direct estimate of the TF's own capture
reads −0.18 dB<!--= b pupu/genelec-30dbfs // genelec.sine_mag.direct (TF capture)@1000Hz --> at
the same column, so the TF's own term is
−0.32 dB<!--= b pupu/genelec-30dbfs // genelec.sine_mag.ac2 TF@1000Hz minus genelec.sine_mag.direct (TF capture)@1000Hz -->.
Over the earlier takes at 1/48 octave this term was
−0.202<!--= r genelec-30-repeats.md // 1/48 octave > Column-to-tone // resolution=1/48; quantity=sine_mag; f Hz=1000; measurement=ac2 TF // processing#0 --> ±
0.167 dB<!--= r genelec-30-repeats.md // 1/48 octave > Column-to-tone // resolution=1/48; quantity=sine_mag; f Hz=1000; measurement=ac2 TF // processing#1 -->
(mean ± sd, 4<!--= r genelec-30-repeats.md // 1/48 octave > Column-to-tone // resolution=1/48; quantity=sine_mag; f Hz=1000; measurement=ac2 TF // takes --> takes),
while ac2's sweep processing at the same tone was
+0.001 dB<!--= r genelec-30-repeats.md // 1/48 octave > Column-to-tone // resolution=1/48; quantity=sine_mag; f Hz=1000; measurement=ac2 sweep 20Hz-5.5s // processing#0 -->.
A plausible cause, unmeasured: the TF's window at 1 kHz is shorter than the capture the direct
estimate reads. Next: replay a TF capture with the window matched to the direct estimate's span.

The sweeps' own −0.16 dB<!--= b pupu/genelec-30dbfs // genelec.sine_mag.ac2 sweep 20Hz-5.5s@1000Hz -->
there passes, as REW's −0.14 dB<!--= b pupu/genelec-30dbfs // genelec.sine_mag.REW offline@1000Hz -->
does: it is the room's fine structure inside one column, which the repeats split out as the
column-to-tone term
(−0.303 dB<!--= r genelec-30-repeats.md // 1/48 octave > Column-to-tone // resolution=1/48; quantity=sine_mag; f Hz=1000; measurement=ac2 sweep 20Hz-5.5s // column-to-tone#0 -->
at 1/48 octave; it scales with the column width).

### Genelec live TF SPL at 51 Hz

The live TF's absolute SPL at the 51 Hz sine reads
+0.51 dB<!--= b pupu/genelec-30dbfs // genelec.spl.ac2 TF × ref level@50Hz --> over the steady
sine, just over its pass limit; the TF's magnitude at that tone is inconclusive. Not
investigated.

### Genelec live TF against its direct estimate

From 100 Hz to 1 kHz, on the columns at γ² ≥ 0.99, the live TF's spread against its direct
estimate is 0.143 dB<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 TF|direct (TF capture).100-1000 -->
with a signed mean of
+0.001 dB<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 TF|direct (TF capture).100-1000 // mean -->,
over 30<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 TF|direct (TF capture).100-1000 // n -->
of the band's
159<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 TF|direct (TF capture).100-1000 // of --> columns.
Five earlier takes, on builds that read a column-sum γ², had a signed band mean of
−0.001<!--= r genelec-30-repeats.md // 1/48 octave > Live TF // check=genelec.mag.ac2 TF|direct (TF capture).100-1000 (signed mean) // mean --> ±
0.011 dB<!--= r genelec-30-repeats.md // 1/48 octave > Live TF // check=genelec.mag.ac2 TF|direct (TF capture).100-1000 (signed mean) // sd -->
and WARN on 4<!--= r genelec-30-repeats.md // 1/48 octave > Live TF // check=genelec.mag.ac2 TF|direct (TF capture).100-1000 (signed mean) // statuses#1 -->
of them (spread up to
0.175 dB<!--= r genelec-30-repeats.md // 1/48 octave > Live TF // check=genelec.mag.ac2 TF|direct (TF capture).100-1000 (signed mean) // spread -->);
at 1/96 octave the spread reached
0.300 dB<!--= r genelec-30-repeats.md // 1/96 octave > Live TF // check=genelec.mag.ac2 TF|direct (TF capture).100-1000 (signed mean) // spread -->.
No take shows a band-average offset, but a stable frequency-dependent error can cancel in a band
mean. Only one take so far reads the per-bin γ². Next: per-column means and scatter across takes
on a common mask, and more per-bin takes.

### REW's deconvolution against numpy

REW's import against numpy cross-spectra of the same recording, 20–100 Hz on the Genelec: spread
0.317 dB<!--= b pupu/genelec-30dbfs // genelec.mag.REW offline|direct (REW recording).20-100 -->
over 21<!--= b pupu/genelec-30dbfs // genelec.mag.REW offline|direct (REW recording).20-100 // n -->
of 111<!--= b pupu/genelec-30dbfs // genelec.mag.REW offline|direct (REW recording).20-100 // of --> columns.
This is REW's analysis, not ac2's; few columns survive the speaker path's masks at LF. Not
investigated.

### The truth chain at the 100 Hz sine

The direct estimate of REW's recording, read narrow at the 100 Hz sine, differs from the steady
sine by −3.72°<!--= b pupu/genelec-30dbfs // genelec.sine_phase.direct (REW recording), narrow at the sine@100Hz -->;
the same estimate at column width passes
(−1.56°<!--= b pupu/genelec-30dbfs // genelec.sine_phase.direct (REW recording)@100Hz -->). This
checks the truth, neither analyser. Not investigated.

### Genelec at −50 dBFS: ac2 sweep repeatability

At −50 dBFS the `dist` sweep against its own capture, 100 Hz–1 kHz, spreads
0.663 dB<!--= b pupu/genelec-50dbfs // genelec.mag.ac2 sweep dist|direct (ac2 capture dist).100-1000 -->,
and `dist-probe` against the 20 Hz / 5.5 s sweep, 1–20 kHz, spreads
0.356 dB<!--= b pupu/genelec-50dbfs // genelec.mag.ac2 sweep dist-probe|ac2 sweep 20Hz-5.5s.1000-20000 -->
and 4.06°<!--= b pupu/genelec-50dbfs // genelec.phase.ac2 sweep dist-probe|ac2 sweep 20Hz-5.5s.1000-20000 -->.
At −30 dBFS the same pairs pass
(0.036 dB<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep dist|direct (ac2 capture dist).100-1000 -->;
0.198 dB<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep dist-probe|ac2 sweep 20Hz-5.5s.1000-20000 -->
and 1.31°<!--= b pupu/genelec-30dbfs // genelec.phase.ac2 sweep dist-probe|ac2 sweep 20Hz-5.5s.1000-20000 -->).
At −50 the room's noise is as much nearer the signal as the drive is lower, but these columns passed the noise gate, so
the cause is not separated on this take. On an earlier −50 take (run 20261007T104217Z) the
equivalent rows sat on a room tone that the gate now catches (docs/rigs/pupu.md). Next:
per-column residuals in the repeats report, and a repeat.

### Genelec at −50 dBFS: REW's SPL at 51 Hz

REW's SPL, calibrated from ac2, reads
+1.17 dB<!--= b pupu/genelec-50dbfs // genelec.spl.REW (SPL, cal from ac2)@50Hz --> over the
steady sine at 51 Hz. At −50 dBFS a speaker leaves the room's noise close at LF; ac2's sweep at
that tone is inconclusive. REW's reading, not ac2's. Not investigated.

### Digital DUT: dut-loud sweep against its capture

Only on the loud path, and not on the other three, the 10 Hz sweeps spread slightly more than
the pass limit against their own capture, at 20–100 Hz and 100 Hz–1 kHz. The signed means stay
within 0.000 dB<!--= b host/dut-10dbfs // dut-loud.mag.ac2 sweep 10Hz-5.5s|direct (ac2 capture 10Hz-5.5s).20-100 // mean -->.
Not investigated. Next: replay the `dut-loud` captures to see whether the spread depends on
level.

### Other open questions

- **Live-TF noise gate**: the gate's uncertainty treats the TF's overlapping FIFO blocks as
  independent averages, and ac2's γ² reads high in noise (see Differences by design), so the
  gate admits more columns than a calibrated one would. Whether its intervals cover the observed
  transfer-error scatter (white, coloured and non-stationary signals, across window crossovers)
  is unvalidated. Next: on the OSM-stage and coherence-study fixtures, compare the gate's σ with
  the scatter of |H| − truth per column.
- **Ambient RTA against its own filters** (stage not run on this build): the worst-band residual
  of the last take, with music in the room, is attributed to ac2's RTA FIFO spanning the recording plus a few seconds of the room
  around it. A quiet-room take (run 20261010T174045Z) read much smaller. Plausible, not
  demonstrated: capture the RTA at the recording's end with a FIFO of the recording's length.
- **REW's RTA against numpy** (INFO): REW's band filters and its averaging are not modelled
  here; not verified.
- **OSM's coherence average count**: at FFT16 OSM behaves like slightly more averages than the
  Welch model gives ([reports/coherence-study.md](reports/coherence-study.md)); the model is
  slightly pessimistic and the PASS stands.
- **Distortion coverage on the rig**: the rig's own harmonics sit near the floor at −30 dBFS, so
  few rig cells are judged (coverage table). The digital DUT carries the distortion evidence.

## Context

INFO rows and bounds whose cause is shown.

- **Digital DUT: H3 at 50 Hz on dut-floor-hp**: this cell lies outside the sweep's model; there
  the DUT's pre-filter changes faster than the sweep's instantaneous-frequency reading can
  follow, at −110 dBFS. It is marked † in [reports/dut-coverage.md](reports/dut-coverage.md),
  left out of the DUT table's worst value and counted in its verdicts. It passes at both levels
  on this build and has warned on earlier takes, which fits scatter about the pass limit; that
  it is only scatter is not shown.
- **Arrival on the digital DUT** (closed): the suite's truth, the IR peak of the capture,
  was interpolated over a span of the IR too short for the DUT's 8th-order 7.2 kHz Butterworth,
  which still rings there. The DFT interpolant treats the span as one period, so the ringing at
  its ends wrapped in and moved the truth early by under a tenth of a sample, which every DUT
  arrival check read as ac2 being late. With a span that holds the ringing the suite's peak
  matches the cascade's closed-form band-limited peak (`tests/test_dsp.py`), and every arrival
  check passes.
- **ac2's RTA against ideal band edges** (INFO; the ambient stage was not run on this build):
  ac2's bands are order-3 Butterworth filters, whose skirts pass a strong tone near a band edge
  into the neighbouring band, so they differ from ideal edges by several dB in such a band.
  Against those filters applied to the same recording ac2's bands passed on the last take, so the
  filter definition, not ac2's processing, accounts for the difference to ideal edges.
- **Xone LF harmonics are bounds**: hum bursts on the Xone's input, intermittent and set by
  whatever device in the building cycles, land in a sweep's harmonic windows when they coincide.
  ac2 reads each harmonic's floor beside its window, so these cells are bounds on ac2 and REW
  alike (coverage table). Evidence: [docs/rigs/pupu.md](../../docs/rigs/pupu.md).
- **ac2 against OSM on the Genelec recording**: the median difference passes; the largest
  differences sit at LF room modes (OSM coverage table), where OSM's single 0.68 s window and
  ac2's multi-time-window ladder see different reflections. Expected; not modelled.
- **ac2's sweep capture against REW's recording, arrival** (INFO): two different stimuli, and a
  loudspeaker's IR peak moves with each sweep's top-octave content. Each analyser's arrival is
  judged against its own capture.

## Refreshing

- `python -m crosscheck comparison` after a baseline or report changes. It rebuilds the three
  generated parts (summary, misses, results) and rewrites every quoted number from its source;
  `--check` writes nothing and fails when either is stale. Baselines: README, "Baselines and
  compare".
- A quoted number carries its source in an HTML comment after it
  (`crosscheck/comparison_quotes.py` lists the forms). `--check` refuses a measured-looking
  number or a build hash in the hand-written text without one, a WARN or FAIL that no open item
  names, and an open item with no miss left (`crosscheck/comparison_summary.py`, `MISS_ITEMS`).
- `reports/`: `python -m crosscheck repeats <runs>` (takes analysed by the same suite; a run's
  `--resolution` keeps it apart), `python -m crosscheck coherence-study --seeds 8`, and the DUT
  matrix (`python -m crosscheck run --rig rigs/host.toml --stages dut …`, README, "On a dummy
  server") then `python -m crosscheck dutcov <runs>`.
