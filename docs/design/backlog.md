# Backlog

Work found in use, not yet scheduled into a phase. Newest first. Move an item to "Done" with
the commit when it lands.

## Performance and platforms (2026-10-05, after the laptop / Pi performance pass)

Targets: laptops with integrated GPUs on battery, and a Pi 4 class daemon for SPL (PLAN
decision 4). Measured numbers: PLAN §9.0.

- **ac2d holds 666 MB with four transfer measurements** (54 MB with SPL + spectrum only; the
  phase 1 run on pupu, `docs/rigs/pupu.md`). Stable over the hour, so not a leak, but large
  for a Pi 4 class daemon: account for the MTW ladders' buffers (and the 65536-point
  spectrum) per job.
- **Audio continuity is not in `ac2 status`**: xruns and capture discontinuities are only
  logged by the daemon (`capture discontinuity at sample …`); a long-run check has to grep the
  log. Counters since the session opened (and the last one's time) in `status` / `--json`
  would make the 24 h check (`docs/rigs/pupu.md`) one command. Memory over time is likewise
  only a `/proc` sample.
- **macOS not verified with an audio interface**: a tester has the dev.9 disk image and
  `testing/macos/README.md` (two cables, four tests, a report table). Results pending.
- **App Nap**: the macOS hand-off timer is no longer coalesced (kqueue `NOTE_CRITICAL`,
  9dc4274), but a hidden app on battery may still be App Napped, throttling a daemon the app
  hosts. Would need an activity assertion from the UI while a session is open.
- **The UI still redraws the whole window per data frame**: under a software renderer, 4
  live measurements cost ≈ 1.7 cores, SPL only ≈ 1.3 (egui has no damage regions). Not yet
  measured on a real laptop iGPU (PLAN §8.3 frame-time target).
- **Software adapter**: see *Windows* below (reduced frame rate when wgpu reports a CPU
  adapter).
- **No flow control to slow clients**: a Wi-Fi client can lag up to the 48-frame send queue;
  client-paced credit is designed in `docs/design/flow-control.md`, not implemented (wire
  change).
- **Delay finder full searches run on the transfer job's thread**: every 2 s while tracking,
  about 33 ms on a desktop core, likely 150–250 ms on a Pi; `detect_period` is ~60 % of it.
- **A weighting could reuse the C filter** (A = C + one more section): two biquads per
  sample saved on the SPL path.
- **Pi 4 class: built and tested in emulation only** (2026-10-05, c59ef65). A Raspberry Pi
  image (Arch Linux ARM, stock `linux-rpi`; JACK on the first USB interface, `ac2d
  --listen`, `ac2-ui` full screen under the `cage` kiosk compositor, the UI self-paired over
  CURVE) is built outside this repo, for a CM4 on a CM4IO and a Pi 4 B from SD. In
  emulation: the aarch64 (cortex-a72) suites of ac2-core, -proto, -zmq, -traces, -scene,
  ac2d, -cli and -client pass under qemu-user against the image's own libraries (711 passed,
  0 failed, 3 ignored); in QEMU the image boots, a JACK session opens from the CLI and the
  kiosk UI shows it. Still open, needs the board: NEON speed of the RTA's 4-band groups and
  the f32 mic-curve FIR, V3D (v3dv) as the wgpu adapter and its frame time (§8.3), SD-card
  writes, a real interface; then an HW gate in PLAN.
- **`ac2-zmq`'s test binary does not link with GNU ld**: on its link line `-lsodium` comes
  before the static `libzmq.a`, so ld drops it (`undefined reference to sodium_init`,
  `crypto_box`, …). The release binaries link (their order differs). Hidden on x86_64 Linux,
  where rustc links with rust-lld by default; seen cross-building for aarch64, and a native
  build on a Pi uses GNU ld too. `build.rs` should emit the libraries in dependency order
  (zmq, then sodium); rust-lld (`-fuse-ld=lld` with rustc's `gcc-ld`) works around it.

## Electrical calibration (left after the first version, `q7-calibration.md` §11)

- **Stored traces do not record the method**: `CalState::Calibrated` names the key and the
  sensitivity, not whether it was electrical or its uncertainty; trace captions say `cal`
  either way. Adding it is a session-format change.
- **Clipping refusal is not exercised end to end**: the fake rig's generator ceiling keeps
  the input below full scale; the clip tracking is covered only by reading.
- **No guided in-line tone**: the operator starts the 1 kHz tone (own source or the
  generator); the dialog could offer to arm a 1 kHz sine under the usual ceiling.

## Windows (2026-10-03, operator's Windows 11 VM, release build of 6c122d5)

- MSI installed cleanly (unsigned: SmartScreen "More info → Run anyway"), Start-menu entry and
  PATH fine; the app ran the simulated rig. Real audio (WASAPI) on Windows is still untested.
- **Sluggish without a GPU** (VM, software adapter). When wgpu reports a CPU / software adapter
  (WARP, llvmpipe, lavapipe), lower the redraw rate and skip costly effects (MSAA, blur), and
  say so once ("software rendering: reduced frame rate"); measure frame time before and after.

## Flaky tests

- **ac2d `remote_stimulus` "timed out waiting for the tone"**: once in a full workspace run
  at load ~30 (no loopback, so the timing job is not involved); passed 4/4 alone.

- **ac2d `traces::capture_average_math_export_import`, unaligned capture at 301.6 Hz:
  −6.37 dB (want −6.02 ± 0.3)**, once, in a full `cargo test --workspace` at load ~40 after
  the capture-slot fix. Unloaded the error there is −0.002 ± 0.028 dB (300 runs, max 0.10);
  200 runs of the traces suite at load 40–130 did not repeat it. Not queue overflow (no
  capture-discontinuity warning in the output). Next: log `eff_avg` and the block count the
  low-frequency stage averaged when the band check fails.

## Parked

- **Genelec 1083 on pupu: 2–6 kHz dip of −5…−9.5 dB** (docs/rigs/pupu.md). Mic at 102 cm
  and 94 cm (re-centred) gives the same dip within ±1.2 dB, and no distortion above 0.15 % goes
  with it, so it is not a reflection at the mic, and an independent JACK + numpy capture confirms it in
  the direct sound (docs/rigs/pupu.md). Two-way box: most likely woofer–tweeter interference off
  the summing axis. Left: heights through the reference axis, 15–30° off axis, rear-panel
  settings, listening position; listen during a burst;
  `ac2 ir capture --ref 2 --mic 1 --out 1,2 --level -50dbfs --duration 6s --repeats 2`.

## Done

Stimulus conveniences (operator, 2026-10-07: "it could stop also stop stimulus if you stop
running transfer measurement, somehow I would expect it"):
- **Stopping the last running transfer measurement stops the stimulus** this app holds
  (armed or playing), the way Esc does, with one toast for both; another running transfer
  measurement keeps it, a sweep and another client's stimulus are never stopped by it.
  `ac2 meas stop` is unchanged (no side effects for scripts). Deleting the last running
  transfer measurement does the same, under the same rule.
- **NO REFERENCE reminds of the stimulus keys** ("when you havent start stimulus but you
  have started transfer there could be reminder that press space to arm and enter to
  begin"): the banner detail says `stimulus off: Space arms, Enter starts it` / `stimulus
  armed: Enter starts it` (the sweep view: `arm it from a transfer pane`), short enough to
  show beside the banner text in every theme; the IR pane's reason has room for the whole
  sentence (`nothing is playing — Space arms, Enter starts the stimulus`). Playing, the
  loopback-patch text as before.
- **Full screen stays the pane alone** ("'arm' and probably driving too interfere
  fullscreen ... if you want things to be fullscreen then let them be fullscreen"; "I dont
  expect even the badge on full screen"): arming, firing, stopping, an arm queued behind a
  stop and a running sweep no longer bring the top bar back in the stage view, so no pane
  resizes; no badge either. Esc / Shift+Esc stop as before; every other layout shows the
  stimulus in the top bar (PLAN §8.2). The sweep progress strip outside full screen is
  drawn over the bottom of the pane area (opaque enough to read, framed in the armed
  colour) instead of taking a row above it: a sweep never resizes or moves the panes.
- **The Leq run line only with the history** ("the online since nn:nn and LAeq total and
  offline would only be shown with shift+B history look"): `running … since … · LAeq total
  … · offline …` is drawn only with Shift+B on, in every SPL view; without it the windows
  and the number get its row. Per-window offline notes and `spl leq watch` unchanged.
- **No caption line on the Leq stage** ("on Leq fullscreen view, the grey title-line
  describing calibration and such is not necessary too"): in full screen the meter /
  calibration line shows only with the history on (or STALE); its room goes to the windows
  and the number. Outside full screen as before.

A measurement owns its traces (operator, 2026-10-07: "this becomes little bit confusing to
see 'measurements' and 'traces' at the transfer screen, I would imagine transfer measurement
includes set of traces and mathing should be within the measurement, as a trace"; "I would
expect new sweep to become new measurement as transfer/spl etc is, and have its traces under
its measurement"):
- **One tree** replaces the Measurements and Traces lists: each measurement with its live
  curve, captures, math channels and sweep runs, then Imported; folding, Shift+A for a whole
  group, Move to measurement… (Shift+F2), legends grouped the same way
  (`docs/design/measurement-tree.md`).
- **Ownership on the wire** (protocol 24, session format 11): `TraceEdit.owner`,
  `MathConfig.owner`; `meas.delete {traces: keep | delete}`, asked every time in the app
  ("I'd ask every time"), Keep the default.
- **Sweep is a measurement kind** (`MeasKind::Sweep`, `sweep.run` replaces `ir.capture`):
  created without playing ("it should wait yes similarly as it works with transfer now"),
  Space/Enter run the selected sweep measurement, each run a trace under it; `ac2 meas new
  sweep`, `ac2 sweep run`, `ac2 ir capture` runs the measurement with its flags' settings.
- **Math lives under the measurement it was made on** ("the math should live on selected
  measurement it was created on"), its captures too; the dialog offers that measurement's
  curves first.

What the keys act on (operator, 2026-10-07: "if I press backspace or A on selected
measurement it says something I dont know what in red box, I would expect it to remove
selected measurement with backspace and hide its measurement traces if A"):
- **A and Delete acted on stored traces only** (a red "select a stored trace first" with a
  measurement selected), **Backspace did nothing** and deleting a measurement was palette
  only → they act on the item selected last, a measurement or a stored trace (selecting a
  measurement deselects the trace; the list fills the row that has the keys and outlines
  the selected measurement while a trace has them). A on a measurement hides its live
  curves in every pane, display only (it keeps measuring; `hidden` in its list row, its
  pane's title and chip list, the IR pane's note; remembered by name in `ui.toml`).
  Delete / Backspace ask (*Delete measurement TF 2? …*) and send `meas.delete`; a math
  channel's operand says which channel to change instead. Backspace in an open window
  still edits text only; held, it never answers the confirmation it opened. Commands
  `toggle_selected` / `delete_selected` replace `toggle_trace` / `trace_delete` /
  `meas_delete`. No wire change.

Sweep level axis (operator, 2026-10-07: "sweep distortion could shift+home as everything
else by default after measurement"):
- **A new sweep result showed on the last range** (or the one remembered in `ui.toml`), often
  tens of dB off → once its data is in, the sweep pane's level axis frames its harmonics and
  THD as Shift+Home does (frequency untouched); a later zoom stays until the next result,
  and the app's start keeps the remembered range.

- **`ir_capture_on_the_simulated_rig` H3 on macOS** was not flaky: the test checked the
  *maximum* H3 over the sweep, which reads −49.03 dB on Linux every run and −48.92 on macOS
  against −50 ± 1. H3 sits near the rig's noise, so the maximum of its scattered estimate is
  biased up; the test now checks the median over 200 Hz–1.5 kHz (−50.04 against the analytic
  −50.08, ±0.5 dB) and keeps the maximum only as a loose bound.

Math channels (operator, 2026-10-06: "instead of 'some A and B' it would be nice to be able
choose trace from dropdown, then choose operator and then choose another trace … isn't it
[spatial average] just math channel with averaging?"; `docs/design/math-channels.md`):
- **A − B / A / B behind Ctrl+K were hard to use, and spectrum A − B landed on the magnitude
  pane** → one concept, the math channel: A, operator, B by name from dropdowns (live
  measurements and stored traces of one kind), or the operands of an average ticked; ÷ × + −
  of transfer functions as complex values on a stated delay reference (+ is the summation
  prediction), − and + of spectra / RTA as level difference / power sum, the average of 2 …
  16 (power / complex / coherence-weighted). Computed live in the daemon, drawn in its kind's
  pane, editable, captured with Ctrl+1 … 9 naming the expression and operands. The spatial
  average is its average (same maths, dropout rules and banners). Shift+M, palette *New
  math channel…* / *Edit the selected math channel…*; `ac2 math new / set` replaces
  `ac2 meas new avg` and `ac2 trace math`; `trace.math` and the A − B / A / B palette entries
  are gone. PROTO 22, session format 10.
From the operator (2026-10-06, "a settings page instead of gazillion configurables under
ctrl+k"; "how do you set system max output limit from UI?"):
- **Settings** (Ctrl+P, ⚙): one full-window view with pages Inputs & outputs, Audio,
  Calibration, SPL / Leq, Recording, Display, Connection; the session dialog, the
  calibrations view and the Leq dialog are pages, their keys open them; each setting says
  this app / the rig (`docs/design/settings.md`).
- **System max level at run time** (`gen.ceiling`, PROTO 23): any client lowers it at once
  (a stimulus above it stops), a raise needs a typed confirmation and silence, never above
  `--max-level`; kept by the daemon across restarts; `ac2 gen ceiling`.
- **Outputs by name**: the rig's output labels (`session.outputs`), S ticks the stimulus
  outputs in Settings; the top bar names them.
- **Server features** on the Connection page: mode, mDNS name, authorized clients (authorize
  a refused key, revoke), refused keys (`server.*`).

From the 645f5ec deploy on pupu and the operator (2026-10-06):
- **"arm+fire to re-sweep with same parameters on sweep view; drive the gen for live
  transfer on transfer view and on spectrum/spectrograph"; "New sweep measurement" in the
  palette** → the focused view decides what Space arms: the sweep view a re-sweep with the
  last sweep's parameters (the dialog when there is none), every other view the generator;
  Enter fires what is armed; the top bar names it (`Enter fires: re-sweep 3 s −50 dBFS`).
  0f9f4ac.
- **Spectrum caption over the per-bin unit in a narrow pane** (long calibration text) → the
  caption shortens (details, window, tail) before the unit leaves `dB SPL/bin`. 137b8e9.
- **STALE tags under AUDIO STOPPED** → curves and readouts say `audio stopped`
  (`Freshness::AudioStopped`), dimmed, no age. ac1cf10.
- **AUDIO STOPPED banner clipped in the SPL pane** → banner headlines drop their parts from
  the end to fit their row. b28deb4.
- **Reopen waited for the next backoff step (12–30 s) after the server returned** → a cheap
  probe (JACK socket, cpal device list) every second during waits of 4 s or more reopens at
  once when the device comes back. 2507777.
- **"G … spectrum pane, spectrograph pane both as individual panes … fullscreen
  spectrograph"** → G steps spectrum → spectrum + spectrograph → spectrograph, remembered.
  878f330.
- **"sweep pane could also G change each view by itself"** → G steps response & distortion
  → impulse response → room table (full pane, large type), remembered; IR scale on Shift+G.
  ee5d794.
- **"should impulse response also say NO REFERENCE when nothing is driving?"** → the IR pane
  carries its transfer measurement's banners and tags, and says why there is no IR.
  b3e545d, f005562.

Audio that stops (operator, 2026-10-05: "ac2-ui on ketunkolo went stale, it would be cool if
it knew how to recover by itself"; `docs/design/audio-recovery.md`):
- **A device that stops delivering was not reported or recovered** (FF400 reset, jackd hung:
  no audio, no error, only STALE; jackd stopped: one reopen, then the session closed) → no
  block for max(1 s, 20 periods) or the host ending the stream is AUDIO STOPPED
  (`session.stopped`, banner, top bar, `ac2 status`); the stream is closed off the control
  thread and the same configuration reopened with backoff (1 s … 30 s) until it opens or a
  client closes the session; same measurements in a new epoch, generator disarmed, SPL log
  gap. Fake backend stall / vanish outages; PROTO 21. Left: how libjack's client open and
  close behave against the hung server (rig only); `session.devices` still enumerates on
  the control thread.
- **Live spatial average of N transfer functions** (PLAN §3.3, phase 7; 2026-10-05): a
  measurement kind naming 2 … 16 transfer measurements, averaged in the daemon per frame
  with `trace.average`'s mathematics (power / complex / coherence-weighted, each member
  re-referred from its own delay), members left out by name and reason, no average below two;
  app dialog by name, legend and banners, `ac2 meas new avg`, captures naming their
  positions. PROTO 19, session format 9 (`docs/design/spatial-average.md`).

ISO 3382-1 room parameters from the sweep IR (2026-10-05, `docs/design/room-metrics.md`):
- EDT / T20 / T30 / C50 / C80 / D50 per octave and one-third octave and broadband, band
  filters run backwards, own trigger per band, Lundeby truncation with tail correction,
  refusals (`noise`, `short`) instead of numbers; sweep `tail` option (1…20 s); `ac2 ir
  metrics`, the table under the sweep IR view, CSV v3, PROTO 18, session format 9.

Raw capture files (2026-10-05, `docs/design/raw-capture.md`, PROTO 17):
- **Record and replay** → `rec.start/stop/list` write chosen inputs to f32 WAV (RF64 past
  4 GiB) + JSON sidecar (devices, channel roles, config timeline, every discontinuity at
  its sample) from a fan-out consumer with a 10 s queue; disk full, bounds, session close,
  shutdown and a killed daemon all leave a finished file. `session.replay` plays one as a
  capture-only device; replayed TF/spectrum/SPL match live to rounding, samples bit-exact.
  CLI `ac2 rec …`, `ac2 session replay`; app palette *Record* / *Replay a recording…* and a
  REC indicator. Left: generator output as a channel, re-applying the timeline on replay,
  fetching files over the protocol.

Spectrograph (operator, 2026-10-05: "spectrograph interests me"; PLAN §3.4, design in
`docs/design/spectrograph.md`):
- **G** in the spectrum pane: the spectrograph of the pane's measurement under the spectrum,
  on its frequency axis, colours over the pane's level range with a colour bar, cursor reads
  frequency · time ago · level; **Shift+G** 10 / 30 / 60 / 120 s. Built from the frames the
  app already receives (no wire or session change); gaps where frames stop; one GPU column
  per new frame (heatmap columns uploaded by identity), max over cells a pixel covers.

Clock drift detection (2026-10-05, `docs/design/multi-device.md`):
- **Drift above one sample per hop read as timing jumps** (83 ppm at 48 kHz: PLAN §12's
  100 ppm example gave 14 OUTPUT TIMING JUMPs in 20 s and no drift) and **a one-sample step
  read as 3 ppm of drift** → the loopback monitor judges windows against a drift line
  (`ac2-core::timing::drift`), jumps are measured against it and taken out of the slope;
  CLOCK DRIFT banner, `ac2 status` clock line, session dialog note, PROTO 16 (`Drift.at`).
  Left: callback-timestamp rate estimate without a stimulus (method B), TF delay slope for
  reference and measurement on different clocks (method C), resampler servo wander on real
  hardware.

Delay change without a full resettle; sub-sample delay (operator, 2026-10-05: "delay change
without resettle sounds useful too"; `docs/design/delay-no-resettle.md`):
- **A delay change restarted the whole MTW ladder** (~2.4 s of settling) → the reference is
  spliced to the new delay and each stage keeps its averages, rotated to it, while its held
  blocks' window correlation stays ≥ 0.995 (full rate 113 samples, 452 and 1356 on the
  decimated stages); only the stages beyond that settle again.
- **Delays were whole samples** → the fraction rotates each block's cross-spectrum (pure
  delay flat within 0.03° per bin to 0.45·fs); the finder's estimate is inserted exactly;
  `applied_samples` is fractional, `delay.nudge`, PROTO 15; Ctrl / Alt + `,` `.` move the
  measurement's delay by 1 / 0.1 sample, `ac2 delay nudge`.

Small app fixes (2026-10-05; operator: "small app fixes yes", "spectrum axis label naming
yes" — "I don't completely understand the spectrum y-axis, why it goes below 0 …"):
- **The spectrum's level axis did not say its levels are per bin** → `dB SPL per 1.46 Hz bin
  (tone, 1/3 oct smoothed)` (the bin spacing fs/N from the frame's grid, not the window's
  ENBW), shortened to fit narrow panes; its tooltip and the user guide say broadband reads
  lower with finer bins and point to the RTA. No wire change.
- **No export of the selected trace from the app** → palette *Export the selected trace (ac2
  CSV) to a file…* (a file or a folder; starts in the last export's folder).
- **Level axis ranges forgotten when the app quits** → `[levels]` in `ui.toml` (written at
  most once a second); a started spectrum still fits on its first frame.
- **The spectrum pane had no legend** → a legend in rows above the plot (name, colour,
  `stopped` / `STALE` / `offset`), shortened to `+N more` on small panes.
- **A − B took the two lowest shown slots** → the selected trace minus the next shown one it
  combines with (slots when nothing is selected).
- **The transfer legend did not mark the selected trace** → a bar and a thicker swatch on its
  row, a line twice as wide (spectrum pane too).
- **egui focus wandered to the sidebar's measurement chip** (Tab / arrows, any dialog) → the
  app clears egui's keyboard focus every pass; the reducer owns the keys and each dialog its
  own focus (tested for the Leq dialog, palette, prompt and help).

Laptop / Pi performance pass (2026-10-04/05; a0d015d … 9dc4274, PLAN §9.0 has the numbers):
- **Autosave rewrote the whole SPL log every minute** (≈15 MB/min at 48 h retention) → the
  log is appended to its own file, traces written only when changed (c8406c7); resuming on
  Windows fixed (ed189b2).
- **Spectrum frames carried all 32769 bins** → display-sized log columns, positional
  headers, PROTO 14 (b5a3622 … d392ef1).
- **Idle wakeups and work for nobody** → batched hand-off, meters computed once, jobs emit
  only new and wanted results, per-topic UI subscriptions, repaint only on visible change,
  low-power GPU request; SPL / RTA / delay-tracking DSP cheaper; timing monitor idles
  without a stimulus (b97a2c9).
- **macOS publish rates at 8–15 Hz** (coalesced timers) → uncoalesced hand-off timer and
  fixed-grid cadence pacing (9dc4274). CI green on Linux, macOS and Windows again
  (c5484e3, ed189b2, 9dc4274) after being red since at least 2026-10-03.

Leq windows and limits, the rest of the first version (operator: "Leq changes yes";
`leq.md`, `q7-calibration.md` §12; PROTO 20, session format 9, SPL log CSV v2):
- **Peak limits not judged** → LCpeak and LAFmax limits per meter (DIN 15905-5 LCpeak 135
  dB, V-NISSG LAFmax 125 dB set by their presets; `--peak-limit`, the Leq dialog), judged on
  the highest second of the last 10 s; columns / tiles of their own, alarms, `peaks` in
  `ac2 spl leq watch --json`, `lcpeak_1s` / `lafmax_1s` in the log.
- **No measuring-position correction** → `SplConfig.position` (energy and peak, as DIN's K1 /
  K2), added to everything the meter reports once calibrated and said everywhere
  ("corrected +4.0 dB", `dB(A) corr.`, alarms); the log keeps what was measured and records
  the correction per second.
- **No hysteresis on the alarms** → a state rises at once and drops only 0.3 dB under its
  boundary or after 10 s under it (`leq.md`, *Hysteresis*).
- **No acoustic calibration in the app** → **C** in the Calibrations view: the calibrator
  dialog beside the electrical one.

Leq view for acting live (field, 2026-10-04/05: "the individual slot db values dont mean much
and those competing on actual SPL number … is bit difficult"; b251aaf … c009b6b):
- the state and "stay ≤ … dB" lead each window, large; the window's value is small, held
  still low in its bar, coloured against what is behind it; the run line centred over the
  meter (own row when the calibration text leaves no centred room); values one size in
  every window, no-limit windows included.

SPL meter and the stage view (field, 2026-10-03/04; 1f7337a, f2593e7, 0921351, 63f9bd1,
ff82db7, c98d8b4, a8c8cf9, 5a157e4):
- **Weightings chosen only when a meter was created** → F / S / I and A / C / Z switched in place (the
  meter runs every combination, so a switch reads settled at once), a held big number
  centred in the pane, statistics headed "meter since … · R resets"; a new meter reads LAF.
- **Meter or windows, not both** → a third view, meter + Leq (the default): the number over
  the windows, one caption; each window names its own unit and weighting (`dB(A)`), since
  the meter above may use another.
- **"gaps: 4:57 of 5:00 measured" read as a puzzle** → "offline for 3 s" on tiles and
  columns, "offline 12 s" in the run caption (13ad620); the unit sits on the value's line,
  small and dim (c98d8b4).
- **A fresh log put every window over at once** → a filling window is judged on its energy
  budget (`ON COURSE — over in 12 min`, "so far · 12:30 / 30:00").
- **"can't recover within …" / "at the limit: back under in …" read badly** → "next 1 min:
  stay ≤ …" and "cooling down in …".
- **Layout** → W cycles split → one pane → full screen (the stage view on any pane), F11 the
  window; the layout is remembered in `ui.toml`.

Keyboard (field, 2026-10-03/04; 548e2c8, 8d15d70, 79fed56):
- **`/` for help is Shift+7 on Nordic layouts** → H (and F1); the panes' H keys moved
  (Shift+I IR, P peak hold, Shift+B history, Shift+W hide sweep). Per-pane key hints
  (Shift+H) and tooltips naming keys.
- **Esc in a window also stopped the noise** → an open window owns the keyboard; Esc closes
  it only; Shift+Esc stops from anywhere (decision K9).
- **S on the transfer pane stopped an SPL meter selected in the list** → S and R act on the
  focused pane's own measurement.

Stopped measurements and the spectrum's level axis (field, 2026-10-05: "if I start transfer
measurement or spectrum and then stop it, then it says STALE … 23s no fresh data"; "make
spectrum to start shift+home-position when it begins"):
- **A stopped measurement read STALE** → its last frame is its final result: no STALE banner
  and none of its last protection flags, the curve not dimmed, tagged `stopped` (transfer
  legend, spectrum caption; SPL readout and Leq windows say STOPPED). STALE stays for
  running measurements whose frames stop.
- **The spectrum pane opened on −100 … 0 dBFS** → each start of a spectrum / RTA fits the
  level axis on its first frame, as Shift+Home (frequency left as it is).

Smaller fixes from the field (2026-10-03/04): CHECK ROUTING flashing beside NO REFERENCE
(e5e4161); a calibrated spectrum drawn above a dBFS-sized axis (ad3cfae: one level range per
scale); renaming a stored trace (a14e1af: F2, double click, `ac2 trace rename`).

Leq windows (field, 2026-10-04: "history seems to reset every time the client is
restarted"; "presets … only show those limits that are stated by the standard"; 247346a,
ef9dcc6):
- **History strip started when the app connected** → the app gets each meter's history
  from the daemon (`spl.history_get`: the log replayed as the job computed it) on connect,
  reconnect, a new meter, changed windows and a new log from any client; live frames
  continue it (`leq.md`, *The history strip*).
- **Presets kept whatever windows were there** → a preset replaces the windows with exactly
  its own (`leq.md`, *Presets*); Insert / `--windows` add more.

Comparing curves (field, 2026-10-03: "offset/change gain of the selected
trace/measurement/spectrum … spread traces a little … spectrum needs to focus on very low
signals"; "remove the selected trace by selecting it and pressing Delete"; "in show-one-pane
mode I would expect the panel to change if I click a different measurement"; a79fbf4):
- **No quick offset** → **Alt+↑/↓** ±1 dB, **Alt+Shift+↑/↓** ±3 dB, **Alt+Home** 0 dB on the
  selected stored trace (any kind; `trace.update`, recorded with the trace) or the focused
  pane's live measurement (display only); **J** types it in the spectrum pane too. The plot
  names every offset curve (transfer legend tag, spectrum note and cursor).
- **Fixed level axes** → per pane: **Ctrl+I/O** zoom, **Ctrl+↑/↓** pan, **Shift+Home** fits
  the shown curves (low-percentile floor), **Ctrl+Home** resets; Ctrl+wheel / Shift+wheel
  with the mouse. `ac2_scene::view::level`.
- **No delete in the app** → **Delete** (palette: Delete selected trace…) asks, Delete or
  Enter deletes (`trace.delete`), the selection moves to the next shown trace; locked traces
  refuse; never a live measurement.
- **Maximised pane ignored the list** → picking a measurement brings up the pane that shows
  it (and focuses it in the split layout); a trace picked while maximised brings up its pane.
  No wire change.

Choosing between stored traces (field, 2026-10-03: "in transfer view where there are several
sweep traces, should I be able to choose between them?"; 80614e0):
- **Only slots 1–9 could be selected or hidden** → the sidebar's **Traces** list holds every
  stored trace (name, kind, slot, hidden, its curve's colour; a click selects, a click on
  the dot shows / hides). **V** / **Shift+V** step through every shown trace, **Alt+V** /
  **Alt+Shift+V** the hidden ones too, **A** shows / hides the selected one, **Move the
  selected trace to slot…** slots it. U, J, `,` `.`, E, K and the mic curve act on any
  selected trace on the transfer pane (they used to act on the live measurement only).
- **The sweep pane's choice was its own** → one selection: a sweep selected in the transfer
  pane is what the sweep pane shows, and N there selects. A finished sweep is selected.
- CLI: `ac2 trace display <t> on|off`, `ac2 trace slot <t> <1-9|none>` (`trace.update`; no
  wire change).

Leq run clock and a new log (operator: "should I see a timer somewhere which shows the whole
measurement time?"; `docs/design/leq.md` "Run clock and total", "A new log"; 774e690):
- **No timer for the whole measurement** → every Leq caption (columns, tiles, stage view) and
  `ac2 spl leq watch` (`run` in `--json`) show `running 2:14:05 since 19:02 · LAeq total
  97.8 · gaps 0:12`: the clock from the log's first kept second (it carries on across app
  and daemon restarts with the log), the energy average over the whole log's measured time,
  the unmeasured time; "last 48 h" once the log is at its retention. Shortened, then moved
  to a row of its own, at narrow widths. Protocol 10: `LeqMeta.run`.
- **No way to start the windows afresh** (show start after a loud soundcheck) → `spl.log_new`:
  the windows, their states, the alarms, the clock and the total start over, windows and
  limits stay; the ended log stays readable as the previous log (`spl.log_get` with `log:
  previous`, `ac2 spl leq export --previous`) until the next new log or a daemon restart.
  App: Shift+R in the SPL pane or "Start a new SPL log…", after a confirmation naming the
  run that ends; CLI: `ac2 spl leq new --yes [--export FILE]`.

Calibration visibility (field, 2026-10-03; `docs/design/q7-calibration.md` §10; de17a13):
- **No calibration view in the app** → the Calibrations view (palette **Calibrations…**,
  **Input setup…** on the selected measurement's input): what each input uses (mic, curve,
  sensitivity with calibrator and age), every mic with its curves, every sensitivity
  calibration; ←/→ choose an input's curve, N names the mic, I imports a curve, R renames,
  Delete (twice) deletes. The session dialog's mic rows say the same and ←/→ choose there too.
- **"mic curve: on" with no curve stored was misleading** → there is no on/off any more: an
  input chooses one of its mic's curves (or off), and says why none applies — `no curve
  stored for MM1 34804`, `choose: 0°, 90°`, `90° — not stored for MM1 34804` — in the input
  setup, `ac2 cal list` / `ac2 status`, the sidebar label and the pane captions.
- Several curves per mic (MM1 0° / 90°), labels from the files, the curve in use recorded
  with captured traces (label + hash). Protocol 8, calibration store version 2 (version 1 is
  set aside), session format 6.
- Rig note kept: the MM1 90° curve was only imported on pupu on 2026-10-03 (after all of the
  day's sweeps); earlier ac2 traces are uncorrected (`ac2 trace mic` fixes them). After the
  update the store is set aside: re-import both MM1 files and calibrate again.

Flaky tests (seen on CI and under load, 2026-10-03), by cause (db95852, d8fc428, 927c69c):
- **A capture could return the result before the one a client was shown** (ac2d
  `traces::spectrum_smoothing_live_and_captured` "bin 1: live … vs ac2-core …",
  `live_smoothing_and_resmoothed_captures` "301 Hz: … vs live …",
  `capture_average_math_export_import` NaN average, `session_save_load_round_trip` "no result
  to capture yet"; Windows CI). A job sent its frame to the I/O thread before storing it as the
  capture slot, so a client that had already received the frame could capture the previous
  result, or none. The slot is now filled first; a 20 ms pause in the old order reproduced
  the CI messages exactly.
- **Full-band pink over a 200–300 ms meter span** (ac2d `stimulus::lease_acquire_force_expiry_
  and_universal_stop` "pink at -20 dBFS RMS on the loopback: -21.5"). Content down to 5 Hz
  gives one or two cycles per span: +2.7/−1.6 dB from seed to seed, ~1 % beyond ±1.5 dB, and
  the daemon seeds each generator afresh. The test plays pink high-passed at 50 Hz (±0.75 dB
  over 3000 seeds).
- **UI tests under load** (load 25–50 on 12 cores, lavapipe): `startup_first_frame` timed
  shader compilation on a software rasteriser (3–4 s loaded); it now bounds the app's own
  startup (App::new through its first UI pass, < 1 s; 1–65 ms measured loaded). Snapshots
  caught wall-clock states ("not responding", STALE, a countdown second, a measurement created
  but not yet started); they are taken from a pass laid out while the link was healthy, the
  progress picture pins its clock, the transfer dialog waits for "running". A test process
  starved past the fake daemon's 1.5 s lost its stimulus lease (fake `expiries` 1, 0–10
  refreshes) and the flow stalled: the UI tests' fake grants 60 s. A sweep submitted while the
  previous stop was in flight was dropped by the app (real bug, fixed: it arms after the
  stop). Waits are condition-based with a 60 s ceiling and report state on timeout; the meters
  round test freezes the reducer's clock instead of a 700 ms bound.
- Evidence: ui + embedded suites 15× under 36 CPU burners (load 45–48): 0 failures (before:
  5/12 and 3/12); ui, embedded, connect, conn and ac2d traces + stimulus 8× beside full
  `cargo test --workspace` runs (load 16–62): 0 failures; full workspace 3×: all pass; ac2d traces + stimulus 20× under
  36 burners: 0 failures.

Autosave (561d944):
- **Sweep results lost on a daemon restart** (a `--max-level` change needs one): a stand-alone
  daemon autosaves measurements and traces, sweep distortion and impulse responses included,
  and restores them disarmed on start; the top bar shows the autosave state.

Trace features (protocol 7, session format 5; f3ea323):
- **Mic curve on stored traces**: `trace.mic_curve` / `ac2 trace mic <t> <mic|none>` / palette
  **Mic curve on the selected trace…** applies the store's curve for a mic to a captured trace
  as a display edit (columns as measured, points kept with the trace, exports name it); a
  capture that has the curve in its columns refuses a second (`q7-calibration.md` §9). Sweep
  traces no longer claim the input's curve in `mic.curve` (their analysis never applied it).
- **Trace smoothing from the CLI**: `ac2 trace smooth <t> 1/12 | none [--phase |
  --magnitude-only]`.
- **A sweep CSV re-import brings the sweep back**: the export (v2) carries `# sweep_info:` and
  the decimated IR as a second table; `trace import` restores a sweep trace the distortion pane
  draws. Imports keep the export's `delay_ms` (all kinds; it was 0.00 ms). A v1 sweep export
  imports as its transfer function with a note (`sweep_without_analysis`). Sessions keep the
  sweep in its one CSV (no `.sweep.json` sidecar).

- **Top bar: ARMED badge overlapped the session text** (076320e) ("2ARMED…") at 1290 px. The bar is now
  fitted before it is drawn: lower-priority texts (key help, next-key hint, autosave, the
  device name) shorten or go first, the state badge and level always stay; `top_bar_never_overlaps`
  checks 640–1600 px.

From the first rig session on pupu (2026-10-03):
- **"No audio session" hint covers stored traces** (927c69c): while the transfer pane draws a
  stored curve the hint is one line in the pane's title strip; over an empty plot it stays
  centred.
- **Finder said "AMBIGUOUS · merged arrivals" with a single candidate** (b01bd87). The ambiguity is real.
  A crossover inside the full band (LR4 sum = allpass) smears the one peak, so its centre is
  more than 1 sample after the onset. Two arrivals inside one pulse width do the same. The finder
  lists the peak and never invents a second arrival. The text now says "arrivals merged into one
  peak", explains the single row, offers only pick 1, and points to another band (q1 §8).
- **Clearer refusal messages** (2565cfd, edf8492). An unpaired client says "not paired with <host>: … run `ac2
  auth pair <host> --server-key …`" (CLI and app). "Not responding" from a CURVE client adds
  "or this client is not authorized on it (this client's fingerprint: …)" and where to
  authorize it. ac2d logs each refused key's fingerprint, key and address, once per key and
  address per 10 s, with the count it suppressed.

Sweep findings from pupu (2026-10-03; 2aa6867, 61e7ea5):
- **Generator re-armed after a sweep** (every run). Not a restore of the pre-sweep state:
  `ir.capture` needs an armed generator, and once the recording was in the daemon only
  cleared `firing` ("generator Set by daemon"), leaving it armed with the sweep; the app
  followed with ARMED and "Enter sweeps again". The daemon now disarms (`Stop` by daemon,
  lease kept) as soon as the sweep has played, and on failure; the app shows STIM OFF, gives
  the lease back once the result (or failure) is in, and ends sweep mode.
- **Sweep dialog steppers** sorted (1 s, 3 s default, 6 s, 12 s) and every dialog stepper
  (choices, inputs, the session dialog's backend and device) stops at the ends.
- **Text fields select their text on focus** (and Ctrl+A), so typing replaces the default
  name or level.
- **Sweep reference** is the loopback input, else "choose the reference": the sweep does not
  arm until one is picked.
- **"output timing jump 0 → 96000 samples"**: not the output record. At 96 kHz the
  acquisition range is 0 … 96 000 samples; a slow sweep's first seconds are a near-tone, and
  GCC-PHAT on a rectangular capture window turned the window's end leakage into confident
  peaks exactly at the range ends (0 and 96 000, also ±64 while tracking). The capture window
  is now tapered: those windows are no measurement (Lost) instead of a false jump.
  Reproduced in `ac2-core` (`a_slow_sweep_start_never_reads_as_a_range_edge`).

Sweep / distortion pane (from the first sweep run on pupu; a9ad7cb):
- **Caption overlapped the axis title** in a small pane: the caption now shortens to fit
  (CLIPPED always kept), the legend wraps and the cursor readout starts below it; tested at
  300 … 1290 px.
- **Only the H2 floor was shaded**: each order within the noise is drawn dashed at its own
  floor in its colour, the shading is under the lowest order's floor (legend `< floor`,
  `noise`). Lone valid points are drawn across their cell, no longer as dots.
- **dB / % had no visible control**: a `dB | %` toggle in the pane's title (tooltip names U);
  percent is a log axis (0.001 … 100 %, decade labels).

Rig findings (19ce861, 27bef5b, cd7d8af):
- **Remote generator played silence on JACK.** Arming reopened the stream to change the
  generator routing, which closed and re-created the JACK client: every connection to
  `ac2:out_N` (the recorder, the hand patch to the interface) was gone while the state said
  FIRING. Routing now changes inside the running stream. The lease gate also never mutes a
  source that has not played, and a mute on an expired deadline disarms with an `expiry`
  event.
- **JACK outputs never connected by the daemon.** The outputs chosen for the stimulus (and the
  loopback output) are connected to the playback ports of the same number, and again after a
  reopen; nothing else is connected or disconnected.
- **`ac2 spl watch` read the wrong level.** `--input` reused any running SPL meter with the same
  settings, typically one a killed watch had left behind, whose Leq integrated since long
  before the tone. The watch now always runs its own meter, cleans it up on SIGTERM/SIGHUP too,
  and prints the input and meter id; `--for` ends it after a set time.
- **Numeric measurement names** resolve: id first, then name; a value that is one measurement's
  id and another's name is refused naming both.
- **Daemon network ports vs host firewall.** `ac2d --listen` names its ports and warns when ufw
  or firewalld is active (one rule per port); "not responding" from a remote client hints at
  the firewall.
- **The daemon never answered mDNS.** `Handle::wait` dropped the advert before blocking, so the
  responder said goodbye and shut down right after "advertising". The advert now lives until the
  daemon stops; responder errors are logged.
- **`ac2 discover` asked on some interfaces only.** The browser now also asks on every interface
  address itself (0, 1, 3 s) and lists where it asked when nothing answered.
