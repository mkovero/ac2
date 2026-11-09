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

## Phase C: the network, no audio

`ac2 selftest net` streams block-sized messages of zeros between two hosts at an audio
block rate over the data wire's transport (CURVE, ROUTER on the listening side, DEALER on
the connecting side, the receiver returning byte credit in a 64 KiB window), for Q6. Who
listens and who sends are independent: let the firewall decide who binds. The listener
prints its key and writes it to `<out>/server.key`; the other side passes it as
`--server-key`. Run it while the app runs as usual, an hour each way, same `--minutes` on
both hosts; `--rate 96000 --frames 256 --channels 1` are the defaults (the sender's apply).

pupu → ketunkolo (ketunkolo listens and receives):

```
# on ketunkolo (192.168.9.25)
ac2 selftest net --listen 0.0.0.0:47990 --role receive --minutes 60 --out ~/timing/net-pupu-to-ketunkolo
# on pupu, with the key ketunkolo printed
ac2 selftest net --connect 192.168.9.25:47990 --server-key '<key>' --role send --minutes 60 --out ~/timing/net-pupu-to-ketunkolo
```

ketunkolo → pupu (ketunkolo still listens, now sends; a new listen prints a new key):

```
# on ketunkolo
ac2 selftest net --listen 0.0.0.0:47990 --role send --minutes 60 --out ~/timing/net-ketunkolo-to-pupu
# on pupu
ac2 selftest net --connect 192.168.9.25:47990 --server-key '<key>' --role receive --minutes 60 --out ~/timing/net-ketunkolo-to-pupu
```

The receiver writes `arrivals.bin` (seq, scheduled, sent, arrived per block; sent and
scheduled on the sender's REALTIME, arrived on the receiver's) and `meta.json`, and ends
with the one-way delay above its floor (the hosts' clock offset drops out), the arrival
interval jitter, missing and reordered blocks, and the suggested link offset (one-way
p99.9 plus one block). The sender's `meta.json` holds its credit stalls and late sends.
Then, on the receiving host's directory:

```
python3 tools/experiments/timing/analyse_net.py ~/timing/net-pupu-to-ketunkolo
```

It prints the same summary, writes `per_minute.csv` (the stats per 60 s of the sender's
schedule) and `histogram.csv` (one-way delay above the floor, 50 µs bins), and lists the
worst minutes.

## Results stay private

Rig recordings and logs are never committed: keep run directories outside the repo (or
under `runs/` here, which is ignored). What a run showed goes into the study's write-up as
numbers, not files.
