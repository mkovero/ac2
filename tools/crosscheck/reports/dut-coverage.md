# Digital DUT: harmonic coverage per order, tone and level

`python -m crosscheck dutcov <runs>` over the DUT matrix runs n10 and n30 (ac2 bdc1bec, dummy JACK,
`rigs/host.toml`). A cell: judged/total at that tone and its worst signed error (reading − exact value, dB);
† the sweep cells the Wiener pre-filter model cannot follow, reported apart.

| H | source | level | 50 Hz | 100 Hz | 200 Hz | 500 Hz | 1000 Hz | 2000 Hz | 5000 Hz | judged | incon. | missing | worst Δ dB | model-limited | status |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| H2 | steady sine | -10 dBFS | 4/4 -0.01 | 4/4 +0.01 | 4/4 +0.00 | 4/4 +0.00 | 4/4 -0.01 | 4/4 -0.00 | 4/4 +0.00 | 28/28 | 0 | 0 | +0.01 | — | PASS |
| H2 | steady sine | -30 dBFS | 4/4 -0.01 | 4/4 +0.01 | 4/4 -0.00 | 4/4 +0.01 | 4/4 -0.01 | 4/4 -0.02 | 4/4 -0.02 | 28/28 | 0 | 0 | -0.02 | — | PASS |
| H2 | ac2 sweep | -10 dBFS | 8/8 -0.03 † | 8/8 -0.03 † | 8/8 +0.06 | 8/8 +0.03 | 8/8 -0.09 | 8/8 +0.07 | 8/8 +0.10 | 56/56 | 0 | 0 | +0.10 | 8 judged, worst +0.09 | PASS |
| H2 | ac2 sweep | -30 dBFS | 8/8 -0.04 † | 8/8 -0.02 † | 8/8 +0.06 | 8/8 +0.06 | 8/8 -0.04 | 8/8 +0.08 | 8/8 +0.10 | 56/56 | 0 | 0 | +0.10 | 8 judged, worst +0.25 | PASS |
| H3 | steady sine | -10 dBFS | 4/4 -0.02 | 4/4 +0.01 | 4/4 -0.02 | 4/4 +0.01 | 4/4 +0.01 | 4/4 -0.00 | 4/4 -0.01 | 28/28 | 0 | 0 | -0.02 | — | PASS |
| H3 | steady sine | -30 dBFS | 4/4 +0.05 | 4/4 +0.01 | 4/4 -0.02 | 4/4 +0.01 | 4/4 +0.01 | 4/4 -0.00 | 4/4 -0.01 | 28/28 | 0 | 0 | +0.05 | — | PASS |
| H3 | ac2 sweep | -10 dBFS | 8/8 +0.01 † | 8/8 +0.03 † | 8/8 +0.03 | 8/8 -0.06 | 8/8 +0.03 | 8/8 +0.17 | 8/8 +0.08 | 56/56 | 0 | 0 | +0.17 | 8 judged, worst +0.36 | PASS |
| H3 | ac2 sweep | -30 dBFS | 8/8 -0.03 † | 8/8 +0.02 † | 8/8 +0.04 | 8/8 -0.09 | 8/8 +0.01 | 8/8 +0.19 | 8/8 +0.08 | 56/56 | 0 | 0 | +0.19 | 8 judged, worst +0.36 | PASS |
| H4 | steady sine | -10 dBFS | 4/4 +0.01 | 4/4 -0.01 | 4/4 +0.01 | 4/4 +0.00 | 4/4 +0.01 | 4/4 -0.02 | 4/4 +0.00 | 28/28 | 0 | 0 | -0.02 | — | PASS |
| H4 | steady sine | -30 dBFS | 4/4 -0.02 | 4/4 -0.01 | 4/4 +0.01 | 4/4 +0.00 | 4/4 +0.01 | 4/4 +0.02 | 4/4 +0.01 | 28/28 | 0 | 0 | +0.02 | — | PASS |
| H4 | ac2 sweep | -10 dBFS | 8/8 -0.06 † | 8/8 +0.03 † | 8/8 -0.07 | 8/8 -0.11 | 8/8 -0.03 | 8/8 -0.11 | 8/8 -0.05 | 56/56 | 0 | 0 | -0.11 | 8 judged, worst +0.09 | PASS |
| H4 | ac2 sweep | -30 dBFS | 8/8 +0.05 † | 8/8 -0.02 † | 8/8 -0.07 | 8/8 -0.07 | 8/8 -0.09 | 8/8 +0.19 | 8/8 -0.08 | 56/56 | 0 | 0 | +0.19 | 8 judged, worst +0.06 | PASS |
| H5 | steady sine | -10 dBFS | 4/4 +0.01 | 4/4 +0.02 | 4/4 -0.01 | 4/4 +0.01 | 4/4 -0.03 | 4/4 +0.01 | 4/4 +0.01 | 28/28 | 0 | 0 | -0.03 | — | PASS |
| H5 | steady sine | -30 dBFS | 4/4 -0.00 | 4/4 +0.01 | 4/4 -0.01 | 4/4 -0.02 | 4/4 -0.02 | 4/4 -0.01 | 4/4 -0.00 | 28/28 | 0 | 0 | -0.02 | — | PASS |
| H5 | ac2 sweep | -10 dBFS | 8/8 -0.06 † | 8/8 -0.09 † | 8/8 -0.09 | 8/8 +0.18 | 8/8 +0.05 | 8/8 +0.17 | 8/8 -0.08 | 56/56 | 0 | 0 | +0.18 | 8 judged, worst +0.14 | PASS |
| H5 | ac2 sweep | -30 dBFS | 8/8 +0.05 † | 8/8 -0.09 † | 8/8 -0.09 | 8/8 +0.18 | 8/8 +0.10 | 8/8 -0.08 | 8/8 -0.06 | 56/56 | 0 | 0 | +0.18 | 8 judged, worst +0.07 | PASS |

Cell: judged/cells over the cases (and sweep rates), then the quasi-static worst signed error reading − truth (dB; in parentheses when only model-limited cells were judged). F/W: a FAIL/WARN in the cell. †: a judged cell where the pre-filter is > 0.1 dB from flat (Wiener model: a sweep sees it only approximately); such cells are kept out of the status and the worst Δ. Not counted: k·f ≥ fs/2 and, for a sweep, k·f beyond its end. incon.: the reading is a bound (not clear of the floor); missing: no reading where one was due.
