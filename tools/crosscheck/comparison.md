# ac2 against REW and Open Sound Meter

ac2, REW 5.40 and Open Sound Meter (OSM) v1.5.2 measured the same signals on one ac2 build. On
the pupu rig (RME Fireface 400 at 96 kHz): an electrical path through an Allen & Heath Xone:62
mixer, a Genelec 1083 speaker in a room with a beyerdynamic MM1 measurement microphone at
56 cm on the Genelec's tweeter axis, 90° grazing, and the same room's ambient noise. On the dev
host: a software device with known distortion on a dummy JACK server, and OSM's DSP on synthetic
test signals and on the rig's recordings. All tools share ac2's stored calibration (below).
Each tool is held to a truth that depends on none of them: a steady sine analysed in numpy,
numpy's direct cross-spectrum of the same recording, closed-form test signals, and the software
device's exact harmonics. Not validated here: the physical calibration chain (microphone
sensitivity, calibrator) and IEC 61672-1 / 61260-1 class conformance.

**Calibration and mic loading.** A microphone's data-sheet sensitivity is open-circuit. Its
source impedance into the input's impedance is a voltage divider, so the input sees less than
the data sheet promises (the values are in the build line below). ac2's electrical calibration
measures the volts at the input, so its stored sensitivity already includes the divider, and the
store records both impedances beside it. REW has no setting for loading: it takes one "dBFS at
94 dB SPL", and is given 94 minus ac2's loaded sensitivity. Given the data-sheet value instead,
REW would read every SPL low against ac2 by the divider; with the loaded value, loading never
shows as a difference between the tools. OSM gets no SPL calibration here: its comparisons are
relative (transfer functions, spectra in dBFS). The suite's preflight refuses a run whose stored
sensitivity disagrees with the data sheet once the divider is applied.

<!-- BEGIN generated build: `python -m crosscheck comparison` from baselines/; do not edit by hand -->

ac2 build **438048f**, runs of 2026-10-13: Xone (electrical) at −10 dBFS; Genelec (speaker) at −30 dBFS; ambient; digital DUT at −10, −30 dBFS; OSM fixtures and rig recordings. SPL calibration: MM1 34804 on input 1, 0 dBFS = 120.47 dB SPL, including its loading (330 Ω into 2000 Ω, −1.33 dB); REW given −26.47 dBFS at 94 dB SPL.

<!-- END generated build -->

## Results

One table per signal path; tables do not compare with each other. A row is one quantity on that
path's signal, held to one limit from `tolerances.toml`, so every cell in it is read on the same
signal against the same limit. A cell is that comparison's worst value in the row and its
verdict. "—": the tool does not measure it here; "not judged": every value sat at a noise floor
or under its own uncertainty. ac2 − REW at a tone is the difference of the two readings against
the same steady sine.

<!-- BEGIN generated table: `python -m crosscheck comparison` from baselines/; do not edit by hand -->

**Xone (electrical), −10 dBFS.** Out of the interface, through the Xone:62 and back in. Truth: the steady sine at each tone, analysed in numpy, for the level, tone, group delay and harmonic rows; numpy's direct cross-spectrum of the same capture for the band rows and the arrival, for the live TF read through the TF's own windows and columns.

| quantity | ac2 vs truth | REW vs truth | ac2 − REW | limit (pass / warn) |
|---|---|---|---|---|
| Reference level | −0.006 dB pass | — | −0.006 dB pass | 0.1 / 0.5 dB |
| Gain, measurement ÷ reference | — | — | −0.001 dB pass | 0.1 / 0.3 dB |
| Magnitude at the tones | +0.008 dB pass | +0.003 dB pass | −0.007 dB pass | 0.05 / 0.2 dB |
| Magnitude 20–100 Hz, sweeps | 0.038 dB pass | 0.003 dB pass | 0.003 dB pass | 0.05 / 0.2 dB |
| Magnitude 100 Hz–1 kHz, sweeps | 0.006 dB pass | 0.000 dB pass | 0.000 dB pass | 0.03 / 0.1 dB |
| Magnitude 1–20 kHz, sweeps | 0.010 dB pass | 0.002 dB pass | 0.009 dB pass | 0.03 / 0.1 dB |
| Magnitude 20 Hz–20 kHz, live TF | 0.014 dB pass | — | — | 0.1 / 0.3 dB |
| Phase at the tones | −0.03° pass | −0.07° pass | +0.09° pass | 1 / 3° |
| Phase 20 Hz–20 kHz | 0.24° pass | 0.09° pass | 0.10° pass | 1 / 3° |
| Group delay below 1 kHz | +2.1 % pass | +1.7 % pass | +2.2 % pass | 10 / 25 % |
| Group delay above 1 kHz | +0.02 µs pass | +0.01 µs pass | +0.01 µs pass | 0.5 / 2 µs |
| Arrival | +0.01 µs pass | — | — | 0.3 / 1 µs |
| Harmonics H2–H5 | +0.682 dB pass | −0.723 dB pass | +4.177 dB **warn** | 3 / 6 dB |

**Genelec (speaker at 56 cm), −30 dBFS.** The Genelec 1083 in the room, the MM1 on its tweeter axis. Truth: the steady sine at each tone, analysed in numpy, for the level, tone, group delay and harmonic rows; numpy's direct cross-spectrum of the same capture for the band rows and the arrival, for the live TF read through the TF's own windows and columns. SPL: the steady sine through the stored calibration.

| quantity | ac2 vs truth | REW vs truth | ac2 − REW | limit (pass / warn) |
|---|---|---|---|---|
| Reference level | −0.004 dB pass | — | −0.004 dB pass | 0.1 / 0.5 dB |
| Gain, measurement ÷ reference | — | — | 0.000 dB pass | 0.1 / 0.3 dB |
| SPL at the tones | −0.209 dB pass | +0.122 dB pass | −0.330 dB pass | 0.5 / 1 dB |
| Magnitude at the tones | +0.168 dB pass | +0.170 dB pass | +0.184 dB pass | 0.3 / 1 dB |
| Magnitude 20–100 Hz, sweeps | 0.114 dB pass | 0.198 dB pass | 0.165 dB pass | 0.3 / 1 dB |
| Magnitude 100 Hz–1 kHz, sweeps | 0.126 dB pass | 0.051 dB pass | 0.098 dB pass | 0.3 / 1 dB |
| Magnitude 1–20 kHz, sweeps | 0.013 dB pass | 0.084 dB pass | 0.192 dB pass | 0.3 / 1 dB |
| Magnitude 20 Hz–20 kHz, live TF | 0.040 dB pass | — | — | 0.1 / 0.3 dB |
| Phase at the tones | +2.71° pass | +2.07° pass | +1.19° pass | 3 / 10° |
| Phase 20 Hz–20 kHz | 1.16° pass | 2.66° pass | 2.16° pass | 3 / 10° |
| Arrival | +0.17 µs pass | — | — | 2 / 10.4 µs |
| Harmonics H2–H5 | −1.853 dB pass | −1.775 dB pass | +2.057 dB pass | 3 / 6 dB |

**Ambient (silent room).** The room's own noise at the MM1, nothing played. Truth: numpy's Leq of the same recording with analytic weightings; for the RTA, ac2's own band filters applied to the recording.

| quantity | ac2 vs truth | REW vs truth | ac2 − REW | limit (pass / warn) |
|---|---|---|---|---|
| Leq, A-weighted | +0.002 dB pass | +0.016 dB pass | −0.014 dB pass | 0.5 / 1 dB |
| Leq, Z- and C-weighted | +0.005 dB pass | — | — | 0.5 / 1 dB |
| 1/3-octave RTA | −0.019 dB pass | — | — | 0.25 / 0.5 dB |

**Digital DUT, −10, −30 dBFS.** A software device with known distortion on a dummy JACK server, in four configurations; a cell is the worst over them and the levels. Truth: the device's exact harmonics, the steady sine at each tone, numpy's direct cross-spectrum of the same capture (the sweep's harmonic products cut) for the bands and the arrival.

| quantity | ac2 vs truth | limit (pass / warn) |
|---|---|---|
| Reference level | 0.000 dB pass | 0.1 / 0.5 dB |
| Magnitude at the tones | −0.005 dB pass | 0.05 / 0.2 dB |
| Magnitude 20–100 Hz, sweeps | 0.011 dB pass | 0.05 / 0.2 dB |
| Magnitude 100 Hz–1 kHz, sweeps | 0.001 dB pass | 0.03 / 0.1 dB |
| Magnitude 1–20 kHz, sweeps | 0.015 dB pass | 0.03 / 0.1 dB |
| Phase at the tones | −0.01° pass | 1 / 3° |
| Phase 20 Hz–20 kHz | 0.08° pass | 1 / 3° |
| Arrival | −0.05 µs pass | 0.3 / 1 µs |
| Harmonics H2–H5 vs exact | −0.252 dB pass | 0.5 / 1 dB |
| Harmonics H2–H5 vs the steady sine | +0.648 dB pass | 3 / 6 dB |

**OSM fixtures.** Synthetic WAV pairs (identity, a peaking biquad, two delays, a polarity flip, uncorrelated noise at three SNRs, a 1 kHz sine), analysed by ac2 and by OSM's DSP. Truth: each fixture's closed form; in noise, each estimator's expected value.

| quantity | ac2 vs truth | OSM vs truth | ac2 − OSM | limit (pass / warn) |
|---|---|---|---|---|
| Magnitude | 0.031 dB pass | 0.024 dB pass | 0.030 dB pass | 0.05 / 0.2 dB |
| Phase | 0.27° pass | 0.22° pass | 0.30° pass | 0.5 / 2° |
| Magnitude in noise, H1 vs 0 dB | −0.048 dB pass | — | — | 0.1 / 0.3 dB |
| Magnitude in noise, mean vs its bias | — | −0.009 dB pass | — | 0.2 / 0.5 dB |
| Coherence γ² | −0.0027 pass | −0.0052 pass | — | 0.02 / 0.05 |
| Delay, phase slope | 0.000 samples pass | 0.000 samples pass | — | 0.02 / 0.1 samples |
| Delay, ac2's sub-sample finder | 0.000 samples pass | — | — | 0.05 / 0.5 samples |
| Delay, OSM's whole-sample finder | — | −0.500 samples pass | — | 0.5 / 1 samples |
| Delay finder, ac2 against OSM | — | — | +0.500 samples pass | 0.6 / 1 samples |
| Spectrum level, 1 kHz sine | 0.000 dB pass | 0.000 dB pass | — | 0.02 / 0.1 dB |
| Spectrum level, white noise | +0.019 dB pass | +0.016 dB pass | — | 0.1 / 0.3 dB |

**OSM on the rig recordings.** ac2 and OSM's DSP on the Xone and Genelec recordings above. No truth: only ac2 − OSM, for magnitude and phase a median difference (their windows see a room differently).

| quantity | ac2 − OSM | limit (pass / warn) |
|---|---|---|
| Xone: magnitude | 0.000 dB pass | 0.3 / 1 dB |
| Xone: phase | 0.00° pass | 3 / 10° |
| Xone: delay finder | +0.502 samples pass | 0.6 / 1 samples |
| Genelec: magnitude | 0.144 dB pass | 0.3 / 1 dB |
| Genelec: phase | 0.80° pass | 3 / 10° |
| Genelec: delay finder | −0.551 samples pass | 0.6 / 1 samples |

<!-- END generated table -->

## Disparities

### Xone: Harmonics H2–H5: ac2 − REW

H2 of the 1040 Hz tone: the ac2 sweeps read
+2.82 dB<!--= b pupu/xone-10dbfs // xone.h2.ac2 sweep 3Hz-5.5s|REW offline import@1040Hz --> to
+4.18 dB<!--= b pupu/xone-10dbfs // xone.h2.ac2 sweep dist-probe|REW offline import@1040Hz -->
more than REW. Against the steady sine both pass: ac2's sweeps by
−0.67 dB<!--= b pupu/xone-10dbfs // xone.h2.ac2 sweep 3Hz-5.5s@1040Hz --> to
+0.68 dB<!--= b pupu/xone-10dbfs // xone.h2.ac2 sweep dist-probe@1040Hz -->, REW by
−0.72 dB<!--= b pupu/xone-10dbfs // xone.h2.REW offline import@1040Hz -->. Neither tool is
off: ac2 divides by the measured reference, REW's export is the measurement input alone, and
each is held to the sine on its own definition. The reference loopback's own H2 at this tone
accounts for most of the gap, the rest is within each tool's limit against the sine. The
ac2 − REW check compares the two definitions directly; closed as a convention difference.

## Notes

### Genelec: the live TF in the tweeter band

ac2's live TF drives the speaker with continuous pink noise; the sweeps and sines are short or
narrow. In both of the day's rig runs the pink-noise recording read lower than the sweep
recordings in the Genelec's tweeter band, by about a decibel, settling within the first seconds
of the noise: the recording's first second matches the sweep, and the Xone path, driven by the
same generator and read through the same loopback, does not show it. ac2's TF agrees with numpy
on its own recording, so the difference is the speaker under that stimulus, not ac2's math; its
mechanism (driver protection or compression) is not established. Each take is therefore held
to the sine with its own path difference counted in its uncertainty, which leaves the live-TF
row judged on its 100 Hz–1 kHz band only: below 100 Hz the TF's eight-block average is too
noisy in this room, and above 1 kHz the speaker's own change is larger than the limit.

## Reproducing

How to run each stage, refresh the baselines and rebuild this page
(`python -m crosscheck comparison`, `--check` to verify): [README.md](README.md),
"Reproducing the results" and "Baselines and compare".
