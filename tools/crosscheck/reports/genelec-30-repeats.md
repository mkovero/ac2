# Repeated takes

Output of `python -m crosscheck repeats <runs>` (one call per resolution; the run directories
are under `/work/ac2-crosscheck/runs`, named below), all takes re-analysed with the same suite.
The per-check "Steady sines" tables are left out here: the column-to-tone split carries each
measurement's total against the sine and its parts. Rerun the command for them and for the
per-check statuses.

Signed values across takes: sd is the sample standard deviation (n − 1); `0 in mean±2se` is whether the mean lies within two standard errors of zero (no bias shown by these takes); `>pass` counts takes with |value| above the pass limit.

## genelec at -30 dBFS, 1/24 octave: 2 takes

Runs: 20261010T151259Z, 20261010T152956Z. ac2 builds: 0.0.0+bdc1bec656f7.

### Live TF vs its direct estimate, per band

| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |
|---|---|---|---|---|---|---|---|---|---|---|---|
| genelec.mag.ac2 TF|direct (TF capture).100-1000 | dB | 0 | | | | | | | | | 2×INCONCLUSIVE |
| genelec.mag.ac2 TF|direct (TF capture).20-100 | dB | 0 | | | | | | | | | 2×INCONCLUSIVE |
| genelec.phase.ac2 TF|direct (TF capture).100-1000 | ° | 0 | | | | | | | | | 2×INCONCLUSIVE |
| genelec.phase.ac2 TF|direct (TF capture).20-100 | ° | 0 | | | | | | | | | 2×INCONCLUSIVE |

### Arrival and delay

| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |
|---|---|---|---|---|---|---|---|---|---|---|---|
| genelec.delay.ac2_arrival.20Hz-5.5s | µs | 2 | -0.068 | 0.517 | -0.434 | +0.298 | yes | yes | 0/2 (>2) |  | 2×PASS |
| genelec.delay.ac2_arrival.dist | µs | 2 | +0.161 | 0.125 | +0.073 | +0.250 | no | yes | 0/2 (>2) |  | 2×PASS |
| genelec.delay.ac2_arrival.dist-probe | µs | 2 | +0.086 | 0.052 | +0.049 | +0.123 | no | no | 0/2 (>2) |  | 2×PASS |
| genelec.delay.capture_vs_rew_recording.20Hz-5.5s | µs | 2 | +1.818 | 0.088 | +1.756 | +1.880 | no | no |  |  | 2×INFO |
| genelec.delay.capture_vs_rew_recording.dist | µs | 2 | +1.673 | 0.338 | +1.435 | +1.912 | no | no |  |  | 2×INFO |
| genelec.delay.capture_vs_rew_recording.dist-probe | µs | 2 | +1.902 | 0.414 | +1.609 | +2.195 | no | no |  |  | 2×INFO |
| genelec.delay.rew.REW offline import.IR peak | µs | 2 | -3641.495 | 0.219 | -3641.650 | -3641.340 | no | no |  |  | 2×INFO |
| genelec.delay.rew.REW offline import.reported delay | µs | 2 | -3664.010 | 0.198 | -3664.150 | -3663.870 | no | no |  |  | 2×INFO |

### Column-to-tone split at the sines

measurement − sine = processing (measurement − its capture's 1/24-oct column) + column-to-tone (column − the same capture narrow at the sine) + capture vs sine (narrow − sine). Mean ± sd over the takes.

| resolution | quantity | f Hz | measurement | capture | takes | total | processing | column-to-tone | capture vs sine |
|---|---|---|---|---|---|---|---|---|---|
| 1/24 | sine_mag | 50 | REW offline | direct (REW recording) | 2 | +0.244 ± 0.020 | -0.314 ± 0.053 | +0.479 ± 0.045 | +0.079 ± 0.118 |
| 1/24 | sine_mag | 50 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.190 ± 0.102 | -0.353 ± 0.123 | +0.530 ± 0.021 | +0.013 ± 0.041 |
| 1/24 | sine_mag | 100 | REW offline | direct (REW recording) | 2 | -0.153 ± 0.021 | -0.032 ± 0.008 | -0.174 ± 0.146 | +0.053 ± 0.175 |
| 1/24 | sine_mag | 100 | ac2 TF | direct (TF capture) | 2 | -0.005 ± 0.042 | +0.058 ± 0.024 | -0.052 ± 0.046 | -0.011 ± 0.029 |
| 1/24 | sine_mag | 100 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -0.077 ± 0.007 | -0.010 ± 0.028 | -0.016 ± 0.181 | -0.052 ± 0.160 |
| 1/24 | sine_mag | 200 | REW offline | direct (REW recording) | 2 | +0.300 ± 0.011 | -0.008 ± 0.000 | +0.255 ± 0.090 | +0.053 ± 0.101 |
| 1/24 | sine_mag | 200 | ac2 TF | direct (TF capture) | 2 | +0.253 ± 0.038 | +0.001 ± 0.040 | +0.338 ± 0.003 | -0.086 ± 0.005 |
| 1/24 | sine_mag | 200 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.306 ± 0.002 | -0.007 ± 0.007 | +0.271 ± 0.015 | +0.043 ± 0.023 |
| 1/24 | sine_mag | 500 | REW offline | direct (REW recording) | 2 | +0.079 ± 0.006 | +0.002 ± 0.000 | +0.111 ± 0.005 | -0.034 ± 0.001 |
| 1/24 | sine_mag | 500 | ac2 TF | direct (TF capture) | 2 | +0.044 ± 0.005 | -0.062 ± 0.007 | +0.072 ± 0.001 | +0.034 ± 0.003 |
| 1/24 | sine_mag | 500 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.080 ± 0.006 | +0.001 ± 0.000 | +0.096 ± 0.028 | -0.017 ± 0.021 |
| 1/24 | sine_mag | 1000 | REW offline | direct (REW recording) | 2 | -0.624 ± 0.015 | +0.009 ± 0.000 | -0.592 ± 0.000 | -0.040 ± 0.016 |
| 1/24 | sine_mag | 1000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -0.609 ± 0.014 | +0.006 ± 0.002 | -0.607 ± 0.007 | -0.008 ± 0.009 |
| 1/24 | sine_mag | 2000 | REW offline | direct (REW recording) | 2 | -1.542 ± 0.020 | +0.010 ± 0.000 | -1.561 ± 0.007 | +0.009 ± 0.027 |
| 1/24 | sine_mag | 2000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -1.436 ± 0.007 | +0.004 ± 0.000 | -1.543 ± 0.002 | +0.103 ± 0.009 |
| 1/24 | sine_mag | 5000 | REW offline | direct (REW recording) | 2 | -0.136 ± 0.124 | +0.013 ± 0.001 | -0.111 ± 0.059 | -0.039 ± 0.182 |
| 1/24 | sine_mag | 5000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.118 ± 0.047 | -0.005 ± 0.001 | -0.131 ± 0.068 | +0.254 ± 0.115 |
| 1/24 | sine_mag | 10000 | REW offline | direct (REW recording) | 2 | +0.130 ± 0.125 | -0.024 ± 0.068 | +0.153 ± 0.050 | +0.000 ± 0.107 |
| 1/24 | sine_mag | 10000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.368 ± 0.035 | -0.005 ± 0.000 | +0.139 ± 0.015 | +0.234 ± 0.021 |
| 1/24 | sine_phase | 50 | REW offline | direct (REW recording) | 2 | -0.161 ± 0.444 | +0.919 ± 1.186 | -0.048 ± 3.303 | -1.032 ± 4.933 |
| 1/24 | sine_phase | 50 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -1.864 ± 3.481 | -1.365 ± 2.421 | +0.246 ± 1.784 | -0.745 ± 2.845 |
| 1/24 | sine_phase | 100 | REW offline | direct (REW recording) | 2 | +1.384 ± 0.132 | +1.988 ± 0.010 | -1.479 ± 1.528 | +0.875 ± 1.406 |
| 1/24 | sine_phase | 100 | ac2 TF | direct (TF capture) | 2 | -2.424 ± 0.718 | -2.101 ± 0.963 | -1.405 ± 0.244 | +1.083 ± 0.489 |
| 1/24 | sine_phase | 100 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -0.993 ± 0.220 | -0.073 ± 0.115 | -0.715 ± 0.510 | -0.205 ± 0.845 |
| 1/24 | sine_phase | 200 | REW offline | direct (REW recording) | 2 | +4.349 ± 0.028 | +3.394 ± 0.001 | +1.193 ± 0.003 | -0.238 ± 0.025 |
| 1/24 | sine_phase | 200 | ac2 TF | direct (TF capture) | 2 | +1.007 ± 0.019 | -0.023 ± 0.017 | +0.789 ± 0.087 | +0.241 ± 0.051 |
| 1/24 | sine_phase | 200 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.941 ± 0.058 | +0.009 ± 0.015 | +0.944 ± 0.040 | -0.012 ± 0.083 |
| 1/24 | sine_phase | 500 | REW offline | direct (REW recording) | 2 | +7.221 ± 0.046 | +8.473 ± 0.003 | -1.214 ± 0.108 | -0.038 ± 0.059 |
| 1/24 | sine_phase | 500 | ac2 TF | direct (TF capture) | 2 | -0.728 ± 0.455 | -0.135 ± 0.422 | -1.404 ± 0.179 | +0.810 ± 0.146 |
| 1/24 | sine_phase | 500 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -1.241 ± 0.040 | -0.013 ± 0.001 | -1.023 ± 0.151 | -0.204 ± 0.190 |
| 1/24 | sine_phase | 1000 | REW offline | direct (REW recording) | 2 | +15.366 ± 0.041 | +16.886 ± 0.001 | -1.304 ± 0.104 | -0.216 ± 0.065 |
| 1/24 | sine_phase | 1000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -1.501 ± 0.082 | -0.052 ± 0.011 | -1.284 ± 0.064 | -0.165 ± 0.156 |
| 1/24 | sine_phase | 2000 | REW offline | direct (REW recording) | 2 | +31.824 ± 0.397 | +33.863 ± 0.000 | -2.008 ± 0.000 | -0.031 ± 0.398 |
| 1/24 | sine_phase | 2000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -1.559 ± 0.075 | -0.076 ± 0.003 | -2.053 ± 0.058 | +0.570 ± 0.130 |
| 1/24 | sine_phase | 5000 | REW offline | direct (REW recording) | 2 | +65.354 ± 0.886 | +84.753 ± 0.002 | -18.517 ± 0.105 | -0.882 ± 0.779 |
| 1/24 | sine_phase | 5000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -18.788 ± 0.014 | -0.043 ± 0.001 | -18.457 ± 0.005 | -0.288 ± 0.020 |
| 1/24 | sine_phase | 10000 | REW offline | direct (REW recording) | 2 | +175.923 ± 0.667 | +169.416 ± 0.088 | +8.140 ± 0.275 | -1.634 ± 0.854 |
| 1/24 | sine_phase | 10000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +6.673 ± 0.491 | -0.064 ± 0.001 | +7.970 ± 0.608 | -1.233 ± 0.116 |

## genelec at -30 dBFS, 1/48 octave: 5 takes

Runs: 20261009T232923Z, 20261010T142303Z, 20261010T143133Z, 20261010T143900Z, 20261010T144613Z. ac2 builds: 0.0.0+8b87f492ebec, 0.0.0+bdc1bec656f7.

### Live TF vs its direct estimate, per band

| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |
|---|---|---|---|---|---|---|---|---|---|---|---|
| genelec.mag.ac2 TF|direct (TF capture).100-1000 (signed mean) | dB | 5 | -0.001 | 0.011 | -0.013 | +0.012 | yes | yes | 0/5 (>0.1) | ≤ 0.175 | 1×PASS, 4×WARN |
| genelec.mag.ac2 TF|direct (TF capture).1000-20000 | dB | 0 | | | | | | | | | 5×INCONCLUSIVE |
| genelec.mag.ac2 TF|direct (TF capture).20-100 | dB | 0 | | | | | | | | | 5×INCONCLUSIVE |
| genelec.phase.ac2 TF|direct (TF capture).100-1000 (signed mean) | ° | 5 | +0.012 | 0.078 | -0.047 | +0.148 | yes | yes | 0/5 (>3) | ≤ 0.890 | 5×PASS |
| genelec.phase.ac2 TF|direct (TF capture).1000-20000 | ° | 0 | | | | | | | | | 5×INCONCLUSIVE |
| genelec.phase.ac2 TF|direct (TF capture).20-100 | ° | 0 | | | | | | | | | 5×INCONCLUSIVE |

### Arrival and delay

| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |
|---|---|---|---|---|---|---|---|---|---|---|---|
| genelec.delay.ac2_arrival.20Hz-5.5s | µs | 5 | +0.173 | 0.377 | -0.395 | +0.553 | yes | yes | 0/5 (>2) |  | 5×PASS |
| genelec.delay.ac2_arrival.dist | µs | 5 | +0.097 | 0.106 | -0.026 | +0.210 | yes | no | 0/5 (>2) |  | 5×PASS |
| genelec.delay.ac2_arrival.dist-probe | µs | 5 | +0.248 | 0.468 | -0.325 | +0.780 | yes | yes | 0/5 (>2) |  | 5×PASS |
| genelec.delay.capture_vs_rew_recording.20Hz-5.5s | µs | 3 | +1.759 | 0.451 | +1.386 | +2.260 | no | no |  |  | 3×INFO |
| genelec.delay.capture_vs_rew_recording.dist | µs | 3 | +1.774 | 0.102 | +1.657 | +1.846 | no | no |  |  | 3×INFO |
| genelec.delay.capture_vs_rew_recording.dist-probe | µs | 3 | +1.981 | 0.308 | +1.635 | +2.226 | no | no |  |  | 3×INFO |
| genelec.delay.rew.REW offline import.IR peak | µs | 3 | -3640.023 | 2.199 | -3641.590 | -3637.510 | no | no |  |  | 3×INFO |
| genelec.delay.rew.REW offline import.reported delay | µs | 3 | -3662.650 | 2.153 | -3664.240 | -3660.200 | no | no |  |  | 3×INFO |

### Column-to-tone split at the sines

measurement − sine = processing (measurement − its capture's 1/48-oct column) + column-to-tone (column − the same capture narrow at the sine) + capture vs sine (narrow − sine). Mean ± sd over the takes.

| resolution | quantity | f Hz | measurement | capture | takes | total | processing | column-to-tone | capture vs sine |
|---|---|---|---|---|---|---|---|---|---|
| 1/48 | sine_mag | 50 | REW offline | direct (REW recording) | 3 | -0.027 ± 0.078 | -0.221 ± 0.128 | +0.211 ± 0.571 | -0.017 ± 0.668 |
| 1/48 | sine_mag | 50 | ac2 TF | direct (TF capture) | 1 | +0.042 | -0.112 | -0.032 | +0.186 |
| 1/48 | sine_mag | 50 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 5 | -0.033 ± 0.481 | -0.741 ± 1.254 | -0.006 ± 0.100 | +0.715 ± 0.765 |
| 1/48 | sine_mag | 100 | REW offline | direct (REW recording) | 3 | -0.073 ± 0.027 | -0.036 ± 0.021 | -0.011 ± 0.135 | -0.026 ± 0.140 |
| 1/48 | sine_mag | 100 | ac2 TF | direct (TF capture) | 5 | -0.111 ± 0.112 | -0.098 ± 0.111 | -0.052 ± 0.080 | +0.039 ± 0.074 |
| 1/48 | sine_mag | 100 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 5 | -0.054 ± 0.018 | -0.009 ± 0.015 | -0.058 ± 0.122 | +0.013 ± 0.127 |
| 1/48 | sine_mag | 200 | REW offline | direct (REW recording) | 3 | +0.028 ± 0.017 | +0.004 ± 0.005 | +0.024 ± 0.015 | -0.000 ± 0.023 |
| 1/48 | sine_mag | 200 | ac2 TF | direct (TF capture) | 5 | -0.020 ± 0.025 | +0.001 ± 0.021 | +0.008 ± 0.052 | -0.028 ± 0.055 |
| 1/48 | sine_mag | 200 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 5 | +0.017 ± 0.013 | +0.011 ± 0.010 | +0.026 ± 0.040 | -0.020 ± 0.049 |
| 1/48 | sine_mag | 500 | REW offline | direct (REW recording) | 3 | +0.020 ± 0.035 | +0.002 ± 0.001 | +0.060 ± 0.037 | -0.042 ± 0.006 |
| 1/48 | sine_mag | 500 | ac2 TF | direct (TF capture) | 5 | +0.010 ± 0.024 | -0.042 ± 0.011 | -0.008 ± 0.036 | +0.060 ± 0.038 |
| 1/48 | sine_mag | 500 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 5 | +0.004 ± 0.021 | +0.001 ± 0.002 | +0.008 ± 0.034 | -0.004 ± 0.019 |
| 1/48 | sine_mag | 1000 | REW offline | direct (REW recording) | 3 | -0.309 ± 0.025 | +0.013 ± 0.001 | -0.272 ± 0.021 | -0.049 ± 0.022 |
| 1/48 | sine_mag | 1000 | ac2 TF | direct (TF capture) | 4 | -0.528 ± 0.149 | -0.202 ± 0.167 | -0.307 ± 0.040 | -0.020 ± 0.019 |
| 1/48 | sine_mag | 1000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 5 | -0.334 ± 0.020 | +0.001 ± 0.001 | -0.303 ± 0.013 | -0.032 ± 0.010 |
| 1/48 | sine_mag | 2000 | REW offline | direct (REW recording) | 3 | -0.667 ± 0.011 | +0.010 ± 0.001 | -0.718 ± 0.002 | +0.041 ± 0.012 |
| 1/48 | sine_mag | 2000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 5 | -0.641 ± 0.050 | -0.003 ± 0.001 | -0.680 ± 0.011 | +0.042 ± 0.051 |
| 1/48 | sine_mag | 5000 | REW offline | direct (REW recording) | 3 | +0.468 ± 0.745 | -0.000 ± 0.001 | +0.446 ± 0.696 | +0.022 ± 0.075 |
| 1/48 | sine_mag | 5000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 5 | +0.350 ± 0.632 | -0.003 ± 0.001 | +0.242 ± 0.590 | +0.111 ± 0.083 |
| 1/48 | sine_mag | 10000 | REW offline | direct (REW recording) | 3 | +0.575 ± 0.594 | -0.003 ± 0.032 | +0.558 ± 0.568 | +0.020 ± 0.023 |
| 1/48 | sine_mag | 10000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 5 | +0.817 ± 0.444 | -0.000 ± 0.000 | +0.716 ± 0.435 | +0.101 ± 0.089 |
| 1/48 | sine_phase | 50 | REW offline | direct (REW recording) | 3 | -0.239 ± 0.575 | -0.514 ± 1.830 | -0.623 ± 1.433 | +0.898 ± 3.090 |
| 1/48 | sine_phase | 50 | ac2 TF | direct (TF capture) | 1 | -2.428 | -1.720 | -0.083 | -0.626 |
| 1/48 | sine_phase | 50 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 5 | +0.155 ± 0.385 | +1.156 ± 1.867 | -2.390 ± 6.784 | +1.390 ± 5.359 |
| 1/48 | sine_phase | 100 | REW offline | direct (REW recording) | 3 | +0.326 ± 0.428 | +0.209 ± 0.030 | -0.008 ± 0.649 | +0.125 ± 0.896 |
| 1/48 | sine_phase | 100 | ac2 TF | direct (TF capture) | 5 | +0.747 ± 0.501 | +0.591 ± 0.398 | -0.341 ± 0.479 | +0.497 ± 0.374 |
| 1/48 | sine_phase | 100 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 5 | -0.060 ± 0.218 | -0.103 ± 0.164 | -0.077 ± 0.496 | +0.120 ± 0.420 |
| 1/48 | sine_phase | 200 | REW offline | direct (REW recording) | 3 | +0.105 ± 0.101 | -0.033 ± 0.045 | -0.049 ± 0.375 | +0.187 ± 0.430 |
| 1/48 | sine_phase | 200 | ac2 TF | direct (TF capture) | 5 | +0.312 ± 0.091 | +0.026 ± 0.089 | +0.180 ± 0.241 | +0.107 ± 0.257 |
| 1/48 | sine_phase | 200 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 5 | +0.138 ± 0.037 | +0.076 ± 0.059 | -0.063 ± 0.438 | +0.125 ± 0.485 |
| 1/48 | sine_phase | 500 | REW offline | direct (REW recording) | 3 | -1.134 ± 0.070 | -0.032 ± 0.004 | -1.082 ± 0.124 | -0.020 ± 0.088 |
| 1/48 | sine_phase | 500 | ac2 TF | direct (TF capture) | 5 | -0.237 ± 0.149 | +0.155 ± 0.150 | -1.389 ± 0.225 | +0.996 ± 0.231 |
| 1/48 | sine_phase | 500 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 5 | -1.101 ± 0.091 | -0.002 ± 0.006 | -1.040 ± 0.093 | -0.059 ± 0.098 |
| 1/48 | sine_phase | 1000 | REW offline | direct (REW recording) | 3 | -0.274 ± 0.211 | -0.085 ± 0.002 | +0.036 ± 0.367 | -0.225 ± 0.199 |
| 1/48 | sine_phase | 1000 | ac2 TF | direct (TF capture) | 4 | +0.676 ± 1.214 | +0.974 ± 1.446 | -0.087 ± 0.489 | -0.211 ± 0.119 |
| 1/48 | sine_phase | 1000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 5 | +0.024 ± 0.199 | -0.022 ± 0.012 | +0.051 ± 0.269 | -0.004 ± 0.171 |
| 1/48 | sine_phase | 2000 | REW offline | direct (REW recording) | 3 | +0.393 ± 0.642 | -0.020 ± 0.011 | +0.276 ± 0.526 | +0.138 ± 0.388 |
| 1/48 | sine_phase | 2000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 5 | +0.895 ± 0.444 | -0.023 ± 0.007 | +0.634 ± 0.371 | +0.284 ± 0.255 |
| 1/48 | sine_phase | 5000 | REW offline | direct (REW recording) | 3 | -11.544 ± 1.990 | -0.025 ± 0.001 | -11.458 ± 1.633 | -0.061 ± 0.368 |
| 1/48 | sine_phase | 5000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 5 | -11.636 ± 1.750 | -0.015 ± 0.004 | -12.031 ± 1.122 | +0.410 ± 0.706 |
| 1/48 | sine_phase | 10000 | REW offline | direct (REW recording) | 3 | +10.863 ± 2.482 | +0.091 ± 0.517 | +11.226 ± 2.158 | -0.454 ± 0.259 |
| 1/48 | sine_phase | 10000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 5 | +10.668 ± 1.909 | -0.033 ± 0.003 | +10.523 ± 1.757 | +0.179 ± 0.734 |

## genelec at -30 dBFS, 1/96 octave: 2 takes

Runs: 20261010T150452Z, 20261010T152112Z. ac2 builds: 0.0.0+bdc1bec656f7.

### Live TF vs its direct estimate, per band

| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |
|---|---|---|---|---|---|---|---|---|---|---|---|
| genelec.mag.ac2 TF|direct (TF capture).100-1000 (signed mean) | dB | 2 | -0.005 | 0.001 | -0.006 | -0.004 | no | no | 0/2 (>0.1) | ≤ 0.300 | 1×FAIL, 1×WARN |
| genelec.mag.ac2 TF|direct (TF capture).1000-20000 (signed mean) | dB | 2 | +0.041 | 0.079 | -0.015 | +0.098 | yes | yes | 0/2 (>0.1) | ≤ 0.099 | 2×PASS |
| genelec.mag.ac2 TF|direct (TF capture).20-100 | dB | 0 | | | | | | | | | 2×INCONCLUSIVE |
| genelec.phase.ac2 TF|direct (TF capture).100-1000 (signed mean) | ° | 2 | -0.019 | 0.071 | -0.069 | +0.030 | yes | yes | 0/2 (>3) | ≤ 1.811 | 2×PASS |
| genelec.phase.ac2 TF|direct (TF capture).1000-20000 (signed mean) | ° | 2 | -0.075 | 0.338 | -0.314 | +0.164 | yes | yes | 0/2 (>3) | ≤ 0.492 | 2×PASS |
| genelec.phase.ac2 TF|direct (TF capture).20-100 | ° | 0 | | | | | | | | | 2×INCONCLUSIVE |

### Arrival and delay

| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |
|---|---|---|---|---|---|---|---|---|---|---|---|
| genelec.delay.ac2_arrival.20Hz-5.5s | µs | 2 | +0.116 | 0.081 | +0.059 | +0.173 | no | no | 0/2 (>2) |  | 2×PASS |
| genelec.delay.ac2_arrival.dist | µs | 2 | -0.017 | 0.006 | -0.021 | -0.012 | no | no | 0/2 (>2) |  | 2×PASS |
| genelec.delay.ac2_arrival.dist-probe | µs | 2 | -0.336 | 0.206 | -0.481 | -0.190 | no | no | 0/2 (>2) |  | 2×PASS |
| genelec.delay.capture_vs_rew_recording.20Hz-5.5s | µs | 2 | +1.591 | 0.072 | +1.540 | +1.642 | no | no |  |  | 2×INFO |
| genelec.delay.capture_vs_rew_recording.dist | µs | 2 | +1.845 | 0.022 | +1.830 | +1.861 | no | no |  |  | 2×INFO |
| genelec.delay.capture_vs_rew_recording.dist-probe | µs | 2 | +2.172 | 0.208 | +2.024 | +2.319 | no | no |  |  | 2×INFO |
| genelec.delay.rew.REW offline import.IR peak | µs | 2 | -3641.345 | 0.120 | -3641.430 | -3641.260 | no | no |  |  | 2×INFO |
| genelec.delay.rew.REW offline import.reported delay | µs | 2 | -3663.870 | 0.071 | -3663.920 | -3663.820 | no | no |  |  | 2×INFO |

### Column-to-tone split at the sines

measurement − sine = processing (measurement − its capture's 1/96-oct column) + column-to-tone (column − the same capture narrow at the sine) + capture vs sine (narrow − sine). Mean ± sd over the takes.

| resolution | quantity | f Hz | measurement | capture | takes | total | processing | column-to-tone | capture vs sine |
|---|---|---|---|---|---|---|---|---|---|
| 1/96 | sine_mag | 50 | REW offline | direct (REW recording) | 2 | -0.173 ± 0.056 | -0.643 ± 0.129 | +0.471 ± 0.487 | -0.000 ± 0.414 |
| 1/96 | sine_mag | 50 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.352 ± 0.193 | +0.124 ± 0.246 | +0.396 ± 0.550 | -0.167 ± 0.602 |
| 1/96 | sine_mag | 100 | REW offline | direct (REW recording) | 2 | -0.105 ± 0.111 | -0.122 ± 0.026 | +0.010 ± 0.093 | +0.007 ± 0.178 |
| 1/96 | sine_mag | 100 | ac2 TF | direct (TF capture) | 2 | +0.095 ± 0.081 | +0.087 ± 0.071 | -0.070 ± 0.057 | +0.078 ± 0.047 |
| 1/96 | sine_mag | 100 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.049 ± 0.023 | +0.048 ± 0.157 | -0.025 ± 0.034 | +0.025 ± 0.101 |
| 1/96 | sine_mag | 200 | REW offline | direct (REW recording) | 2 | +0.030 ± 0.067 | -0.003 ± 0.032 | -0.041 ± 0.011 | +0.073 ± 0.045 |
| 1/96 | sine_mag | 200 | ac2 TF | direct (TF capture) | 2 | -0.023 ± 0.011 | +0.027 ± 0.048 | +0.022 ± 0.033 | -0.072 ± 0.004 |
| 1/96 | sine_mag | 200 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.019 ± 0.001 | +0.006 ± 0.001 | -0.037 ± 0.010 | +0.050 ± 0.010 |
| 1/96 | sine_mag | 500 | REW offline | direct (REW recording) | 2 | +0.052 ± 0.002 | +0.005 ± 0.003 | +0.060 ± 0.027 | -0.013 ± 0.027 |
| 1/96 | sine_mag | 500 | ac2 TF | direct (TF capture) | 2 | +0.110 ± 0.022 | +0.042 ± 0.018 | +0.004 ± 0.021 | +0.064 ± 0.024 |
| 1/96 | sine_mag | 500 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.032 ± 0.006 | +0.001 ± 0.002 | +0.026 ± 0.019 | +0.005 ± 0.015 |
| 1/96 | sine_mag | 1000 | REW offline | direct (REW recording) | 2 | -0.173 ± 0.013 | -0.006 ± 0.000 | -0.137 ± 0.009 | -0.030 ± 0.003 |
| 1/96 | sine_mag | 1000 | ac2 TF | direct (TF capture) | 2 | -0.119 ± 0.069 | +0.085 ± 0.045 | -0.204 ± 0.059 | -0.001 ± 0.035 |
| 1/96 | sine_mag | 1000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -0.176 ± 0.002 | +0.000 ± 0.000 | -0.153 ± 0.009 | -0.023 ± 0.007 |
| 1/96 | sine_mag | 2000 | REW offline | direct (REW recording) | 2 | -0.312 ± 0.163 | -0.003 ± 0.000 | -0.274 ± 0.002 | -0.035 ± 0.161 |
| 1/96 | sine_mag | 2000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -0.197 ± 0.001 | -0.003 ± 0.002 | -0.244 ± 0.016 | +0.050 ± 0.016 |
| 1/96 | sine_mag | 5000 | REW offline | direct (REW recording) | 2 | +0.273 ± 0.304 | +0.009 ± 0.001 | +0.436 ± 0.015 | -0.172 ± 0.288 |
| 1/96 | sine_mag | 5000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.530 ± 0.005 | +0.001 ± 0.001 | +0.409 ± 0.017 | +0.121 ± 0.022 |
| 1/96 | sine_mag | 10000 | REW offline | direct (REW recording) | 2 | +0.876 ± 0.257 | -0.027 ± 0.071 | +0.950 ± 0.004 | -0.046 ± 0.183 |
| 1/96 | sine_mag | 10000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +1.059 ± 0.052 | -0.008 ± 0.000 | +0.963 ± 0.004 | +0.104 ± 0.057 |
| 1/96 | sine_phase | 50 | REW offline | direct (REW recording) | 2 | +1.854 ± 3.470 | +0.119 ± 2.473 | +1.083 ± 4.768 | +0.651 ± 5.765 |
| 1/96 | sine_phase | 50 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.959 ± 0.081 | -0.462 ± 1.163 | +2.717 ± 1.890 | -1.295 ± 0.645 |
| 1/96 | sine_phase | 100 | REW offline | direct (REW recording) | 2 | +0.303 ± 0.367 | +0.632 ± 1.182 | -0.302 ± 0.612 | -0.027 ± 1.427 |
| 1/96 | sine_phase | 100 | ac2 TF | direct (TF capture) | 2 | +0.377 ± 0.522 | +0.001 ± 0.848 | +0.777 ± 0.134 | -0.402 ± 0.460 |
| 1/96 | sine_phase | 100 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.119 ± 0.107 | +0.221 ± 0.092 | +0.312 ± 0.274 | -0.414 ± 0.473 |
| 1/96 | sine_phase | 200 | REW offline | direct (REW recording) | 2 | -0.075 ± 0.231 | +0.003 ± 0.070 | -0.377 ± 0.740 | +0.299 ± 0.579 |
| 1/96 | sine_phase | 200 | ac2 TF | direct (TF capture) | 2 | -0.052 ± 0.113 | -0.157 ± 0.159 | +0.093 ± 0.000 | +0.012 ± 0.046 |
| 1/96 | sine_phase | 200 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.159 ± 0.123 | -0.143 ± 0.191 | +0.235 ± 0.225 | +0.067 ± 0.089 |
| 1/96 | sine_phase | 500 | REW offline | direct (REW recording) | 2 | -0.493 ± 0.080 | -0.032 ± 0.008 | -0.498 ± 0.154 | +0.037 ± 0.226 |
| 1/96 | sine_phase | 500 | ac2 TF | direct (TF capture) | 2 | +0.035 ± 0.147 | -0.253 ± 0.170 | -0.422 ± 0.070 | +0.710 ± 0.048 |
| 1/96 | sine_phase | 500 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -0.385 ± 0.026 | +0.007 ± 0.003 | -0.237 ± 0.019 | -0.156 ± 0.042 |
| 1/96 | sine_phase | 1000 | REW offline | direct (REW recording) | 2 | +0.085 ± 0.095 | +0.047 ± 0.002 | +0.246 ± 0.059 | -0.208 ± 0.152 |
| 1/96 | sine_phase | 1000 | ac2 TF | direct (TF capture) | 2 | -1.051 ± 0.729 | -1.144 ± 0.340 | +0.092 ± 0.494 | +0.001 ± 0.104 |
| 1/96 | sine_phase | 1000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.190 ± 0.169 | -0.009 ± 0.005 | +0.213 ± 0.034 | -0.014 ± 0.209 |
| 1/96 | sine_phase | 2000 | REW offline | direct (REW recording) | 2 | -0.316 ± 0.724 | -0.082 ± 0.001 | -0.257 ± 0.025 | +0.023 ± 0.748 |
| 1/96 | sine_phase | 2000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +0.536 ± 0.293 | -0.035 ± 0.017 | +0.090 ± 0.079 | +0.481 ± 0.355 |
| 1/96 | sine_phase | 5000 | REW offline | direct (REW recording) | 2 | -7.697 ± 1.045 | -0.033 ± 0.000 | -7.319 ± 0.194 | -0.345 ± 1.238 |
| 1/96 | sine_phase | 5000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | -6.896 ± 0.557 | +0.003 ± 0.005 | -7.221 ± 0.197 | +0.322 ± 0.749 |
| 1/96 | sine_phase | 10000 | REW offline | direct (REW recording) | 2 | +5.630 ± 0.772 | +0.141 ± 0.122 | +5.698 ± 0.118 | -0.209 ± 1.012 |
| 1/96 | sine_phase | 10000 | ac2 sweep 20Hz-5.5s | direct (ac2 capture 20Hz-5.5s) | 2 | +6.277 ± 0.741 | -0.004 ± 0.001 | +5.722 ± 0.053 | +0.560 ± 0.793 |

