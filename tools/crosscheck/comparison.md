# ac2 against REW and Open Sound Meter

The accuracy of ac2, next to REW 5.40 and Open Sound Meter (OSM) v1.5.2, that the committed
baselines demonstrate: the newest baseline per rig and stage, each under its own heading's run,
ac2 build and level. The stages sit on different builds, and a claim about another build needs
its own runs:

- **pupu, xone and genelec** (both at −30 dBFS, the speaker ceiling of that session): ac2
  d08cc70, 2026-10-10.
- **pupu ambient** (silent; the operator was playing music in the room), the **OSM stage** and
  the **digital DUT** matrix (−10 and −30 dBFS on a dummy JACK server): ac2 6f0e5b5, 2026-10-11.

Since d08cc70, ac2 forms the live TF's γ² per bin and averages it over the column weighted by Gxx
(3248315c); d08cc70 read \|ΣGxy\|²/(ΣGxx·ΣGyy) over the column, which falls where the phase turns
across a column. The pupu rows that rest on ac2's live-TF γ² are therefore **stale against
current ac2**: the γ² ≥ 0.99 share, every live-TF row gated on it (against the direct estimate per
band, with its n of columns, and against the steady sine) and the live-TF noise gate. They await
new xone and genelec runs at operator-approved levels. The pupu sweep, REW, steady-sine, harmonic
and arrival rows do not read ac2's γ², and the later protocol changes (sessions, IR WAV import and
export, sweep IRs kept up to 65536 samples, packed live frames) leave the full-precision outputs
the suite reads (`trace capture`, `trace export --csv`, `spl watch --json`) as they were; still,
those rows were measured on d08cc70 only.

Repeated takes, the coherence model and the DUT matrix are in [reports/](reports/): the Genelec
repeats at 1/48 octave are five takes on two builds (8b87f49 and bdc1bec; REW's recording is in
three of them, ac2's live TF at 1 kHz in four), those at 1/24 and 1/96 octave two takes each on
bdc1bec (all nine re-analysed with this suite; bdc1bec differs from d08cc70 only in the harmonic
floor, and like it reads the column-sum γ²). The coherence study ran on c0244e4, the per-bin γ² of
3248315c before it was rebased onto main.
Every analyser is judged against a truth first (numpy steady sines,
direct cross-spectra of the same recording, closed forms for synthetic WAV pairs), then against
the other tool. A direct estimate from the same recording isolates the processing; it does not
by itself validate the physical measurement chain. Limits are in `tolerances.toml`; the README
explains each stage.

- **REW**: the suite plays REW's sweep file itself and imports the recording (REW deconvolves
  it offline); live REW needs a Pro licence. No live TF, coherence or arrival time from REW.
  REW's SPL calibration is taken from ac2, so the REW absolute-SPL row checks consistency with a
  shared calibration, not the physical calibration itself.
- **OSM**: OSM's own `Measurement` class run offline on 96 kHz WAV pairs through
  [osm-harness](https://github.com/mkovero/osm-harness) (GPL, outside this repo). OSM has no
  sweeps or IEC level meters; its only distortion reading is THD+N at 1 kHz, so sweep H2–H5,
  SPL, Leq and RTA are outside the OSM comparison.

A cell is the worst judged value of its checks, with the band, tone or sweep, then the verdict
counts. A signed value is a − b: its sign is the direction. An unsigned value has no direction:
the per-band rows show the larger of |mean difference| and the spread about that mean (the
sweep-vs-REW magnitude row the spread alone: its level offset is the absolute-level row), and
the OSM transfer columns the maximum |difference| (a rig take: the median). INCONCLUSIVE: the
data cannot tell pass from fail (near the floor, or noise wider than the limit). "ac2 and REW
miss the truth alike (ac2 − REW within tolerance)" is an annotation on two WARN/FAIL checks, not
a verdict: both miss with one sign, their difference is within the ac2-vs-REW pass limit and
their targets compare. It says the two analysers read the same number, not why the truth
differs; both checks keep their verdict, appear in the worst-cell values and are held to their
baselines. Two misses without it are each analyser's own ("not alike"). INFO: context,
not judged. Verdict counts are counts of checks over overlapping bands, tones and sweeps, not
independent trials or a confidence level; a PASS covers its own band or tone only. The coverage
of the transfer rows is the γ² ≥ 0.99 share row (live TF) and the INCONCLUSIVE counts; the
distortion table shows it per harmonic order. `[mean …, n … of …]` beside an unsigned band value
is that band's signed mean difference, the number of columns it stands on after every mask
(mains, sweep edges, coherence gate, noise, comb nulls, harmonic content) and the band's columns
before them. In the DUT table a sweep row's value is the worst over the quasi-static cells, while
its verdict counts include the cells the sweep model cannot follow (the one sweep WARN per level
in this baseline is such a cell; [reports/dut-coverage.md](reports/dut-coverage.md) marks them †).

## Results

<!-- BEGIN generated by `python -m crosscheck comparison` from baselines/: do not edit by hand -->

### pupu: ac2 against REW, per quantity

| quantity | xone −30 dBFS (20261010T172222Z, ac2 d08cc70) | genelec −30 dBFS (20261010T173215Z, ac2 d08cc70) |
|---|---|---|
| response magnitude | as REW: 9/9 judged; ac2 − REW 0.010 dB @ 1000–20000 Hz [mean −0.005 dB, n 196 of 208] (PASS 3) | as REW, both miss alike, ac2 − REW WARN: 3/8 judged; ac2 and REW miss the truth alike (ac2 − REW within tolerance) at 1000 Hz; ac2 − REW 0.323 dB @ 20–100 Hz [mean −0.011 dB, n 20 of 111] (WARN 1, PASS 2) |
| response phase | as REW: 9/9 judged; ac2 − REW 0.090° @ 20–100 Hz [mean +0.006°, n 99 of 111] (PASS 3) | as REW: 5/8 judged; ac2 − REW 2.280° @ 20–100 Hz [mean −0.186°, n 20 of 111] (PASS 3) |
| absolute level | — | as REW: 5/8 judged; ac2 − REW +0.009 dB [n 96] (PASS 1) |
| group delay (±1/12-oct fit) | as REW: 8/8 judged | — |
| harmonics H2–H5 | not judged (floor): 0/30 judged; ac2 − REW INCONCLUSIVE 150 | as REW, both miss alike: 8/26 judged; ac2 and REW miss the truth alike (ac2 − REW within tolerance) at H3 500 Hz; ac2 − REW +0.711 dB @ H2 2000 Hz (PASS 10, INCONCLUSIVE 68) |
| LF H2 at 22 Hz (10 Hz sweep) | not judged (floor): 0/1 judged | — |
| arrival (vs its own capture) | PASS: 5/5 judged | PASS: 3/3 judged |

### pupu: electrical (xone) and speaker (genelec) paths, every row

| check | xone −30 dBFS (20261010T172222Z, ac2 d08cc70) | genelec −30 dBFS (20261010T173215Z, ac2 d08cc70) |
|---|---|---|
| verdicts | PASS 214, INCONCLUSIVE 347, INFO 90 | WARN 15, PASS 145, INCONCLUSIVE 186, INFO 72 |
| ac2 sweep vs REW offline import, magnitude spread per band | 0.010 dB @ 1000–20000 Hz [mean −0.005 dB, n 196 of 208] (PASS 3) | 0.323 dB @ 20–100 Hz [mean −0.011 dB, n 20 of 111] (WARN 1, PASS 2) |
| ac2 sweep vs REW offline import, phase, max(\|mean\|, spread) per band | 0.090° @ 20–100 Hz [mean +0.006°, n 99 of 111] (PASS 3) | 2.280° @ 20–100 Hz [mean −0.186°, n 20 of 111] (PASS 3) |
| REW offline vs direct (same recording), magnitude, max(\|mean\|, spread) per band | 0.012 dB @ 20–100 Hz [mean 0.000 dB, n 99 of 111] (PASS 3) | 0.180 dB @ 20–100 Hz [mean −0.008 dB, n 19 of 111] (PASS 3) |
| REW offline vs direct (same recording), phase, max(\|mean\|, spread) per band | 0.121° @ 20–100 Hz [mean −0.002°, n 99 of 111] (PASS 3) | 1.767° @ 20–100 Hz [mean +0.026°, n 19 of 111] (PASS 3) |
| ac2 sweep vs direct (its own capture), magnitude, max(\|mean\|, spread) per band | 0.009 dB @ 1000–20000 Hz [mean −0.005 dB, n 196 of 208] (PASS 3) | 0.163 dB @ 20–100 Hz [mean +0.001 dB, n 24 of 111] (PASS 3) |
| ac2 sweep vs direct (its own capture), phase, max(\|mean\|, spread) per band | 0.097° @ 20–100 Hz [mean +0.003°, n 99 of 111] (PASS 3) | 0.865° @ 20–100 Hz [mean +0.048°, n 24 of 111] (PASS 3) |
| ac2 live TF vs direct (γ² ≥ 0.99), magnitude, max(\|mean\|, spread) per band | 0.022 dB @ 20–100 Hz [mean 0.000 dB, n 73 of 111] (PASS 3) | 0.208 dB @ 100–1000 Hz [mean 0.000 dB, n 21 of 159] (WARN 1, INCONCLUSIVE 2) |
| ac2 live TF vs direct (γ² ≥ 0.99), phase, max(\|mean\|, spread) per band | 0.147° @ 20–100 Hz [mean −0.006°, n 73 of 111] (PASS 3) | 1.224° @ 100–1000 Hz [mean +0.030°, n 21 of 159] (PASS 1, INCONCLUSIVE 2) |
| ac2 sweep vs steady sine, magnitude | +0.008 dB @ 15 Hz (PASS 9) | −0.320 dB @ 1000 Hz (WARN 1, PASS 2, INCONCLUSIVE 5) |
| REW offline vs steady sine, magnitude | +0.004 dB @ 15 Hz (PASS 9) | −0.324 dB @ 1000 Hz (WARN 1, PASS 2, INCONCLUSIVE 5) |
| ac2 live TF vs steady sine, magnitude | −0.002 dB @ 60.5 Hz (PASS 7) | −0.006 dB @ 200 Hz (PASS 2, INCONCLUSIVE 1) |
| ac2 sweep vs steady sine, phase | +0.014° @ 26.25 Hz (PASS 9) | +2.728° @ 2000 Hz (PASS 5, INCONCLUSIVE 3) |
| REW offline vs steady sine, phase | −0.067° @ 15 Hz (PASS 9) | +1.879° @ 2000 Hz (PASS 5, INCONCLUSIVE 3) |
| ac2 live TF vs steady sine, phase | −0.018° @ 35 Hz (PASS 7) | +0.400° @ 100 Hz (PASS 3) |
| ac2 meas÷ref vs REW (meas − ref), absolute level | −0.001 dB [n 96] (PASS 1) | +0.009 dB [n 96] (PASS 1) |
| ac2 reference level vs REW's | −0.006 dB (PASS 1) | −0.005 dB (PASS 1) |
| ac2 reported arrival vs direct, every sweep | +0.01 µs @ dist-probe (PASS 5) | −0.53 µs @ 20Hz-5.5s (PASS 3) |
| ac2 arrival + phase slope vs direct, every sweep | 0.00 µs @ 3Hz-5.5s (PASS 5) | — |
| group delay vs sine pairs: ac2, ±1/12-oct fit | −0.03 µs @ 1040 Hz; +7.8 % @ 93.9 Hz (PASS 8) | — |
| group delay vs sine pairs: ac2 as displayed | −0.01 µs @ 10000 Hz; +7.6 % @ 93.9 Hz (PASS 3, INCONCLUSIVE 5) | — |
| group delay vs sine pairs: REW, ±1/12-oct fit | +0.03 µs @ 1040 Hz; +3.6 % @ 93.9 Hz (PASS 8) | — |
| group delay vs sine pairs: REW's own GD export | −0.12 µs @ 1040 Hz; +7.3 % @ 93.9 Hz (INFO 8) | — |
| H2–H5 vs steady sine: ac2 sweep | INCONCLUSIVE 30 | +5.133 dB @ H3 500 Hz (WARN 1, PASS 4, INCONCLUSIVE 21) |
| H2–H5 vs steady sine: REW offline import | INCONCLUSIVE 30 | +5.034 dB @ H3 500 Hz (WARN 1, PASS 3, INCONCLUSIVE 22) |
| absolute SPL vs steady sine: ac2 sweep | — | −0.326 dB @ 1000 Hz (PASS 5, INCONCLUSIVE 3) |
| absolute SPL vs steady sine: REW (cal from ac2) | — | +0.547 dB @ 10000 Hz (WARN 1, PASS 7) |
| live TF: share of columns at γ² ≥ 0.99, 31 Hz–20 kHz | 100.0 % (PASS 1) | 32.6 % (INFO 1) |

### pupu: distortion coverage per harmonic order (steady-sine tones judged)

| check | xone −30 dBFS (20261010T172222Z, ac2 d08cc70) | genelec −30 dBFS (20261010T173215Z, ac2 d08cc70) |
|---|---|---|
| H2 ac2 sweep | INCONCLUSIVE 8 | +1.919 dB @ 5000 Hz (PASS 1, INCONCLUSIVE 6) |
| H3 ac2 sweep | INCONCLUSIVE 8 | +5.133 dB @ 500 Hz (WARN 1, PASS 1, INCONCLUSIVE 5) |
| H4 ac2 sweep | INCONCLUSIVE 7 | INCONCLUSIVE 6 |
| H5 ac2 sweep | INCONCLUSIVE 7 | +0.913 dB @ 500 Hz (PASS 2, INCONCLUSIVE 4) |
| H2 REW offline import | INCONCLUSIVE 8 | +2.213 dB @ 5000 Hz (PASS 2, INCONCLUSIVE 5) |
| H3 REW offline import | INCONCLUSIVE 8 | +5.034 dB @ 500 Hz (WARN 1, PASS 1, INCONCLUSIVE 5) |
| H4 REW offline import | INCONCLUSIVE 7 | INCONCLUSIVE 6 |
| H5 REW offline import | INCONCLUSIVE 7 | INCONCLUSIVE 6 |

### pupu: ambient

| quantity | ambient silent (20261011T002931Z, ac2 6f0e5b5) |
|---|---|
| LAeq | as REW: 1/1 judged; ac2 − REW −0.003 dB (PASS 1) |
| 1/3-oct RTA (vs its own band filters) | PASS: 1/1 judged |

| check | ambient silent (20261011T002931Z, ac2 6f0e5b5) |
|---|---|
| verdicts | PASS 6, INFO 31 |
| LZeq ac2 vs numpy | +0.005 dB (PASS 1) |
| LAeq ac2 vs numpy | +0.001 dB (PASS 1) |
| LCeq ac2 vs numpy | +0.006 dB (PASS 1) |
| LAeq REW vs numpy | +0.005 dB (PASS 1) |
| LAeq ac2 vs REW | −0.003 dB (PASS 1) |
| 1/3-oct bands 25 Hz–16 kHz: ac2 RTA vs its own filters' response applied to the recording, worst band | +0.118 dB [n 29] (PASS 1) |
| 1/3-oct bands 25 Hz–16 kHz: ac2 RTA vs numpy at ideal band edges, largest \|difference\| (context) | 8.675 dB [n 29] (INFO 1) |
| 1/3-oct bands 25 Hz–16 kHz: REW RTA vs numpy at ideal band edges, largest \|difference\| (context) | 6.861 dB [n 29] (INFO 1) |

### OSM stage (osm-6f0e5b57, ac2 6f0e5b5, v1.5.2: PASS 86, INFO 77)

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

| order, analyser | dut −10 dBFS (20261011T005229Z, ac2 6f0e5b5) | dut −30 dBFS (20261011T010051Z, ac2 6f0e5b5) |
|---|---|---|
| verdicts | WARN 12, PASS 752, INFO 116 | WARN 12, PASS 752, INFO 116 |
| H2 steady sine | +0.054 dB (PASS 28) | +0.021 dB (PASS 28) |
| H2 ac2 sweep | +0.181 dB (PASS 56) | +0.126 dB (PASS 56) |
| H3 steady sine | +0.026 dB (PASS 28) | +0.041 dB (PASS 28) |
| H3 ac2 sweep | −0.189 dB (WARN 1, PASS 55) | −0.130 dB (WARN 1, PASS 55) |
| H4 steady sine | +0.026 dB (PASS 28) | +0.015 dB (PASS 28) |
| H4 ac2 sweep | −0.116 dB (PASS 56) | −0.115 dB (PASS 56) |
| H5 steady sine | −0.021 dB (PASS 28) | +0.017 dB (PASS 28) |
| H5 ac2 sweep | −0.135 dB (PASS 56) | −0.129 dB (PASS 56) |

<!-- END generated -->

## How the definitions differ

| quantity | ac2 | REW (imported sweep) | OSM v1.5.2 | how the suite compares |
|---|---|---|---|---|
| transfer | live H1 = G_rm/G_rr over a multi-time-window ladder; sweep meas ÷ measured loopback ref | each input deconvolved by REW's stimulus, meas ÷ ref | mean of \|M\|/\|R\| per 80 ms tick (not H1); phase a mean of unit phasors | magnitudes only where both γ² ≥ 0.95; in noise OSM against E\|1 + N/R\|, ac2 against 0 dB |
| coherence | γ² per bin from accumulated cross and auto spectra (≈ 7.6–8.1 effective averages per bin at 8 blocks), averaged over the column weighted by Gxx (since 3248315c; the pupu xone/genelec baselines on d08cc70 still read the earlier \|ΣGxy\|²/(ΣGxx·ΣGyy) over the column). The weight biases γ² upward beyond each bin's own finite-average bias, towards γ² + (1 − γ²)/n over many bins; ac2 keeps that bias by design | none | γ over a fixed 21 overlapping ticks (≈ 5.4 independent averages) | in noise both against their own E[γ̂²]: OSM Carter's at its Welch count (γ squared), ac2 Carter's per bin plus the weight's bias (k bins 1 − 1/k of the way to the limit, `coherence_study.ac2_column_g2`); ac2 against the true γ² where that is 1, and beside it (INFO) elsewhere |
| grid | 1/48-octave columns | native bins, unsmoothed | native bins; smoothing is display only | both power-averaged into ac2's columns |
| delay finder | sub-sample (regularised H1, Hilbert IR) | removed by its timing markers | integer argmax of IFFT(M/R), not applied automatically | ac2 within 0.05 sample of truth, OSM ±0.5; in the delay cases and the speaker recording the harness inserts each finder's result (OSM's whole sample as `--delay`) and replays, as an operator would |
| spectrum | dBFS peak convention, power mean of noise | not compared | RMS (a sine reads −3.01 dB); a linear mean of amplitudes | +3.01 dB added to OSM; on noise also +1.05 dB, the Rayleigh amplitude mean's √(π/4) under the power mean |
| SPL, Leq, RTA | filters designed to IEC 61672-1 / 61260-1; tested here on one ambient take: Z/A/C Leq against numpy, and the 1/3-oct RTA against its own order-3 Butterworth bands applied to the recording; not class conformance | SPL meter, RTA | rectangular F/S averages, Leq sampled once a second, FFT-bin bands | Leq: ac2 and REW against numpy. RTA: ac2 against its own band filters (largest \|residual\| over 29 bands, 0.25 / 0.5 dB); ac2 and REW against ideal band edges as context (INFO). OSM not compared |
| phase | meas ÷ ref, its reported arrival put back | REW's timing-marker delay restored from the direct estimate of the same recording | phase of the complex mean | against the steady sine on the absolute reference; between two sources a pure delay difference is fitted (1–20 kHz electrical, 200 Hz–5 kHz speaker) and taken out, so the phase rows compare the aligned phase shape; arrival is judged separately against each sweep's own capture |
| distortion | exponential sweep, H2–H5, ÷ measured ref | distortion export of the meas input alone | THD+N at 1 kHz only; sweep H2–H5 not compared | ac2 against the steady sine net of the ref's own harmonics carried through the path; REW against the meas input's steady sine alone (the two truths differ where the reference carries harmonics) |
| level and gain | meas ÷ ref × reference level; SPL with the mic sensitivity | meas − ref; SPL with the calibration taken from ac2 | not compared | electrical gain (ac2 meas÷ref vs REW meas − ref, both paths) apart from acoustic SPL against the steady sine (speaker path only): the Xone has no SPL, so its parity "absolute level" is a dash while its electrical gain row reads −0.001 dB |

## Where ac2 stands against REW

The question is not whether the rig's devices are clean but whether ac2 measures them as well as
REW does. Each check against the truth (the steady sine on the same path) is paired with REW's
check of the same quantity at the same tone: where only ac2 misses, ac2 is wrong; where both
miss alike (one sign, ac2 − REW within tolerance, comparable targets), the two analysers read
the same number and the difference from the steady sine needs its own explanation, which the
annotation does not supply. The per-quantity table above is the summary; the rows below it are
the evidence.

**As good as REW:**

- Response magnitude and phase. Xone: ac2 − REW signed mean ≤ 0.005 dB / 0.006° in every band,
  spread ≤ 0.010 dB / 0.09°. Genelec: signed mean ≤ 0.011 dB below 1 kHz and +0.04 dB above,
  phase −0.09 … −0.19°; the spread is 0.19–0.32 dB / 1.5–2.3° (the two tools run different
  sweeps through a room, so single columns differ take to take: 20–100 Hz is WARN on this take
  and was PASS at 0.20 dB on the last; the signed means show no material band-average offset
  on this take, which does not rule out a local, frequency-dependent one). At the sines ac2 and
  REW land within 0.01 dB of each other (1 kHz: ac2 −0.320, REW −0.324).
- Electrical gain, ac2 meas÷ref vs REW meas − ref: −0.001 dB (Xone), +0.009 dB (Genelec). The
  Genelec's absolute SPL is a consistency check only (REW calibrated from ac2); the Xone has no
  SPL rows.
- Harmonics where the rig gives a reading: on the Genelec every judged ac2 − REW harmonic
  passes (10 on this take, largest +0.71 dB; at most +1.51 dB over the six bdc1bec takes that
  have REW's recording, of eight bdc1bec Genelec takes at 1/24–1/96 octave, and +1.44 dB on the
  8b87f49 take; those runs re-analysed with this suite).
- Group delay (Xone, ±1/12-oct fit) and ambient LAeq (−0.003 dB on the 6f0e5b5 take, −0.016 dB on d08cc70).
- Arrival: ac2's reported arrival against the IR peak of its own sweep capture is within
  ±0.8 µs on every Genelec take and ±0.007 µs on the Xone. (REW reports no arrival to compare.)

**ac2 and REW miss the truth alike** (ac2 − REW within tolerance; both stay WARN):

| difference | ac2 | REW | evidence |
|---|---|---|---|
| Genelec magnitude at 1 kHz vs steady sine | −0.320 dB | −0.324 dB | room fine structure inside a column: the column-to-tone term scales with the column width, −0.607 / −0.303 / −0.153 dB at 1/24 / 1/48 / 1/96 octave, while ac2's sweep processing (sweep − its own capture's column) at 1 kHz is +0.006 ± 0.002 / +0.001 ± 0.001 / +0.000 ± 0.000 dB (mean ± sd over 2 / 5 / 2 takes). At 1/48 octave over 100 Hz–10 kHz (5 takes) the processing term's mean is within ±0.011 dB (sd ≤ 0.015 dB); at 50 Hz it is −0.741 ± 1.254 dB, noise-limited and outside that bound. [reports/genelec-30-repeats.md](reports/genelec-30-repeats.md), "Column-to-tone split at the sines" per resolution |
| Genelec H3 at 500 Hz vs steady sine | +4.76…+5.13 dB (three sweeps) | +5.03 dB | a shared sweep-versus-sine difference with no demonstrated mechanism yet; the two truths (ac2's net of the reference, REW's meas alone) are within 1.5 dB of each other, so the targets compare |

**ac2 worse, or ac2 alone:** on the ac2-vs-REW parity rows above, no rig quantity has ac2
missing where REW passes. Outside them, ac2 alone has WARNs only on the digital DUT: 12 per level
(open, below). The ambient RTA, judged against its own band filters, passes (+0.118 dB); against
ideal band edges it reads 8.675 dB, context explained below by the filter skirts. On bdc1bec the Xone's H2 at 22 Hz on the 10 Hz / 5.5 s sweep failed
(+10.8 dB over the steady sine, REW unable to tell): hum bursts on the Xone's input, below;
d08cc70 reads that floor and the cell is a bound.

**Explained by measurement** (residuals against the truth with a quantified, reproduced cause):

| difference | numbers | evidence |
|---|---|---|
| Xone LF harmonics on the sweeps vs steady sine (bdc1bec: H2 at 22 Hz FAIL on the 10 Hz / 5.5 s sweep; H2/H3 at 35–40 Hz on the 3 Hz sweep missed by ac2 and REW, same sign) | +10.8 dB; −75…−77 dBr against a truth below −89 | hum bursts on the Xone's input (in 5): 35–95 Hz at −84…−92 dBFS for 0.25–0.75 s every 2–3 s over a quiet −118, the reference input clean at −127. A burst at the moment the sweep passes a fundamental lands in its harmonic windows; the post-roll noise windows read 0.5–1 s later. Its level in dBFS does not follow the drive (−50 … −20 dBFS records), an independent numpy separation of the same record reads the same (−82.0 vs −83.6 dBr) and linear twins read nothing (−160 dBr). d08cc70 reads each harmonic's floor beside its window, the noise of the same moment: these cells are now bounds, on ac2 and REW alike. [docs/rigs/pupu.md](../../docs/rigs/pupu.md) |
| ac2 RTA vs numpy at ideal band edges, ambient (INFO) | 6f0e5b5 take (music in the room): largest \|difference\| 8.675 dB at 251 Hz over 29 bands; d08cc70 take (quiet): 1.797 dB | the filter definition: ac2's order-3 Butterworth bands (closed-form response, computed independently of ac2) applied to the same recording account for +8.681 dB of the 8.675 at 251 Hz, where the room carried strong energy near the 224 Hz edge between the 200 and 250 Hz bands (200 Hz band 62.9 dB SPL) that an order-3 skirt passes into its neighbour. Against those filters ac2's worst band is +0.118 dB at 7.9 kHz (median +0.013; judged, PASS), −0.012 dB on the quiet d08cc70 take. Ambient table |
| ac2 vs OSM on the Genelec rig take | median 0.14 dB / 0.80°, 95th pct 1.05 dB / 5.7°, max 3.59 dB @ 69 Hz / 20.4° @ 55 Hz over 307 columns above the γ² gate | windows: OSM's one 0.68 s window and ac2's MTW ladder see different reflections; the tails sit at LF room modes. On the Xone path the same comparison is 0.005 dB / 0.13° at worst |
| ac2 sweep capture vs REW's recording, arrival | +1.28…+2.10 µs on the d08cc70 take (three sweeps); +1.39…+2.32 µs over the seven earlier takes with REW's recording (1/24–1/96 repeats) | two different stimuli: the IR peak moves with each sweep's top-octave content. Context only (INFO); each analyser's arrival is judged against its own capture |

**By design** (a known difference of definition, not an error):

| difference | numbers | note |
|---|---|---|
| OSM magnitude in noise | +0.171 / +1.216 / +5.630 dB at 20 / 10 / 0 dB SNR; ac2 H1 within 0.05 dB | OSM's mean ratio, within 0.009 dB of its model E\|1 + N/R\| |
| OSM γ² vs truth | 0.550 at 0 dB SNR, true 0.5 | OSM's 21 overlapped ticks; see the open item on its average count |
| ac2 γ² above the true γ² in noise | +0.008 / +0.049 at 10 / 0 dB SNR (OSM stage, 6f0e5b5); 0.548 ± 0.009 (mean ± sd, 8 seeds) at a true 0.5, 0 dB SNR, 8 blocks, in the coherence study (c0244e4) | the finite-average bias of ac2's estimator, kept by design: each bin's γ̂² reads high by Carter's bias (≈ 7.6–8.1 effective averages per bin), and the Gxx weight adds its own (a bin whose reference read strong also reads more coherent), towards γ² + (1 − γ²)/n over many bins (0.562 at n ≈ 8). Against that model, 0.552 at 0 dB, the residual is within 1.4 standard errors at every SNR and both settings. [reports/coherence-study.md](reports/coherence-study.md) |
| Delay rounding | delay10_5: ac2 10.500, OSM 10; genelec-rig: ac2 348.449, OSM 349 | OSM's finder is an integer argmax |
| OSM masked bins | NaN phase on exactly-zero bins; a float32 floor 70 dB under the reference peak | OSM bugs, masked; counts per case in the OSM coverage table |

**Open** (not an ac2-vs-REW verdict):

| difference | numbers | status |
|---|---|---|
| pupu xone and genelec on current ac2 | baselines on d08cc70 (column-sum γ²); current main 6f0e5b5 | the live-TF γ² share, the γ² ≥ 0.99-gated live-TF rows and the noise gate on these paths describe the old coherence; the sweep, REW, sine and arrival rows have no reason to move but are unmeasured on 6f0e5b5. Needs xone and genelec runs at operator-approved levels (the Genelec stage is audible: operator present) |
| Genelec live TF at 1 kHz, its own term | −0.202 ± 0.167 dB at 1/48 (4 takes), +0.085 ± 0.045 at 1/96 (2) (mean ± sd) | the TF's MTW window at 1 kHz is shorter than the capture the direct estimate reads; a window effect is plausible, unmeasured |
| Genelec live TF 100–1000 Hz vs its direct estimate (γ² ≥ 0.99 columns) | 1/48 octave: WARN on 4 of 5 earlier takes (max(\|mean\|, spread) ≤ 0.175 dB) and on the d08cc70 take (0.208 dB, n 21 of 159 columns); signed band mean −0.001 ± 0.011 dB over the five (mean ± sd; phase +0.012 ± 0.078°). 1/96 octave (2 takes, bdc1bec): FAIL and WARN, spread up to 0.300 dB with signed mean −0.005 dB | no material band-average offset shown by the five 1/48 takes, but the WARN spread itself is unexplained: a stable frequency-dependent error can cancel in a band mean, and per-frequency means and scatter across takes on a common mask are not yet compared. Every take here read the column-sum γ² (pre-3248315c), so which columns pass the γ² ≥ 0.99 gate changes on current ac2: needs new Genelec takes. [reports/genelec-30-repeats.md](reports/genelec-30-repeats.md), 1/48 and 1/96 octave, "Live TF vs its direct estimate, per band" |
| Live-TF noise gate | Genelec 100–1000 Hz on the d08cc70 take: n 21 of 159 columns after every mask, against a 32.6 % γ² ≥ 0.99 share over 31 Hz–20 kHz | the gate's uncertainty takes the TF's FIFO block count as independent averages, though the blocks overlap, so it is a lower bound; it does not use the per-column effective-average model of the coherence study. Since 3248315c ac2's γ² is a Gxx-weighted per-bin mean that reads high in noise (0.548 at a true 0.5, 8 blocks), so σ from it is lower again and the gate admits more columns; the share and n here are from d08cc70's column-sum γ², stale until new rig runs. Whether its intervals cover the observed transfer-error scatter (white, coloured and non-stationary signals, across MTW crossovers) is unvalidated |
| DUT, 12 WARN at each level (−10 and −30 dBFS, 6f0e5b5) | 8 arrival: +0.69 … +0.76 µs (0.07 sample) on the two 10 Hz sweeps of the four DUT paths. 3 sweep-vs-direct magnitude on `dut-loud`: 20–100 Hz spread 0.092–0.111 dB on both 10 Hz sweeps, 100–1000 Hz spread 0.035 dB on 10 Hz / 5.5 s; signed means ≤ 0.001 dB. 1 model-limited harmonic (†): H3 at 50 Hz on `dut-floor-hp`, 10 Hz / 5.5 s, +0.54 / +0.56 dB | open. Arrival: the DUT's 7.2 kHz low-pass broadens the IR peak; how ac2's and the suite's peak interpolation differ there is unmeasured. Magnitude: only on the loud path, not on the other three; cause not investigated. The H3 cell is one the sweep's Wiener model cannot follow, at −110 dBFS: it read +0.31 … +0.58 dB over seven earlier matrix runs, WARN in two (d08cc70 baselines: +0.42 / +0.31), so its WARN here is that scatter, kept out of the coverage verdict. Both levels ran with a 4096-frame dummy period: at 2048 the host's load made xruns. `baselines/host/dut-*.json` |
| Ambient RTA against its own filters: what the residual is | +0.118 dB worst band (7.9 kHz) with music in the room (6f0e5b5), −0.012 dB in a quiet room (d08cc70); bands across the range moved up by 0.02–0.12 dB against the quiet take | the filter model is exact, so the remainder is attributed to ac2's RTA FIFO spanning the recording plus a few seconds of the room around it, which a non-stationary room shows; plausible, not demonstrated (e.g. by capturing the RTA at the recording's end with a FIFO of the recording's length). The limit (0.25 / 0.5 dB) assumes it |
| OSM's coherence average count | behaves like ≈ 5.8 averages at FFT16 against the 5.4 the Welch model gives; FFT14 matches its 19.4 | the model is slightly pessimistic; the PASS stands |
| REW RTA vs numpy | largest \|difference\| 6.861 dB over 29 bands on the 6f0e5b5 take (music), 4.106 dB on the quiet d08cc70 take (INFO) | REW's RTA averaging (Forever) consistent with averaging in dB, and REW's band filters are not modelled here; not verified |
| Distortion coverage on the rig | harmonic × tone cells judged on any ac2 sweep: xone 0 of 30, genelec 8 of 26 | the rig's own harmonics sit near the floor at −30 dBFS; the DUT matrix carries the distortion evidence: of 448 judged ac2 sweep cells (two levels, 6f0e5b5), the 384 quasi-static ones PASS within 0.19 dB, and the 64 cells where the sweep model cannot follow the DUT's pre-filter are reported apart (worst +0.56 dB, one WARN per level). [reports/dut-coverage.md](reports/dut-coverage.md) |

The OSM noise columns judge each analyser against a different question: ac2's H1 against the
physical truth (0 dB), OSM's mean ratio against its expected bias. Both coherences are judged
against their own estimator's expected value, ac2's beside its residual against the true γ².

## Refreshing

- Tables between the markers: `python -m crosscheck comparison` after a baseline changes
  (`tests/test_comparison.py` fails when they are stale).
- Rig baselines: `./rig-run.sh …`, review, `python -m crosscheck baseline runs/<UTC time>`.
- OSM baseline: `OSM_HARNESS=… AC2_BIN_DIR=… python -m crosscheck osm`, review,
  `python -m crosscheck baseline runs/osm-<UTC time>`.
- The tables in the last section are written by hand from the runs' reports and `reports/`. An
  explanation stays **open** until a report, plot or controlled test shows it accounts for the
  residual.
- Repeated takes: `python -m crosscheck repeats runs/<t1> runs/<t2> …` (the takes must be
  analysed by the same suite; a run's `--resolution` keeps it apart). Coherence model:
  `python -m crosscheck coherence-study --seeds 8`. DUT matrix: `./rig-run.sh --rig rigs/host.toml …`
  then `python -m crosscheck dutcov <runs>`.
