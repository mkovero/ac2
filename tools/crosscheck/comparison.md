# ac2 against REW and Open Sound Meter

ac2, REW 5.40 and Open Sound Meter (OSM) v1.5.2 measured the same signals on one ac2 build. On
the pupu rig (RME Fireface 400 at 96 kHz): an electrical path through an Allen & Heath Xone:62
mixer, a Genelec 1083 speaker in a room with a beyerdynamic MM1 measurement microphone at
54 cm on the Genelec's tweeter axis, 90° grazing, a second speaker standing beside it, and the same room's ambient noise. On the dev
host: a software device with known distortion on a dummy JACK server, and OSM's DSP on synthetic
test signals and on the rig's recordings. Each tool is calibrated the way its own user would
calibrate it (below). Each tool is held to a truth that depends on none of them: a steady sine
analysed in numpy, numpy's direct cross-spectrum of the same recording, closed-form test
signals, and the software device's exact harmonics. Not validated here: the physical
calibration chain (microphone sensitivity, calibrator) and IEC 61672-1 / 61260-1 class
conformance.

**Calibration.** Each tool gets the interface and the microphone in its own terms.
- *ac2* divides by the loopback (out 2 → in 2) live, so the interface's converters cancel, and
  applies the stored mic curve and its electrical SPL calibration of input 1.
- *REW* gets its own loopback calibration of the interface: its soundcard calibration wizard,
  run in REW's GUI on the same out 2 → in 2 pair, saved as a cal file and applied to every
  import. It also gets the microphone as REW's input calibration: the mic curve as a cal file
  and "dBFS at 94 dB SPL". With both applied, REW's measurement import alone is the corrected
  response, as a REW user would measure it.
- *OSM* gets the mic curve as its Measurement calibration on the speaker recording, as an OSM
  user loads a mic file; it gets no SPL calibration (its comparisons here are transfer
  functions and spectra in dBFS).

A microphone's data-sheet sensitivity is open-circuit; its source impedance into the input's is
a voltage divider, so the input sees less than the data sheet promises (the values are in the
build line below). ac2's electrical calibration measures the volts at the input, so its stored
sensitivity already includes the divider. REW has no setting for loading and is given 94 minus
ac2's loaded sensitivity, so loading never shows as a difference between the tools. The suite's
preflight refuses a run whose stored sensitivity disagrees with the data sheet once the divider
is applied.

<!-- BEGIN generated build: `python -m crosscheck comparison` from baselines/; do not edit by hand -->

ac2 build **7f896c1**, runs of 2026-10-17: Xone (electrical) at −10 dBFS; Genelec (speaker) at −30 dBFS; ambient; digital DUT at −10, −30 dBFS; OSM fixtures and rig recordings. SPL calibration: MM1 34804 on input 1, 0 dBFS = 120.47 dB SPL, including its loading (330 Ω into 2000 Ω, −1.33 dB); REW given −26.47 dBFS at 94 dB SPL.

<!-- END generated build -->

## Results

One table per signal path; tables do not compare with each other. A row is one quantity on that
path's signal, held to one limit from `tolerances.toml`, so every cell in it is read on the same
signal against the same limit. A cell is that comparison's worst value in the row and its
verdict, then in brackets how much of it the verdict stands on, wherever that is short of all:
the judged checks of all the cell's checks, and the worst check's columns that survived its
masks (mains lines, the noise gate, for the live TF also γ² ≥ 0.99) of all its band's columns.
"—": the tool does not measure it here; "not judged": every value sat at a noise floor or under
its own uncertainty. Within a check, a tone row is the signed difference at the tone. A band row
is max(|mean|, spread) of its column differences, the spread being the largest deviation from
their signed mean: not an RMS, nor the largest absolute column error. A band's phase is compared
after the pure delay left between its two sources is fitted away (over 1–20 kHz on the electrical paths,
200 Hz–5 kHz on the Genelec), so it judges the phase's shape; ac2's arrival is its own row. The
OSM fixtures' rows are the largest absolute difference over the columns; on the rig recordings
the median absolute difference, with its population and tails after it.
ac2 − REW at a tone is the difference of the two readings against
the same steady sine. REW imports the measurement and reference recordings, each against its
stimulus. "REW" is the measurement import with REW's soundcard and mic calibration, referred to
the interface's gain at 1 kHz (REW's cal file is normalised there). "REW ÷ ref" is REW's own division
of the two inputs (Arithmetic A / B), formed from imports without the soundcard calibration
(Notes). Both lose the path delay to REW's timing markers; it is put back from the direct
cross-spectrum of the same recording before the phase is judged (which restores REW's phase;
it does not check REW's own arrival). Harmonics are each input's own
against its own fundamental, in every tool: ac2 and REW separate them against the emitted sweep
(ac2 uses the reference only for the response and its timing), and the steady-sine truth reads
the measurement input alone.

<!-- BEGIN generated table: `python -m crosscheck comparison` from baselines/; do not edit by hand -->

**Xone (electrical), −10 dBFS.** Out of the interface, through the Xone:62 and back in. Truth: the steady sine at each tone, analysed in numpy, for the level, tone, group delay and harmonic rows; numpy's direct cross-spectrum of the same capture for the band rows and the arrival, for the live TF read through the TF's own windows and columns.

| quantity | ac2 vs truth | REW vs truth | REW ÷ ref vs truth | ac2 − REW | ac2 − REW ÷ ref | limit (pass / warn) |
|---|---|---|---|---|---|---|
| Reference level | −0.006 dB pass | — | — | −0.006 dB pass | — | 0.1 / 0.5 dB |
| Gain, measurement ÷ reference | — | — | — | −0.001 dB pass | — | 0.1 / 0.3 dB |
| Magnitude at the tones | +0.008 dB pass | +0.005 dB pass | +0.008 dB pass | −0.007 dB pass | −0.007 dB pass | 0.05 / 0.2 dB |
| Magnitude 20–100 Hz, sweeps | 0.038 dB pass (105 of 111 columns) | 0.004 dB pass (105 of 111 columns) | 0.009 dB pass (105 of 111 columns) | 0.004 dB pass (105 of 111 columns) | 0.008 dB pass (105 of 111 columns) | 0.05 / 0.2 dB |
| Magnitude 100 Hz–1 kHz, sweeps | 0.006 dB pass (127 of 159 columns) | 0.000 dB pass (127 of 159 columns) | 0.000 dB pass (127 of 159 columns) | 0.001 dB pass (127 of 159 columns) | 0.000 dB pass (127 of 159 columns) | 0.03 / 0.1 dB |
| Magnitude 1–20 kHz, sweeps | 0.010 dB pass (182 of 208 columns) | 0.002 dB pass (182 of 208 columns) | 0.002 dB pass (182 of 208 columns) | 0.009 dB pass (182 of 208 columns) | 0.009 dB pass (182 of 208 columns) | 0.03 / 0.1 dB |
| Magnitude 20–100 Hz, live TF | 0.007 dB pass (63 of 111 columns) | — | — | — | — | 0.1 / 0.3 dB |
| Magnitude 100 Hz–1 kHz, live TF | 0.005 dB pass (127 of 159 columns) | — | — | — | — | 0.1 / 0.3 dB |
| Magnitude 1–20 kHz, live TF | 0.003 dB pass (182 of 208 columns) | — | — | — | — | 0.1 / 0.3 dB |
| Phase at the tones | +0.04° pass | −0.11° pass | −0.07° pass | +0.13° pass | +0.09° pass | 1 / 3° |
| Phase 20 Hz–20 kHz | 0.24° pass (105 of 111 columns) | 0.09° pass (105 of 111 columns) | 0.07° pass (105 of 111 columns) | 0.11° pass (105 of 111 columns) | 0.09° pass (105 of 111 columns) | 1 / 3° |
| Group delay below 1 kHz | +2.3 % pass | +1.8 % pass | — | +3.3 % pass | — | 10 / 25 % |
| Group delay above 1 kHz | +0.01 µs pass | 0.00 µs pass | — | +0.01 µs pass | — | 0.5 / 2 µs |
| Arrival | +0.01 µs pass | — | — | — | — | 0.3 / 1 µs |
| Harmonics H2–H5 | +0.958 dB pass (18 of 150 judged) | +8.777 dB **FAIL** (9 of 30 judged) | — | −3.578 dB **warn** (18 of 150 judged) | — | 3 / 6 dB |

**Genelec (speaker at 54 cm), −30 dBFS.** The Genelec 1083 in the room, the MM1 on its tweeter axis. Truth: the steady sine at each tone, analysed in numpy, for the level, tone, group delay and harmonic rows; numpy's direct cross-spectrum of the same capture for the band rows and the arrival, for the live TF read through the TF's own windows and columns. SPL: the steady sine through the stored calibration.

| quantity | ac2 vs truth | REW vs truth | REW ÷ ref vs truth | ac2 − REW | ac2 − REW ÷ ref | limit (pass / warn) |
|---|---|---|---|---|---|---|
| Reference level | −0.004 dB pass | — | — | −0.004 dB pass | — | 0.1 / 0.5 dB |
| Gain, measurement ÷ reference | — | — | — | +0.004 dB pass | — | 0.1 / 0.3 dB |
| SPL at the tones | +0.551 dB **warn** (9 of 12 judged) | −0.080 dB pass (7 of 8 judged) | — | +0.485 dB pass | — | 0.5 / 1 dB |
| Magnitude at the tones | +0.172 dB pass (6 of 12 judged) | +0.154 dB pass (4 of 8 judged) | +0.154 dB pass (4 of 8 judged) | +0.049 dB pass | +0.049 dB pass | 0.3 / 1 dB |
| Magnitude 20–100 Hz, sweeps | 0.190 dB pass (38 of 111 columns) | 0.205 dB pass (35 of 111 columns) | 0.205 dB pass (35 of 111 columns) | 0.304 dB **warn** (36 of 111 columns) | 0.303 dB **warn** (36 of 111 columns) | 0.3 / 1 dB |
| Magnitude 100 Hz–1 kHz, sweeps | 0.036 dB pass (142 of 159 columns) | 0.060 dB pass (142 of 159 columns) | 0.060 dB pass (142 of 159 columns) | 0.066 dB pass (142 of 159 columns) | 0.066 dB pass (142 of 159 columns) | 0.3 / 1 dB |
| Magnitude 1–20 kHz, sweeps | 0.014 dB pass (192 of 208 columns) | 0.292 dB pass (192 of 208 columns) | 0.292 dB pass (192 of 208 columns) | 0.361 dB **warn** (192 of 208 columns) | 0.361 dB **warn** (192 of 208 columns) | 0.3 / 1 dB |
| Magnitude 20–100 Hz, live TF | not judged | — | — | — | — | — |
| Magnitude 100 Hz–1 kHz, live TF | 0.038 dB pass (25 of 159 columns) | — | — | — | — | 0.1 / 0.3 dB |
| Magnitude 1–20 kHz, live TF | not judged | — | — | — | — | — |
| Phase at the tones | +2.93° pass (8 of 12 judged) | +1.93° pass (4 of 8 judged) | +1.94° pass (4 of 8 judged) | +1.00° pass | +1.00° pass | 3 / 10° |
| Phase 20 Hz–20 kHz | 1.35° pass (10 of 12 judged; 54 of 111 columns) | 2.78° pass (35 of 111 columns) | 2.78° pass (35 of 111 columns) | 4.53° **warn** (192 of 208 columns) | 4.53° **warn** (192 of 208 columns) | 3 / 10° |
| Arrival | +0.30 µs pass | — | — | — | — | 2 / 10.4 µs |
| Harmonics H2–H5 | +2.660 dB pass (18 of 78 judged) | −2.132 dB pass (5 of 26 judged) | — | +0.509 dB pass (15 of 78 judged) | — | 3 / 6 dB |

**Ambient (silent room).** The room's own noise at the MM1, nothing played. Truth: numpy's Leq of the same recording with analytic weightings; for the RTA, ac2's own band filters applied to the recording.

| quantity | ac2 vs truth | REW vs truth | ac2 − REW | limit (pass / warn) |
|---|---|---|---|---|
| Leq, A-weighted | 0.000 dB pass | +0.026 dB pass | −0.025 dB pass | 0.5 / 1 dB |
| Leq, Z- and C-weighted | +0.004 dB pass | — | — | 0.5 / 1 dB |
| 1/3-octave RTA | +0.034 dB pass | — | — | 0.25 / 0.5 dB |

**Digital DUT, −10, −30 dBFS.** A software device with known distortion on a dummy JACK server, in four configurations; a cell is the worst over them and the levels. Truth: the device's exact harmonics, the steady sine at each tone, numpy's direct cross-spectrum of the same capture (the sweep's harmonic products cut) for the bands and the arrival.

| quantity | ac2 vs truth | limit (pass / warn) |
|---|---|---|
| Reference level | 0.000 dB pass | 0.1 / 0.5 dB |
| Magnitude at the tones | −0.005 dB pass | 0.05 / 0.2 dB |
| Magnitude 20–100 Hz, sweeps | 0.011 dB pass | 0.05 / 0.2 dB |
| Magnitude 100 Hz–1 kHz, sweeps | 0.001 dB pass | 0.03 / 0.1 dB |
| Magnitude 1–20 kHz, sweeps | 0.015 dB pass (199 of 208 columns) | 0.03 / 0.1 dB |
| Phase at the tones | −0.01° pass | 1 / 3° |
| Phase 20 Hz–20 kHz | 0.08° pass | 1 / 3° |
| Arrival | −0.03 µs pass | 0.3 / 1 µs |
| Harmonics H2–H5 vs exact | +0.178 dB pass | 0.5 / 1 dB |
| Harmonics H2–H5 vs the steady sine | +0.746 dB pass | 3 / 6 dB |

**OSM fixtures.** Synthetic WAV pairs (identity, a peaking biquad, two delays, a polarity flip, uncorrelated noise at three SNRs, a 1 kHz sine), analysed by ac2 and by OSM's DSP. Truth: each fixture's closed form; in noise, each estimator's expected value.

| quantity | ac2 vs truth | OSM vs truth | ac2 − OSM | limit (pass / warn) |
|---|---|---|---|---|
| Magnitude | 0.031 dB pass | 0.024 dB pass | 0.030 dB pass | 0.05 / 0.2 dB |
| Phase | 0.27° pass | 0.22° pass | 0.30° pass | 0.5 / 2° |
| Magnitude in noise, H1 vs 0 dB | −0.048 dB pass | — | — | 0.1 / 0.3 dB |
| Magnitude in noise, mean vs its bias | — | −0.009 dB pass | — | 0.2 / 0.5 dB |
| Coherence γ² | −0.0027 pass (8 of 11 judged) | −0.0052 pass | — | 0.02 / 0.05 |
| Delay, phase slope | 0.000 samples pass | 0.000 samples pass | — | 0.02 / 0.1 samples |
| Delay, ac2's sub-sample finder | 0.000 samples pass | — | — | 0.05 / 0.5 samples |
| Delay, OSM's whole-sample finder | — | −0.500 samples pass | — | 0.5 / 1 samples |
| Delay finder, ac2 against OSM | — | — | +0.500 samples pass | 0.6 / 1 samples |
| Spectrum level, 1 kHz sine | 0.000 dB pass | 0.000 dB pass | — | 0.02 / 0.1 dB |
| Spectrum level, white noise | +0.019 dB pass | +0.016 dB pass | — | 0.1 / 0.3 dB |

**OSM on the rig recordings.** ac2 and OSM's DSP on the Xone and Genelec recordings above. No truth: only ac2 − OSM, for magnitude and phase a median difference (their windows see a room differently).

| quantity | ac2 − OSM | limit (pass / warn) |
|---|---|---|
| Xone: magnitude, median | 0.000 dB pass; 412 columns, 95th percentile 0.002 dB, max 0.004 dB at 67.2 Hz | 0.3 / 1 dB |
| Xone: phase, median | 0.00° pass; 412 columns, 95th percentile 0.03°, max 0.12° at 60.7 Hz | 3 / 10° |
| Xone: delay finder | +0.485 samples pass | 0.6 / 1 samples |
| Genelec: magnitude, median | 0.114 dB pass; 378 columns, 95th percentile 0.608 dB, max 2.221 dB at 66.2 Hz | 0.3 / 1 dB |
| Genelec: phase, median | 0.70° pass; 378 columns, 95th percentile 3.33°, max 13.23° at 57.3 Hz | 3 / 10° |
| Genelec: delay finder | −0.987 samples **warn** | 0.6 / 1 samples |

<!-- END generated table -->

## Disparities

### Genelec: Magnitude 1–20 kHz, sweeps: ac2 − REW

ac2's sweep and REW's sweep are two takes in the room:
±0.361<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|REW offline.1000-20000 --> dB about a mean of
+0.049<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|REW offline.1000-20000 // mean --> dB over
192<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|REW offline.1000-20000 // n --> of
208<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|REW offline.1000-20000 // of --> columns. Two of
ac2's own sweeps in the same run differ by as much (sections below), and ac2's sweep against
numpy on its own capture stays within
0.012<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|direct (ac2 capture 20Hz-5.5s).1000-20000 --> dB, so
the spread lies between the takes, not in either tool's analysis. Since this baseline a second
speaker stands beside the Genelec: its cabinet reflects an arrival about 3 ms after the direct
sound (docs/rigs/pupu.md, "Genelec cold start"), whose comb puts nulls through the tweeter band.
A delayed path makes the response at the mic more sensitive to small changes in air and
position between takes. The working explanation, not a tested one.

### Genelec: Magnitude 1–20 kHz, sweeps: ac2 − REW ÷ ref

REW's own division reads the same,
±0.361<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|REW ÷ ref.1000-20000 --> dB: the same pair of
takes.

### Genelec: Phase 20 Hz–20 kHz: ac2 − REW

The tweeter band again: after a pure delay is removed, ac2's sweep and REW's differ by
±4.53<!--= b pupu/genelec-30dbfs // genelec.phase.ac2 sweep 20Hz-5.5s|REW offline.1000-20000 -->° about a mean of
−0.80<!--= b pupu/genelec-30dbfs // genelec.phase.ac2 sweep 20Hz-5.5s|REW offline.1000-20000 // mean -->°, while ac2's
sweep against numpy on its own capture stays within
0.113<!--= b pupu/genelec-30dbfs // genelec.phase.ac2 sweep 20Hz-5.5s|direct (ac2 capture 20Hz-5.5s).1000-20000 -->°. Below
1 kHz the two takes agree within
0.420<!--= b pupu/genelec-30dbfs // genelec.phase.ac2 sweep 20Hz-5.5s|REW offline.100-1000 -->°. The same take-to-take
change as the magnitude rows above, seen in phase.

### Genelec: Phase 20 Hz–20 kHz: ac2 − REW ÷ ref

The same pair of takes against REW's own division:
±4.53<!--= b pupu/genelec-30dbfs // genelec.phase.ac2 sweep 20Hz-5.5s|REW ÷ ref.1000-20000 -->°.

### Genelec: mag.ac2 sweep 20Hz-5.5s|direct (REW recording).1000-20000

ac2's sweep against numpy on REW's recording, the other take:
0.320<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|direct (REW recording).1000-20000 --> dB. With
REW's analysis taken out, the difference stays: it is between the captures. A diagnostic between
takes, which is why no table row shows it.

### Genelec: phase.ac2 sweep 20Hz-5.5s|direct (REW recording).1000-20000

The same diagnostic in phase:
3.61<!--= b pupu/genelec-30dbfs // genelec.phase.ac2 sweep 20Hz-5.5s|direct (REW recording).1000-20000 -->°.

### Genelec: mag.ac2 sweep dist|ac2 sweep 20Hz-5.5s.1000-20000

Two of ac2's sweeps, separate takes in the same run, differ in the tweeter band by
0.377<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep dist|ac2 sweep 20Hz-5.5s.1000-20000 --> dB about a mean of
+0.045<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep dist|ac2 sweep 20Hz-5.5s.1000-20000 // mean --> dB. Each
against numpy on its own capture stays within
0.014<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep dist|direct (ac2 capture dist).1000-20000 --> dB and
0.012<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|direct (ac2 capture 20Hz-5.5s).1000-20000 --> dB, so
all but those hundredths lies between the two captures: the room or the speaker changed between
the takes, not the analysis.

### Genelec: mag.ac2 sweep dist-probe|ac2 sweep 20Hz-5.5s.1000-20000

A third take against the same sweep:
0.412<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep dist-probe|ac2 sweep 20Hz-5.5s.1000-20000 --> dB about a mean of
+0.016<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep dist-probe|ac2 sweep 20Hz-5.5s.1000-20000 // mean --> dB,
as the section above.

### Genelec: phase.ac2 sweep dist|ac2 sweep 20Hz-5.5s.1000-20000

The two takes above in phase:
5.19<!--= b pupu/genelec-30dbfs // genelec.phase.ac2 sweep dist|ac2 sweep 20Hz-5.5s.1000-20000 -->°, while each against
its own capture stays within
0.107<!--= b pupu/genelec-30dbfs // genelec.phase.ac2 sweep dist|direct (ac2 capture dist).1000-20000 -->°.

### Genelec: phase.ac2 sweep dist-probe|ac2 sweep 20Hz-5.5s.1000-20000

The third take in phase:
3.07<!--= b pupu/genelec-30dbfs // genelec.phase.ac2 sweep dist-probe|ac2 sweep 20Hz-5.5s.1000-20000 -->°.

### Genelec: Magnitude 20–100 Hz, sweeps: ac2 − REW

ac2's sweep and REW's sweep are two takes in the room:
±0.304<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|REW offline.20-100 --> dB about a mean of
+0.023<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|REW offline.20-100 // mean --> dB over the
36<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|REW offline.20-100 // n --> of
111<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|REW offline.20-100 // of --> columns whose
noise leaves them judgeable. Each tool against numpy on its own recording is tighter:
0.053<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|direct (ac2 capture 20Hz-5.5s).20-100 --> dB for this sweep, and
0.205<!--= b pupu/genelec-30dbfs // genelec.mag.REW offline|direct (REW recording).20-100 --> dB for REW. A spread
of a difference is at most the sum of the spreads it is made of, so the two analyses' own
residuals account for at most their sum and the rest lies between the two captures; those
residuals' populations differ, so the split is a bound, not a decomposition. Two takes of a
noise-limited band: REW's take is a single sweep at −30 dBFS, and below 100 Hz the room's noise
sits close under it.

### Genelec: Magnitude 20–100 Hz, sweeps: ac2 − REW ÷ ref

The same pair of takes:
±0.303<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|REW ÷ ref.20-100 --> dB against REW's own division.

### Genelec: SPL at the tones: ac2 vs truth

At the 500 Hz tone the live TF, times the reference's level, reads
+0.551<!--= b pupu/genelec-30dbfs // genelec.spl.ac2 TF × ref level@500Hz --> dB above the steady sine, just past the
pass limit, while ac2's sweep reads
+0.158<!--= b pupu/genelec-30dbfs // genelec.spl.ac2 sweep@500Hz --> dB and REW
+0.066<!--= b pupu/genelec-30dbfs // genelec.spl.REW (SPL, cal from ac2)@500Hz --> dB at the same tone. The TF
runs last in the stage, on pink noise, a separate take from the sine, and a steady tone reads
the room's fine structure at one frequency, which the reflection added since the last baseline
sharpens. A take difference is the likely reading, not an established one.

### Xone: Harmonics H2–H5: REW vs truth

H2 and H3 of the tone near 60 Hz, about a hundred decibels below the fundamental: REW reads H2
+8.78<!--= b pupu/xone-10dbfs // xone.h2.REW offline import@60.5Hz --> dB and H3
+4.14<!--= b pupu/xone-10dbfs // xone.h3.REW offline import@60.5Hz --> dB above the steady sine. Input 5 carries hum
bursts that come and go every few seconds (docs/rigs/pupu.md, "Low-frequency harmonics on
electrical paths"); a sweep that passes a harmonic's frequency during a burst reads the burst as
distortion. ac2 reads the noise beside each harmonic window as well and reports a floor bound
there; REW has no such check. In the earlier takes of the day these cells were INCONCLUSIVE or
passed: they move with where the bursts fall. A REW reading, not an ac2 one.

### Xone: Harmonics H2–H5: ac2 − REW

The same cell between the tools: ac2's dist-probe sweep reads H3 of the tone near 60 Hz
−3.58<!--= b pupu/xone-10dbfs // xone.h3.ac2 sweep dist-probe|REW offline import@60.5Hz --> dB from REW, and
+0.561<!--= b pupu/xone-10dbfs // xone.h3.ac2 sweep dist-probe@60.5Hz --> dB from the steady sine: the difference is
REW's burst reading of the section above.

### OSM on rig recordings: Genelec: delay finder: ac2 − OSM

On the Genelec recording ac2's finder and OSM's differ by
−0.987<!--= b host/osm // osm.genelec-rig.ac2_delay_finder_vs_osm_s --> samples, against a
limit of a rounding plus ac2's stated tenth of a sample. ac2's finder interpolates the peak
between samples; OSM's takes the strongest whole sample. A speaker's impulse in a room is not
symmetric about its peak, so its strongest sample need not be the one nearest the interpolated
peak. With each finder's delay
inserted, ac2 and OSM agree to a median
0.114<!--= b host/osm // osm.genelec-rig.ac2_vs_osm_h --> dB and
0.70<!--= b host/osm // osm.genelec-rig.ac2_vs_osm_phase -->°. Those medians do not bound the
delay's effect at single frequencies, and no replay with the two delays swapped has been run.
The two peak definitions are the likely explanation; comparing them on the same impulse would
settle it. Open, at the edge of its limit.

## Notes

### REW's A / B of calibrated imports

REW 5.40's Arithmetic A / B of two imports that both carry the soundcard calibration reads the
right magnitude but subtracts the calibration's phase twice more, although the calibration
divides out of the quotient exactly (README, Calibration). The suite therefore forms "REW ÷
ref" from a second pair of imports without the soundcard calibration. REW's mic calibration
dialog also shows a "Full scale SPL" a sine-to-square-wave factor above 94 minus its "dBFS at
94 dB SPL"; the suite sets the latter, which REW's SPL readings follow, and its SPL rows agree
with ac2's.

### Genelec: the live TF in the tweeter band

ac2's live TF drives the speaker with continuous pink noise; the sweeps and sines are short or
narrow. In the rig runs the pink-noise recording read lower than the sweep recordings in the
Genelec's tweeter band, by about a decibel, settling within the first seconds of the noise:
the recording's first second matches the sweep, and the Xone path, driven by the same
generator and read through the same loopback, does not show it. ac2's TF agrees with numpy on
its own recording, so the difference is the speaker under that stimulus, not ac2's math; its
mechanism (driver protection or compression) is not established. That change enters the tone
rows: each take is held to the sine with its own path difference counted in its uncertainty.
The band rows are a separate check, the TF against numpy on its own capture through its own
kernel, and there the speaker's change cancels; what limits them is noise. The live TF is
judged on 100 Hz–1 kHz only: in its other two bands every column at γ² ≥ 0.99 carries more
noise (two standard deviations, from its coherence over eight blocks) than the pass limit, so
too few columns are left to judge.

## Reproducing

How to run each stage, refresh the baselines and rebuild this page
(`python -m crosscheck comparison`, `--check` to verify): [README.md](README.md),
"Reproducing the results" and "Baselines and compare".
