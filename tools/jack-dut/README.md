# ac2-jack-dut

A software device under test for the cross-check suite (`tools/crosscheck`): a JACK client
whose filters and harmonic distortion are known exactly, so harmonic and transfer
measurements (ac2, REW) can be compared against an analytic truth.

```
ref_out = ref_in + noise_r
dut_out = post(poly(pre(dut_in))) + noise_d
```

All math is f64 with state carried across JACK cycles; the process callback does not
allocate, lock or make syscalls. Linux only (JACK); on other OS the binary exits 2.

## CLI

```
ac2-jack-dut [--name ac2-dut] --poly c0,c1,c2,... [--pre b0,b1,b2,a1,a2]... [--post b0,b1,b2,a1,a2]... [--noise-dbfs -120|off]
```

- `--poly` (required, at least `c0,c1`): `poly(u) = c0 + c1 u + c2 u² + …` (Horner).
  For `A sin θ` in: c2 gives H2 = c2·A²/2, c3 gives H3 = c3·A³/4 and fundamental
  A·(1 + 3c3A²/4).
- `--pre`, `--post` (repeatable, applied in the order given): one biquad each.
- `--noise-dbfs`: white noise on each output, omitted or `off` for none.

Non-finite numbers and unstable biquads (outside |a2| < 1, |a1| < 1 + a2) are refused.

Ports: `ref_in`, `ref_out`, `dut_in`, `dut_out`. After activation it prints one line
`ready <client_name> <sample_rate> <buffer_size>`, then runs until SIGINT/SIGTERM or end of
stdin, deactivates and prints `xruns <n>`. A cycle the server reports as an xrun can leave a
corrupted block in the outputs; a measurement taken across a non-zero xrun count is suspect.

## Coefficient convention (the contract)

Normalised with a0 = 1, given as `b0,b1,b2,a1,a2`:

```
y[n] = b0 x[n] + b1 x[n-1] + b2 x[n-2] − a1 y[n-1] − a2 y[n-2]
H(z) = (b0 + b1 z⁻¹ + b2 z⁻²) / (1 + a1 z⁻¹ + a2 z⁻²)
```

realised as Direct Form II transposed.

## Noise level

Gaussian white noise (Box–Muller over xorshift64*), fixed independent seeds per output. The
level is in dBFS where 0 dBFS is a full-scale sine of peak 1.0, so the noise RMS is
`10^(dB/20)/√2` (−120 dBFS → RMS 7.07e-7).

## Safety

- The client never connects its own ports; the caller does all patching.
- Outputs are hard-limited to ±1.0 as a defence; the suite keeps signals far below it.
- For tests use a JACK dummy server (`jackd -n dut-test -d dummy`, clients with
  `JACK_DEFAULT_SERVER=dut-test`), never a real audio device.
