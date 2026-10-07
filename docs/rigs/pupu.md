# Rig: pupu

Dedicated audio test rig, shared with the archived `ac` project (its profile:
`~/src/ac/docs/rigs/pupu.md`, the authority for wiring and baseline restore). This page holds
what ac2 learned on it.

- **Access:** private (`$AC_HOME/rig-hosts/pupu.access.env`). Never build on the rig; ship
  binaries. Take the rig lock first: `~/src/ac/bin/rig.sh --lock "<who, what>"`.
- **Audio:** RME Fireface 400, JACK 96 kHz / 256 frames / 3 periods, `-S`.
- **Services (systemd user units of mui, lingering; since 2026-10-07):** `jack-ac.service`
  (jackd; counts as started once `jack_wait` sees the server), `ac2d.service` (`Wants=` and
  `After=jack-ac`, not `BindsTo=`: ac2d keeps its session through a JACK restart and reopens
  it), `snd-fireface-ctl.service` (FF400 mixer control). Restart on failure; logs in the user
  journal (`journalctl --user -u ac2d -u jack-ac`). The old system `jack-ac.service` is
  disabled. Checked 2026-10-07 (ac2d 1303a63): `systemctl --user restart jack-ac` left ac2d
  running and the session reopened by itself after 1.1 s without audio. A restarted ac2d
  brings back its measurements but not the audio session: reopen it (step 5 below).
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
  (100.100.44.45), `ac2pi` (mui's CLI on the netbooted Pi 4, 192.168.9.249) and `ac2pi-kiosk`
  (that Pi's kiosk UI, which shows pupu's SPL meter; image and setup in the sys repo, `ac2pi/`).
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
3. Swap: on ketunkolo `pkill -x ac2-ui`; on pupu `systemctl --user stop ac2d` (SIGTERM: clean
   shutdown flushes the autosave and SPL logs), then `cp -f bin.new/* bin/` on both.
4. Start the daemon on pupu: `systemctl --user start ac2d` (flags in
   `~/.config/systemd/user/ac2d.service`: `--listen tcp://0.0.0.0 --name pupu --max-level -50`).
   Check `journalctl --user -u ac2d -n 30`: "autosave restored … no audio session opened" is
   normal.
   `--max-level -50` is now the **hard bound**: the system max level in force can be lowered
   (or raised again up to −50) from any client — Settings › Inputs & outputs, or
   `bin/ac2 --remote 192.168.9.27 gen ceiling -60dbfs` — and is kept in
   `~/.config/ac2/rig.json`, so a restart comes up at min(kept, bound). A restart with a
   higher `--max-level` does not raise a level that was lowered; raise it explicitly
   (`gen ceiling -50dbfs --yes`). The daemon log's `ac2d::audit` lines say who changed it.
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

## SPL log over 32 h (2026-10-04/05, ac2d b97a2c9, `spl leq export`)

FOH SPL (in 1, MM1 34804, electrical cal) logged 2026-10-04 08:59:44 → 2026-10-05 17:13:11 UTC,
exported over the network in 0.5 s (115 995 rows, 10 MB CSV). Checked with a script against the
CSV, not against ac2's own summary:
- **One gap, 13.1 s**, at 2026-10-04 15:15:14 UTC: the daemon redeploy (ac2d started 15:15:16).
  Since then 26 h of continuous run with no capture discontinuity in `d-net.log` (the fan-out
  logs every one).
- Every row a whole measured second (`measured_s` = 1), no non-finite values, LAeq and LCeq never
  above LZeq, one sensitivity (119.13 dB) throughout, no stuck values. Totals over the log:
  LAeq 40.1, LCeq 52.7, LZeq 55.8 dB.
- Row times step by 1 s ± one JACK period (2.67 ms at 96 kHz / 256): the wall time is read per
  period; the steps alternate and never accumulate.
- **The interface's sample clock runs +5.56 ppm against the system clock** (NTP), stable to
  ±0.3 ppm over 20+ stretches of 1 000–25 000 s: 0.48 s per day. A single device is one clock
  domain, so this is harmless; it is the size of drift `docs/design/multi-device.md` must tell
  apart from device-to-device drift.
- After 26 h: ac2d RSS 167 MB, of which 105 MB JACK shared memory and 54 MB its own (one
  sample, not a trend); the data directory (autosave, log, traces) 9.1 MB in all.

## Wiring and FF400 reset recovery (2026-10-05)

Measured with a −60 dBFS 1 kHz sine from ac2 on one output at a time, all eight captures read by
an independent JACK client (`~/ac2-test/probe_levels.py`, venv `venv-meas`); crosstalk elsewhere
below −118 dBFS:

| out | goes to | reads | gain |
|---|---|---|---|
| 1 | Genelec 1083 | — (never probed) | |
| 2 | in 2 (loopback reference) | −57.6 dBFS | +2.4 dB |
| 3 | Xone:62 ch 1 (RCA L) → Xone mono out → in 5 (no speaker on the Xone; EQ off) | −74.8 dBFS | −14.8 dB (Xone gain/fader) |
| 4 | in 6 | −66.0 dBFS | −6.0 dB |
| 5 | in 7 | −66.1 dBFS | −6.1 dB |
| 6 | in 8 | −66.1 dBFS | −6.1 dB |

In 1 is the MM1 (phantom, mic gain 20); ins 3–4 are empty. Session:
`session open --backend jack --in 1-8 --outputs 6 --loopback-out 2 --loopback-in 2 --mic "1=MM1 34804"`.

**An FF400 reset (front-panel or bus) takes three steps to recover**, as on 2026-10-05. Since
protocol 21 ac2 does its part by itself (`docs/design/audio-recovery.md`): within a second of
the last block the app shows AUDIO STOPPED (`ac2 status`: `audio STOPPED: …`), ac2d closes
its JACK client off its control thread and keeps reopening the same session (backoff 1 s …
30 s, the state saying what it waits for, e.g. no JACK server), so after step 1 the session,
its measurements and the SPL log (with the outage as gap) come back without `session open`;
the generator comes back disarmed. Restarting jackd and steps 2–3 stay the operator's:
1. The FireWire layer re-creates the device, but jackd keeps the old card open: `snd_card_free`
   blocks in the kernel (`hung task … fw_device_shutdown`), jackd stops answering (`jack_lsp`
   times out) and ac2 gets no audio and no error — the app shows STALE. A hung jackd is not a
   failed one, so its unit does not restart it: `systemctl --user restart jack-ac` once the
   bus has settled (it exited cleanly on SIGTERM). Before protocol 21 ac2d closed the session after one
   failed reopen and `session open` as above was needed; now it reopens by itself once jackd
   answers.
2. The control service (`snd-fireface-ctl`, user unit) dies with the reset:
   `systemctl --user restart snd-fireface-ctl`.
3. The device is back at driver defaults (phantom off, gains 0: the mic read a dead −102 dBFS
   and failed a clap test; no output reached any input) while ALSA readback still shows the old
   values. Restore with the toggle writes in `~/src/ac/docs/rigs/pupu.md` ("Restore with toggle
   writes"; not `ff400.sh`, which forces phantom off) and verify by emission: the table above,
   mic room noise ≈ −67 dBFS RMS.


## Phase 1 hardware run (2026-10-05, ac2d b97a2c9)

JACK at the gate's **48 kHz / 128 frames** (3 periods), session 8 in / 6 out, pink noise
−63 dBFS RMS on outs 2–6 (the electrical loops; out 1 unconnected — −50 dBFS RMS was refused
by the peak ceiling), four transfer measurements against the in 2 loopback plus the running SPL
meter and a 65536-point spectrum. Sampled every 5 min for 65 min (noise ran 92 min):

- **0 xruns** (`jackd` log), **0 capture discontinuities** (ac2d log).
- CPU (one core of the i5-2415M): ac2d 28–36 %, jackd 6–9 %.
- ac2d anonymous memory 666 MB, growing 16 kB per 5 min (≈ 55 B/s: the per-second SPL log
  held in memory, bounded by its 48 h retention). The 666 MB against 54 MB at 96 kHz with
  SPL + spectrum only is the four transfer ladders' state — worth a look (backlog).
- Results against the expected answer (ref in 2 reads +2.4 dB, rear ins −6 dB → −8.4 dB):
  lines 6/7/8 −8.38…−8.65 dB, flat within 0.2 dB 31 Hz–20 kHz, coherence ≥ 0.9987, delay
  0 samples; phase 0° mid-band, −7.7° at 31 Hz (different input coupling corners). The Xone
  path: −17.4 dB at 1 kHz (expected −17.2), rising +1.8 dB to 20 kHz with −22° there, and
  coherence down to 0.94 (0.98 at 50 Hz: mains hum in the mixer path).
- Afterwards JACK went back to 96 kHz / 256 and the session reopened; the FF400 kept its
  settings over both JACK restarts (mic alive, tone levels unchanged at 48 kHz).

## Deploy of 645f5ec and audio recovery on the rig (2026-10-06)

Deployed 645f5ec (protocol 21, session format 9). The format-8 autosave was set aside as
`autosave.v8` as expected; before the deploy `state dump`, `spl leq export` (48 h, 15 MB) and a
CSV export of each of the 10 traces were saved (`/work/ac2-scratch/deploy-645f5ec/` on the dev
host). Afterwards FOH SPL (with its five C windows, limits 70 dB, warn 3 dB, horizon 1 min),
Spectrum 1/2 and TF 1 were recreated with `meas new` / `spl leq set`, and the 10 traces imported
(the four test transfer measurements of the phase 1 run were not recreated). Calibration survived
(the store's format did not change). A CLI `gen pink` from the phase 1 run on ketunkolo had never
exited; the new daemon refused it at protocol 14 and it was killed.

**Recovery, real hardware:** `pkill -TERM jackd` at 19:03:13 UTC → within the same second the
daemon logged the host ending the stream; every pane showed `AUDIO STOPPED · audio host ended
the stream at 22:03`, the top bar `audio stopped · reopening (attempt 3, next in 3 s)`, `ac2
status` the backend's reason ("No JACK server: start JACK"). jackd started again at 19:03:32 →
the session reopened by itself on attempt 6 at 19:03:44 (the next backoff step), epoch 2, the
same measurements running, SPL fresh, the Leq caption counting "offline 31 s"; the FF400 kept
its settings. Not tried: a hung (not stopped) jackd, as after the FF400 reset.

## Deploy of 360c6b8 (2026-10-07)

Protocol 23, session format 10 (math channels, Settings, stimulus by view). Same procedure as
645f5ec: `state dump`, both SPL meters' logs and all 8 traces saved first
(`/work/ac2-scratch/deploy-360c6b8/` on the dev host); after the swap the six measurements were
recreated (FOH SPL with five C windows at 70 dB; a second FOH SPL with `--preset din15905`,
which brings LAeq 30 min ≤ 99 and LCpeak ≤ 135; Spectrum 1 at 1/3 oct, Spectrum 2 unsmoothed;
TF 1 stopped, TF 2 running) and the traces imported hidden. The system max level reads −50 dBFS,
the `--max-level` bound; changes made in Settings now persist in the daemon's `rig.json`.

**Reopen probe, real hardware:** jackd stopped and started again 6 s later; the daemon logged
"the audio device is back: reopening now" and reopened at once (8.1 s without audio in all,
against 12 s after jackd's return with the backoff alone on 645f5ec).

## Deploy of 1303a63 (2026-10-07)

Protocol 24, session format 11 (measurement tree, sweep as a measurement kind, IR/sweep zoom
and cursor). Backup in `/work/ac2-scratch/deploy-1303a63/`; restored as for 360c6b8, plus the
operator's two math channels on stored traces (`ac2 math new --name … --op div --a "1083 94cm"
--b "1083 94cm -30" --imported --start`, and `-30 ÷ 50 v2`), which came back with the same ids.

## Deploy of a680dcd (2026-10-07)

ac2d only (protocol 24 and the session format unchanged since 1303a63; the apps stay): math
channels form a result only when a live operand has one. `bin.prev` is 1ae159c (what ran
before). Deployed with the user unit (`systemctl --user stop/start ac2d`), session reopened
from ketunkolo. Per-topic traffic over 30 s with a throwaway CURVE subscriber on the Pi:
689.7 → 238.2 KiB/s (the two ÷ channels of stored traces 30 → 3.8 msg/s, the spectrum sum
30 → 17.4 msg/s); ac2d CPU over 60 s 60 → 48 % of a core (the sum's thread 20.8 → 13.0 %).

## Deploy of 6136f8a (2026-10-07, all three hosts)

Protocol 25, session format 12 (a measurement's delay steps kept apart from its measured
arrival, colour families, toasts sized to their text with a log, no calibration line on the
SPL stage). Backup in `/work/ac2-scratch/deploy-6136f8a/` (state, SPL logs, 15 traces as
CSV). The restore is now a script, `restore.sh` there, generated from the state dump: it
recreates the 13 measurements in id order so ids match, then imports the traces and moves each
under its owner, slot and visibility, then sets TF 2's delay (44 samples). Run it from
ketunkolo only after ketunkolo has the new CLI: the old one is refused at the protocol check.
The PipeWire bridge REW uses (see below) stayed up across the `ac2d` restart.
The Pi's kiosk must be restarted after its nfsroot update; the old one is refused as
protocol 24 until then.

## REW cross-check, electrical (2026-10-07, ac2d a680dcd, REW 5.40 beta 135)

REW runs on pupu itself (`~/REW`, headless on Xvfb `:5`, its API on localhost:4735, the GUI
driven with xdotool). REW talks only ALSA and jackd owns the FF400, so REW reaches the
interface through **PipeWire as a JACK client**: `pipewire`, `pipewire-alsa`, `wireplumber`
and `pipewire-jack-client` (module-jack-tunnel; *not* `pipewire-jack`, which replaces libjack).
`~/.config/pipewire/pipewire.conf.d/90-jack-tunnel.conf` fixes the graph at 96 kHz and makes a
duplex tunnel client `pw_rew` with `jack.connect = false`;
`~/.config/wireplumber/wireplumber.conf.d/90-rig-no-devices.conf` disables every device monitor,
so PipeWire never opens the FF400 or the onboard card. The tunnel is wired by hand after each
PipeWire start (a restart leaves it unconnected): `pw_rew:playback_FL → system:playback_2`,
`playback_FR → system:playback_3`, `system:capture_5 → pw_rew:capture_FL`,
`system:capture_2 → pw_rew:capture_FR`. In REW: device "PCM: pipewire", stereo only, input L,
reference input R, Measure with "loopback as cal and timing reference", reference output L.
Checked with a −60 dBFS REW sine: in 2 −57.6, in 5 −74.9 dBFS (the wiring table), no tone on
ins 1, 3–4, 6–8. Dead ends: the alsa-plugins `jack` PCM (arecord works, REW's Java sound
refuses every format on it) and `snd-aloop` (not built for the RT kernel). REW's API starts
measurements only with a Pro licence; device, generator and reading results work without.

Path: out 3 → Xone → in 5, reference out 2 → in 2. Ground truth: in 5 ÷ in 2 computed directly
(numpy, cross-spectrum over the whole recording, 1/48-octave bands) from a recording of the
same path.

| comparison (1/48 oct, 31 Hz – 20 kHz) | magnitude | phase |
|---|---|---|
| ac2 sweep −50 dBFS vs REW offline import (same recording as truth) | ≤ 0.02 dB mean, ±0.08 dB below 100 Hz, ±0.02 above 1 kHz | — |
| ac2 sweep −50 dBFS vs truth | ±0.17 dB below 100 Hz, ±0.03 dB above 1 kHz | ±0.65° |
| ac2 sweep −30 dBFS vs REW live −30 dBFS (constant offset removed, see below) | ±0.017 dB below 100 Hz, ±0.005 dB above 1 kHz | ±0.7° |
| ac2 transfer (pink-noise style FIFO of 8 blocks) fed one REW sweep | only where γ² ≥ 0.99 (138 points): 0.074 dB max | — |

The live transfer measurement is not a sweep analyser: with the default 8-block FIFO a one-shot
sweep leaves the average seconds later and coherence drops to 0.0001 there, which it shows.
REW's offline import refers each channel to its own timing marker, which removes the 3.7 µs
between in 5 and in 2 (a constant 3.25 µs in phase); live, with the loopback as timing
reference, REW keeps it (3.7 µs, as the direct computation).

**Level convention.** At 1 kHz: ac2 −17.33 dB, REW live −44.87 "dBFS" for a −30 dBFS sweep.
A sine probe gives out 3 → in 5 −14.9 dB and out 2 → in 2 +2.4 dB (in 2's preamp trim). ac2
reports measurement ÷ reference, −14.9 − 2.4 = −17.3 dB, the dual-channel convention: 0 dB means
the measurement point carries what the reference point carries, so the loopback's own gain is
in the number. REW's "loopback as cal" uses the loopback only for timing and shape (normalised to
0 dB) and keeps the measurement channel's absolute level re the digital stimulus: −44.87 + 30 =
−14.87 dB, the path's gain re what was emitted. Both are right for what they state; ac2's sweep
already measures the difference (`reference_level`, 2.33 dB on this run, "reference +2.3 dB" in
`ac2 ir` / `sweep run`).

**Harmonic distortion disagrees** (−30 dBFS; the operator allowed −30 for electrical paths
only, ac2d raised with a runtime drop-in and returned to −50 afterwards). Ground truth: steady
sines at −30 dBFS (`~/rew-dl/jsine.py`, Blackman FFT of 2 s): the Xone path shows H2/H3 ≈ −71
… −80 dBr at every frequency (bin noise −108 dBr); the loopback alone is clean (−110 … −120).

| f | truth H2 | ac2 sweep H2 (its floor) | REW live H2 |
|---|---|---|---|
| 20 Hz | −75 | −53 (−75) | −90 |
| 31.5 Hz | −71 | −60 (−82) | −87 |
| 50 Hz | −73 | −69 (−87) | −97 |
| 100 Hz | −76 | −86 (−84) | −92 |
| 1 kHz | −79 | −79 (−80) | −88 |

ac2 overstates H2 below about 63 Hz (22 dB at 20 Hz, falling about 6 dB per third octave) while
calling it well above its floor: under investigation. Above 100 Hz ac2 agrees within a few dB
but places the harmonics at its own floor; REW reads 8–15 dB below the steady sines throughout,
unexplained. At −50 dBFS both are floor-limited.
