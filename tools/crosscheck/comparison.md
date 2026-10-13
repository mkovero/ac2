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

Each cell is the worst value over every level, path and frequency of that comparison, and its
verdict against the limits in `tolerances.toml` (speaker-path limits are wider: a room is
noisier than a cable). "—": the tool does not measure it here; "not judged": every value sat at
a noise floor or under its own uncertainty. ac2 − REW at a tone is the difference of the two
readings against the same steady sine.

<!-- BEGIN generated table: `python -m crosscheck comparison` from baselines/; do not edit by hand -->

| quantity | ac2 vs truth | REW vs truth | OSM vs truth | ac2 − REW | ac2 − OSM | limits (pass / warn) |
|---|---|---|---|---|---|---|
| Gain / level | −0.006 dB pass | — | — | −0.006 dB pass | — | 0.1 / 0.3–0.5 dB; speaker 0.1 / 0.3–0.5 dB |
| SPL | −0.396 dB pass | +0.193 dB pass | — | −0.339 dB pass | — | speaker 0.5 / 1 dB |
| Magnitude | 0.111 dB **warn** | 0.199 dB pass | 0.024 dB pass | 0.244 dB pass | 0.030 dB pass | 0.03–0.3 / 0.1–1 dB; speaker 0.1–0.3 / 0.3–1 dB |
| Phase | 0.83° pass | 2.49° pass | 0.22° pass | 2.45° pass | 0.30° pass | 0.5–3 / 2–10°; speaker 3 / 10° |
| Group delay | +2.1 % pass | +1.7 % pass | — | +1.9 % pass | — | 10 / 25 %; 0.5 / 2 µs |
| Arrival / delay | −0.66 µs pass | — | −0.500 samples pass | — | −0.551 samples pass | 0.02–0.6 / 0.1–1 samples; 0.3 / 1 µs; speaker 0.6 / 1 samples; speaker 2 / 10.4 µs |
| Coherence γ² | −0.0027 pass | — | −0.0052 pass | — | — | 0.02 / 0.05 |
| Harmonics H2–H5 | +0.607 dB **warn** | −1.776 dB pass | — | +4.054 dB **warn** | — | 0.5–3 / 1–6 dB; speaker 3 / 6 dB |
| Leq (Z, A, C) | +0.008 dB pass | +0.035 dB pass | — | −0.031 dB pass | — | 0.5 / 1 dB |
| 1/3-octave RTA | −0.040 dB pass | — | — | — | — | 0.25 / 0.5 dB |
| Spectrum level | +0.019 dB pass | — | +0.016 dB pass | — | — | 0.02–0.1 / 0.1–0.3 dB |

<!-- END generated table -->

## Disparities

### Magnitude: ac2 vs truth

Two separate misses, both ac2's; REW and OSM have no counterpart to them.

- **Genelec live TF against its direct estimate.** On the columns at γ² ≥ 0.99 the TF spreads
  0.176 dB<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 TF|direct (TF capture).20-100 --> at
  20–100 Hz and
  0.180 dB<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 TF|direct (TF capture).100-1000 --> at
  100 Hz–1 kHz about numpy's estimate of the same capture, with band means of only
  +0.029 dB<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 TF|direct (TF capture).20-100 // mean -->
  and
  −0.006 dB<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 TF|direct (TF capture).100-1000 // mean -->:
  column-to-column scatter, not an offset. Against the steady sines the TF passes at every
  judged tone, e.g.
  +0.07 dB<!--= b pupu/genelec-30dbfs // genelec.sine_mag.ac2 TF@500Hz --> at 500 Hz and
  +0.06 dB<!--= b pupu/genelec-30dbfs // genelec.sine_mag.ac2 TF@1000Hz --> at 1 kHz. At
  20–100 Hz only
  5<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 TF|direct (TF capture).20-100 // n --> of
  111<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 TF|direct (TF capture).20-100 // of -->
  columns pass the coherence gate. Likely cause, not shown: the TF's frequency-dependent windows
  read the room's reflections differently from the direct estimate's whole capture. Open.
- **Digital device, loud case.** The 10 Hz sweeps spread up to
  0.111 dB<!--= b host/dut-30dbfs // dut-loud.mag.ac2 sweep 10Hz-5.5s|direct (ac2 capture 10Hz-5.5s).20-100 -->
  at 20–100 Hz and
  0.034 dB<!--= b host/dut-30dbfs // dut-loud.mag.ac2 sweep 10Hz-5.5s|direct (ac2 capture 10Hz-5.5s).100-1000 -->
  at 100 Hz–1 kHz about their own capture, with means of
  0.000 dB<!--= b host/dut-30dbfs // dut-loud.mag.ac2 sweep 10Hz-5.5s|direct (ac2 capture 10Hz-5.5s).20-100 // mean -->,
  the same at both levels. Against the steady sines the same sweep is within
  0.005 dB<!--= b host/dut-30dbfs // dut-loud.sine_mag.ac2 sweep 10Hz-5.5s@5000Hz --> at every
  tone, and the device's three other cases pass. Ripple between the tones, cause not
  investigated. Open.

### Harmonics H2–H5: ac2 vs truth

On the digital device's `dut-floor-hp` case at −10 dBFS, the 10 Hz / 5.5 s sweep reads H3 of
the 50 Hz tone
+0.61 dB<!--= b host/dut-10dbfs // dut-floor-hp.dut.h3.ac2 sweep 10Hz-5.5s.50 --> over its
exact value; the 10 Hz / 11 s sweep reads
+0.35 dB<!--= b host/dut-10dbfs // dut-floor-hp.dut.h3.ac2 sweep 10Hz-11s.50 --> and the
steady sine
−0.05 dB<!--= b host/dut-10dbfs // dut-floor-hp.dut.h3.steady sine.50 -->. There the device's
pre-filter changes faster than a short sweep's instantaneous-frequency reading can follow, so
the cell sits at the edge of the sweep model, not of ac2's analysis
([reports/dut-coverage.md](reports/dut-coverage.md)). Open (scatter about the limit not shown).

### Harmonics H2–H5: ac2 − REW

On the Xone at −10 dBFS, H2 of the 1040 Hz tone: the ac2 sweeps read
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

## Reproducing

How to run each stage, refresh the baselines and rebuild this page
(`python -m crosscheck comparison`, `--check` to verify): [README.md](README.md),
"Reproducing the results" and "Baselines and compare".
