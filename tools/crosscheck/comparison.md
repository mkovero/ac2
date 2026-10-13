# ac2 against REW and Open Sound Meter

ac2, REW 5.40 and Open Sound Meter (OSM) v1.5.2 measured the same signals on one ac2 build: on
the pupu rig (RME Fireface 400 at 96 kHz) an electrical path through an Allen & Heath Xone:62
mixer, and a Genelec 1083 speaker in a living room picked up by an MM1 measurement microphone;
on the dev host a software device with known distortion on a dummy JACK server, and OSM's DSP on
synthetic test signals and on the rig's recordings. Each tool is held to a truth that depends on
none of them: a steady sine analysed in numpy, numpy's direct cross-spectrum of the same
recording, closed-form test signals, and the software device's exact harmonics. Not validated
here: the physical calibration chain (microphone sensitivity, calibrator; REW's SPL is
calibrated from ac2) and IEC 61672-1 / 61260-1 class conformance.

<!-- BEGIN generated build: `python -m crosscheck comparison` from baselines/; do not edit by hand -->

ac2 build **2c07efe**, runs of 2026-10-11 to 2026-10-13: Xone (electrical) at −30 dBFS; Genelec (speaker) at −30, −50 dBFS; digital DUT at −10, −30 dBFS; OSM fixtures and rig recordings. Not run on this build: ambient.

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
| Gain / level | −0.006 dB pass | — | — | +0.014 dB pass | — | 0.1 / 0.3–0.5 dB; speaker 0.1 / 0.3–0.5 dB |
| SPL | +0.505 dB **warn** | +1.172 dB **FAIL** | — | +0.567 dB **warn** | — | speaker 0.5 / 1 dB |
| Magnitude | 0.115 dB **warn** | 0.317 dB **warn** | 0.024 dB pass | −0.353 dB **warn** | 0.030 dB pass | 0.03–0.3 / 0.1–1 dB; speaker 0.1–0.3 / 0.3–1 dB |
| Phase | 0.84° pass | 1.93° pass | 0.22° pass | 2.19° pass | 0.30° pass | 0.5–3 / 2–10°; speaker 3 / 10° |
| Group delay | −8.0 % pass | +4.6 % pass | — | −4.9 % pass | — | 10 / 25 %; 0.5 / 2 µs |
| Arrival / delay | −1.04 µs pass | — | −0.500 samples pass | — | −0.551 samples pass | 0.02–0.6 / 0.1–1 samples; 0.3 / 1 µs; speaker 0.6 / 1 samples; speaker 2 / 10.4 µs |
| Coherence γ² | −0.0027 pass | — | −0.0052 pass | — | — | 0.02 / 0.05 |
| Harmonics H2–H5 | +5.760 dB **warn** | +4.807 dB **warn** | — | −1.284 dB pass | — | 0.5–3 / 1–6 dB; speaker 3 / 6 dB |
| Leq (Z, A, C) | not run | not run | — | not run | — | — |
| 1/3-octave RTA | not run | — | — | — | — | — |
| Spectrum level | +0.019 dB pass | — | +0.016 dB pass | — | — | 0.02–0.1 / 0.1–0.3 dB |

<!-- END generated table -->

## Disparities

### SPL: ac2 vs truth

The live transfer function (TF) on the Genelec, scaled to SPL by its reference level, reads
−0.50 dB<!--= b pupu/genelec-30dbfs // genelec.spl.ac2 TF × ref level@1000Hz --> at 1 kHz and
+0.51 dB<!--= b pupu/genelec-30dbfs // genelec.spl.ac2 TF × ref level@50Hz --> at the 50 Hz
tone against the steady sine, just over the pass limit; ac2's sweeps pass at both tones. The
1 kHz miss is the TF's magnitude miss (Magnitude: ac2 vs truth). The 50 Hz one is not
investigated. Open.

### SPL: REW vs truth

At −50 dBFS REW's SPL (calibrated from ac2) reads
+1.17 dB<!--= b pupu/genelec-50dbfs // genelec.spl.REW (SPL, cal from ac2)@50Hz --> over the
steady sine at the 50 Hz tone; at −30 dBFS it passes there
(−0.06 dB<!--= b pupu/genelec-30dbfs // genelec.spl.REW (SPL, cal from ac2)@50Hz -->). Likely
cause: at −50 dBFS the room's low-frequency noise is close to the signal; ac2's sweep at that
tone sits under its own uncertainty, so it is not judged. REW's reading, not ac2's. Open, not
investigated.

### SPL: ac2 − REW

The same two live-TF tones seen against REW: ac2 − REW is
−0.51 dB<!--= b pupu/genelec-30dbfs // genelec.spl.ac2 TF × ref level@1000Hz minus genelec.spl.REW (SPL, cal from ac2)@1000Hz -->
at 1 kHz and
+0.57 dB<!--= b pupu/genelec-30dbfs // genelec.spl.ac2 TF × ref level@50Hz minus genelec.spl.REW (SPL, cal from ac2)@50Hz -->
at 50 Hz. ac2's sweep and REW agree within the pass limit there
(−0.17 dB<!--= b pupu/genelec-30dbfs // genelec.spl.ac2 sweep@1000Hz minus genelec.spl.REW (SPL, cal from ac2)@1000Hz -->
at 1 kHz). Open, with the TF misses.

### Magnitude: ac2 vs truth

Three separate misses:

- **Live TF on the Genelec.** At 1 kHz the TF reads
  −0.50 dB<!--= b pupu/genelec-30dbfs // genelec.sine_mag.ac2 TF@1000Hz --> against the steady
  sine, while the sweep reads
  −0.16 dB<!--= b pupu/genelec-30dbfs // genelec.sine_mag.ac2 sweep 20Hz-5.5s@1000Hz -->. From
  100 Hz to 1 kHz, on the columns at γ² ≥ 0.99, the TF spreads
  0.143 dB<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 TF|direct (TF capture).100-1000 --> about
  its direct estimate with a mean of only
  +0.001 dB<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 TF|direct (TF capture).100-1000 // mean -->,
  so no band offset, but column-to-column scatter. A plausible cause, unmeasured: the TF's
  window at 1 kHz is shorter than the capture the direct estimate reads. Earlier takes:
  [reports/genelec-30-repeats.md](reports/genelec-30-repeats.md). Open.
- **Genelec at −50 dBFS, `dist` sweep.** Against its own capture, 100 Hz–1 kHz, it spreads
  0.663 dB<!--= b pupu/genelec-50dbfs // genelec.mag.ac2 sweep dist|direct (ac2 capture dist).100-1000 -->;
  at −30 dBFS the same comparison passes
  (0.036 dB<!--= b pupu/genelec-30dbfs // genelec.mag.ac2 sweep dist|direct (ac2 capture dist).100-1000 -->).
  Likely the room's noise, which sits as much closer to the signal as the drive is lower, but
  these columns passed the noise gate, so that is not shown. Open.
- **Digital device, loud case.** The 10 Hz sweeps spread
  0.115 dB<!--= b host/dut-30dbfs // dut-loud.mag.ac2 sweep 10Hz-5.5s|direct (ac2 capture 10Hz-5.5s).20-100 -->
  about their own capture at 20–100 Hz (a little over the pass limit, at both levels), with a
  mean of
  0.000 dB<!--= b host/dut-30dbfs // dut-loud.mag.ac2 sweep 10Hz-5.5s|direct (ac2 capture 10Hz-5.5s).20-100 // mean -->.
  The device's three other cases pass. Not investigated. Open.

### Magnitude: REW vs truth

REW's import against numpy's cross-spectrum of the same recording, 20–100 Hz on the Genelec at
−30 dBFS, spreads
0.317 dB<!--= b pupu/genelec-30dbfs // genelec.mag.REW offline|direct (REW recording).20-100 -->,
just over the pass limit; few columns survive the speaker path's low-frequency masks there. REW's
analysis, not ac2's. Open, not investigated.

### Magnitude: ac2 − REW

The live TF's 1 kHz miss seen against REW: ac2 TF − REW is
−0.35 dB<!--= b pupu/genelec-30dbfs // genelec.sine_mag.ac2 TF@1000Hz minus genelec.sine_mag.REW offline@1000Hz -->.
ac2's sweep − REW at the same tone is
−0.01 dB<!--= b pupu/genelec-30dbfs // genelec.sine_mag.ac2 sweep 20Hz-5.5s@1000Hz minus genelec.sine_mag.REW offline@1000Hz -->,
and the sweep against REW over whole bands passes everywhere. Open, with the TF miss.

### Harmonics H2–H5: ac2 vs truth

On the Genelec at −30 dBFS:

- **H2 at 5 kHz.** All three ac2 sweeps read it above the steady sine, by
  +3.52 dB<!--= b pupu/genelec-30dbfs // genelec.h2.ac2 sweep 20Hz-5.5s@5000Hz --> to
  +4.21 dB<!--= b pupu/genelec-30dbfs // genelec.h2.ac2 sweep dist-probe@5000Hz -->, and REW
  reads it +4.81 dB<!--= b pupu/genelec-30dbfs // genelec.h2.REW offline import@5000Hz -->. ac2
  and REW agree within their pass limit, so both analysers read the same harmonic; why the
  sweeps differ from the steady sine (the speaker's distortion under a sweep, or the sine's own
  floor at 10 kHz) is not shown. Open.
- **H4 at 500 Hz, `dist` sweep only.** It reads
  +5.76 dB<!--= b pupu/genelec-30dbfs // genelec.h4.ac2 sweep dist@500Hz --> over the steady
  sine; the shorter sweeps and REW only give bounds there (harmonic at their noise floor), so
  nothing confirms or refutes it. One take. Open.

### Harmonics H2–H5: REW vs truth

REW's H2 at 5 kHz on the Genelec,
+4.81 dB<!--= b pupu/genelec-30dbfs // genelec.h2.REW offline import@5000Hz --> over the steady
sine: the same reading as ac2's (previous section). Open.

## Reproducing

How to run each stage, refresh the baselines and rebuild this page
(`python -m crosscheck comparison`, `--check` to verify): [README.md](README.md),
"Reproducing the results" and "Baselines and compare".
