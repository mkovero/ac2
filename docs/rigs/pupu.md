# Rig: pupu

Dedicated audio test rig, shared with the archived `ac` project (its profile:
`~/src/ac/docs/rigs/pupu.md`, the authority for wiring and baseline restore). This page holds
what ac2 learned on it.

- **Access:** private (`$AC_HOME/rig-hosts/pupu.access.env`). Never build on the rig; ship
  binaries. Take the rig lock first: `~/src/ac/bin/rig.sh --lock "<who, what>"`.
- **Audio:** RME Fireface 400, JACK `jack-ac.service`, 96 kHz / 256 frames / 3 periods, `-S`.
  JACK port order can move (ADAT block before/after analog): check silently before emitting —
  captures 9–14 read exact digital zero when analog is first.
- **Wiring:** out 1 (AN1) → Genelec 1083 (two-way: woofer and tweeter); out 2 (AN2) → cable → in 2 (reference
  loopback); in 1 = measurement mic, beyerdynamic MM1 (449350, s/n 34804), **mounted at 90°
  (pointing up)** → use `~/src/ac/beyer/449350_34804_90Grad.txt`.
- **Emission ceilings:** −40 dBFS standing, **−50 dBFS on anything that drives the speaker**.
- **Baseline check (2026-10-03):** 1 kHz at −60 dBFS on out 2 only → in 2 −57.7 dBFS (profile
  −57.5), no leakage into in 1 (−126 dBFS at 1 kHz); mic room noise ≈ −72 dBFS, with occasional
  60–200 Hz rumble bursts up to ≈ −53 dBFS.
- **Network:** ufw active. Since 2026-10-03 (operator approved): `47820/tcp`, `47821/tcp` and
  `5353/udp` allowed from 192.168.9.0/24 only, one rule per port — the RT kernel lacks the
  iptables `multiport` module, so a ufw port *range* is listed but silently not enforced.
  Also `47820/tcp`, `47821/tcp` from 100.100.44.45 (rantu over the VPN; there is a further
  firewall between the VPN and this LAN that must allow those ports too). mDNS does not cross the
  VPN: connect by address. Clients authorized: `ketunkolo` (192.168.9.25), `rantu`
  (100.100.44.45).
- **Operator policy (2026-10-04):** pupu is the ac2 agent's machine to deploy to and restart,
  and the ac2 app on ketunkolo may be (re)started freely. Emission ceilings above still apply;
  anything louder needs the operator's approval for that run. Other changes on ketunkolo
  (system config, non-ac2 software) need asking.

## Deploying a build

Hosts: daemon on pupu (192.168.9.27), app on ketunkolo (192.168.9.25, X display `:0`). Both keep
binaries in `~/ac2-test/bin`, the previous set in `~/ac2-test/bin.prev`. Wrap every `ssh` in
`timeout` with `</dev/null`; start long-running processes with `setsid nohup … </dev/null &`.

1. Build locally: `cargo build --release -p ac2d -p ac2-cli -p ac2-ui` (never on the rig).
2. Ship: on each host `cd ~/ac2-test && rm -rf bin.prev bin.new && cp -a bin bin.prev && mkdir
   bin.new`, then `scp target/release/{ac2d,ac2,ac2-ui} mui@<host>:ac2-test/bin.new/`.
3. Swap: on ketunkolo `pkill -x ac2-ui`; on pupu `pkill -TERM -x ac2d` (clean shutdown flushes the
   autosave and SPL logs), wait for it to exit, `cp -f bin.new/* bin/` on both.
4. Start the daemon on pupu:
   `cd ~/ac2-test && setsid nohup bin/ac2d --listen tcp://0.0.0.0 --name pupu --max-level -50 > d-net.log 2>&1 </dev/null &`.
   Check `d-net.log`: "autosave restored … no audio session opened" is normal.
5. Reopen audio (from ketunkolo):
   `bin/ac2 --remote 192.168.9.27 session open --backend jack --in 1-2 --outputs 2 --loopback-out 2 --loopback-in 2 --mic "1=MM1 34804"`.
   Restored measurements resume; check `meas list`.
6. App on ketunkolo:
   `cd ~/ac2-test && DISPLAY=:0 setsid nohup bin/ac2-ui --remote 192.168.9.27 > ui-net.log 2>&1 </dev/null &`.
   Screenshot: `DISPLAY=:0 xfce4-screenshooter -f -s <file>`.

**When the session format changes**, the daemon sets the old autosave aside as `autosave.vN` and
starts empty. Before such a deploy, save `bin/ac2 --remote 192.168.9.27 state dump` and
`spl leq export --meas <name> <file>` on ketunkolo; afterwards recreate the measurements from
the dump and `trace import` the CSVs in `autosave.vN/traces/` (copy them over with `scp -3`).
The calibration store (`~/.config/ac2/calibrations.json`) survives unless its own format
changes; then redo the electrical calibration with the operator.

**Reading thread costs:** `ac2d-control` at one wake per audio period (≈375/s at 96 kHz/256) is
JACK's process thread (libjack threads take the name of the thread that opened the client), not
the control loop. Most of the daemon's RSS is JACK's `/dev/shm/jack-*` mapping; compare
`Anonymous` in `/proc/<pid>/smaps_rollup`.

## First measurement (2026-10-03, ac2 850a3a4)

Pink noise −50 dBFS on out 1 + out 2, transfer in 1 vs in 2, delay finder: 3.32 ms (319
samples), mic ≈ 1 m. ac2's transfer function agreed with an independent numpy H1 within 0.5 dB
from 100 Hz to 16 kHz. Response re 1 kHz (1/6 oct): within ±3 dB 100 Hz–1.6 kHz; **broad dip
−5…−9.5 dB from 2 to 6.3 kHz** (MM1 90° correction changes it by < 1 dB) — not normal for a
1083 on axis; check mic height/aim against the speaker's reference axis, or the mid/tweeter
section (the `ac` profile notes intermittent distortion). Floor bounce ≈ +3.5 ms at −23 dB;
strong late reflection at +17.9 ms (−18 dB); broadband T20 ≈ 0.1 s. Data:
`/home/mui/ac2-1083-transfer*.png`.

## Network test (2026-10-03)

ketunkolo → pupu over the LAN, CURVE: remote CLI (status, session, measurements) ✓; app on
ketunkolo's display with live JACK data ✓; CLI + app on one daemon stay in sync ✓; unpaired and
pinned-but-unauthorized clients refused ✓ (messages unclear — backlog); daemon killed → app shows
DAEMON NOT RESPONDING + STALE with traces kept, restarted → app resyncs by itself ✓. mDNS
discovery ✗ and remote generator ✗ at first — both fixed in f7991d9 and re-verified on the rig:
`ac2 discover` on ketunkolo lists pupu; remote 1 kHz −60 dBFS on out 2 → `ac2:out_2` −60.0,
in 2 −57.7 dBFS; the daemon connects only the chosen outputs (`out_N → system:playback_N`).
Also verified: rantu over the VPN (100.100.44.45) via a further firewall.

Remote measurement (ketunkolo CLI, −50 dBFS pink, f7991d9): delay 3.32 ms (identical), response
repeats the local measurement within 0.7 dB from 100 Hz to 16 kHz.

## First sweep / distortion run (2026-10-03, ac2 c062151, from the app on ketunkolo)

Daemon `--max-level -50` (hard ceiling). ESS 20 Hz–20 kHz, 6 s × 2 at −50 dBFS on out 1 + out 2,
mic (in 1) re loopback (in 2): arrival 3.33 ms (same as the noise measurements); response matches
the transfer measurement. **No harmonic measurably above its floor anywhere** (0 of 420 points
≥ 6 dB over floor). Upper bounds on THD at this drive: < −28 dB (4 %) 60–120 Hz, < −41 dB (0.9 %)
120–300 Hz, < −49…−52 dB (0.25–0.35 %) 300 Hz–2 kHz, **< −46 dB (0.5 %) in the 2–6 kHz dip
region**. So the dip is not accompanied by measurable distortion at this (quiet) level; the
intermittent fault from the `ac` profile was not visible. Floor is set by room noise and the low
drive: more repeats (8×: about −6 dB) or a louder drive (needs the operator to lift the −50 dBFS
speaker ceiling) would be needed to see distortion. Data: `/home/mui/ac2-1083-sweep*`.

## Mic moved: 94 cm height, re-centred on axis, 1 m (2026-10-03, ac2 c062151, from the app)

Same daemon and session, 12 s sweeps at −50 dBFS: `1083 94cm` (2×) and `1083 94cm 8x` (8×).
Arrival 3.45 ms (+0.11 ms, ≈ 4 cm longer path than at 102 cm).

- **The 2–6 kHz dip did not move.** Third-octave levels re the 200 Hz–1 kHz mean, 102 cm → 94 cm:
  2 k −7.8 → −8.7, 2.5 k −5.0 → −3.8, 3.15 k −6.7 → −7.0, 4 k −7.9 → −7.3, 5 k −5.0 → −4.6,
  6.3 k −3.9 → −3.1; band mean −6.1 → −5.9 dB. 100 Hz–1.6 kHz and 6.3–16 kHz also agree
  within ±1.3 dB. An 8 cm height change shifts floor-bounce and other path-difference notches
  noticeably, so the dip is not a mic-geometry comb: it is the speaker on this axis (or the
  room as a broad feature). Next checks: rear-panel tone/room switch settings, a near-field
  look at each driver, and the response 20–30° off axis.
- **Distortion (8×: floor ≈ 5–6 dB lower than 2×).** THD at or within 1–3 dB of the floor
  everywhere: < −36 dB 60–120 Hz, < −48 dB 120–300 Hz, < −57 dB 300 Hz–2 kHz, < −55 dB
  2–6 kHz. Clusters 6–10 dB over the floor in the 2× run (H2 at 130–140 Hz around −32 dB,
  H4 near 160 Hz) fell to the floor at 8×: room noise during that run, not the speaker.
  One stable feature: **H3 of 510–680 Hz excitation (harmonic at 1.5–2 kHz), −53…−58 dB re
  fundamental (≈ 0.15 %), 8–11 dB over its floor**, seen at the same level in all three
  sweeps. Part of that ratio is the fundamental sitting in a 3–4 dB response dip at 530–570 Hz.
  Small and not a fault indication at this drive. The intermittent fault from the `ac`
  profile was not seen.
- Recount of the first run over all H2–H5 points with the same ≥ 6 dB-over-floor test: 11 of
  1540 points, isolated. The "0 of 420" figure above used a narrower selection.

Data: `/home/mui/ac2-1083-sweep2-94cm.csv`, `/home/mui/ac2-1083-sweep3-94cm-8x.csv`.

## One sweep at −30 dBFS (2026-10-03, operator-requested, ceiling lifted for this run only)

Daemon restarted with `--max-level -30` for one sweep (`1083 94cm -30`, 12 s × 2, same
geometry), then restarted with `--max-level -50` again. Data:
`/home/mui/ac2-1083-sweep4-94cm-30dbfs.csv`.

- **Response unchanged by drive level**: within ±0.6 dB of the −50 dBFS 8× run from 40 Hz to
  16 kHz, overall gain −0.2 dB. No compression; the 2–6 kHz dip is the same.
- **Floor 15–20 dB lower; distortion now clearly measured, third-harmonic dominated.** THD
  (median over band): 0.3 % 30–150 Hz (at the floor below 100 Hz), 0.4 % 150–400 Hz,
  **0.7 % 400–700 Hz (peak H3 −38 dB, 1.3 %, 39 dB over its floor)**, 0.3 % 700 Hz–1.2 kHz,
  ≤ 0.1 % above 1.2 kHz. H2 stays ≤ −49 dB; H4/H5 ≤ −52 dB.
- The H3 region is the one that stood out at −50 dBFS (510–680 Hz, −53…−58 dB). It rose about
  15 dB for 20 dB more drive. A pure cubic nonlinearity would rise 40 dB, so the −50 dBFS value
  was mostly something level-independent (noise/leakage) and the H3 seen here is the speaker.
  An odd-order (symmetric) nonlinearity in the woofer (the box is two-way) is the likely source.
  A −40 dBFS run would pin down its growth with level.
- The daemon logged two "output timing jump 0 → 96000 samples" warnings around arming the
  sweep; the sweep's arrival (3.45 ms) is unchanged, so the measurement is not affected.
  (Explained: false timing peaks at the search-range ends during the sweep's low part; see
  the backlog's Done list.)


## New-build check (2026-10-03, ac2 ac82dab, −50 dBFS, from the app)

Daemon and app updated (protocol 7, session format 5; the v4 autosave was set aside as
`autosave.v4`). Sweep `1083 94cm 50 v2`, 12 s × 2 at −50 dBFS: progress strip ("sweep 2 of 2",
time left, Stop), REF/MEAS-marked input meters (room noise on the MM1 reads ≈ −65 dBFS), STIM OFF
after the sweep, "autosaved just now", dB | % toggle with the log % axis. The export is CSV v2
(sweep info + IR) and re-imports as a sweep: `/home/mui/ac2-1083-sweep5-94cm-50-v2.csv`.
Arrival 3.62 ms, 0.17 ms (≈ 6 cm) later than the morning's 94 cm runs: the mic or the speaker
moved slightly in between. The four morning sweeps were re-imported from their v1 CSVs as
transfer traces (delay kept, distortion not recoverable from v1).

## Independent check of the 2–6 kHz dip (2026-10-03, no ac2 code in the chain)

A Python JACK client on pupu (`~/ac2-test/jsweep.py`, venv `~/ac2-test/venv-meas`) played its
own 12 s exponential sweep × 2 at −50 dBFS RMS to playback_1 + playback_2 and recorded
capture_1 (mic) and capture_2 (loopback) in the same JACK cycles; numpy deconvolved mic re
loopback. Repeat-to-repeat IR difference −36 dB; mic noise −68 dBFS vs −56 dBFS during the sweep.

- **Same answer as ac2:** with ac2's 100 ms window, within 0.5 dB of ac2's sweep in every
  third-octave band 63 Hz–16 kHz. Plot: `/home/mui/ac2-1083-independent-check.png`.
- **The dip is in the direct sound:** gated to 3 ms after the arrival (before the floor bounce at
  +3.44 ms; the only earlier reflection, +2.15 ms at −21.6 dB, can move the level ±0.7 dB at
  most) it keeps the same shape. At 1/12 octave it is two notches, −9…−11 dB at 1.8–2.1 kHz and
  −8…−10 dB at 3–4 kHz, with a peak at 2.5 kHz between them, back to ≈ 0 dB by 7 kHz.
- **Likely cause: woofer–tweeter interference around the one crossover**, i.e. the mic off the
  axis where the two drivers sum in phase. That is vertical (the drivers are stacked); 102 →
  94 cm at 1 m is only ≈ 5°. Next: 3–4 heights through the marked reference axis, 15–30°
  horizontally, and the listening position; check the rear-panel switches.

## Electrical SPL calibration of input 1 (2026-10-04, ac2 54defea)

In-line method: Keysight 34461A in ACV (100 mV range, slow filter) across XLR pins 2–3 of
input 1 through a breakout, MM1 34804 connected with phantom on, gain as for use. (A first
attempt read about 2 mV in a silent room: the leads were on one leg and ground, which shows
half the signal plus common-mode noise; across 2–3 the silent floor reads ≈ 0.00 mV.)
Daemon ceiling lifted to −30 dBFS for this only (operator's approval), 1 kHz sine at
−30 dBFS on out 1 + out 2: DMM 4.315–4.325 mV (4.320 mV used), ac2 read −36.0 dBFS →
0 dBFS = 271.3 mV; with the data-sheet 15.0 mV/Pa → **0 dBFS = 119.1 dB SPL** on input 1
(±1 dB, labelled "electrical in-line, data sheet"). The tone was ≈ 83.2 dB SPL at the mic.
Ceiling back at −50 dBFS afterwards. The calibration holds only at this preamp gain.
