# Digital DUT: harmonic coverage per order, tone and level

`python -m crosscheck dutcov <runs>` over the DUT matrix runs `dut-10dbfs` and `dut-30dbfs` of 2026-10-14 (ac2 70655493, dummy JACK with an 8192-frame period,
`rigs/host.toml`), the runs behind `baselines/host/dut-*`. Legend below the table.

| H | source | level | 50 Hz | 100 Hz | 200 Hz | 500 Hz | 1000 Hz | 2000 Hz | 5000 Hz | judged | incon. | missing | worst Δ dB | model-limited | status |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| H2 | steady sine | -10 dBFS | 4/4 +0.03 | 4/4 +0.02 | 4/4 -0.00 | 4/4 +0.01 | 4/4 +0.00 | 4/4 +0.01 | 4/4 +0.01 | 28/28 | 0 | 0 | +0.03 | — | PASS |
| H2 | steady sine | -30 dBFS | 4/4 +0.03 | 4/4 +0.02 | 4/4 -0.00 | 4/4 +0.01 | 4/4 +0.00 | 4/4 +0.01 | 4/4 +0.01 | 28/28 | 0 | 0 | +0.03 | — | PASS |
| H2 | ac2 sweep | -10 dBFS | 8/8 -0.02 † | 8/8 -0.03 † | 8/8 -0.04 | 8/8 +0.07 | 8/8 +0.06 | 8/8 +0.10 | 8/8 +0.10 | 56/56 | 0 | 0 | +0.10 | 8 judged, worst -0.09 | PASS |
| H2 | ac2 sweep | -30 dBFS | 8/8 -0.02 † | 8/8 -0.05 † | 8/8 +0.03 | 8/8 -0.04 | 8/8 -0.13 | 8/8 +0.09 | 8/8 +0.10 | 56/56 | 0 | 0 | -0.13 | 8 judged, worst -0.11 | PASS |
| H3 | steady sine | -10 dBFS | 4/4 -0.05 | 4/4 -0.02 | 4/4 -0.02 | 4/4 +0.01 | 4/4 -0.01 | 4/4 +0.00 | 4/4 -0.02 | 28/28 | 0 | 0 | -0.05 | — | PASS |
| H3 | steady sine | -30 dBFS | 4/4 -0.06 | 4/4 -0.02 | 4/4 -0.02 | 4/4 +0.00 | 4/4 +0.01 | 4/4 +0.00 | 4/4 -0.02 | 28/28 | 0 | 0 | -0.06 | — | PASS |
| H3 | ac2 sweep | -10 dBFS | 8/8 +0.03 † | 8/8 -0.01 † | 8/8 -0.04 | 8/8 +0.03 | 8/8 -0.08 | 8/8 +0.12 | 8/8 +0.05 | 56/56 | 0 | 0 | +0.12 | 8 judged, worst +0.03 | PASS |
| H3 | ac2 sweep | -30 dBFS | 8/8 +0.01 † | 8/8 +0.02 † | 8/8 -0.06 | 8/8 +0.03 | 8/8 +0.10 | 8/8 +0.12 | 8/8 -0.07 | 56/56 | 0 | 0 | +0.12 | 8 judged, worst +0.17 | PASS |
| H4 | steady sine | -10 dBFS | 4/4 +0.01 | 4/4 +0.00 | 4/4 +0.00 | 4/4 +0.01 | 4/4 +0.02 | 4/4 -0.00 | 4/4 -0.01 | 28/28 | 0 | 0 | +0.02 | — | PASS |
| H4 | steady sine | -30 dBFS | 4/4 +0.01 | 4/4 +0.00 | 4/4 +0.01 | 4/4 +0.01 | 4/4 +0.02 | 4/4 +0.00 | 4/4 -0.01 | 28/28 | 0 | 0 | +0.02 | — | PASS |
| H4 | ac2 sweep | -10 dBFS | 8/8 -0.03 † | 8/8 -0.08 † | 8/8 +0.04 | 8/8 +0.10 | 8/8 +0.07 | 8/8 -0.11 | 8/8 +0.09 | 56/56 | 0 | 0 | -0.11 | 8 judged, worst +0.06 | PASS |
| H4 | ac2 sweep | -30 dBFS | 8/8 -0.01 † | 8/8 -0.09 † | 8/8 -0.05 | 8/8 +0.10 | 8/8 -0.03 | 8/8 +0.07 | 8/8 -0.02 | 56/56 | 0 | 0 | +0.10 | 8 judged, worst +0.02 | PASS |
| H5 | steady sine | -10 dBFS | 4/4 -0.01 | 4/4 +0.02 | 4/4 -0.00 | 4/4 +0.01 | 4/4 -0.00 | 4/4 -0.01 | 4/4 -0.00 | 28/28 | 0 | 0 | +0.02 | — | PASS |
| H5 | steady sine | -30 dBFS | 4/4 +0.01 | 4/4 +0.02 | 4/4 -0.00 | 4/4 +0.01 | 4/4 +0.00 | 4/4 -0.01 | 4/4 -0.00 | 28/28 | 0 | 0 | +0.02 | — | PASS |
| H5 | ac2 sweep | -10 dBFS | 8/8 +0.04 † | 8/8 +0.04 † | 8/8 -0.07 | 8/8 +0.08 | 8/8 +0.11 | 8/8 +0.07 | 8/8 -0.08 | 56/56 | 0 | 0 | +0.11 | 8 judged, worst +0.04 | PASS |
| H5 | ac2 sweep | -30 dBFS | 8/8 +0.03 † | 8/8 +0.01 † | 8/8 -0.07 | 8/8 -0.05 | 8/8 +0.11 | 8/8 -0.07 | 8/8 -0.08 | 56/56 | 0 | 0 | +0.11 | 8 judged, worst -0.05 | PASS |

Cell: judged/cells over the cases (and sweep rates), then the quasi-static worst signed error reading − truth (dB; in parentheses when only model-limited cells were judged). F/W: a FAIL/WARN in the cell. †: a judged cell where the pre-filter is > 0.1 dB from flat (Wiener model: a sweep sees it only approximately); such cells are kept out of the status and the worst Δ. Not counted: k·f ≥ fs/2 and, for a sweep, k·f beyond its end. incon.: the reading is a bound (not clear of the floor); missing: no reading where one was due.
