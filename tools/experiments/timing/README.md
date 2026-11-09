# timing: the clock study's logger runs and their analysis

Phase A of `docs/design/timing-study.md` §5: how well each rig's block stamps sit on a line
(Q1) and how the rigs' audio clocks run against each other through REALTIME (Q2).

| path | what it is |
|---|---|
| `run.sh` | `ac2 selftest stamps` plus a 10 s side log of `chronyc tracking` (and `pmc … TIME_STATUS_NP` where it answers without root) |
| `analyse.py` | one or two run directories → fit, residual, wander, pair maps, and the rig-to-rig ratio |

The logger opens capture only, no outputs: nothing can sound. On JACK it is a client of its
own (`ac2-stamps`) beside the running daemon, so the daemon need not stop; rate and buffer
are the server's.

## Phase A on two hosts

Start both runs within a minute of each other, for an hour, at each rig's own settings:

```
# on each host, from the repo (or with AC2=/path/to/ac2)
tools/experiments/timing/run.sh ~/timing/$(hostname)-a1 60 --backend jack --in 1
```

Then one more hour each at another buffer size (change it in the JACK server first). Ctrl-C
ends a run early with its files whole. Each directory holds `blocks.bin`, `pairs.csv`,
`meta.json`, `summary.txt`, `chrony.log` and, where `pmc` answers, `pmc.log`; the formats
are in `crates/ac2-cli/src/selftest/stamps.rs`.

Copy both directories to one machine and run:

```
python3 tools/experiments/timing/analyse.py ~/timing/pupu-a1 ~/timing/ketunkolo-a1
```

It prints per run the capture-stamp line (ppm against the stamp clock, residual sigma, p99,
max), wander per 60 s window, the stamp clock against CLOCK_MONOTONIC and CLOCK_REALTIME,
which of MONOTONIC / MONOTONIC_RAW the stamp clock reads as, and for two runs the rate ratio
between the rigs per window. It writes `residual.csv`, `wander.csv` (each run) and
`ratio.csv` (first run) beside the logs; `--plot` adds `residual.png` when matplotlib is
installed.

## Results stay private

Rig recordings and logs are never committed: keep run directories outside the repo (or
under `runs/` here, which is ignored). What a run showed goes into the study's write-up as
numbers, not files.
