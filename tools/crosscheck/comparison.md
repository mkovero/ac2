# ac2 against REW and Open Sound Meter

ac2, REW 5.40 and Open Sound Meter (OSM) v1.5.2 measured the same signals on one ac2 build. On
the pupu rig (RME Fireface 400 at 96 kHz): an electrical path through an Allen & Heath Xone:62
mixer, a Genelec 1083 speaker in a room with a beyerdynamic MM1 measurement microphone at
56 cm on the Genelec's tweeter axis, 90° grazing, and the same room's ambient noise. On the dev
host: a software device with known distortion on a dummy JACK server, and OSM's DSP on synthetic
test signals and on the rig's recordings. All tools share ac2's stored calibration, which
includes the mic's loading into the input (−1.33 dB); REW is set to the same dBFS at 94 dB SPL.
Each tool is held to a truth that depends on none of them: a steady sine analysed in numpy,
numpy's direct cross-spectrum of the same recording, closed-form test signals, and the software
device's exact harmonics. Not validated here: the physical calibration chain (microphone
sensitivity, calibrator) and IEC 61672-1 / 61260-1 class conformance.

<!-- BEGIN generated build: `python -m crosscheck comparison` from baselines/; do not edit by hand -->

ac2 build **7e863a5**, runs of 2026-10-13: Xone (electrical) at −10 dBFS; Genelec (speaker) at −30 dBFS; ambient; digital DUT at −10, −30 dBFS; OSM fixtures and rig recordings.

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
| Magnitude 20–100 Hz, sweeps | 0.006 dB pass | 0.003 dB pass | 0.003 dB pass | 0.05 / 0.2 dB |
| Magnitude 100 Hz–1 kHz, sweeps | 0.000 dB pass | 0.000 dB pass | 0.000 dB pass | 0.03 / 0.1 dB |
| Magnitude 1–20 kHz, sweeps | 0.010 dB pass | 0.002 dB pass | 0.009 dB pass | 0.03 / 0.1 dB |
| Magnitude 20 Hz–20 kHz, live TF | 0.003 dB pass | — | — | 0.1 / 0.3 dB |
| Phase at the tones | +0.02° pass | −0.06° pass | +0.09° pass | 1 / 3° |
| Phase 20 Hz–20 kHz | 0.05° pass | 0.09° pass | 0.10° pass | 1 / 3° |
| Group delay below 1 kHz | +2.1 % pass | +1.7 % pass | +1.9 % pass | 10 / 25 % |
| Group delay above 1 kHz | +0.01 µs pass | 0.00 µs pass | 0.00 µs pass | 0.5 / 2 µs |
| Arrival | +0.01 µs pass | — | — | 0.3 / 1 µs |
| Harmonics H2–H5 | −0.959 dB pass | −1.630 dB pass | +4.054 dB **warn** | 3 / 6 dB |

**Genelec (speaker at 56 cm), −30 dBFS.** The Genelec 1083 in the room, the MM1 on its tweeter axis. Truth: the steady sine at each tone, analysed in numpy, for the level, tone, group delay and harmonic rows; numpy's direct cross-spectrum of the same capture for the band rows and the arrival, for the live TF read through the TF's own windows and columns. SPL: the steady sine through the stored calibration.

| quantity | ac2 vs truth | REW vs truth | ac2 − REW | limit (pass / warn) |
|---|---|---|---|---|
| Reference level | −0.004 dB pass | — | −0.004 dB pass | 0.1 / 0.5 dB |
| Gain, measurement ÷ reference | — | — | −0.005 dB pass | 0.1 / 0.3 dB |
| SPL at the tones | −0.396 dB pass | +0.193 dB pass | −0.339 dB pass | 0.5 / 1 dB |
| Magnitude at the tones | +0.071 dB pass | −0.058 dB pass | +0.117 dB pass | 0.3 / 1 dB |
| Magnitude 20–100 Hz, sweeps | 0.122 dB pass | 0.199 dB pass | 0.244 dB pass | 0.3 / 1 dB |
| Magnitude 100 Hz–1 kHz, sweeps | 0.051 dB pass | 0.055 dB pass | 0.114 dB pass | 0.3 / 1 dB |
| Magnitude 1–20 kHz, sweeps | 0.014 dB pass | 0.061 dB pass | 0.118 dB pass | 0.3 / 1 dB |
| Magnitude 20 Hz–20 kHz, live TF | 0.093 dB pass | — | — | 0.1 / 0.3 dB |
| Phase at the tones | +2.19° pass | +1.74° pass | +0.85° pass | 3 / 10° |
| Phase 20 Hz–20 kHz | 1.34° pass | 2.49° pass | 2.45° pass | 3 / 10° |
| Arrival | −0.66 µs pass | — | — | 2 / 10.4 µs |
| Harmonics H2–H5 | −1.835 dB pass | −1.776 dB pass | +0.956 dB pass | 3 / 6 dB |

**Ambient (silent room).** The room's own noise at the MM1, nothing played. Truth: numpy's Leq of the same recording with analytic weightings; for the RTA, ac2's own band filters applied to the recording.

| quantity | ac2 vs truth | REW vs truth | ac2 − REW | limit (pass / warn) |
|---|---|---|---|---|
| Leq, A-weighted | +0.004 dB pass | +0.035 dB pass | −0.031 dB pass | 0.5 / 1 dB |
| Leq, Z- and C-weighted | +0.008 dB pass | — | — | 0.5 / 1 dB |
| 1/3-octave RTA | −0.040 dB pass | — | — | 0.25 / 0.5 dB |

**Digital DUT, −10, −30 dBFS.** A software device with known distortion on a dummy JACK server, in four configurations; a cell is the worst over them and the levels. Truth: the device's exact harmonics, the steady sine at each tone, numpy's direct cross-spectrum of the same capture for the bands and the arrival.

| quantity | ac2 vs truth | limit (pass / warn) |
|---|---|---|
| Reference level | 0.000 dB pass | 0.1 / 0.5 dB |
| Magnitude at the tones | −0.005 dB pass | 0.05 / 0.2 dB |
| Magnitude 20–100 Hz, sweeps | 0.111 dB **warn** | 0.05 / 0.2 dB |
| Magnitude 100 Hz–1 kHz, sweeps | 0.034 dB **warn** | 0.03 / 0.1 dB |
| Magnitude 1–20 kHz, sweeps | 0.028 dB pass | 0.03 / 0.1 dB |
| Phase at the tones | −0.01° pass | 1 / 3° |
| Phase 20 Hz–20 kHz | 0.83° pass | 1 / 3° |
| Arrival | −0.03 µs pass | 0.3 / 1 µs |
| Harmonics H2–H5 vs exact | +0.607 dB **warn** | 0.5 / 1 dB |
| Harmonics H2–H5 vs the steady sine | +0.842 dB pass | 3 / 6 dB |

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
+3.30 dB<!--= b pupu/xone-10dbfs // xone.h2.ac2 sweep 3Hz-5.5s|REW offline import@1040Hz --> to
+4.05 dB<!--= b pupu/xone-10dbfs // xone.h2.ac2 sweep 10Hz-5.5s|REW offline import@1040Hz -->
more than REW. Against the steady sine both pass: ac2's sweeps by
−0.20 dB<!--= b pupu/xone-10dbfs // xone.h2.ac2 sweep 10Hz-5.5s@1040Hz --> to
−0.96 dB<!--= b pupu/xone-10dbfs // xone.h2.ac2 sweep 3Hz-5.5s@1040Hz -->, REW by
−1.63 dB<!--= b pupu/xone-10dbfs // xone.h2.REW offline import@1040Hz -->. Neither tool is
off: ac2 divides by the measured reference, REW's export is the measurement input alone, and
each is held to the sine on its own definition. The reference loopback's own H2 at this tone
accounts for most of the gap, the rest is within each tool's limit against the sine. The
ac2 − REW check compares the two definitions directly; closed as a convention difference.

### digital DUT: Magnitude 20–100 Hz, sweeps: ac2 vs truth

In the loud case the 10 Hz sweeps spread up to
0.111 dB<!--= b host/dut-30dbfs // dut-loud.mag.ac2 sweep 10Hz-5.5s|direct (ac2 capture 10Hz-5.5s).20-100 -->
about their own capture, with a mean of
0.000 dB<!--= b host/dut-30dbfs // dut-loud.mag.ac2 sweep 10Hz-5.5s|direct (ac2 capture 10Hz-5.5s).20-100 // mean -->,
the same at both levels. Against the steady sines the same sweep is within
0.005 dB<!--= b host/dut-30dbfs // dut-loud.sine_mag.ac2 sweep 10Hz-5.5s@5000Hz --> at every
tone, and the device's three other cases pass. Ripple between the tones, cause not
investigated. Open.

### digital DUT: Magnitude 100 Hz–1 kHz, sweeps: ac2 vs truth

The same loud-case sweeps as at 20–100 Hz (above), spreading
0.034 dB<!--= b host/dut-30dbfs // dut-loud.mag.ac2 sweep 10Hz-5.5s|direct (ac2 capture 10Hz-5.5s).100-1000 -->
about their own capture: the same ripple, smaller. Open.

### digital DUT: Harmonics H2–H5 vs exact: ac2 vs truth

In the `dut-floor-hp` case at −10 dBFS, the 10 Hz / 5.5 s sweep reads H3 of the 50 Hz tone
+0.61 dB<!--= b host/dut-10dbfs // dut-floor-hp.dut.h3.ac2 sweep 10Hz-5.5s.50 --> over its
exact value; the 10 Hz / 11 s sweep reads
+0.35 dB<!--= b host/dut-10dbfs // dut-floor-hp.dut.h3.ac2 sweep 10Hz-11s.50 --> and the
steady sine
−0.05 dB<!--= b host/dut-10dbfs // dut-floor-hp.dut.h3.steady sine.50 -->. There the device's
pre-filter changes faster than a short sweep's instantaneous-frequency reading can follow, so
the cell sits at the edge of the sweep model, not of ac2's analysis
([reports/dut-coverage.md](reports/dut-coverage.md)). Open (scatter about the limit not shown).

## Reproducing

How to run each stage, refresh the baselines and rebuild this page
(`python -m crosscheck comparison`, `--check` to verify): [README.md](README.md),
"Reproducing the results" and "Baselines and compare".
