# Repeated takes

Output of `python -m crosscheck repeats 20261014T015540Z 20261014T021152Z` (run directories
under `/work/ac2-crosscheck/runs`): the two full rig runs of 2026-10-14 on ac2 70655493, the
second of them the baseline, analysed with the same suite. The first run's REW A / B was
re-imported from its recordings (`rew-import`) after the A / B fix. The per-check "Steady
sines" tables are left out here: the column-to-tone split carries each measurement's total
against the sine and its parts. Rerun the command for them and for the per-check statuses.

## ambient: 2 takes

Runs: 20261014T015540Z, 20261014T021152Z. ac2 builds: 0.0.0+70655493866d.

Signed values across takes: sd is the sample standard deviation (n − 1); `0 in mean±2se` is whether the mean lies within two standard errors of zero (no bias shown by these takes); `>pass` counts takes with |value| above the pass limit.

## genelec at -30 dBFS, 1/48 octave: 2 takes

Runs: 20261014T015540Z, 20261014T021152Z. ac2 builds: 0.0.0+70655493866d.

Signed values across takes: sd is the sample standard deviation (n − 1); `0 in mean±2se` is whether the mean lies within two standard errors of zero (no bias shown by these takes); `>pass` counts takes with |value| above the pass limit.

### Live TF vs its direct estimate, per band

| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |
|---|---|---|---|---|---|---|---|---|---|---|---|
| genelec.mag.ac2 TF|direct (TF capture).100-1000 (signed mean) | dB | 2 | -0.005 | 0.006 | -0.010 | -0.000 | no | yes | 0/2 (>0.1) | ≤ 0.034 | 2×PASS |
| genelec.mag.ac2 TF|direct (TF capture).1000-20000 | dB | 0 | | | | | | | | | 2×INCONCLUSIVE |
| genelec.mag.ac2 TF|direct (TF capture).20-100 | dB | 0 | | | | | | | | | 2×INCONCLUSIVE |
| genelec.phase.ac2 TF|direct (TF capture).100-1000 (signed mean) | ° | 2 | +0.057 | 0.001 | +0.056 | +0.058 | no | no | 0/2 (>3) | ≤ 0.450 | 2×PASS |
| genelec.phase.ac2 TF|direct (TF capture).1000-20000 | ° | 0 | | | | | | | | | 2×INCONCLUSIVE |
| genelec.phase.ac2 TF|direct (TF capture).20-100 | ° | 0 | | | | | | | | | 2×INCONCLUSIVE |

### Arrival and delay

| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |
|---|---|---|---|---|---|---|---|---|---|---|---|
| genelec.delay.ac2_arrival.20Hz-5.5s | µs | 2 | -0.317 | 0.011 | -0.325 | -0.310 | no | no | 0/2 (>2) |  | 2×PASS |
| genelec.delay.ac2_arrival.dist | µs | 2 | +0.035 | 0.054 | -0.003 | +0.074 | yes | yes | 0/2 (>2) |  | 2×PASS |
| genelec.delay.ac2_arrival.dist-probe | µs | 2 | -0.398 | 0.292 | -0.605 | -0.192 | no | yes | 0/2 (>2) |  | 2×PASS |
| genelec.delay.capture_vs_rew_recording.20Hz-5.5s | µs | 2 | +1.560 | 0.279 | +1.363 | +1.758 | no | no |  |  | 2×INFO |
| genelec.delay.capture_vs_rew_recording.dist | µs | 2 | +1.459 | 0.060 | +1.417 | +1.501 | no | no |  |  | 2×INFO |
| genelec.delay.capture_vs_rew_recording.dist-probe | µs | 2 | +1.942 | 0.109 | +1.864 | +2.019 | no | no |  |  | 2×INFO |
| genelec.delay.rew.REW offline import.IR peak | µs | 2 | -2219.315 | 0.247 | -2219.490 | -2219.140 | no | no |  |  | 2×INFO |
| genelec.delay.rew.REW offline import.reported delay | µs | 2 | -2242.060 | 0.311 | -2242.280 | -2241.840 | no | no |  |  | 2×INFO |

### Column-to-tone split at the sines

measurement − sine = processing (measurement − its capture's 1/48-oct column) + column-to-tone (column − the same capture narrow at the sine) + capture vs sine (narrow − sine). Mean ± sd over the takes.

| resolution | quantity | f Hz | measurement | capture | takes | total | processing | column-to-tone | capture vs sine |
|---|---|---|---|---|---|---|---|---|---|
| 1/48 | sine_mag | 50 | REW offline | direct (REW recording) | 2 | -0.132 ± 0.280 | -0.090 ± 0.185 | +0.066 ± 0.035 | -0.108 ± 0.060 |
| 1/48 | sine_mag | 50 | REW ÷ ref | direct (REW recording) | 2 | -0.132 ± 0.281 | -0.090 ± 0.186 | +0.066 ± 0.035 | -0.108 ± 0.060 |
| 1/48 | sine_mag | 50 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.040 ± 0.054 | +0.013 ± 0.050 | -0.007 ± 0.104 | +0.034 ± 0.108 |
| 1/48 | sine_mag | 100 | REW offline | direct (REW recording) | 2 | -0.052 ± 0.025 | -0.009 ± 0.001 | -0.042 ± 0.022 | -0.001 ± 0.005 |
| 1/48 | sine_mag | 100 | REW ÷ ref | direct (REW recording) | 2 | -0.052 ± 0.025 | -0.009 ± 0.001 | -0.042 ± 0.022 | -0.001 ± 0.005 |
| 1/48 | sine_mag | 100 | ac2 TF | direct (TF capture) | 2 | -0.066 ± 0.015 | -0.007 ± 0.016 | -0.062 ± 0.062 | +0.003 ± 0.063 |
| 1/48 | sine_mag | 100 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -0.025 ± 0.014 | -0.007 ± 0.002 | -0.084 ± 0.079 | +0.066 ± 0.095 |
| 1/48 | sine_mag | 200 | REW offline | direct (REW recording) | 2 | -0.045 ± 0.051 | -0.017 ± 0.001 | +0.014 ± 0.020 | -0.042 ± 0.032 |
| 1/48 | sine_mag | 200 | REW ÷ ref | direct (REW recording) | 2 | -0.045 ± 0.051 | -0.017 ± 0.001 | +0.014 ± 0.020 | -0.042 ± 0.032 |
| 1/48 | sine_mag | 200 | ac2 TF | direct (TF capture) | 2 | +0.016 ± 0.002 | +0.064 ± 0.018 | -0.057 ± 0.031 | +0.009 ± 0.012 |
| 1/48 | sine_mag | 200 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -0.027 ± 0.042 | -0.009 ± 0.001 | -0.031 ± 0.010 | +0.013 ± 0.032 |
| 1/48 | sine_mag | 500 | REW offline | direct (REW recording) | 2 | -0.035 ± 0.000 | -0.012 ± 0.000 | +0.003 ± 0.020 | -0.025 ± 0.020 |
| 1/48 | sine_mag | 500 | REW ÷ ref | direct (REW recording) | 2 | -0.035 ± 0.000 | -0.012 ± 0.000 | +0.003 ± 0.020 | -0.025 ± 0.020 |
| 1/48 | sine_mag | 500 | ac2 TF | direct (TF capture) | 2 | +0.094 ± 0.067 | +0.001 ± 0.051 | +0.072 ± 0.013 | +0.021 ± 0.003 |
| 1/48 | sine_mag | 500 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -0.020 ± 0.009 | -0.003 ± 0.000 | -0.007 ± 0.008 | -0.009 ± 0.017 |
| 1/48 | sine_mag | 1000 | REW offline | direct (REW recording) | 2 | +0.025 ± 0.002 | -0.004 ± 0.000 | +0.082 ± 0.002 | -0.052 ± 0.004 |
| 1/48 | sine_mag | 1000 | REW ÷ ref | direct (REW recording) | 2 | +0.025 ± 0.002 | -0.004 ± 0.000 | +0.082 ± 0.002 | -0.052 ± 0.004 |
| 1/48 | sine_mag | 1000 | ac2 TF | direct (TF capture) | 2 | +0.071 ± 0.044 | +0.001 ± 0.036 | +0.088 ± 0.008 | -0.017 ± 0.017 |
| 1/48 | sine_mag | 1000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.037 ± 0.001 | -0.000 ± 0.000 | +0.049 ± 0.002 | -0.011 ± 0.003 |
| 1/48 | sine_mag | 2000 | REW offline | direct (REW recording) | 2 | +0.205 ± 0.051 | -0.002 ± 0.000 | +0.181 ± 0.019 | +0.026 ± 0.032 |
| 1/48 | sine_mag | 2000 | REW ÷ ref | direct (REW recording) | 2 | +0.205 ± 0.051 | -0.002 ± 0.000 | +0.181 ± 0.019 | +0.026 ± 0.032 |
| 1/48 | sine_mag | 2000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.231 ± 0.050 | -0.003 ± 0.000 | +0.208 ± 0.011 | +0.026 ± 0.040 |
| 1/48 | sine_mag | 5000 | REW offline | direct (REW recording) | 2 | -0.313 ± 0.065 | -0.001 ± 0.000 | -0.466 ± 0.019 | +0.154 ± 0.083 |
| 1/48 | sine_mag | 5000 | REW ÷ ref | direct (REW recording) | 2 | -0.313 ± 0.065 | -0.001 ± 0.000 | -0.466 ± 0.019 | +0.154 ± 0.083 |
| 1/48 | sine_mag | 5000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -0.352 ± 0.070 | -0.002 ± 0.001 | -0.430 ± 0.048 | +0.080 ± 0.118 |
| 1/48 | sine_mag | 10000 | REW offline | direct (REW recording) | 2 | -1.421 ± 0.072 | +0.032 ± 0.043 | -1.440 ± 0.005 | -0.012 ± 0.024 |
| 1/48 | sine_mag | 10000 | REW ÷ ref | direct (REW recording) | 2 | -1.421 ± 0.072 | +0.031 ± 0.043 | -1.440 ± 0.005 | -0.012 ± 0.024 |
| 1/48 | sine_mag | 10000 | ac2 TF | direct (TF capture) | 2 | -3.609 ± 0.249 | -0.240 ± 0.275 | -1.661 ± 0.021 | -1.708 ± 0.005 |
| 1/48 | sine_mag | 10000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -1.376 ± 0.078 | -0.004 ± 0.000 | -1.437 ± 0.008 | +0.065 ± 0.087 |
| 1/48 | sine_phase | 50 | REW offline | direct (REW recording) | 2 | -0.507 ± 0.890 | -0.718 ± 0.386 | -1.263 ± 1.725 | +1.475 ± 3.001 |
| 1/48 | sine_phase | 50 | REW ÷ ref | direct (REW recording) | 2 | -0.506 ± 0.888 | -0.718 ± 0.388 | -1.263 ± 1.725 | +1.475 ± 3.001 |
| 1/48 | sine_phase | 50 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -0.636 ± 0.256 | -0.146 ± 0.215 | -0.518 ± 0.140 | +0.027 ± 0.182 |
| 1/48 | sine_phase | 100 | REW offline | direct (REW recording) | 2 | +0.389 ± 0.084 | +0.241 ± 0.030 | -0.099 ± 0.050 | +0.248 ± 0.164 |
| 1/48 | sine_phase | 100 | REW ÷ ref | direct (REW recording) | 2 | +0.389 ± 0.083 | +0.241 ± 0.030 | -0.099 ± 0.050 | +0.248 ± 0.164 |
| 1/48 | sine_phase | 100 | ac2 TF | direct (TF capture) | 2 | +0.759 ± 0.083 | +0.086 ± 0.096 | +0.161 ± 0.096 | +0.512 ± 0.084 |
| 1/48 | sine_phase | 100 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.057 ± 0.086 | +0.051 ± 0.025 | +0.148 ± 0.571 | -0.141 ± 0.632 |
| 1/48 | sine_phase | 200 | REW offline | direct (REW recording) | 2 | +0.262 ± 0.278 | +0.101 ± 0.014 | -0.023 ± 0.299 | +0.185 ± 0.035 |
| 1/48 | sine_phase | 200 | REW ÷ ref | direct (REW recording) | 2 | +0.262 ± 0.278 | +0.100 ± 0.015 | -0.023 ± 0.299 | +0.185 ± 0.035 |
| 1/48 | sine_phase | 200 | ac2 TF | direct (TF capture) | 2 | +0.259 ± 0.073 | -0.134 ± 0.194 | -0.210 ± 0.079 | +0.603 ± 0.199 |
| 1/48 | sine_phase | 200 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -0.105 ± 0.012 | +0.097 ± 0.118 | -0.252 ± 0.103 | +0.049 ± 0.004 |
| 1/48 | sine_phase | 500 | REW offline | direct (REW recording) | 2 | +1.040 ± 0.013 | -0.012 ± 0.002 | +1.035 ± 0.084 | +0.017 ± 0.070 |
| 1/48 | sine_phase | 500 | REW ÷ ref | direct (REW recording) | 2 | +1.040 ± 0.012 | -0.012 ± 0.003 | +1.035 ± 0.084 | +0.017 ± 0.070 |
| 1/48 | sine_phase | 500 | ac2 TF | direct (TF capture) | 2 | +2.333 ± 0.269 | +0.211 ± 0.207 | +1.346 ± 0.244 | +0.776 ± 0.306 |
| 1/48 | sine_phase | 500 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +1.066 ± 0.013 | -0.012 ± 0.001 | +1.205 ± 0.207 | -0.127 ± 0.221 |
| 1/48 | sine_phase | 1000 | REW offline | direct (REW recording) | 2 | +2.057 ± 0.126 | -0.052 ± 0.000 | +2.269 ± 0.020 | -0.160 ± 0.146 |
| 1/48 | sine_phase | 1000 | REW ÷ ref | direct (REW recording) | 2 | +2.057 ± 0.126 | -0.052 ± 0.000 | +2.269 ± 0.020 | -0.160 ± 0.146 |
| 1/48 | sine_phase | 1000 | ac2 TF | direct (TF capture) | 2 | +2.305 ± 0.510 | -0.131 ± 0.479 | +2.481 ± 0.173 | -0.045 ± 0.205 |
| 1/48 | sine_phase | 1000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +1.996 ± 0.020 | +0.002 ± 0.002 | +2.206 ± 0.004 | -0.212 ± 0.014 |
| 1/48 | sine_phase | 2000 | REW offline | direct (REW recording) | 2 | +5.694 ± 0.448 | -0.066 ± 0.000 | +5.312 ± 0.020 | +0.448 ± 0.469 |
| 1/48 | sine_phase | 2000 | REW ÷ ref | direct (REW recording) | 2 | +5.694 ± 0.449 | -0.066 ± 0.000 | +5.312 ± 0.020 | +0.448 ± 0.469 |
| 1/48 | sine_phase | 2000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +5.761 ± 0.303 | -0.004 ± 0.003 | +5.612 ± 0.025 | +0.154 ± 0.275 |
| 1/48 | sine_phase | 5000 | REW offline | direct (REW recording) | 2 | +7.929 ± 0.583 | -0.033 ± 0.005 | +7.950 ± 0.099 | +0.012 ± 0.478 |
| 1/48 | sine_phase | 5000 | REW ÷ ref | direct (REW recording) | 2 | +7.929 ± 0.583 | -0.034 ± 0.006 | +7.950 ± 0.099 | +0.012 ± 0.478 |
| 1/48 | sine_phase | 5000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +8.284 ± 0.079 | +0.012 ± 0.003 | +7.931 ± 0.142 | +0.340 ± 0.060 |
| 1/48 | sine_phase | 10000 | REW offline | direct (REW recording) | 2 | -0.471 ± 1.568 | -0.038 ± 0.398 | +0.131 ± 0.158 | -0.564 ± 1.013 |
| 1/48 | sine_phase | 10000 | REW ÷ ref | direct (REW recording) | 2 | -0.471 ± 1.569 | -0.038 ± 0.398 | +0.131 ± 0.158 | -0.564 ± 1.013 |
| 1/48 | sine_phase | 10000 | ac2 TF | direct (TF capture) | 2 | +2.228 ± 0.128 | +0.752 ± 0.900 | +1.106 ± 0.202 | +0.369 ± 0.569 |
| 1/48 | sine_phase | 10000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -0.030 ± 0.379 | -0.014 ± 0.002 | +0.386 ± 0.282 | -0.403 ± 0.663 |

## xone at -10 dBFS, 1/48 octave: 2 takes

Runs: 20261014T015540Z, 20261014T021152Z. ac2 builds: 0.0.0+70655493866d.

Signed values across takes: sd is the sample standard deviation (n − 1); `0 in mean±2se` is whether the mean lies within two standard errors of zero (no bias shown by these takes); `>pass` counts takes with |value| above the pass limit.

### Live TF vs its direct estimate, per band

| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |
|---|---|---|---|---|---|---|---|---|---|---|---|
| xone.mag.ac2 TF|direct (TF capture).100-1000 (signed mean) | dB | 2 | +0.000 | 0.000 | +0.000 | +0.000 | no | no | 0/2 (>0.1) | ≤ 0.004 | 2×PASS |
| xone.mag.ac2 TF|direct (TF capture).1000-20000 (signed mean) | dB | 2 | +0.000 | 0.000 | +0.000 | +0.000 | no | yes | 0/2 (>0.1) | ≤ 0.003 | 2×PASS |
| xone.mag.ac2 TF|direct (TF capture).20-100 (signed mean) | dB | 2 | +0.000 | 0.000 | -0.000 | +0.000 | yes | yes | 0/2 (>0.1) | ≤ 0.008 | 2×PASS |
| xone.phase.ac2 TF|direct (TF capture).100-1000 (signed mean) | ° | 2 | +0.000 | 0.000 | -0.000 | +0.000 | yes | yes | 0/2 (>1) | ≤ 0.030 | 2×PASS |
| xone.phase.ac2 TF|direct (TF capture).1000-20000 (signed mean) | ° | 2 | -0.000 | 0.001 | -0.001 | +0.001 | yes | yes | 0/2 (>1) | ≤ 0.045 | 2×PASS |
| xone.phase.ac2 TF|direct (TF capture).20-100 (signed mean) | ° | 2 | -0.002 | 0.001 | -0.003 | -0.001 | no | yes | 0/2 (>1) | ≤ 0.099 | 2×PASS |

### Arrival and delay

| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |
|---|---|---|---|---|---|---|---|---|---|---|---|
| xone.delay.ac2_arrival.10Hz-11s | µs | 2 | +0.008 | 0.000 | +0.008 | +0.008 | no | no | 0/2 (>0.3) |  | 2×PASS |
| xone.delay.ac2_arrival.10Hz-5.5s | µs | 2 | +0.007 | 0.001 | +0.006 | +0.007 | no | no | 0/2 (>0.3) |  | 2×PASS |
| xone.delay.ac2_arrival.3Hz-5.5s | µs | 2 | +0.007 | 0.001 | +0.007 | +0.008 | no | no | 0/2 (>0.3) |  | 2×PASS |
| xone.delay.ac2_arrival.dist | µs | 2 | +0.009 | 0.001 | +0.008 | +0.010 | no | no | 0/2 (>0.3) |  | 2×PASS |
| xone.delay.ac2_arrival.dist-probe | µs | 2 | +0.011 | 0.003 | +0.009 | +0.013 | no | no | 0/2 (>0.3) |  | 2×PASS |
| xone.delay.ac2_total.10Hz-11s | µs | 2 | -0.000 | 0.000 | -0.000 | -0.000 | no | no | 0/2 (>0.3) |  | 2×PASS |
| xone.delay.ac2_total.10Hz-5.5s | µs | 2 | -0.000 | 0.000 | -0.000 | -0.000 | no | no | 0/2 (>0.3) |  | 2×PASS |
| xone.delay.ac2_total.3Hz-5.5s | µs | 2 | -0.000 | 0.000 | -0.000 | -0.000 | no | no | 0/2 (>0.3) |  | 2×PASS |
| xone.delay.ac2_total.dist | µs | 2 | -0.000 | 0.000 | -0.000 | -0.000 | no | no | 0/2 (>0.3) |  | 2×PASS |
| xone.delay.ac2_total.dist-probe | µs | 2 | -0.000 | 0.000 | -0.000 | -0.000 | no | no | 0/2 (>0.3) |  | 2×PASS |
| xone.delay.capture_vs_rew_recording.10Hz-11s | µs | 2 | +0.052 | 0.000 | +0.052 | +0.052 | no | no |  |  | 2×INFO |
| xone.delay.capture_vs_rew_recording.10Hz-5.5s | µs | 2 | +0.053 | 0.001 | +0.052 | +0.053 | no | no |  |  | 2×INFO |
| xone.delay.capture_vs_rew_recording.3Hz-5.5s | µs | 2 | +0.052 | 0.001 | +0.052 | +0.053 | no | no |  |  | 2×INFO |
| xone.delay.capture_vs_rew_recording.dist | µs | 2 | +0.051 | 0.001 | +0.050 | +0.051 | no | no |  |  | 2×INFO |
| xone.delay.capture_vs_rew_recording.dist-probe | µs | 2 | +0.049 | 0.003 | +0.047 | +0.051 | no | no |  |  | 2×INFO |
| xone.delay.rew.REW offline import.IR peak | µs | 2 | -5.575 | 0.000 | -5.575 | -5.575 | no | no |  |  | 2×INFO |
| xone.delay.rew.REW offline import.reported delay | µs | 2 | -6.225 | 0.000 | -6.225 | -6.225 | no | no |  |  | 2×INFO |

### Column-to-tone split at the sines

measurement − sine = processing (measurement − its capture's 1/48-oct column) + column-to-tone (column − the same capture narrow at the sine) + capture vs sine (narrow − sine). Mean ± sd over the takes.

| resolution | quantity | f Hz | measurement | capture | takes | total | processing | column-to-tone | capture vs sine |
|---|---|---|---|---|---|---|---|---|---|
| 1/48 | sine_mag | 15 | REW offline | direct (REW recording) | 2 | +0.003 ± 0.000 | +0.002 ± 0.000 | +0.000 ± 0.000 | +0.000 ± 0.000 |
| 1/48 | sine_mag | 15 | REW ÷ ref | direct (REW recording) | 2 | +0.008 ± 0.000 | +0.007 ± 0.000 | +0.000 ± 0.000 | +0.000 ± 0.000 |
| 1/48 | sine_mag | 15 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | +0.008 ± 0.000 | +0.005 ± 0.001 | +0.003 ± 0.001 | -0.000 ± 0.000 |
| 1/48 | sine_mag | 26.25 | REW offline | direct (REW recording) | 2 | +0.001 ± 0.000 | +0.002 ± 0.000 | -0.000 ± 0.000 | -0.001 ± 0.000 |
| 1/48 | sine_mag | 26.25 | REW ÷ ref | direct (REW recording) | 2 | -0.005 ± 0.000 | -0.004 ± 0.000 | -0.000 ± 0.000 | -0.001 ± 0.000 |
| 1/48 | sine_mag | 26.25 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | -0.001 ± 0.000 | -0.000 ± 0.000 | -0.000 ± 0.000 | -0.000 ± 0.000 |
| 1/48 | sine_mag | 35 | REW offline | direct (REW recording) | 2 | -0.000 ± 0.000 | -0.000 ± 0.001 | -0.000 ± 0.000 | +0.000 ± 0.001 |
| 1/48 | sine_mag | 35 | REW ÷ ref | direct (REW recording) | 2 | -0.004 ± 0.000 | -0.004 ± 0.001 | -0.000 ± 0.000 | +0.000 ± 0.001 |
| 1/48 | sine_mag | 35 | ac2 TF | direct (TF capture) | 2 | +0.000 ± 0.001 | -0.003 ± 0.003 | -0.005 ± 0.006 | +0.008 ± 0.010 |
| 1/48 | sine_mag | 35 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | +0.000 ± 0.000 | -0.000 ± 0.000 | +0.000 ± 0.000 | +0.000 ± 0.000 |
| 1/48 | sine_mag | 40.5 | REW offline | direct (REW recording) | 2 | -0.001 ± 0.000 | -0.000 ± 0.001 | +0.000 ± 0.000 | -0.000 ± 0.000 |
| 1/48 | sine_mag | 40.5 | REW ÷ ref | direct (REW recording) | 2 | -0.001 ± 0.000 | -0.000 ± 0.001 | +0.000 ± 0.000 | -0.000 ± 0.000 |
| 1/48 | sine_mag | 40.5 | ac2 TF | direct (TF capture) | 2 | -0.001 ± 0.000 | +0.000 ± 0.002 | -0.001 ± 0.000 | -0.001 ± 0.002 |
| 1/48 | sine_mag | 40.5 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | -0.000 ± 0.000 | -0.001 ± 0.000 | +0.002 ± 0.000 | -0.001 ± 0.000 |
| 1/48 | sine_mag | 56.2 | REW offline | direct (REW recording) | 2 | +0.000 ± 0.000 | -0.000 ± 0.000 | -0.001 ± 0.000 | +0.001 ± 0.000 |
| 1/48 | sine_mag | 56.2 | REW ÷ ref | direct (REW recording) | 2 | +0.000 ± 0.000 | -0.000 ± 0.000 | -0.001 ± 0.000 | +0.001 ± 0.000 |
| 1/48 | sine_mag | 56.2 | ac2 TF | direct (TF capture) | 2 | +0.002 ± 0.002 | +0.000 ± 0.003 | -0.000 ± 0.004 | +0.002 ± 0.005 |
| 1/48 | sine_mag | 56.2 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | +0.000 ± 0.000 | +0.001 ± 0.000 | -0.001 ± 0.001 | +0.000 ± 0.000 |
| 1/48 | sine_mag | 60.5 | REW offline | direct (REW recording) | 2 | +0.000 ± 0.000 | -0.000 ± 0.000 | +0.000 ± 0.000 | +0.000 ± 0.000 |
| 1/48 | sine_mag | 60.5 | REW ÷ ref | direct (REW recording) | 2 | +0.000 ± 0.000 | +0.000 ± 0.000 | +0.000 ± 0.000 | +0.000 ± 0.000 |
| 1/48 | sine_mag | 60.5 | ac2 TF | direct (TF capture) | 2 | +0.000 ± 0.000 | +0.001 ± 0.001 | +0.003 ± 0.002 | -0.004 ± 0.003 |
| 1/48 | sine_mag | 60.5 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | -0.000 ± 0.000 | +0.000 ± 0.000 | -0.001 ± 0.000 | +0.000 ± 0.000 |
| 1/48 | sine_mag | 93.9 | REW offline | direct (REW recording) | 2 | -0.000 ± 0.000 | -0.000 ± 0.000 | +0.000 ± 0.000 | -0.000 ± 0.001 |
| 1/48 | sine_mag | 93.9 | REW ÷ ref | direct (REW recording) | 2 | -0.000 ± 0.000 | -0.000 ± 0.000 | +0.000 ± 0.000 | -0.000 ± 0.001 |
| 1/48 | sine_mag | 93.9 | ac2 TF | direct (TF capture) | 2 | +0.000 ± 0.000 | -0.001 ± 0.001 | -0.002 ± 0.002 | +0.003 ± 0.004 |
| 1/48 | sine_mag | 93.9 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | -0.000 ± 0.000 | +0.000 ± 0.000 | -0.000 ± 0.000 | +0.000 ± 0.001 |
| 1/48 | sine_mag | 1040 | REW offline | direct (REW recording) | 2 | +0.000 ± 0.000 | +0.000 ± 0.000 | +0.000 ± 0.000 | +0.000 ± 0.000 |
| 1/48 | sine_mag | 1040 | REW ÷ ref | direct (REW recording) | 2 | +0.000 ± 0.000 | +0.000 ± 0.000 | +0.000 ± 0.000 | +0.000 ± 0.000 |
| 1/48 | sine_mag | 1040 | ac2 TF | direct (TF capture) | 2 | +0.000 ± 0.001 | -0.000 ± 0.001 | -0.001 ± 0.001 | +0.001 ± 0.002 |
| 1/48 | sine_mag | 1040 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | -0.001 ± 0.000 | -0.001 ± 0.000 | +0.000 ± 0.000 | -0.000 ± 0.000 |
| 1/48 | sine_mag | 10000 | REW offline | direct (REW recording) | 2 | +0.000 ± 0.000 | +0.000 ± 0.000 | -0.000 ± 0.000 | +0.000 ± 0.000 |
| 1/48 | sine_mag | 10000 | REW ÷ ref | direct (REW recording) | 2 | -0.000 ± 0.000 | -0.000 ± 0.000 | -0.000 ± 0.000 | +0.000 ± 0.000 |
| 1/48 | sine_mag | 10000 | ac2 TF | direct (TF capture) | 2 | +0.000 ± 0.000 | -0.000 ± 0.000 | -0.000 ± 0.000 | +0.001 ± 0.000 |
| 1/48 | sine_mag | 10000 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | -0.007 ± 0.000 | -0.007 ± 0.000 | -0.000 ± 0.000 | -0.000 ± 0.000 |
| 1/48 | sine_phase | 15 | REW offline | direct (REW recording) | 2 | -0.074 ± 0.000 | -0.067 ± 0.000 | -0.001 ± 0.000 | -0.006 ± 0.001 |
| 1/48 | sine_phase | 15 | REW ÷ ref | direct (REW recording) | 2 | -0.073 ± 0.000 | -0.066 ± 0.000 | -0.001 ± 0.000 | -0.006 ± 0.001 |
| 1/48 | sine_phase | 15 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | +0.021 ± 0.000 | +0.002 ± 0.007 | +0.023 ± 0.015 | -0.003 ± 0.008 |
| 1/48 | sine_phase | 26.25 | REW offline | direct (REW recording) | 2 | -0.034 ± 0.002 | -0.037 ± 0.004 | +0.004 ± 0.000 | -0.002 ± 0.006 |
| 1/48 | sine_phase | 26.25 | REW ÷ ref | direct (REW recording) | 2 | -0.023 ± 0.002 | -0.026 ± 0.004 | +0.004 ± 0.000 | -0.002 ± 0.006 |
| 1/48 | sine_phase | 26.25 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | +0.006 ± 0.001 | +0.019 ± 0.006 | -0.015 ± 0.006 | +0.003 ± 0.002 |
| 1/48 | sine_phase | 35 | REW offline | direct (REW recording) | 2 | -0.018 ± 0.001 | -0.011 ± 0.001 | +0.000 ± 0.001 | -0.007 ± 0.002 |
| 1/48 | sine_phase | 35 | REW ÷ ref | direct (REW recording) | 2 | -0.013 ± 0.001 | -0.006 ± 0.001 | +0.000 ± 0.001 | -0.007 ± 0.002 |
| 1/48 | sine_phase | 35 | ac2 TF | direct (TF capture) | 2 | -0.036 ± 0.071 | -0.038 ± 0.081 | +0.009 ± 0.042 | -0.007 ± 0.032 |
| 1/48 | sine_phase | 35 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | +0.001 ± 0.001 | +0.013 ± 0.002 | -0.011 ± 0.001 | -0.001 ± 0.004 |
| 1/48 | sine_phase | 40.5 | REW offline | direct (REW recording) | 2 | +0.010 ± 0.001 | +0.011 ± 0.001 | -0.001 ± 0.002 | -0.000 ± 0.003 |
| 1/48 | sine_phase | 40.5 | REW ÷ ref | direct (REW recording) | 2 | +0.012 ± 0.001 | +0.013 ± 0.001 | -0.001 ± 0.002 | -0.000 ± 0.003 |
| 1/48 | sine_phase | 40.5 | ac2 TF | direct (TF capture) | 2 | -0.001 ± 0.015 | -0.001 ± 0.019 | -0.036 ± 0.061 | +0.036 ± 0.057 |
| 1/48 | sine_phase | 40.5 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | +0.003 ± 0.001 | -0.002 ± 0.001 | +0.002 ± 0.003 | +0.003 ± 0.001 |
| 1/48 | sine_phase | 56.2 | REW offline | direct (REW recording) | 2 | -0.004 ± 0.002 | -0.005 ± 0.001 | +0.002 ± 0.001 | -0.002 ± 0.000 |
| 1/48 | sine_phase | 56.2 | REW ÷ ref | direct (REW recording) | 2 | -0.004 ± 0.002 | -0.004 ± 0.001 | +0.002 ± 0.001 | -0.002 ± 0.000 |
| 1/48 | sine_phase | 56.2 | ac2 TF | direct (TF capture) | 2 | -0.007 ± 0.013 | -0.015 ± 0.001 | +0.006 ± 0.002 | +0.002 ± 0.013 |
| 1/48 | sine_phase | 56.2 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | +0.001 ± 0.001 | +0.004 ± 0.000 | -0.009 ± 0.005 | +0.006 ± 0.006 |
| 1/48 | sine_phase | 60.5 | REW offline | direct (REW recording) | 2 | -0.004 ± 0.001 | -0.004 ± 0.001 | -0.002 ± 0.000 | +0.002 ± 0.002 |
| 1/48 | sine_phase | 60.5 | REW ÷ ref | direct (REW recording) | 2 | -0.004 ± 0.001 | -0.004 ± 0.001 | -0.002 ± 0.000 | +0.002 ± 0.002 |
| 1/48 | sine_phase | 60.5 | ac2 TF | direct (TF capture) | 2 | +0.007 ± 0.007 | -0.001 ± 0.000 | +0.032 ± 0.030 | -0.024 ± 0.037 |
| 1/48 | sine_phase | 60.5 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | +0.000 ± 0.002 | +0.005 ± 0.001 | -0.003 ± 0.003 | -0.001 ± 0.004 |
| 1/48 | sine_phase | 93.9 | REW offline | direct (REW recording) | 2 | +0.002 ± 0.001 | +0.002 ± 0.000 | -0.000 ± 0.000 | +0.000 ± 0.001 |
| 1/48 | sine_phase | 93.9 | REW ÷ ref | direct (REW recording) | 2 | +0.002 ± 0.001 | +0.002 ± 0.000 | -0.000 ± 0.000 | +0.000 ± 0.001 |
| 1/48 | sine_phase | 93.9 | ac2 TF | direct (TF capture) | 2 | -0.002 ± 0.001 | -0.005 ± 0.004 | -0.008 ± 0.007 | +0.011 ± 0.012 |
| 1/48 | sine_phase | 93.9 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | -0.000 ± 0.001 | -0.003 ± 0.001 | +0.003 ± 0.002 | -0.001 ± 0.002 |
| 1/48 | sine_phase | 1040 | REW offline | direct (REW recording) | 2 | +0.000 ± 0.000 | +0.000 ± 0.000 | +0.002 ± 0.001 | -0.002 ± 0.000 |
| 1/48 | sine_phase | 1040 | REW ÷ ref | direct (REW recording) | 2 | +0.000 ± 0.000 | +0.000 ± 0.000 | +0.002 ± 0.001 | -0.002 ± 0.000 |
| 1/48 | sine_phase | 1040 | ac2 TF | direct (TF capture) | 2 | +0.003 ± 0.001 | -0.000 ± 0.000 | +0.004 ± 0.004 | +0.000 ± 0.003 |
| 1/48 | sine_phase | 1040 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | +0.000 ± 0.000 | +0.000 ± 0.000 | -0.001 ± 0.001 | +0.001 ± 0.001 |
| 1/48 | sine_phase | 10000 | REW offline | direct (REW recording) | 2 | +0.001 ± 0.000 | +0.000 ± 0.000 | +0.000 ± 0.000 | +0.001 ± 0.000 |
| 1/48 | sine_phase | 10000 | REW ÷ ref | direct (REW recording) | 2 | +0.001 ± 0.000 | +0.000 ± 0.000 | +0.000 ± 0.000 | +0.001 ± 0.000 |
| 1/48 | sine_phase | 10000 | ac2 TF | direct (TF capture) | 2 | +0.043 ± 0.004 | +0.014 ± 0.004 | -0.002 ± 0.001 | +0.030 ± 0.001 |
| 1/48 | sine_phase | 10000 | ac2 sweep 10Hz-5.5s | direct (ac2 capture 10Hz-5.5s) | 2 | +0.000 ± 0.000 | +0.000 ± 0.000 | -0.000 ± 0.001 | +0.000 ± 0.001 |

