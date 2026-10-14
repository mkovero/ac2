# ac2 against REW and Open Sound Meter

ac2, REW 5.40 and Open Sound Meter (OSM) v1.5.2 measured the same signals on one ac2 build. On
the pupu rig (RME Fireface 400 at 96 kHz): an electrical path through an Allen & Heath Xone:62
mixer, a Genelec 1083 speaker in a room with a beyerdynamic MM1 measurement microphone at
56 cm on the Genelec's tweeter axis, 90° grazing, and the same room's ambient noise. On the dev
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

ac2 build **7065549**, runs of 2026-10-14: Xone (electrical) at −10 dBFS; Genelec (speaker) at −30 dBFS; ambient; digital DUT at −10, −30 dBFS; OSM fixtures and rig recordings. SPL calibration: MM1 34804 on input 1, 0 dBFS = 120.47 dB SPL, including its loading (330 Ω into 2000 Ω, −1.33 dB); REW given −26.47 dBFS at 94 dB SPL.

<!-- END generated build -->

## Results

One table per signal path; tables do not compare with each other. A row is one quantity on that
path's signal, held to one limit from `tolerances.toml`, so every cell in it is read on the same
signal against the same limit. A cell is that comparison's worst value in the row and its
verdict. "—": the tool does not measure it here; "not judged": every value sat at a noise floor
or under its own uncertainty. ac2 − REW at a tone is the difference of the two readings against
the same steady sine. REW imports the measurement and reference recordings, each against its
stimulus. "REW" is the measurement import with REW's soundcard and mic calibration, referred to
the interface's gain at 1 kHz (REW's cal file is normalised there). "REW ÷ ref" is REW's own division
of the two inputs (Arithmetic A / B), formed from imports without the soundcard calibration
(Notes). Both lose the path delay to REW's timing markers; it is put back from the direct
cross-spectrum of the same recording before the phase is judged. Harmonics are each input's own
against its own fundamental, in every tool: ac2 and REW separate them against the emitted sweep
(ac2 uses the reference only for the response and its timing), and the steady-sine truth reads
the measurement input alone.

<!-- BEGIN generated table: `python -m crosscheck comparison` from baselines/; do not edit by hand -->

**Xone (electrical), −10 dBFS.** Out of the interface, through the Xone:62 and back in. Truth: the steady sine at each tone, analysed in numpy, for the level, tone, group delay and harmonic rows; numpy's direct cross-spectrum of the same capture for the band rows and the arrival, for the live TF read through the TF's own windows and columns.

| quantity | ac2 vs truth | REW vs truth | REW ÷ ref vs truth | ac2 − REW | ac2 − REW ÷ ref | limit (pass / warn) |
|---|---|---|---|---|---|---|
| Reference level | −0.006 dB pass | — | — | −0.006 dB pass | — | 0.1 / 0.5 dB |
| Gain, measurement ÷ reference | — | — | — | −0.001 dB pass | — | 0.1 / 0.3 dB |
| Magnitude at the tones | +0.008 dB pass | +0.003 dB pass | +0.008 dB pass | −0.007 dB pass | −0.007 dB pass | 0.05 / 0.2 dB |
| Magnitude 20–100 Hz, sweeps | 0.037 dB pass | 0.003 dB pass | 0.009 dB pass | 0.003 dB pass | 0.009 dB pass | 0.05 / 0.2 dB |
| Magnitude 100 Hz–1 kHz, sweeps | 0.006 dB pass | 0.000 dB pass | 0.000 dB pass | 0.001 dB pass | 0.000 dB pass | 0.03 / 0.1 dB |
| Magnitude 1–20 kHz, sweeps | 0.010 dB pass | 0.001 dB pass | 0.002 dB pass | 0.009 dB pass | 0.009 dB pass | 0.03 / 0.1 dB |
| Magnitude 20 Hz–20 kHz, live TF | 0.008 dB pass | — | — | — | — | 0.1 / 0.3 dB |
| Phase at the tones | −0.09° pass | −0.07° pass | −0.07° pass | +0.09° pass | +0.09° pass | 1 / 3° |
| Phase 20 Hz–20 kHz | 0.24° pass | 0.09° pass | 0.08° pass | 0.09° pass | 0.09° pass | 1 / 3° |
| Group delay below 1 kHz | +2.0 % pass | +2.4 % pass | — | +2.2 % pass | — | 10 / 25 % |
| Group delay above 1 kHz | 0.00 µs pass | 0.00 µs pass | — | 0.00 µs pass | — | 0.5 / 2 µs |
| Arrival | +0.01 µs pass | — | — | — | — | 0.3 / 1 µs |
| Harmonics H2–H5 | −1.878 dB pass | +1.416 dB pass | — | +1.558 dB pass | — | 3 / 6 dB |

**Genelec (speaker at 56 cm), −30 dBFS.** The Genelec 1083 in the room, the MM1 on its tweeter axis. Truth: the steady sine at each tone, analysed in numpy, for the level, tone, group delay and harmonic rows; numpy's direct cross-spectrum of the same capture for the band rows and the arrival, for the live TF read through the TF's own windows and columns. SPL: the steady sine through the stored calibration.

| quantity | ac2 vs truth | REW vs truth | REW ÷ ref vs truth | ac2 − REW | ac2 − REW ÷ ref | limit (pass / warn) |
|---|---|---|---|---|---|---|
| Reference level | −0.004 dB pass | — | — | −0.004 dB pass | — | 0.1 / 0.5 dB |
| Gain, measurement ÷ reference | — | — | — | +0.011 dB pass | — | 0.1 / 0.3 dB |
| SPL at the tones | +0.262 dB pass | −0.295 dB pass | — | +0.234 dB pass | — | 0.5 / 1 dB |
| Magnitude at the tones | +0.267 dB pass | +0.241 dB pass | +0.241 dB pass | +0.175 dB pass | +0.176 dB pass | 0.3 / 1 dB |
| Magnitude 20–100 Hz, sweeps | 0.191 dB pass | 0.402 dB **warn** | 0.402 dB **warn** | 0.588 dB **warn** | 0.588 dB **warn** | 0.3 / 1 dB |
| Magnitude 100 Hz–1 kHz, sweeps | 0.057 dB pass | 0.050 dB pass | 0.050 dB pass | 0.071 dB pass | 0.071 dB pass | 0.3 / 1 dB |
| Magnitude 1–20 kHz, sweeps | 0.012 dB pass | 0.118 dB pass | 0.118 dB pass | 0.273 dB pass | 0.273 dB pass | 0.3 / 1 dB |
| Magnitude 20 Hz–20 kHz, live TF | 0.034 dB pass | — | — | — | — | 0.1 / 0.3 dB |
| Phase at the tones | +2.67° pass | +2.15° pass | +2.15° pass | +1.50° pass | +1.50° pass | 3 / 10° |
| Phase 20 Hz–20 kHz | 1.42° pass | 2.68° pass | 2.67° pass | 2.11° pass | 2.11° pass | 3 / 10° |
| Arrival | −0.61 µs pass | — | — | — | — | 2 / 10.4 µs |
| Harmonics H2–H5 | +3.353 dB **warn** | −1.408 dB pass | — | +0.867 dB pass | — | 3 / 6 dB |

**Ambient (silent room).** The room's own noise at the MM1, nothing played. Truth: numpy's Leq of the same recording with analytic weightings; for the RTA, ac2's own band filters applied to the recording.

| quantity | ac2 vs truth | REW vs truth | ac2 − REW | limit (pass / warn) |
|---|---|---|---|---|
| Leq, A-weighted | −0.003 dB pass | +0.026 dB pass | −0.028 dB pass | 0.5 / 1 dB |
| Leq, Z- and C-weighted | +0.003 dB pass | — | — | 0.5 / 1 dB |
| 1/3-octave RTA | +0.032 dB pass | — | — | 0.25 / 0.5 dB |

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
| Harmonics H2–H5 vs exact | +0.168 dB pass | 0.5 / 1 dB |
| Harmonics H2–H5 vs the steady sine | +0.883 dB pass | 3 / 6 dB |

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
| Xone: delay finder | +0.485 samples pass | 0.6 / 1 samples |
| Genelec: magnitude | 0.121 dB pass | 0.3 / 1 dB |
| Genelec: phase | 0.69° pass | 3 / 10° |
| Genelec: delay finder | −0.603 samples **warn** | 0.6 / 1 samples |

<!-- END generated table -->

## Disparities

### Genelec: Magnitude 20–100 Hz, sweeps: REW vs truth

REW against numpy's cross-spectrum of the very same recording spreads
±0.402<!--= b pupu/genelec-30dbfs // genelec.mag.REW offline|direct (REW recording).20-100 --> dB about a mean of
−0.007<!--= b pupu/genelec-30dbfs // genelec.mag.REW offline|direct (REW recording).20-100 // mean --> dB over the
43<!--= b pupu/genelec-30dbfs // genelec.mag.REW offline|direct (REW recording).20-100 // n --> of
111<!--= b pupu/genelec-30dbfs // genelec.mag.REW offline|direct (REW recording).20-100 // of --> columns whose
noise leaves them judgeable. REW's take is a single sweep at −30 dBFS, and below 100 Hz the
room's noise sits close under it (most columns there are left out for it); with no level
offset, the spread is that noise as REW's deconvolution window and numpy's whole-record
average each see it. ac2's sweeps against their own
captures stay within
0.191<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep dist|direct (ac2 capture dist).20-100 --> dB on the same
band. In the day's first run (reports/rig-repeats.md) this row passed: it moves with the take.
Not a REW error; a noise-limited band.

### Genelec: Magnitude 20–100 Hz, sweeps: REW ÷ ref vs truth

REW's own division reads the same,
±0.402<!--= b pupu/genelec-30dbfs // genelec.mag.REW ÷ ref|direct (REW recording).20-100 --> dB: REW's soundcard
calibration and the recorded reference give the same answer, so the spread is not the
calibration.

### Genelec: Magnitude 20–100 Hz, sweeps: ac2 − REW

ac2's sweep and REW's sweep are two takes in the room:
±0.588<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|REW offline.20-100 --> dB about a mean of
+0.035<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|REW offline.20-100 // mean --> dB. ac2's sweep
against numpy on REW's recording, the other take, spreads
±0.779<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|direct (REW recording).20-100 --> dB, while each tool
against numpy on its own recording is far tighter (rows above). The difference is between
takes of a noise-limited band, not between the analyses.

### Genelec: Magnitude 20–100 Hz, sweeps: ac2 − REW ÷ ref

The same pair of takes:
±0.588<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep 20Hz-5.5s|REW ÷ ref.20-100 --> dB against REW's own division.

### Genelec: Harmonics H2–H5: ac2 vs truth

H2 of the 500 Hz tone: ac2's three sweeps read
+3.35<!--= b pupu/genelec-30dbfs // genelec.h2.ac2 sweep 20Hz-5.5s@500Hz --> dB,
+2.19<!--= b pupu/genelec-30dbfs // genelec.h2.ac2 sweep dist@500Hz --> dB and
+2.60<!--= b pupu/genelec-30dbfs // genelec.h2.ac2 sweep dist-probe@500Hz --> dB above the steady sine, agreeing with
each other within about a decibel. ac2 separates harmonics against its own synthesised sweep,
so the loopback's harmonics do not enter. REW cannot arbitrate here: its H2 at this tone sits
under its floor in this take. Earlier builds showed the same gap. The sine is a steady-state reading in the room, the sweep's harmonic is gated by its
window, so the room's response at 1 kHz enters the two differently; that is a candidate, not an
established cause. Open.

### OSM on rig recordings: Genelec: delay finder: ac2 − OSM

On the Genelec recording ac2's finder and OSM's differ by
−0.603<!--= b host/osm // osm.genelec-rig.ac2_delay_finder_vs_osm_s --> samples, against a
limit of a rounding plus ac2's stated tenth of a sample. ac2's finder interpolates the peak
between samples; OSM's takes the strongest whole sample. A speaker's impulse in a room is not
symmetric about its peak, so its strongest sample need not be the one nearest the interpolated
peak. With each finder's delay
inserted, ac2 and OSM agree to a median
0.121<!--= b host/osm // osm.genelec-rig.ac2_vs_osm_h --> dB and
0.69<!--= b host/osm // osm.genelec-rig.ac2_vs_osm_phase -->°, so the half-sample difference
costs neither tool anything. A definition difference at the edge of its limit, not an error.

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
mechanism (driver protection or compression) is not established. Each take is therefore held
to the sine with its own path difference counted in its uncertainty, which leaves the live-TF
row judged on its 100 Hz–1 kHz band only: below 100 Hz the TF's eight-block average is too
noisy in this room, and above 1 kHz the speaker's own change is larger than the limit.

## Reproducing

How to run each stage, refresh the baselines and rebuild this page
(`python -m crosscheck comparison`, `--check` to verify): [README.md](README.md),
"Reproducing the results" and "Baselines and compare".
