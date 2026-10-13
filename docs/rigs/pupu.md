# Rig: pupu

Dedicated audio test rig, shared with the archived `ac` project (its profile:
`~/src/ac/docs/rigs/pupu.md`, the authority for wiring and baseline restore). This page holds
what ac2 learned on it.

- **Access:** private (`$AC_HOME/rig-hosts/pupu.access.env`). Never build on the rig; ship
  binaries. Take the rig lock first: `~/src/ac/bin/rig.sh --lock "<who, what>"`.
- **Audio:** RME Fireface 400, JACK 96 kHz / 256 frames / 3 periods, `-S`.
- **Services (systemd user units of mui, lingering):** `jack-ac.service` (jackd; counts as
  started once `jack_wait` sees the server), `ac2d.service` (`Wants=` and `After=jack-ac`, not
  `BindsTo=`: ac2d keeps its session through a JACK restart and reopens it),
  `snd-fireface-ctl.service` (FF400 mixer control). Restart on failure; logs in the user journal
  (`journalctl --user -u ac2d -u jack-ac`). The old system `jack-ac.service` is disabled.
  `systemctl --user restart jack-ac` leaves ac2d running and the session reopens by itself
  (about 1 s without audio). A restarted ac2d brings back its measurements but not the audio
  session: reopen it (step 5 below).
  JACK port order can move (ADAT block before/after analog): check silently before emitting —
  captures 9–14 read exact digital zero when analog is first.
- **Wiring:** out 1 (AN1) → Genelec 1083 (two-way: woofer and tweeter); out 2 (AN2) → cable →
  in 2 (reference loopback); in 1 = measurement mic, beyerdynamic MM1 (449350, s/n 34804),
  **mounted at 90° (pointing up)** → use `~/src/ac/beyer/449350_34804_90Grad.txt`. Full table:
  *Wiring and FF400 reset recovery*.
  Position (2026-10-13): MM1 at 56 cm from the Genelec on its tweeter axis, 90° grazing.
- **Analog Discovery 2** (used 2026-10-12, removed after; plug it back for an electrical calibration): scope 1+/1- across in 1's hot/cold through 10 µF 100 V electrolytics (+ to the XLR; phantom is on, so the ±2.5 V range needs the AC coupling). Reads the mic's loaded voltage for electrical calibrations: 1 kHz at −50 dBFS on out 1 → ≈0.39 mV. Host setup (udev, Digilent runtime, `~/ad2/` scripts) in `~/src/sys/pupu.md`.
- **Emission ceilings:** −40 dBFS standing, **−50 dBFS on anything that drives the speaker**.
- **Baseline check:** 1 kHz at −60 dBFS on out 2 only → in 2 −57.7 dBFS (profile −57.5), no
  leakage into in 1 (−126 dBFS at 1 kHz); mic room noise ≈ −72 dBFS, with occasional 60–200 Hz
  rumble bursts up to ≈ −53 dBFS.
- **Network:** ufw active (operator approved): `47820/tcp`, `47821/tcp` and `5353/udp` allowed
  from 192.168.9.0/24 only, one rule per port — the RT kernel lacks the iptables `multiport`
  module, so a ufw port *range* is listed but silently not enforced. Also `47820/tcp`,
  `47821/tcp` from 100.100.44.45 (rantu over the VPN; a further firewall between the VPN and
  this LAN must allow those ports too). mDNS does not cross the VPN: connect by address. ac2d
  warns at start that ufw is enabled; remote clients connect regardless. Clients authorized:
  `ketunkolo` (192.168.9.25), `rantu` (100.100.44.45), `ac2pi` (mui's CLI on the netbooted
  Pi 4, 192.168.9.249) and `ac2pi-kiosk` (that Pi's kiosk UI, which shows pupu's SPL meter;
  image and setup in the sys repo, `ac2pi/`).
- **Operator policy:** pupu is the ac2 agent's machine to deploy to and restart, and the ac2 app
  on ketunkolo may be (re)started freely. Emission ceilings above still apply; anything louder
  needs the operator's approval for that run. Other changes on ketunkolo (system config, non-ac2
  software) need asking.

## Deploying a build

Hosts: daemon on pupu (192.168.9.27), app on ketunkolo (192.168.9.25, X display `:0`). Both keep
binaries in `~/ac2-test/bin`, the previous set in `~/ac2-test/bin.prev`. Wrap every `ssh` in
`timeout` with `</dev/null`; start long-running processes with `setsid nohup … </dev/null &`.

1. Build locally: `cargo build --release -p ac2d -p ac2-cli -p ac2-ui` (never on the rig).
2. Ship: on each host `cd ~/ac2-test && rm -rf bin.prev bin.new && cp -a bin bin.prev && mkdir
   bin.new`, then `scp target/release/{ac2d,ac2,ac2-ui} mui@<host>:ac2-test/bin.new/`.
3. Swap: on ketunkolo `pkill -x ac2-ui`; on pupu `systemctl --user stop ac2d` (SIGTERM: clean
   shutdown flushes the autosave and SPL logs), then `cp -f bin.new/* bin/` on both (`cp`, not a
   directory swap, keeps `ac2-jack-dut` in pupu's `bin/` for `tools/crosscheck`).
4. Start the daemon on pupu: `systemctl --user start ac2d` (flags in
   `~/.config/systemd/user/ac2d.service`: `--listen tcp://0.0.0.0 --name pupu --max-level -50`).
   Check `journalctl --user -u ac2d -n 30`: "autosave restored … no audio session opened" is
   normal.
   `--max-level -50` is the **hard bound**: the system max level in force can be lowered (or
   raised again up to −50) from any client — Settings › Audio & I/O, or
   `bin/ac2 --remote 192.168.9.27 gen ceiling -60dbfs` — and is kept in
   `~/.config/ac2/rig.json`, so a restart comes up at min(kept, bound). A restart with a
   higher `--max-level` does not raise a level that was lowered; raise it explicitly
   (`gen ceiling -50dbfs --yes`). The daemon log's `ac2d::audit` lines say who changed it.
5. Reopen audio (from ketunkolo):
   `bin/ac2 --remote 192.168.9.27 session open --backend jack --in 1-2 --outputs 2 --loopback-out 2 --loopback-in 2 --mic "1=MM1 34804"`.
   Use the inputs and outputs the operator had (lately `--in 1-10`; the old autosave's
   `session.json` records them). Restored measurements resume; check `meas list`.
6. App on ketunkolo:
   `cd ~/ac2-test && DISPLAY=:0 setsid nohup bin/ac2-ui --remote 192.168.9.27 > ui-net.log 2>&1 </dev/null &`.
   Screenshot: `DISPLAY=:0 xfce4-screenshooter -f -s <file>`.

The Pi kiosk is the third host: cross build with `~/src/sys/ac2pi/build-ac2.sh` (steps in the
sys repo), then restart the kiosk after the nfsroot update — an old kiosk is refused at the
protocol check. If the cross build fails on types the checkout no longer has, it reused stale
crates: `find crates -name '*.rs' -exec touch {} +` and rebuild.

**When the session format changes**, the daemon sets the old autosave aside as
`~/.local/share/ac2/autosave.vN` and starts empty. After step 5 (audio reopened with the
operator's inputs, so the running measurements can start), bring it back from ketunkolo;
the CLI reads the folder, so copy it over first (pupu's daemon listens on the network only):
`cd ~/ac2-test && scp -rq mui@192.168.9.27:.local/share/ac2/autosave.vN /tmp/ && bin/ac2 --remote 192.168.9.27 session salvage /tmp/autosave.vN`.
(ketunkolo may lack pupu's SSH host key; then copy by way of the build host, `scp -3`.)
It makes each measurement again with its settings, colour, layout, delay and tracking,
puts each stored trace back under its measurement, rebuilds math channels, starts what was
running, and prints a table of what was restored or skipped and every setting not read as
saved; keep that summary in the deploy log below. It does not bring back the SPL meters'
per-second Leq history (their windows start again; before the deploy, `spl leq export
--meas <name> -o <file>` keeps it) or a mic curve applied to a trace after capture. Run it
once: a second run adds copies.
The calibration store (`~/.config/ac2/calibrations.json`) survives unless its own format
changes; then redo the electrical calibration with the operator.
When an enum in the app's `ui.toml` loses a value, the app drops the whole section holding it
(no compatibility shims): edit the value on ketunkolo and the Pi with the app stopped.

**Reading thread costs:** `ac2d-control` at one wake per audio period (≈375/s at 96 kHz/256) is
JACK's process thread (libjack threads take the name of the thread that opened the client), not
the control loop. Most of the daemon's RSS is JACK's `/dev/shm/jack-*` mapping; compare
`Anonymous` in `/proc/<pid>/smaps_rollup`.

### Deploy log

Last few only; `git log` and `STATUS.md` have the rest. All three hosts unless noted.

| ac2 | date | proto / session fmt | notable |
|---|---|---|---|
| f530d1f9 | 2026-10-13 | 50 / 25 | app only (ketunkolo, Pi; pupu daemon stays on 438048f2): T also takes the band view's and Leq columns' 10 dB lines (no grid) and scale numbers (traces only) |
| 47d7298a | 2026-10-13 | 50 / 25 | app only (ketunkolo, Pi; pupu daemon stays on 438048f2): T on an SPL pane hides the limit text (OK/NEAR/OVER, stay ≤, limit), the Shift+L status-text row is gone; no `ui.toml` had `[leq] status_text`; stray own layouts of FOH SPL, Spectrum 1 and Spectrum 2 cleared with `meas layout clear` |
| 438048f2 | 2026-10-13 | 50 / 25 | pupu only (daemon and CLI; app hosts stay on 7e863a50): session loads check SPL logs and trace data; autosave restored as is (5 measurements, 3 traces, nothing set aside), session reopened `--in 1-8 --outputs 6` |
| 7e863a50 | 2026-10-13 | 50 / 25 | drag rows onto panes (edge-drop splits, Shift-drop compare), Ctrl+Z / Ctrl+Shift+Z undo of pane edits, Leq status text "colours only" (Shift+L, off by default), file.recover / file.salvage as file jobs; autosave restored as is, session reopened `--in 1-8 --outputs 6` |
| 26508046 | 2026-10-13 | 50 / 25 | edits go only to the chosen row's own layout (a chip pick no longer writes the picked measurement's), rate in bits, the audit fixes (gen.acquire disarm, request dedup, file I/O workers, per-pane level axes, hide/compare by id); autosave restored as is |
| 6630842e | 2026-10-13 | 50 / 25 | top-bar marquee + fixed number slots, Ctrl+S, T hides marks, viewer keeps its panes; FOH SPL's stray own layout (a copy of TF 1's, written by the desk under the fixed bug) cleared with `meas layout clear`; Pi `ui.toml` rewritten pinned to FOH SPL (`ui.toml.bak-1791600215` had `ir_mode` and `transfer_view = "response"`, which drop the section) |
| 6d5a4275 | 2026-10-13 | 50 / 25 | top-bar health meter (host CPU, DSP load, xruns, MB/s, fps); autosave restored as is, session reopened `--in 1-8 --outputs 6` |
| 5f18a3ef | 2026-10-12 | 49 / 25 | autosave restored as is (no format change since 2c07efef); input 1 recalibrated electrically with the mic loading (MM1 330 Ω → FF400 XLR 2 kΩ, −1.3 dB): 0 dBFS = 120.5 dB SPL (was 119.1), volts from the AD2 at 1 kHz −40 dBFS |
| 0fef18ea | 2026-10-11 | 43 / 25 | app only (ketunkolo, Pi): a pane whose measurement disappears waits for its name to return |
| 2c07efef | 2026-10-11 | 43 / 25 | salvage of `autosave.v24` restored all twelve measurements; the Pi kiosk fell back to TF 1 during salvage and saved that layout (fixed by 0fef18ea; old file `ui.toml.bak-tf1`) |
| c7bd24b1 | 2026-10-11 | 42 / 24 | first format change restored with `session salvage`; only the per-second Leq history skipped |
| 6f0e5b57 | 2026-10-11 | 39 / 22 | `transfer_view = "response"` in `ui.toml` changed to `"magnitude"` by hand; session cleared by the operator before (`autosave.scratched-2026-10-11`) |

## Network test (2026-10-03)

ketunkolo → pupu over the LAN, CURVE: remote CLI, the app on ketunkolo with live JACK data, CLI
and app on one daemon in sync, unpaired and pinned-but-unauthorized clients refused, a killed
daemon shown as DAEMON NOT RESPONDING + STALE with traces kept and resynced after restart,
`ac2 discover` listing pupu, a remote generator (1 kHz −60 dBFS on out 2 → in 2 −57.7 dBFS; the
daemon connects only the chosen outputs, `out_N → system:playback_N`). rantu over the VPN works
too. A remote −50 dBFS pink measurement repeated the local one within 0.7 dB 100 Hz–16 kHz,
delay identical (3.32 ms).

## Genelec 1083 on axis: the 2–6 kHz dip (2026-10-03)

Mic ≈ 1 m, first at 102 cm height, then 94 cm re-centred on axis. Arrival 3.32–3.62 ms (moves
with small mic/speaker shifts). Floor bounce ≈ +3.5 ms at −23 dB; late reflection +17.9 ms
(−18 dB); broadband T20 ≈ 0.1 s.

- ac2's transfer function agreed with an independent numpy H1 within 0.5 dB 100 Hz–16 kHz, and
  with an independent Python JACK sweep + numpy deconvolution (no ac2 code) within 0.5 dB in
  every third octave 63 Hz–16 kHz.
- Response re 1 kHz: within ±3 dB 100 Hz–1.6 kHz; **broad dip −5…−9.5 dB from 2 to 6.3 kHz**
  (MM1 90° correction changes it by < 1 dB). Moving the mic 8 cm in height changed it by
  ≤ 1.3 dB per third octave while path-difference notches moved, so it is not a mic-geometry comb.
- **The dip is in the direct sound:** gated 3 ms after arrival (before the floor bounce) it keeps
  its shape — at 1/12 octave two notches, −9…−11 dB at 1.8–2.1 kHz and −8…−10 dB at 3–4 kHz,
  a peak at 2.5 kHz between, back to ≈ 0 dB by 7 kHz. Likely woofer–tweeter interference
  around the crossover (mic off the vertical axis where the stacked drivers sum in phase).
  Unchecked: heights through the marked reference axis, 15–30° horizontally, rear-panel
  switches.
- **Distortion:** at −50 dBFS no harmonic stands clear of the room-noise floor except H3 of
  510–680 Hz (≈ −55 dBr, 8–11 dB over its floor). At −30 dBFS (operator-approved, one run) the
  response is unchanged within ±0.6 dB 40 Hz–16 kHz (no compression); THD 0.3 % 30–150 Hz,
  0.4 % 150–400 Hz, **0.7 % 400–700 Hz (H3 peak −38 dB, 1.3 %)**, ≤ 0.1 % above 1.2 kHz;
  H2 ≤ −49 dB. H3 rose 15 dB for 20 dB more drive (a pure cubic would rise 40), so the −50 dBFS
  value was mostly level-independent noise and the −30 value is the woofer's odd-order
  nonlinearity. The intermittent fault in the `ac` profile was not seen.

## Electrical SPL calibration of input 1 (2026-10-04)

Keysight 34461A in ACV (100 mV range, slow filter) across XLR pins 2–3 of input 1 through a
breakout, MM1 34804 connected with phantom on, gain as for use (leads on one leg and ground read
half the signal plus common-mode noise). 1 kHz at −30 dBFS (operator-approved) on out 1 + out 2:
DMM 4.320 mV, ac2 −36.0 dBFS → 0 dBFS = 271.3 mV; with the data-sheet 15.0 mV/Pa →
**0 dBFS = 119.1 dB SPL** on input 1 (±1 dB, labelled "electrical in-line, data sheet"). The tone
was ≈ 83.2 dB SPL at the mic. Holds only at this preamp gain.

## SPL log over 32 h (2026-10-04/05, `spl leq export`)

FOH SPL (in 1, electrical cal), 115 995 rows, exported over the network in 0.5 s (10 MB CSV),
checked by script against the CSV:
- One gap, 13.1 s, at a daemon redeploy; then 26 h continuous with no capture discontinuity.
- Every row a whole measured second, all finite, LAeq and LCeq never above LZeq, one sensitivity
  throughout, no stuck values.
- Row times step 1 s ± one JACK period (wall time read per period); the steps never accumulate.
- **The interface's sample clock runs +5.56 ppm against the system clock** (NTP), stable to
  ±0.3 ppm: 0.48 s per day. Harmless within one device; it is the size of drift
  `docs/design/multi-device.md` must tell apart from device-to-device drift.
- After 26 h: ac2d RSS 167 MB (105 MB JACK shared memory, 54 MB its own); data directory 9.1 MB.

## Wiring and FF400 reset recovery (2026-10-05)

Rewired 2026-10-08 (balanced, more Xone gain). Measured with a −50 dBFS 1 kHz sine on one
output at a time from an independent JACK client (the suite's `jackio`), all eight captures read;
crosstalk elsewhere below −118 dBFS except the Xone's own L/R (−68 dB):

| out | goes to | reads | gain |
|---|---|---|---|
| 1 | Genelec 1083 | — (never probed) | |
| 2 | in 2 (loopback reference) | −47.6 dBFS | +2.4 dB |
| 3, 4 | nothing | — | |
| 5 | Xone:62 ch 1 line L (RCA) → master L → in 5, balanced (no speaker on the Xone) | −47.2 dBFS | +2.8 dB |
| 6 | Xone:62 ch 1 line R → master R → in 6 | −47.2 dBFS | +2.8 dB |

The FF400's jacks: ins 1–4 on the front, 5–8 on the back. In silence ins 5/6 hold the Xone's
noise (−90 dBFS rms) and hum at −111 dBFS (50, 150 Hz). Findings below dated before 2026-10-08
used the old wiring (out 3 → Xone, unbalanced, −14.8 dB → in 5; outs 4–6 → ins 6–8 at −6 dB).

In 1 is the MM1 (phantom, mic gain 20); ins 3–4 are empty. Session:
`session open --backend jack --in 1-8 --outputs 6 --loopback-out 2 --loopback-in 2 --mic "1=MM1 34804"`.

**An FF400 reset (front-panel or bus) takes three steps to recover.** ac2 does its part by
itself (`docs/design/audio-recovery.md`): within a second of the last block the app shows AUDIO
STOPPED (`ac2 status`: `audio STOPPED: …`), ac2d closes its JACK client off its control thread
and keeps reopening the same session (backoff 1 s … 30 s, reopening at once when JACK reports
the device back), so after step 1 the session, its measurements and the SPL log (with the outage
as a gap) come back without `session open`; the generator comes back disarmed. Verified on the
rig by stopping and restarting jackd. Restarting jackd and steps 2–3 stay the operator's:
1. The FireWire layer re-creates the device, but jackd keeps the old card open: `snd_card_free`
   blocks in the kernel (`hung task … fw_device_shutdown`), jackd stops answering (`jack_lsp`
   times out) and ac2 gets no audio and no error — the app shows STALE. A hung jackd is not a
   failed one, so its unit does not restart it: `systemctl --user restart jack-ac` once the
   bus has settled (it exits cleanly on SIGTERM).
2. The control service dies with the reset: `systemctl --user restart snd-fireface-ctl`.
3. The device is back at driver defaults (phantom off, gains 0: the mic reads a dead −102 dBFS;
   no output reaches any input) while ALSA readback still shows the old values. Restore with
   the toggle writes in `~/src/ac/docs/rigs/pupu.md` ("Restore with toggle writes"; not
   `ff400.sh`, which forces phantom off) and verify by emission: the table above, mic room
   noise ≈ −67 dBFS RMS.

## Phase 1 hardware run (2026-10-05)

JACK at the gate's **48 kHz / 128 frames** (3 periods), session 8 in / 6 out, pink noise
−63 dBFS RMS on outs 2–6 (electrical loops; out 1 unconnected), four transfer measurements
against the in 2 loopback plus the SPL meter and a 65536-point spectrum, 65 min:

- **0 xruns**, **0 capture discontinuities**.
- CPU (one core of the i5-2415M): ac2d 28–36 %, jackd 6–9 %.
- ac2d anonymous memory 666 MB, growing ≈ 55 B/s (the per-second SPL log, bounded by its 48 h
  retention). The 666 MB against 54 MB with SPL + spectrum only is the four transfer ladders'
  state (backlog).
- Lines read their expected gain within 0.2 dB, flat 31 Hz–20 kHz, coherence ≥ 0.9987, delay 0;
  phase −7.7° at 31 Hz (input coupling corners). Xone path −17.4 dB at 1 kHz (expected −17.2),
  coherence 0.98 at 50 Hz (mains hum in the mixer path).
- The FF400 kept its settings over both JACK rate changes.

## REW cross-check, electrical (2026-10-07, REW 5.40 beta 135)

Current standings per quantity: `tools/crosscheck/comparison.md`; how to run:
`tools/crosscheck/README.md`.

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
`system:capture_2 → pw_rew:capture_FR` (as on the pre-2026-10-08 wiring;
`tools/crosscheck/rigs/pupu.toml` has the current ports). In REW: device "PCM: pipewire",
stereo only, input L, reference input R, Measure with "loopback as cal and timing reference",
reference output L. Dead ends: the alsa-plugins `jack` PCM (REW's Java sound refuses every
format on it) and `snd-aloop` (not built for the RT kernel). REW's API starts measurements only
with a Pro licence; device, generator and reading results work without. The PipeWire bridge
stays up across an ac2d restart.

Path: Xone → in 5, reference out 2 → in 2; ground truth in 5 ÷ in 2 by direct cross-spectrum
(numpy, 1/48 octave) of the same recording. ac2's sweep agreed with REW's offline import within
0.02 dB mean, with the truth within ±0.17 dB below 100 Hz and ±0.03 dB above 1 kHz, ±0.65°.

**Level convention.** At 1 kHz ac2 read −17.33 dB, REW live −44.87 "dBFS" for a −30 dBFS
sweep. ac2 reports measurement ÷ reference (the dual-channel convention: 0 dB means the
measurement point carries what the reference point carries), so the loopback's own +2.4 dB
gain is in the number: −14.9 − 2.4 = −17.3 dB. REW's "loopback as cal" uses the loopback only for
timing and shape and keeps the measurement channel's level re the digital stimulus:
−44.87 + 30 = −14.87 dB. Both are right for what they state; ac2's sweep reports the difference
as `reference_level` ("reference +2.3 dB" in `ac2 ir` / `sweep run`).

**Delay and group delay.** The path delay is 3.7 µs (REW and direct cross-spectrum); ac2 keeps
the sub-sample part in its phase, which agrees with REW's
(`docs/design/subsample-arrival-group-delay.md`). Phase against steady sines: ac2 within 0.03°
from 31.5 Hz, REW 0.66° off at 16 Hz. Group delay as a central difference of neighbouring
1/48-oct columns was off by up to 31 % at 20 Hz (a 0.03° phase ripple differentiated over
0.6 Hz); a slope fitted over ±1/12 oct is within 7 % from 16 Hz.

**Hand-script level convention.** Early steady-sine scripts (`~/rew-dl/jsine.py`, `jphase.py`)
scaled "−30 dBFS" as a peak of √2·10^(−30/20), i.e. −27 dBFS in the full-scale-sine convention
ac2 and REW use. `tools/crosscheck` uses the full-scale-sine convention throughout and asserts
it.

### Low-frequency harmonics on electrical paths: what was found

The suite drives electrical paths up to −10 dBFS (`electrical_max_dbfs`, backstop −6 in code;
the speaker stays at −50). Baselines in `tools/crosscheck/baselines/pupu/`. Each finding below
was an apparent LF H2 excess on the Xone path; the conclusions hold for any path:

- **Onset and DC-blocking reference.** The loopback (out 2 → in 2) has the interface's
  DC-blocking high-pass, 29 dB below the emitted sweep at 1 Hz, so the reference meets the
  deconvolution's regularisation (−60 dB re peak) near 2 Hz with a steep edge. That edge rings
  as a 0–6 Hz swell around the arrival, which the short harmonic windows' side lobes carry into
  the 40–60 Hz bands (+22 dB at 22 Hz on a 20 Hz sweep; a linear prediction of the path read the
  same, so not the Xone). Fixes: the emitted sweep starts two octaves below the asked start
  (`docs/design/sweep-distortion.md`), and the harmonic and noise windows read the response
  through a fourth-order **minimum-phase** Butterworth high-pass at 0.75 × the lowest analysed
  fundamental. A zero-phase high-pass rang before the arrival into H2's window
  (−L·ln 2 ≈ −0.49 s); minimum phase rings only after it (linear prediction −136 dBr); eighth
  order lifts the noise windows.
- **Truth net of the reference.** ac2 divides by the measured reference, so harmonics the
  reference carries (RME DA out 2 → AD in 2, H2/H3 ≈ −98/−104 dBr at LF) come off the
  measurement's. ac2's sweeps are judged against D_m − T(k·f)·D_r with the sine stage fitting both
  inputs' harmonic phasors jointly with the fundamental (one at a time leaks ~45 dB); REW, which
  deconvolves each input by its own stimulus, against the meas input alone.
- **Mains bands.** pupu shows a line at every 50 Hz multiple to 2 kHz; a harmonic whose ±17 Hz
  band holds one cannot be judged. The Xone tones (15, 26.25, 35, 40.5, 56.2, 60.5, 93.9, 1040,
  10000 Hz) each keep at least two of H2..H5 clear (`tests/test_rig_tones.py`).
- **The Xone's own H2 rise after a fade-in.** A sweep starting at 20 Hz crosses 20–26 Hz within
  0.2 s of full level; the Xone's H2 there reads 3–4 dB high for the first ≈ 100 ms (steady
  22 Hz after a glide: −94.3 dBr at 50 ms, −98…−99.5 from 200 ms; the loopback stays flat). Real,
  in the capture. The suite reports a sweep that reaches 22 Hz less than `lf_onset_settle_s`
  (0.3 s) after full level as INFO.
- **Hum bursts on in 5/6.** 35–95 Hz bursts of −84…−92 dBFS, 0.5–0.75 s every ≈ 2.2 s, over a
  quiet −118, identical on ins 5 and 6 (correlation 0.9995, L − R −120.7 dBFS); the loopback and
  ins 7–8 clean at −128. A burst while the sweep passes f lands in f's harmonic windows; its
  level in dBFS does not follow the drive, so it is added, not generated. ac2 now reads each
  harmonic's floor in the IR gaps beside its window (the noise of the same moment) and takes it
  where it is 3 dB over the post-roll's: such cells are bounds, on ac2 and REW alike. Passive
  captures: the bursts need the FF400 ↔ Xone cable loop (gone with out 5 unplugged, back on any
  FF400 output, not the Xone channel strip, not CPU load). The FF400 (AC transformer supply) and
  pupu (Mac mini, Class II) have no PE; the only ground ties are the FireWire and audio cable
  shields. Most likely switched loads next door coupling into that shield loop; source not
  located.

### Room tone near 240 Hz (2026-10-11, passive capture on in 1)

A narrow tone drifting between 238.5 and 240.75 Hz sits about 16 dB above the neighbouring room
floor on the MM1. It stays with the 1086s off and the 1083 is passively cooled, so it is the
building's plant. It comes and goes within minutes, so a noise gate fed only by the suite's
silent recording can judge that column as signal; the suite's noise gate also reads each take's
own quiet parts before and after its sweep and keeps the larger noise per column.

### Digital DUT findings (2026-10-09, −10 dBFS)

- **Floor +3 dB** in the suite's model: ac2 divides by a noisy reference too (H = M/R adds
  −H·Nr/S; with |H| ≈ 1 and equal noise on both inputs the floor rises 3 dB).
  `dsp.deconvolve_noise` takes both inputs.
- **Fade-out edge:** an order is reported only where its whole band lies below the sweep's
  fade-out (on a 40 kHz sweep H5's top column 7772 Hz straddled it and read high).
- **LF harmonics fine** keeps the shared noise windows where standard has them: later lags put
  a DUT's aliased products at lower frequencies (fa = fs·x/(k+x), x = e^(−τ/L)) and hid H5.
  Fine reads 12 and 15 Hz +0.3…+0.7 dB above the steady sine behind the DUT's 20 Hz high-pass:
  band averaging over a steep slope at its lowest columns; a longer sweep reduces it.

### Genelec at −30 dBFS (2026-10-09/10, operator-approved)

The suite lifts its −50 speaker ceiling only with `--allow-speaker-level` (−30 at most) and
ac2d's bound only for the speaker stage; `gen ceiling` read bound −50 after every run.

- Magnitude (1/48 oct) against numpy on REW's recording: ac2 sweep within 0.05 dB, REW (its mic
  curve taken back out) likewise; ac2 − REW phase −0.4…+0.2°.
- Live TF (8 blocks, delay inserted): mean within 0.07 dB; its per-column spread (±0.4 dB) is
  the estimate's variance, σ = √((1 − γ²)/(2γ²n)) ≥ 0.2 dB at γ² = 0.99, not a bias.
- Harmonics where both are above floor agree within 0.3 dB (H3 at 500 Hz / 1 kHz, H2 at
  5 kHz); H3 = 1504.5 Hz sits on the 1500 Hz mains line, so that cell is INCONCLUSIVE.
- A level row comparing REW's single 0.37 Hz bin to ac2's 14 Hz-wide 1/48-oct column read the
  room's fine structure (−0.136 dB); REW's bins power-averaged into ac2's columns give +0.005 dB
  median over 500 Hz–2 kHz.
- Ambient with an averaged RTA (`--average fifo:N` over the Leq window): LZ / LA / LC within
  0.005 dB of numpy, LAeq ac2 vs REW −0.009 dB; worst third octave +1.69 dB at 79 Hz (filter
  skirts against ideal band edges in a falling LF ambient).
