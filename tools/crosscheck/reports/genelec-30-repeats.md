# Repeated takes

## genelec at -30 dBFS, 1/24 octave: 2 takes

Runs: 20261010T151259Z, 20261010T152956Z. ac2 builds: 0.0.0+bdc1bec656f7.

Signed values across takes: sd is the sample standard deviation (n − 1); `0 in mean±2se` is whether the mean lies within two standard errors of zero (no bias shown by these takes); `>pass` counts takes with |value| above the pass limit.

### Steady sines: magnitude

| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |
|---|---|---|---|---|---|---|---|---|---|---|---|
| genelec.sine_mag.REW offline@10000Hz | dB | 2 | +0.130 | 0.125 | +0.041 | +0.218 | no | yes | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.REW offline@1000Hz | dB | 2 | -0.624 | 0.015 | -0.635 | -0.613 | no | no | 2/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.REW offline@100Hz | dB | 2 | -0.153 | 0.021 | -0.167 | -0.138 | no | no | 0/2 (>0.3) |  | 1×INCONCLUSIVE, 1×PASS |
| genelec.sine_mag.REW offline@2000Hz | dB | 2 | -1.542 | 0.020 | -1.556 | -1.528 | no | no | 2/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.REW offline@200Hz | dB | 2 | +0.300 | 0.011 | +0.292 | +0.307 | no | no | 1/2 (>0.3) |  | 1×INCONCLUSIVE, 1×METHOD |
| genelec.sine_mag.REW offline@5000Hz | dB | 2 | -0.136 | 0.124 | -0.224 | -0.048 | no | yes | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.REW offline@500Hz | dB | 2 | +0.079 | 0.006 | +0.075 | +0.083 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.REW offline@50Hz | dB | 2 | +0.244 | 0.020 | +0.230 | +0.258 | no | no | 0/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.ac2 TF@100Hz | dB | 2 | -0.005 | 0.042 | -0.035 | +0.024 | yes | yes | 0/2 (>0.3) |  | 1×INCONCLUSIVE, 1×PASS |
| genelec.sine_mag.ac2 TF@200Hz | dB | 2 | +0.253 | 0.038 | +0.227 | +0.280 | no | no | 0/2 (>0.3) |  | 1×INCONCLUSIVE, 1×PASS |
| genelec.sine_mag.ac2 TF@500Hz | dB | 2 | +0.044 | 0.005 | +0.040 | +0.047 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@10000Hz | dB | 2 | +0.368 | 0.035 | +0.343 | +0.393 | no | no | 2/2 (>0.3) |  | 2×WARN |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@1000Hz | dB | 2 | -0.609 | 0.014 | -0.619 | -0.599 | no | no | 2/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@100Hz | dB | 2 | -0.077 | 0.007 | -0.083 | -0.072 | no | no | 0/2 (>0.3) |  | 1×INCONCLUSIVE, 1×PASS |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@2000Hz | dB | 2 | -1.436 | 0.007 | -1.441 | -1.431 | no | no | 2/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@200Hz | dB | 2 | +0.306 | 0.002 | +0.305 | +0.307 | no | no | 2/2 (>0.3) |  | 1×INCONCLUSIVE, 1×METHOD |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@5000Hz | dB | 2 | +0.118 | 0.047 | +0.085 | +0.152 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@500Hz | dB | 2 | +0.080 | 0.006 | +0.076 | +0.085 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@50Hz | dB | 2 | +0.190 | 0.102 | +0.118 | +0.262 | no | no | 0/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.direct (REW recording), narrow at the sine@10000Hz | dB | 2 | +0.000 | 0.107 | -0.075 | +0.076 | yes | yes | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@1000Hz | dB | 2 | -0.040 | 0.016 | -0.051 | -0.029 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@100Hz | dB | 2 | +0.053 | 0.175 | -0.070 | +0.177 | yes | yes | 0/2 (>0.3) |  | 1×INCONCLUSIVE, 1×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@2000Hz | dB | 2 | +0.009 | 0.027 | -0.010 | +0.029 | yes | yes | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@200Hz | dB | 2 | +0.053 | 0.101 | -0.019 | +0.125 | yes | yes | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@5000Hz | dB | 2 | -0.039 | 0.182 | -0.168 | +0.090 | yes | yes | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@500Hz | dB | 2 | -0.034 | 0.001 | -0.035 | -0.034 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@50Hz | dB | 2 | +0.079 | 0.118 | -0.004 | +0.163 | yes | yes | 0/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.direct (REW recording)@10000Hz | dB | 2 | +0.154 | 0.057 | +0.113 | +0.194 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording)@1000Hz | dB | 2 | -0.632 | 0.016 | -0.643 | -0.621 | no | no | 2/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.direct (REW recording)@100Hz | dB | 2 | -0.121 | 0.029 | -0.141 | -0.101 | no | no | 0/2 (>0.3) |  | 1×INCONCLUSIVE, 1×PASS |
| genelec.sine_mag.direct (REW recording)@2000Hz | dB | 2 | -1.552 | 0.020 | -1.566 | -1.538 | no | no | 2/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.direct (REW recording)@200Hz | dB | 2 | +0.308 | 0.011 | +0.300 | +0.316 | no | no | 2/2 (>0.3) |  | 1×INCONCLUSIVE, 1×WARN |
| genelec.sine_mag.direct (REW recording)@5000Hz | dB | 2 | -0.149 | 0.123 | -0.236 | -0.062 | no | yes | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording)@500Hz | dB | 2 | +0.076 | 0.006 | +0.072 | +0.080 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording)@50Hz | dB | 2 | +0.558 | 0.073 | +0.506 | +0.609 | no | no | 2/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.direct (TF capture), narrow at the sine@10000Hz | dB | 2 | -1.583 | 0.153 | -1.691 | -1.475 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@1000Hz | dB | 2 | +0.001 | 0.003 | -0.002 | +0.003 | yes | yes | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@100Hz | dB | 2 | -0.011 | 0.029 | -0.031 | +0.009 | yes | yes | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@2000Hz | dB | 2 | -0.959 | 0.106 | -1.034 | -0.884 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@200Hz | dB | 2 | -0.086 | 0.005 | -0.089 | -0.082 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@5000Hz | dB | 2 | -1.737 | 0.076 | -1.790 | -1.683 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@500Hz | dB | 2 | +0.034 | 0.003 | +0.032 | +0.036 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@50Hz | dB | 2 | +0.021 | 0.162 | -0.093 | +0.136 | yes | yes | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture)@10000Hz | dB | 2 | -1.450 | 0.113 | -1.530 | -1.370 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture)@1000Hz | dB | 2 | -0.660 | 0.018 | -0.673 | -0.648 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture)@100Hz | dB | 2 | -0.063 | 0.017 | -0.075 | -0.051 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture)@2000Hz | dB | 2 | -2.546 | 0.079 | -2.602 | -2.491 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture)@200Hz | dB | 2 | +0.252 | 0.002 | +0.251 | +0.254 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture)@5000Hz | dB | 2 | -1.841 | 0.182 | -1.969 | -1.712 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture)@500Hz | dB | 2 | +0.106 | 0.002 | +0.105 | +0.107 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture)@50Hz | dB | 2 | +0.488 | 0.160 | +0.375 | +0.601 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@10000Hz | dB | 2 | +0.234 | 0.021 | +0.220 | +0.249 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@1000Hz | dB | 2 | -0.008 | 0.009 | -0.014 | -0.002 | no | yes | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@100Hz | dB | 2 | -0.052 | 0.160 | -0.165 | +0.062 | yes | yes | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@2000Hz | dB | 2 | +0.103 | 0.009 | +0.097 | +0.109 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@200Hz | dB | 2 | +0.043 | 0.023 | +0.027 | +0.059 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@5000Hz | dB | 2 | +0.254 | 0.115 | +0.173 | +0.336 | no | no | 1/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@500Hz | dB | 2 | -0.017 | 0.021 | -0.032 | -0.002 | no | yes | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@50Hz | dB | 2 | +0.013 | 0.041 | -0.016 | +0.042 | yes | yes | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@10000Hz | dB | 2 | +0.373 | 0.035 | +0.348 | +0.398 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@1000Hz | dB | 2 | -0.615 | 0.016 | -0.627 | -0.604 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@100Hz | dB | 2 | -0.068 | 0.021 | -0.082 | -0.053 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@2000Hz | dB | 2 | -1.440 | 0.007 | -1.444 | -1.435 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@200Hz | dB | 2 | +0.314 | 0.008 | +0.308 | +0.319 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@5000Hz | dB | 2 | +0.123 | 0.047 | +0.090 | +0.156 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@500Hz | dB | 2 | +0.079 | 0.006 | +0.074 | +0.084 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@50Hz | dB | 2 | +0.543 | 0.021 | +0.528 | +0.557 | no | no | 2/2 (>0.3) |  | 2×INFO |

### Steady sines: phase

| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |
|---|---|---|---|---|---|---|---|---|---|---|---|
| genelec.sine_phase.REW offline@10000Hz | ° | 2 | +175.923 | 0.667 | +175.451 | +176.394 | no | no | 2/2 (>3) |  | 2×FAIL |
| genelec.sine_phase.REW offline@1000Hz | ° | 2 | +15.366 | 0.041 | +15.337 | +15.394 | no | no | 2/2 (>3) |  | 2×FAIL |
| genelec.sine_phase.REW offline@100Hz | ° | 2 | +1.384 | 0.132 | +1.291 | +1.478 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.REW offline@2000Hz | ° | 2 | +31.824 | 0.397 | +31.543 | +32.105 | no | no | 2/2 (>3) |  | 2×FAIL |
| genelec.sine_phase.REW offline@200Hz | ° | 2 | +4.349 | 0.028 | +4.330 | +4.369 | no | no | 2/2 (>3) |  | 2×WARN |
| genelec.sine_phase.REW offline@5000Hz | ° | 2 | +65.354 | 0.886 | +64.728 | +65.981 | no | no | 2/2 (>3) |  | 2×FAIL |
| genelec.sine_phase.REW offline@500Hz | ° | 2 | +7.221 | 0.046 | +7.189 | +7.254 | no | no | 2/2 (>3) |  | 2×WARN |
| genelec.sine_phase.REW offline@50Hz | ° | 2 | -0.161 | 0.444 | -0.474 | +0.153 | yes | yes | 0/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.ac2 TF@100Hz | ° | 2 | -2.424 | 0.718 | -2.931 | -1.916 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.ac2 TF@200Hz | ° | 2 | +1.007 | 0.019 | +0.994 | +1.020 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.ac2 TF@500Hz | ° | 2 | -0.728 | 0.455 | -1.050 | -0.406 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@10000Hz | ° | 2 | +6.673 | 0.491 | +6.326 | +7.020 | no | no | 2/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@1000Hz | ° | 2 | -1.501 | 0.082 | -1.559 | -1.443 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@100Hz | ° | 2 | -0.993 | 0.220 | -1.149 | -0.837 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@2000Hz | ° | 2 | -1.559 | 0.075 | -1.612 | -1.506 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@200Hz | ° | 2 | +0.941 | 0.058 | +0.900 | +0.982 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@5000Hz | ° | 2 | -18.788 | 0.014 | -18.798 | -18.778 | no | no | 2/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@500Hz | ° | 2 | -1.241 | 0.040 | -1.269 | -1.212 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@50Hz | ° | 2 | -1.864 | 3.481 | -4.326 | +0.598 | yes | yes | 1/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.direct (REW recording), narrow at the sine@10000Hz | ° | 2 | -1.634 | 0.854 | -2.238 | -1.030 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@1000Hz | ° | 2 | -0.216 | 0.065 | -0.262 | -0.170 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@100Hz | ° | 2 | +0.875 | 1.406 | -0.119 | +1.869 | yes | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@2000Hz | ° | 2 | -0.031 | 0.398 | -0.312 | +0.250 | yes | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@200Hz | ° | 2 | -0.238 | 0.025 | -0.256 | -0.220 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@5000Hz | ° | 2 | -0.882 | 0.779 | -1.432 | -0.331 | no | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@500Hz | ° | 2 | -0.038 | 0.059 | -0.079 | +0.004 | yes | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@50Hz | ° | 2 | -1.032 | 4.933 | -4.520 | +2.457 | yes | yes | 1/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.direct (REW recording)@10000Hz | ° | 2 | +6.506 | 0.578 | +6.097 | +6.915 | no | no | 2/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.direct (REW recording)@1000Hz | ° | 2 | -1.520 | 0.040 | -1.548 | -1.492 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording)@100Hz | ° | 2 | -0.604 | 0.122 | -0.691 | -0.517 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording)@2000Hz | ° | 2 | -2.039 | 0.397 | -2.320 | -1.758 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording)@200Hz | ° | 2 | +0.955 | 0.029 | +0.935 | +0.975 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording)@5000Hz | ° | 2 | -19.399 | 0.884 | -20.024 | -18.774 | no | no | 2/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.direct (REW recording)@500Hz | ° | 2 | -1.251 | 0.049 | -1.286 | -1.217 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording)@50Hz | ° | 2 | -1.080 | 1.630 | -2.233 | +0.072 | yes | yes | 0/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.direct (TF capture), narrow at the sine@10000Hz | ° | 2 | +0.150 | 0.253 | -0.029 | +0.328 | yes | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@1000Hz | ° | 2 | -0.007 | 0.861 | -0.616 | +0.602 | yes | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@100Hz | ° | 2 | +1.083 | 0.489 | +0.737 | +1.429 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@2000Hz | ° | 2 | -4.175 | 0.529 | -4.550 | -3.801 | no | no | 2/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@200Hz | ° | 2 | +0.241 | 0.051 | +0.205 | +0.278 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@5000Hz | ° | 2 | -2.758 | 0.115 | -2.839 | -2.677 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@500Hz | ° | 2 | +0.810 | 0.146 | +0.707 | +0.913 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@50Hz | ° | 2 | -0.916 | 2.541 | -2.713 | +0.881 | yes | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture)@10000Hz | ° | 2 | +8.132 | 0.306 | +7.916 | +8.349 | no | no | 2/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture)@1000Hz | ° | 2 | -1.567 | 0.031 | -1.589 | -1.546 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture)@100Hz | ° | 2 | -0.322 | 0.245 | -0.496 | -0.149 | no | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture)@2000Hz | ° | 2 | -5.966 | 0.205 | -6.111 | -5.821 | no | no | 2/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture)@200Hz | ° | 2 | +1.030 | 0.036 | +1.005 | +1.055 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture)@5000Hz | ° | 2 | -21.662 | 0.252 | -21.840 | -21.485 | no | no | 2/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture)@500Hz | ° | 2 | -0.594 | 0.033 | -0.617 | -0.570 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture)@50Hz | ° | 2 | +0.395 | 0.731 | -0.123 | +0.912 | yes | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@10000Hz | ° | 2 | -1.233 | 0.116 | -1.314 | -1.151 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@1000Hz | ° | 2 | -0.165 | 0.156 | -0.276 | -0.055 | no | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@100Hz | ° | 2 | -0.205 | 0.845 | -0.803 | +0.392 | yes | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@2000Hz | ° | 2 | +0.570 | 0.130 | +0.478 | +0.662 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@200Hz | ° | 2 | -0.012 | 0.083 | -0.071 | +0.047 | yes | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@5000Hz | ° | 2 | -0.288 | 0.020 | -0.302 | -0.274 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@500Hz | ° | 2 | -0.204 | 0.190 | -0.339 | -0.070 | no | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@50Hz | ° | 2 | -0.745 | 2.845 | -2.756 | +1.267 | yes | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@10000Hz | ° | 2 | +6.737 | 0.492 | +6.389 | +7.085 | no | no | 2/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@1000Hz | ° | 2 | -1.449 | 0.092 | -1.514 | -1.384 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@100Hz | ° | 2 | -0.920 | 0.335 | -1.157 | -0.683 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@2000Hz | ° | 2 | -1.483 | 0.072 | -1.534 | -1.432 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@200Hz | ° | 2 | +0.932 | 0.043 | +0.902 | +0.963 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@5000Hz | ° | 2 | -18.745 | 0.015 | -18.756 | -18.734 | no | no | 2/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@500Hz | ° | 2 | -1.227 | 0.039 | -1.255 | -1.200 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@50Hz | ° | 2 | -0.498 | 1.061 | -1.248 | +0.252 | yes | yes | 0/2 (>3) |  | 2×INFO |

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

Signed values across takes: sd is the sample standard deviation (n − 1); `0 in mean±2se` is whether the mean lies within two standard errors of zero (no bias shown by these takes); `>pass` counts takes with |value| above the pass limit.

### Steady sines: magnitude

| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |
|---|---|---|---|---|---|---|---|---|---|---|---|
| genelec.sine_mag.REW offline@10000Hz | dB | 3 | +0.575 | 0.594 | -0.097 | +1.029 | yes | yes | 2/3 (>0.3) |  | 2×INCONCLUSIVE, 1×PASS |
| genelec.sine_mag.REW offline@1000Hz | dB | 3 | -0.309 | 0.025 | -0.337 | -0.289 | no | no | 2/3 (>0.3) |  | 2×METHOD, 1×PASS |
| genelec.sine_mag.REW offline@100Hz | dB | 3 | -0.073 | 0.027 | -0.100 | -0.047 | no | no | 0/3 (>0.3) |  | 2×INCONCLUSIVE, 1×PASS |
| genelec.sine_mag.REW offline@2000Hz | dB | 3 | -0.667 | 0.011 | -0.677 | -0.655 | no | no | 3/3 (>0.3) |  | 3×INCONCLUSIVE |
| genelec.sine_mag.REW offline@200Hz | dB | 3 | +0.028 | 0.017 | +0.008 | +0.039 | no | no | 0/3 (>0.3) |  | 3×PASS |
| genelec.sine_mag.REW offline@5000Hz | dB | 3 | +0.468 | 0.745 | -0.002 | +1.327 | yes | yes | 1/3 (>0.3) |  | 1×INCONCLUSIVE, 2×PASS |
| genelec.sine_mag.REW offline@500Hz | dB | 3 | +0.020 | 0.035 | -0.006 | +0.060 | yes | yes | 0/3 (>0.3) |  | 3×PASS |
| genelec.sine_mag.REW offline@50Hz | dB | 3 | -0.027 | 0.078 | -0.095 | +0.059 | yes | yes | 0/3 (>0.3) |  | 3×INCONCLUSIVE |
| genelec.sine_mag.ac2 TF@1000Hz | dB | 4 | -0.528 | 0.149 | -0.746 | -0.422 | no | no | 4/4 (>0.3) |  | 4×WARN |
| genelec.sine_mag.ac2 TF@100Hz | dB | 5 | -0.111 | 0.112 | -0.265 | +0.020 | yes | no | 0/5 (>0.3) |  | 2×INCONCLUSIVE, 3×PASS |
| genelec.sine_mag.ac2 TF@200Hz | dB | 5 | -0.020 | 0.025 | -0.049 | +0.008 | yes | yes | 0/5 (>0.3) |  | 5×PASS |
| genelec.sine_mag.ac2 TF@500Hz | dB | 5 | +0.010 | 0.024 | -0.011 | +0.050 | yes | yes | 0/5 (>0.3) |  | 5×PASS |
| genelec.sine_mag.ac2 TF@50Hz | dB | 1 | +0.042 |  | +0.042 | +0.042 |  |  | 0/1 (>0.3) |  | 1×INCONCLUSIVE |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@10000Hz | dB | 5 | +0.817 | 0.444 | +0.061 | +1.178 | no | no | 4/5 (>0.3) |  | 1×FAIL, 2×INCONCLUSIVE, 1×PASS, 1×WARN |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@1000Hz | dB | 5 | -0.334 | 0.020 | -0.359 | -0.306 | no | no | 5/5 (>0.3) |  | 2×METHOD, 3×WARN |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@100Hz | dB | 5 | -0.054 | 0.018 | -0.068 | -0.023 | no | no | 0/5 (>0.3) |  | 2×INCONCLUSIVE, 3×PASS |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@2000Hz | dB | 5 | -0.641 | 0.050 | -0.688 | -0.562 | no | no | 5/5 (>0.3) |  | 3×INCONCLUSIVE, 2×WARN |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@200Hz | dB | 5 | +0.017 | 0.013 | -0.001 | +0.036 | yes | no | 0/5 (>0.3) |  | 5×PASS |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@5000Hz | dB | 5 | +0.350 | 0.632 | -0.033 | +1.467 | yes | yes | 1/5 (>0.3) |  | 1×INCONCLUSIVE, 4×PASS |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@500Hz | dB | 5 | +0.004 | 0.021 | -0.013 | +0.040 | yes | yes | 0/5 (>0.3) |  | 5×PASS |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@50Hz | dB | 5 | -0.033 | 0.481 | -0.873 | +0.312 | yes | yes | 2/5 (>0.3) |  | 3×INCONCLUSIVE, 2×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@10000Hz | dB | 3 | +0.020 | 0.023 | +0.001 | +0.045 | no | yes | 0/3 (>0.3) |  | 3×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@1000Hz | dB | 3 | -0.049 | 0.022 | -0.069 | -0.025 | no | no | 0/3 (>0.3) |  | 3×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@100Hz | dB | 3 | -0.026 | 0.140 | -0.156 | +0.122 | yes | yes | 0/3 (>0.3) |  | 2×INCONCLUSIVE, 1×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@2000Hz | dB | 3 | +0.041 | 0.012 | +0.030 | +0.054 | no | no | 0/3 (>0.3) |  | 3×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@200Hz | dB | 3 | -0.000 | 0.023 | -0.022 | +0.023 | yes | yes | 0/3 (>0.3) |  | 3×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@5000Hz | dB | 3 | +0.022 | 0.075 | -0.063 | +0.077 | yes | yes | 0/3 (>0.3) |  | 3×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@500Hz | dB | 3 | -0.042 | 0.006 | -0.047 | -0.035 | no | no | 0/3 (>0.3) |  | 3×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@50Hz | dB | 3 | -0.017 | 0.668 | -0.579 | +0.722 | yes | yes | 2/3 (>0.3) |  | 3×INCONCLUSIVE |
| genelec.sine_mag.direct (REW recording)@10000Hz | dB | 3 | +0.578 | 0.576 | -0.079 | +0.994 | yes | yes | 2/3 (>0.3) |  | 2×INCONCLUSIVE, 1×PASS |
| genelec.sine_mag.direct (REW recording)@1000Hz | dB | 3 | -0.322 | 0.024 | -0.348 | -0.303 | no | no | 3/3 (>0.3) |  | 3×WARN |
| genelec.sine_mag.direct (REW recording)@100Hz | dB | 3 | -0.037 | 0.044 | -0.087 | -0.006 | no | yes | 0/3 (>0.3) |  | 2×INCONCLUSIVE, 1×PASS |
| genelec.sine_mag.direct (REW recording)@2000Hz | dB | 3 | -0.677 | 0.010 | -0.686 | -0.666 | no | no | 3/3 (>0.3) |  | 3×INCONCLUSIVE |
| genelec.sine_mag.direct (REW recording)@200Hz | dB | 3 | +0.024 | 0.021 | -0.000 | +0.038 | yes | yes | 0/3 (>0.3) |  | 3×PASS |
| genelec.sine_mag.direct (REW recording)@5000Hz | dB | 3 | +0.468 | 0.744 | -0.000 | +1.326 | yes | yes | 1/3 (>0.3) |  | 1×INCONCLUSIVE, 2×PASS |
| genelec.sine_mag.direct (REW recording)@500Hz | dB | 3 | +0.018 | 0.035 | -0.008 | +0.058 | yes | yes | 0/3 (>0.3) |  | 3×PASS |
| genelec.sine_mag.direct (REW recording)@50Hz | dB | 3 | +0.194 | 0.170 | -0.001 | +0.308 | yes | yes | 1/3 (>0.3) |  | 3×INCONCLUSIVE |
| genelec.sine_mag.direct (TF capture), narrow at the sine@10000Hz | dB | 5 | -1.570 | 0.416 | -1.896 | -0.859 | no | no | 5/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@1000Hz | dB | 5 | -0.017 | 0.018 | -0.035 | +0.006 | yes | no | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@100Hz | dB | 5 | +0.039 | 0.074 | -0.046 | +0.155 | yes | yes | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@2000Hz | dB | 5 | -0.875 | 0.234 | -1.011 | -0.457 | no | no | 5/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@200Hz | dB | 5 | -0.028 | 0.055 | -0.088 | +0.032 | yes | yes | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@5000Hz | dB | 5 | -1.756 | 0.508 | -2.195 | -0.889 | no | no | 5/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@500Hz | dB | 5 | +0.060 | 0.038 | -0.004 | +0.087 | yes | no | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@50Hz | dB | 5 | +0.445 | 0.440 | +0.115 | +1.177 | no | no | 2/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (TF capture)@10000Hz | dB | 5 | -0.920 | 0.662 | -1.855 | -0.018 | no | no | 4/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (TF capture)@1000Hz | dB | 5 | -0.332 | 0.040 | -0.391 | -0.293 | no | no | 4/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (TF capture)@100Hz | dB | 5 | -0.013 | 0.012 | -0.030 | -0.001 | no | no | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (TF capture)@2000Hz | dB | 5 | -1.637 | 0.235 | -1.791 | -1.222 | no | no | 5/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (TF capture)@200Hz | dB | 5 | -0.020 | 0.011 | -0.035 | -0.005 | no | no | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (TF capture)@5000Hz | dB | 5 | -1.498 | 0.709 | -2.130 | -0.552 | no | no | 5/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (TF capture)@500Hz | dB | 5 | +0.051 | 0.015 | +0.041 | +0.077 | no | no | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (TF capture)@50Hz | dB | 5 | +0.321 | 0.248 | +0.133 | +0.744 | no | no | 2/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@10000Hz | dB | 5 | +0.101 | 0.089 | +0.006 | +0.231 | no | no | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@1000Hz | dB | 5 | -0.032 | 0.010 | -0.044 | -0.020 | no | no | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@100Hz | dB | 5 | +0.013 | 0.127 | -0.114 | +0.194 | yes | yes | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@2000Hz | dB | 5 | +0.042 | 0.051 | -0.001 | +0.126 | yes | yes | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@200Hz | dB | 5 | -0.020 | 0.049 | -0.098 | +0.023 | yes | yes | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@5000Hz | dB | 5 | +0.111 | 0.083 | +0.006 | +0.213 | no | no | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@500Hz | dB | 5 | -0.004 | 0.019 | -0.020 | +0.016 | yes | yes | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@50Hz | dB | 5 | +0.715 | 0.765 | +0.135 | +2.029 | no | no | 3/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@10000Hz | dB | 5 | +0.817 | 0.444 | +0.061 | +1.179 | no | no | 4/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@1000Hz | dB | 5 | -0.335 | 0.020 | -0.359 | -0.306 | no | no | 5/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@100Hz | dB | 5 | -0.045 | 0.027 | -0.069 | -0.011 | no | no | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@2000Hz | dB | 5 | -0.639 | 0.050 | -0.686 | -0.560 | no | no | 5/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@200Hz | dB | 5 | +0.006 | 0.016 | -0.008 | +0.034 | yes | yes | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@5000Hz | dB | 5 | +0.353 | 0.631 | -0.030 | +1.469 | yes | yes | 1/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@500Hz | dB | 5 | +0.003 | 0.022 | -0.015 | +0.041 | yes | yes | 0/5 (>0.3) |  | 5×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@50Hz | dB | 5 | +0.708 | 0.816 | -0.001 | +2.086 | yes | yes | 3/5 (>0.3) |  | 5×INFO |

### Steady sines: phase

| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |
|---|---|---|---|---|---|---|---|---|---|---|---|
| genelec.sine_phase.REW offline@10000Hz | ° | 3 | +10.863 | 2.482 | +9.288 | +13.724 | no | no | 3/3 (>3) |  | 3×INCONCLUSIVE |
| genelec.sine_phase.REW offline@1000Hz | ° | 3 | -0.274 | 0.211 | -0.508 | -0.098 | no | no | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.REW offline@100Hz | ° | 3 | +0.326 | 0.428 | -0.115 | +0.738 | yes | yes | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.REW offline@2000Hz | ° | 3 | +0.393 | 0.642 | -0.190 | +1.081 | yes | yes | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.REW offline@200Hz | ° | 3 | +0.105 | 0.101 | +0.044 | +0.222 | no | yes | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.REW offline@5000Hz | ° | 3 | -11.544 | 1.990 | -12.792 | -9.250 | no | no | 3/3 (>3) |  | 3×INCONCLUSIVE |
| genelec.sine_phase.REW offline@500Hz | ° | 3 | -1.134 | 0.070 | -1.185 | -1.054 | no | no | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.REW offline@50Hz | ° | 3 | -0.239 | 0.575 | -0.769 | +0.372 | yes | yes | 0/3 (>3) |  | 3×INCONCLUSIVE |
| genelec.sine_phase.ac2 TF@1000Hz | ° | 4 | +0.676 | 1.214 | -0.505 | +2.376 | yes | yes | 0/4 (>3) |  | 4×PASS |
| genelec.sine_phase.ac2 TF@100Hz | ° | 5 | +0.747 | 0.501 | +0.121 | +1.420 | no | no | 0/5 (>3) |  | 5×PASS |
| genelec.sine_phase.ac2 TF@200Hz | ° | 5 | +0.312 | 0.091 | +0.177 | +0.416 | no | no | 0/5 (>3) |  | 5×PASS |
| genelec.sine_phase.ac2 TF@500Hz | ° | 5 | -0.237 | 0.149 | -0.409 | -0.016 | no | no | 0/5 (>3) |  | 5×PASS |
| genelec.sine_phase.ac2 TF@50Hz | ° | 1 | -2.428 |  | -2.428 | -2.428 |  |  | 0/1 (>3) |  | 1×INCONCLUSIVE |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@10000Hz | ° | 5 | +10.668 | 1.909 | +9.114 | +13.780 | no | no | 5/5 (>3) |  | 1×FAIL, 3×INCONCLUSIVE, 1×WARN |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@1000Hz | ° | 5 | +0.024 | 0.199 | -0.299 | +0.244 | yes | yes | 0/5 (>3) |  | 5×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@100Hz | ° | 5 | -0.060 | 0.218 | -0.403 | +0.182 | yes | yes | 0/5 (>3) |  | 5×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@2000Hz | ° | 5 | +0.895 | 0.444 | +0.286 | +1.333 | no | no | 0/5 (>3) |  | 5×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@200Hz | ° | 5 | +0.138 | 0.037 | +0.089 | +0.189 | no | no | 0/5 (>3) |  | 5×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@5000Hz | ° | 5 | -11.636 | 1.750 | -12.708 | -8.539 | no | no | 5/5 (>3) |  | 2×FAIL, 3×INCONCLUSIVE |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@500Hz | ° | 5 | -1.101 | 0.091 | -1.169 | -0.945 | no | no | 0/5 (>3) |  | 5×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@50Hz | ° | 5 | +0.155 | 0.385 | -0.381 | +0.538 | yes | yes | 0/5 (>3) |  | 3×INCONCLUSIVE, 2×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@10000Hz | ° | 3 | -0.454 | 0.259 | -0.632 | -0.157 | no | no | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@1000Hz | ° | 3 | -0.225 | 0.199 | -0.437 | -0.042 | no | yes | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@100Hz | ° | 3 | +0.125 | 0.896 | -0.885 | +0.827 | yes | yes | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@2000Hz | ° | 3 | +0.138 | 0.388 | -0.256 | +0.520 | yes | yes | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@200Hz | ° | 3 | +0.187 | 0.430 | -0.247 | +0.614 | yes | yes | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@5000Hz | ° | 3 | -0.061 | 0.368 | -0.364 | +0.349 | yes | yes | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@500Hz | ° | 3 | -0.020 | 0.088 | -0.090 | +0.078 | yes | yes | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@50Hz | ° | 3 | +0.898 | 3.090 | -1.474 | +4.392 | yes | yes | 1/3 (>3) |  | 3×INCONCLUSIVE |
| genelec.sine_phase.direct (REW recording)@10000Hz | ° | 3 | +10.772 | 2.029 | +9.275 | +13.081 | no | no | 3/3 (>3) |  | 3×INCONCLUSIVE |
| genelec.sine_phase.direct (REW recording)@1000Hz | ° | 3 | -0.189 | 0.213 | -0.424 | -0.011 | no | yes | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.direct (REW recording)@100Hz | ° | 3 | +0.117 | 0.438 | -0.319 | +0.556 | yes | yes | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.direct (REW recording)@2000Hz | ° | 3 | +0.413 | 0.651 | -0.183 | +1.107 | yes | yes | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.direct (REW recording)@200Hz | ° | 3 | +0.138 | 0.059 | +0.091 | +0.205 | no | no | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.direct (REW recording)@5000Hz | ° | 3 | -11.520 | 1.990 | -12.767 | -9.224 | no | no | 3/3 (>3) |  | 3×INCONCLUSIVE |
| genelec.sine_phase.direct (REW recording)@500Hz | ° | 3 | -1.102 | 0.074 | -1.154 | -1.017 | no | no | 0/3 (>3) |  | 3×PASS |
| genelec.sine_phase.direct (REW recording)@50Hz | ° | 3 | +0.275 | 2.014 | -1.807 | +2.213 | yes | yes | 0/3 (>3) |  | 3×INCONCLUSIVE |
| genelec.sine_phase.direct (TF capture), narrow at the sine@10000Hz | ° | 5 | +0.585 | 1.429 | -0.704 | +2.936 | yes | yes | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@1000Hz | ° | 5 | -0.134 | 0.199 | -0.380 | +0.170 | yes | yes | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@100Hz | ° | 5 | +0.497 | 0.374 | -0.072 | +0.883 | yes | no | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@2000Hz | ° | 5 | -4.632 | 1.207 | -6.466 | -3.119 | no | no | 5/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@200Hz | ° | 5 | +0.107 | 0.257 | -0.183 | +0.492 | yes | yes | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@5000Hz | ° | 5 | -2.799 | 0.996 | -4.026 | -1.762 | no | no | 3/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@500Hz | ° | 5 | +0.996 | 0.231 | +0.767 | +1.368 | no | no | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@50Hz | ° | 5 | +0.087 | 2.065 | -2.557 | +3.156 | yes | yes | 1/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (TF capture)@10000Hz | ° | 5 | +11.138 | 2.294 | +8.987 | +14.063 | no | no | 5/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (TF capture)@1000Hz | ° | 5 | -0.247 | 0.356 | -0.754 | +0.141 | yes | yes | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (TF capture)@100Hz | ° | 5 | +0.157 | 0.226 | -0.147 | +0.387 | yes | yes | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (TF capture)@2000Hz | ° | 5 | -3.899 | 1.274 | -5.126 | -1.949 | no | no | 4/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (TF capture)@200Hz | ° | 5 | +0.286 | 0.021 | +0.264 | +0.320 | no | no | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (TF capture)@5000Hz | ° | 5 | -14.769 | 1.853 | -16.561 | -11.920 | no | no | 5/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (TF capture)@500Hz | ° | 5 | -0.392 | 0.022 | -0.419 | -0.357 | no | no | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (TF capture)@50Hz | ° | 5 | +0.262 | 1.066 | -0.708 | +2.083 | yes | yes | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@10000Hz | ° | 5 | +0.179 | 0.734 | -0.437 | +1.369 | yes | yes | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@1000Hz | ° | 5 | -0.004 | 0.171 | -0.198 | +0.190 | yes | yes | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@100Hz | ° | 5 | +0.120 | 0.420 | -0.260 | +0.776 | yes | yes | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@2000Hz | ° | 5 | +0.284 | 0.255 | -0.085 | +0.633 | yes | no | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@200Hz | ° | 5 | +0.125 | 0.485 | -0.369 | +0.796 | yes | yes | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@5000Hz | ° | 5 | +0.410 | 0.706 | -0.142 | +1.527 | yes | yes | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@500Hz | ° | 5 | -0.059 | 0.098 | -0.187 | +0.069 | yes | yes | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@50Hz | ° | 5 | +1.390 | 5.359 | -4.529 | +10.014 | yes | yes | 2/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@10000Hz | ° | 5 | +10.701 | 1.907 | +9.151 | +13.809 | no | no | 5/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@1000Hz | ° | 5 | +0.046 | 0.206 | -0.288 | +0.273 | yes | yes | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@100Hz | ° | 5 | +0.043 | 0.307 | -0.462 | +0.259 | yes | yes | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@2000Hz | ° | 5 | +0.918 | 0.444 | +0.318 | +1.363 | no | no | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@200Hz | ° | 5 | +0.062 | 0.080 | -0.013 | +0.168 | yes | yes | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@5000Hz | ° | 5 | -11.621 | 1.754 | -12.696 | -8.517 | no | no | 5/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@500Hz | ° | 5 | -1.099 | 0.089 | -1.156 | -0.944 | no | no | 0/5 (>3) |  | 5×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@50Hz | ° | 5 | -1.001 | 1.692 | -3.866 | +0.636 | yes | yes | 1/5 (>3) |  | 5×INFO |

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

Signed values across takes: sd is the sample standard deviation (n − 1); `0 in mean±2se` is whether the mean lies within two standard errors of zero (no bias shown by these takes); `>pass` counts takes with |value| above the pass limit.

### Steady sines: magnitude

| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |
|---|---|---|---|---|---|---|---|---|---|---|---|
| genelec.sine_mag.REW offline@10000Hz | dB | 2 | +0.876 | 0.257 | +0.694 | +1.058 | no | no | 2/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.REW offline@1000Hz | dB | 2 | -0.173 | 0.013 | -0.182 | -0.165 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.REW offline@100Hz | dB | 2 | -0.105 | 0.111 | -0.183 | -0.026 | no | yes | 0/2 (>0.3) |  | 1×INCONCLUSIVE, 1×PASS |
| genelec.sine_mag.REW offline@2000Hz | dB | 2 | -0.312 | 0.163 | -0.427 | -0.196 | no | no | 1/2 (>0.3) |  | 1×PASS, 1×WARN |
| genelec.sine_mag.REW offline@200Hz | dB | 2 | +0.030 | 0.067 | -0.018 | +0.077 | yes | yes | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.REW offline@5000Hz | dB | 2 | +0.273 | 0.304 | +0.058 | +0.488 | no | yes | 1/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.REW offline@500Hz | dB | 2 | +0.052 | 0.002 | +0.050 | +0.054 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.REW offline@50Hz | dB | 2 | -0.173 | 0.056 | -0.212 | -0.133 | no | no | 0/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.ac2 TF@1000Hz | dB | 2 | -0.119 | 0.069 | -0.168 | -0.070 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.ac2 TF@100Hz | dB | 2 | +0.095 | 0.081 | +0.037 | +0.152 | no | yes | 0/2 (>0.3) |  | 1×INCONCLUSIVE, 1×PASS |
| genelec.sine_mag.ac2 TF@200Hz | dB | 2 | -0.023 | 0.011 | -0.030 | -0.015 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.ac2 TF@500Hz | dB | 2 | +0.110 | 0.022 | +0.095 | +0.125 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@10000Hz | dB | 2 | +1.059 | 0.052 | +1.021 | +1.096 | no | no | 2/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@1000Hz | dB | 2 | -0.176 | 0.002 | -0.177 | -0.174 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@100Hz | dB | 2 | +0.049 | 0.023 | +0.033 | +0.065 | no | no | 0/2 (>0.3) |  | 1×INCONCLUSIVE, 1×PASS |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@2000Hz | dB | 2 | -0.197 | 0.001 | -0.198 | -0.196 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@200Hz | dB | 2 | +0.019 | 0.001 | +0.018 | +0.019 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@5000Hz | dB | 2 | +0.530 | 0.005 | +0.527 | +0.534 | no | no | 2/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@500Hz | dB | 2 | +0.032 | 0.006 | +0.028 | +0.036 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.ac2 sweep 20Hz-5.5s@50Hz | dB | 2 | +0.352 | 0.193 | +0.216 | +0.489 | no | no | 1/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.direct (REW recording), narrow at the sine@10000Hz | dB | 2 | -0.046 | 0.183 | -0.176 | +0.083 | yes | yes | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@1000Hz | dB | 2 | -0.030 | 0.003 | -0.032 | -0.028 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@100Hz | dB | 2 | +0.007 | 0.178 | -0.119 | +0.133 | yes | yes | 0/2 (>0.3) |  | 1×INCONCLUSIVE, 1×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@2000Hz | dB | 2 | -0.035 | 0.161 | -0.148 | +0.079 | yes | yes | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@200Hz | dB | 2 | +0.073 | 0.045 | +0.041 | +0.106 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@5000Hz | dB | 2 | -0.172 | 0.288 | -0.376 | +0.032 | yes | yes | 1/2 (>0.3) |  | 1×PASS, 1×WARN |
| genelec.sine_mag.direct (REW recording), narrow at the sine@500Hz | dB | 2 | -0.013 | 0.027 | -0.032 | +0.007 | yes | yes | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording), narrow at the sine@50Hz | dB | 2 | -0.000 | 0.414 | -0.293 | +0.292 | yes | yes | 0/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.direct (REW recording)@10000Hz | dB | 2 | +0.903 | 0.186 | +0.772 | +1.035 | no | no | 2/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.direct (REW recording)@1000Hz | dB | 2 | -0.168 | 0.013 | -0.176 | -0.159 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording)@100Hz | dB | 2 | +0.017 | 0.085 | -0.043 | +0.077 | yes | yes | 0/2 (>0.3) |  | 1×INCONCLUSIVE, 1×PASS |
| genelec.sine_mag.direct (REW recording)@2000Hz | dB | 2 | -0.309 | 0.163 | -0.424 | -0.194 | no | no | 1/2 (>0.3) |  | 1×PASS, 1×WARN |
| genelec.sine_mag.direct (REW recording)@200Hz | dB | 2 | +0.033 | 0.035 | +0.008 | +0.057 | no | yes | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording)@5000Hz | dB | 2 | +0.265 | 0.303 | +0.050 | +0.479 | no | yes | 1/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.direct (REW recording)@500Hz | dB | 2 | +0.047 | 0.000 | +0.047 | +0.047 | no | no | 0/2 (>0.3) |  | 2×PASS |
| genelec.sine_mag.direct (REW recording)@50Hz | dB | 2 | +0.471 | 0.073 | +0.419 | +0.522 | no | no | 2/2 (>0.3) |  | 2×INCONCLUSIVE |
| genelec.sine_mag.direct (TF capture), narrow at the sine@10000Hz | dB | 2 | -1.380 | 0.606 | -1.808 | -0.952 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@1000Hz | dB | 2 | -0.001 | 0.035 | -0.026 | +0.024 | yes | yes | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@100Hz | dB | 2 | +0.078 | 0.047 | +0.044 | +0.111 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@2000Hz | dB | 2 | -0.796 | 0.305 | -1.011 | -0.580 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@200Hz | dB | 2 | -0.072 | 0.004 | -0.075 | -0.069 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@5000Hz | dB | 2 | -1.481 | 0.684 | -1.965 | -0.997 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@500Hz | dB | 2 | +0.064 | 0.024 | +0.047 | +0.081 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture), narrow at the sine@50Hz | dB | 2 | -0.510 | 1.037 | -1.244 | +0.223 | yes | yes | 1/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture)@10000Hz | dB | 2 | -0.416 | 0.636 | -0.866 | +0.034 | yes | yes | 1/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture)@1000Hz | dB | 2 | -0.204 | 0.024 | -0.221 | -0.188 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture)@100Hz | dB | 2 | +0.008 | 0.010 | +0.001 | +0.015 | no | yes | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture)@2000Hz | dB | 2 | -1.044 | 0.360 | -1.298 | -0.790 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture)@200Hz | dB | 2 | -0.050 | 0.038 | -0.077 | -0.024 | no | yes | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture)@5000Hz | dB | 2 | -0.980 | 0.699 | -1.474 | -0.486 | no | yes | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture)@500Hz | dB | 2 | +0.068 | 0.004 | +0.066 | +0.071 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (TF capture)@50Hz | dB | 2 | +0.106 | 0.098 | +0.036 | +0.175 | no | yes | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@10000Hz | dB | 2 | +0.104 | 0.057 | +0.064 | +0.144 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@1000Hz | dB | 2 | -0.023 | 0.007 | -0.028 | -0.018 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@100Hz | dB | 2 | +0.025 | 0.101 | -0.046 | +0.097 | yes | yes | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@2000Hz | dB | 2 | +0.050 | 0.016 | +0.039 | +0.062 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@200Hz | dB | 2 | +0.050 | 0.010 | +0.043 | +0.057 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@5000Hz | dB | 2 | +0.121 | 0.022 | +0.105 | +0.136 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@500Hz | dB | 2 | +0.005 | 0.015 | -0.005 | +0.015 | yes | yes | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s), narrow at the sine@50Hz | dB | 2 | -0.167 | 0.602 | -0.593 | +0.259 | yes | yes | 1/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@10000Hz | dB | 2 | +1.067 | 0.052 | +1.030 | +1.104 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@1000Hz | dB | 2 | -0.176 | 0.002 | -0.177 | -0.174 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@100Hz | dB | 2 | +0.001 | 0.135 | -0.095 | +0.096 | yes | yes | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@2000Hz | dB | 2 | -0.194 | 0.000 | -0.194 | -0.194 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@200Hz | dB | 2 | +0.013 | 0.000 | +0.013 | +0.013 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@5000Hz | dB | 2 | +0.529 | 0.005 | +0.526 | +0.533 | no | no | 2/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@500Hz | dB | 2 | +0.031 | 0.004 | +0.028 | +0.034 | no | no | 0/2 (>0.3) |  | 2×INFO |
| genelec.sine_mag.direct (ac2 capture 20Hz-5.5s)@50Hz | dB | 2 | +0.229 | 0.052 | +0.192 | +0.265 | no | no | 0/2 (>0.3) |  | 2×INFO |

### Steady sines: phase

| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |
|---|---|---|---|---|---|---|---|---|---|---|---|
| genelec.sine_phase.REW offline@10000Hz | ° | 2 | +5.630 | 0.772 | +5.084 | +6.175 | no | no | 2/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.REW offline@1000Hz | ° | 2 | +0.085 | 0.095 | +0.017 | +0.152 | no | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.REW offline@100Hz | ° | 2 | +0.303 | 0.367 | +0.043 | +0.563 | no | yes | 0/2 (>3) |  | 1×INCONCLUSIVE, 1×PASS |
| genelec.sine_phase.REW offline@2000Hz | ° | 2 | -0.316 | 0.724 | -0.828 | +0.196 | yes | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.REW offline@200Hz | ° | 2 | -0.075 | 0.231 | -0.239 | +0.088 | yes | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.REW offline@5000Hz | ° | 2 | -7.697 | 1.045 | -8.436 | -6.958 | no | no | 2/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.REW offline@500Hz | ° | 2 | -0.493 | 0.080 | -0.550 | -0.437 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.REW offline@50Hz | ° | 2 | +1.854 | 3.470 | -0.600 | +4.307 | yes | yes | 1/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.ac2 TF@1000Hz | ° | 2 | -1.051 | 0.729 | -1.566 | -0.535 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.ac2 TF@100Hz | ° | 2 | +0.377 | 0.522 | +0.008 | +0.746 | no | yes | 0/2 (>3) |  | 1×INCONCLUSIVE, 1×PASS |
| genelec.sine_phase.ac2 TF@200Hz | ° | 2 | -0.052 | 0.113 | -0.132 | +0.028 | yes | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.ac2 TF@500Hz | ° | 2 | +0.035 | 0.147 | -0.069 | +0.139 | yes | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@10000Hz | ° | 2 | +6.277 | 0.741 | +5.753 | +6.801 | no | no | 2/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@1000Hz | ° | 2 | +0.190 | 0.169 | +0.071 | +0.310 | no | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@100Hz | ° | 2 | +0.119 | 0.107 | +0.043 | +0.195 | no | yes | 0/2 (>3) |  | 1×INCONCLUSIVE, 1×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@2000Hz | ° | 2 | +0.536 | 0.293 | +0.328 | +0.743 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@200Hz | ° | 2 | +0.159 | 0.123 | +0.072 | +0.246 | no | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@5000Hz | ° | 2 | -6.896 | 0.557 | -7.290 | -6.502 | no | no | 2/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@500Hz | ° | 2 | -0.385 | 0.026 | -0.404 | -0.367 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.ac2 sweep 20Hz-5.5s@50Hz | ° | 2 | +0.959 | 0.081 | +0.902 | +1.017 | no | no | 0/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.direct (REW recording), narrow at the sine@10000Hz | ° | 2 | -0.209 | 1.012 | -0.925 | +0.506 | yes | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@1000Hz | ° | 2 | -0.208 | 0.152 | -0.316 | -0.101 | no | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@100Hz | ° | 2 | -0.027 | 1.427 | -1.036 | +0.982 | yes | yes | 0/2 (>3) |  | 1×INCONCLUSIVE, 1×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@2000Hz | ° | 2 | +0.023 | 0.748 | -0.506 | +0.552 | yes | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@200Hz | ° | 2 | +0.299 | 0.579 | -0.111 | +0.709 | yes | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@5000Hz | ° | 2 | -0.345 | 1.238 | -1.221 | +0.531 | yes | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@500Hz | ° | 2 | +0.037 | 0.226 | -0.123 | +0.197 | yes | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording), narrow at the sine@50Hz | ° | 2 | +0.651 | 5.765 | -3.425 | +4.728 | yes | yes | 2/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.direct (REW recording)@10000Hz | ° | 2 | +5.489 | 0.894 | +4.857 | +6.121 | no | no | 2/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.direct (REW recording)@1000Hz | ° | 2 | +0.038 | 0.093 | -0.028 | +0.104 | yes | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording)@100Hz | ° | 2 | -0.329 | 0.815 | -0.906 | +0.247 | yes | yes | 0/2 (>3) |  | 1×INCONCLUSIVE, 1×PASS |
| genelec.sine_phase.direct (REW recording)@2000Hz | ° | 2 | -0.234 | 0.723 | -0.745 | +0.277 | yes | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording)@200Hz | ° | 2 | -0.078 | 0.161 | -0.192 | +0.036 | yes | yes | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording)@5000Hz | ° | 2 | -7.664 | 1.044 | -8.403 | -6.926 | no | no | 2/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.direct (REW recording)@500Hz | ° | 2 | -0.461 | 0.072 | -0.512 | -0.410 | no | no | 0/2 (>3) |  | 2×PASS |
| genelec.sine_phase.direct (REW recording)@50Hz | ° | 2 | +1.734 | 0.996 | +1.030 | +2.439 | no | no | 0/2 (>3) |  | 2×INCONCLUSIVE |
| genelec.sine_phase.direct (TF capture), narrow at the sine@10000Hz | ° | 2 | +1.063 | 0.436 | +0.755 | +1.371 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@1000Hz | ° | 2 | +0.001 | 0.104 | -0.073 | +0.075 | yes | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@100Hz | ° | 2 | -0.402 | 0.460 | -0.727 | -0.076 | no | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@2000Hz | ° | 2 | -3.824 | 1.514 | -4.894 | -2.753 | no | no | 1/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@200Hz | ° | 2 | +0.012 | 0.046 | -0.021 | +0.044 | yes | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@5000Hz | ° | 2 | -1.812 | 1.147 | -2.623 | -1.001 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@500Hz | ° | 2 | +0.710 | 0.048 | +0.676 | +0.744 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture), narrow at the sine@50Hz | ° | 2 | +2.954 | 3.454 | +0.512 | +5.396 | no | yes | 1/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture)@10000Hz | ° | 2 | +6.521 | 0.408 | +6.232 | +6.809 | no | no | 2/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture)@1000Hz | ° | 2 | +0.093 | 0.389 | -0.182 | +0.368 | yes | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture)@100Hz | ° | 2 | +0.376 | 0.326 | +0.145 | +0.606 | no | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture)@2000Hz | ° | 2 | -3.762 | 1.128 | -4.560 | -2.965 | no | no | 1/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture)@200Hz | ° | 2 | +0.105 | 0.046 | +0.073 | +0.137 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture)@5000Hz | ° | 2 | -9.417 | 1.138 | -10.221 | -8.612 | no | no | 2/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture)@500Hz | ° | 2 | +0.288 | 0.023 | +0.272 | +0.304 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (TF capture)@50Hz | ° | 2 | -1.070 | 0.168 | -1.190 | -0.951 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@10000Hz | ° | 2 | +0.560 | 0.793 | -0.001 | +1.121 | yes | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@1000Hz | ° | 2 | -0.014 | 0.209 | -0.162 | +0.133 | yes | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@100Hz | ° | 2 | -0.414 | 0.473 | -0.749 | -0.080 | no | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@2000Hz | ° | 2 | +0.481 | 0.355 | +0.230 | +0.732 | no | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@200Hz | ° | 2 | +0.067 | 0.089 | +0.003 | +0.130 | no | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@5000Hz | ° | 2 | +0.322 | 0.749 | -0.208 | +0.851 | yes | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@500Hz | ° | 2 | -0.156 | 0.042 | -0.186 | -0.126 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s), narrow at the sine@50Hz | ° | 2 | -1.295 | 0.645 | -1.752 | -0.839 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@10000Hz | ° | 2 | +6.282 | 0.740 | +5.758 | +6.805 | no | no | 2/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@1000Hz | ° | 2 | +0.199 | 0.174 | +0.075 | +0.322 | no | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@100Hz | ° | 2 | -0.102 | 0.199 | -0.243 | +0.039 | yes | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@2000Hz | ° | 2 | +0.571 | 0.276 | +0.376 | +0.766 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@200Hz | ° | 2 | +0.301 | 0.314 | +0.079 | +0.524 | no | yes | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@5000Hz | ° | 2 | -6.899 | 0.551 | -7.289 | -6.509 | no | no | 2/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@500Hz | ° | 2 | -0.393 | 0.024 | -0.409 | -0.376 | no | no | 0/2 (>3) |  | 2×INFO |
| genelec.sine_phase.direct (ac2 capture 20Hz-5.5s)@50Hz | ° | 2 | +1.421 | 1.245 | +0.541 | +2.302 | no | yes | 0/2 (>3) |  | 2×INFO |

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

