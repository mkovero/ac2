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

ac2 build **7065549**, runs of 2026-10-13: Xone (electrical) at −10 dBFS; Genelec (speaker) at −30 dBFS; ambient; digital DUT at −10, −30 dBFS; OSM fixtures and rig recordings. SPL calibration: MM1 34804 on input 1, 0 dBFS = 120.47 dB SPL, including its loading (330 Ω into 2000 Ω, −1.33 dB); REW given −26.47 dBFS at 94 dB SPL.

<!-- END generated build -->

## Results

One table per signal path; tables do not compare with each other. A row is one quantity on that
path's signal, held to one limit from `tolerances.toml`, so every cell in it is read on the same
signal against the same limit. A cell is that comparison's worst value in the row and its
verdict. "—": the tool does not measure it here; "not judged": every value sat at a noise floor
or under its own uncertainty. ac2 − REW at a tone is the difference of the two readings against
the same steady sine. REW imports the measurement and reference recordings separately, each
against its stimulus: "REW" is those two imports divided by the suite, "REW ÷ ref" REW's own
division of them (Arithmetic A / B). Both lose the path delay to REW's timing markers; it is put
back from the direct cross-spectrum of the same recording before the phase is judged. Harmonics
are each input's own against its own fundamental, in every tool: ac2 and REW separate them
against the emitted sweep (ac2 uses the reference only for the response and its timing), and
the steady-sine truth reads the measurement input alone.

<!-- BEGIN generated table: `python -m crosscheck comparison` from baselines/; do not edit by hand -->

**Xone (electrical), −10 dBFS.** Out of the interface, through the Xone:62 and back in. Truth: the steady sine at each tone, analysed in numpy, for the level, tone, group delay and harmonic rows; numpy's direct cross-spectrum of the same capture for the band rows and the arrival, for the live TF read through the TF's own windows and columns.

| quantity | ac2 vs truth | REW vs truth | REW ÷ ref vs truth | ac2 − REW | ac2 − REW ÷ ref | limit (pass / warn) |
|---|---|---|---|---|---|---|
| Reference level | −0.006 dB pass | — | — | −0.006 dB pass | — | 0.1 / 0.5 dB |
| Gain, measurement ÷ reference | — | — | — | −0.001 dB pass | — | 0.1 / 0.3 dB |
| Magnitude at the tones | +0.008 dB pass | +0.003 dB pass | +0.008 dB pass | −0.007 dB pass | −0.007 dB pass | 0.05 / 0.2 dB |
| Magnitude 20–100 Hz, sweeps | 0.038 dB pass | 0.003 dB pass | 0.009 dB pass | 0.003 dB pass | 0.009 dB pass | 0.05 / 0.2 dB |
| Magnitude 100 Hz–1 kHz, sweeps | 0.006 dB pass | 0.000 dB pass | 0.000 dB pass | 0.000 dB pass | 0.000 dB pass | 0.03 / 0.1 dB |
| Magnitude 1–20 kHz, sweeps | 0.010 dB pass | 0.002 dB pass | 0.002 dB pass | 0.009 dB pass | 0.009 dB pass | 0.03 / 0.1 dB |
| Magnitude 20 Hz–20 kHz, live TF | 0.011 dB pass | — | — | — | — | 0.1 / 0.3 dB |
| Phase at the tones | +0.02° pass | −0.06° pass | −0.07° pass | +0.09° pass | +0.09° pass | 1 / 3° |
| Phase 20 Hz–20 kHz | 0.23° pass | 0.08° pass | 0.07° pass | 0.10° pass | 0.09° pass | 1 / 3° |
| Group delay below 1 kHz | +1.7 % pass | +1.9 % pass | — | +1.9 % pass | — | 10 / 25 % |
| Group delay above 1 kHz | 0.00 µs pass | 0.00 µs pass | — | 0.00 µs pass | — | 0.5 / 2 µs |
| Arrival | +0.01 µs pass | — | — | — | — | 0.3 / 1 µs |
| Harmonics H2–H5 | +1.924 dB pass | +2.263 dB pass | — | +2.196 dB pass | — | 3 / 6 dB |

**Genelec (speaker at 56 cm), −30 dBFS.** The Genelec 1083 in the room, the MM1 on its tweeter axis. Truth: the steady sine at each tone, analysed in numpy, for the level, tone, group delay and harmonic rows; numpy's direct cross-spectrum of the same capture for the band rows and the arrival, for the live TF read through the TF's own windows and columns. SPL: the steady sine through the stored calibration.

| quantity | ac2 vs truth | REW vs truth | REW ÷ ref vs truth | ac2 − REW | ac2 − REW ÷ ref | limit (pass / warn) |
|---|---|---|---|---|---|---|
| Reference level | −0.005 dB pass | — | — | −0.005 dB pass | — | 0.1 / 0.5 dB |
| Gain, measurement ÷ reference | — | — | — | +0.003 dB pass | — | 0.1 / 0.3 dB |
| SPL at the tones | −0.227 dB pass | +0.185 dB pass | — | −0.412 dB pass | — | 0.5 / 1 dB |
| Magnitude at the tones | +0.206 dB pass | +0.195 dB pass | +0.195 dB pass | +0.134 dB pass | +0.134 dB pass | 0.3 / 1 dB |
| Magnitude 20–100 Hz, sweeps | 0.214 dB pass | 0.163 dB pass | 0.163 dB pass | 0.265 dB pass | 0.265 dB pass | 0.3 / 1 dB |
| Magnitude 100 Hz–1 kHz, sweeps | 0.025 dB pass | 0.058 dB pass | 0.058 dB pass | 0.080 dB pass | 0.080 dB pass | 0.3 / 1 dB |
| Magnitude 1–20 kHz, sweeps | 0.015 dB pass | 0.077 dB pass | 0.077 dB pass | 0.250 dB pass | 0.250 dB pass | 0.3 / 1 dB |
| Magnitude 20 Hz–20 kHz, live TF | 0.048 dB pass | — | — | — | — | 0.1 / 0.3 dB |
| Phase at the tones | +3.09° **warn** | +1.96° pass | +1.96° pass | +2.75° pass | +2.75° pass | 3 / 10° |
| Phase 20 Hz–20 kHz | 1.16° pass | 2.37° pass | 2.37° pass | 4.42° **warn** | 4.42° **warn** | 3 / 10° |
| Arrival | −0.20 µs pass | — | — | — | — | 2 / 10.4 µs |
| Harmonics H2–H5 | +3.294 dB **warn** | −1.309 dB pass | — | +1.784 dB pass | — | 3 / 6 dB |

**Ambient (silent room).** The room's own noise at the MM1, nothing played. Truth: numpy's Leq of the same recording with analytic weightings; for the RTA, ac2's own band filters applied to the recording.

| quantity | ac2 vs truth | REW vs truth | ac2 − REW | limit (pass / warn) |
|---|---|---|---|---|
| Leq, A-weighted | +0.002 dB pass | +0.013 dB pass | −0.011 dB pass | 0.5 / 1 dB |
| Leq, Z- and C-weighted | +0.003 dB pass | — | — | 0.5 / 1 dB |
| 1/3-octave RTA | −0.023 dB pass | — | — | 0.25 / 0.5 dB |

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
| Arrival | −0.04 µs pass | 0.3 / 1 µs |
| Harmonics H2–H5 vs exact | −0.240 dB pass | 0.5 / 1 dB |
| Harmonics H2–H5 vs the steady sine | +0.879 dB pass | 3 / 6 dB |

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

### Genelec: Phase 20 Hz–20 kHz: ac2 − REW

Above 1 kHz ac2's short sweep and REW's sweep differ by
+4.42 °<!--= b pupu/genelec-30dbfs // genelec.phase.ac2 sweep 20Hz-5.5s|REW offline.1000-20000 -->
after the pure delay is removed. They are two takes of the speaker: ac2 against numpy on its
own capture differs by
+0.09 °<!--= b pupu/genelec-30dbfs // genelec.phase.ac2 sweep 20Hz-5.5s|direct (ac2 capture 20Hz-5.5s).1000-20000 -->,
REW against numpy on its own capture by
+0.63 °<!--= b pupu/genelec-30dbfs // genelec.phase.REW offline|direct (REW recording).1000-20000 -->,
and ac2's own sweeps of the same run differ from each other by up to
+2.57 °<!--= b pupu/genelec-30dbfs // genelec.phase.ac2 sweep dist-probe|ac2 sweep 20Hz-5.5s.1000-20000 -->.
The Genelec's phase above 1 kHz changes from one take to the next by a few degrees; a second run on this build showed the same spread between ac2's own sweeps.
Neither tool's analysis is off; open as a property of the speaker path, not a tool difference.

### Genelec: Phase 20 Hz–20 kHz: ac2 − REW ÷ ref

The same pair of takes as the row above: REW's own division gives
+4.42 °<!--= b pupu/genelec-30dbfs // genelec.phase.ac2 sweep 20Hz-5.5s|REW ÷ ref.1000-20000 -->,
the same as the suite's division of REW's imports.

### Genelec: Phase at the tones: ac2 vs truth

ac2's live TF at 1 kHz reads
+3.09 °<!--= b pupu/genelec-30dbfs // genelec.sine_phase.ac2 TF@1000Hz --> from the steady
sine. The TF is its own take (continuous pink noise): numpy's direct cross-spectrum of the
TF's capture differs from the sine by
+2.38 °<!--= b pupu/genelec-30dbfs // genelec.sine_phase.direct (TF capture)@1000Hz -->, while
ac2's sweep, a take close to the sine, reads
+1.97 °<!--= b pupu/genelec-30dbfs // genelec.sine_phase.ac2 sweep 20Hz-5.5s@1000Hz -->. The
difference is between takes of the speaker, as in the phase rows above, with the tweeter-band
change under pink noise (Notes) on top.

### Genelec: Harmonics H2–H5: ac2 vs truth

H2 of the 500 Hz tone: ac2's three sweeps read
+3.29 dB<!--= b pupu/genelec-30dbfs // genelec.h2.ac2 sweep 20Hz-5.5s@500Hz -->,
+2.41 dB<!--= b pupu/genelec-30dbfs // genelec.h2.ac2 sweep dist@500Hz --> and
+2.32 dB<!--= b pupu/genelec-30dbfs // genelec.h2.ac2 sweep dist-probe@500Hz --> above the
steady sine, agreeing with each other within a decibel. The reference loopback's own H2 is far
below this reading, so how the reference is used does not enter. REW's import cannot arbitrate:
its H2 here sits under its floor. On the previous build this row went unjudged (a mains line in
the harmonic band of the noise recording) and the gap was already there. The sine is a
steady-state reading in the room, the sweep's harmonic is gated by its window, so the room's
response at 1 kHz enters the two differently; that is a candidate, not an established cause.
Open.

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
