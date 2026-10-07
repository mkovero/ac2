# Backlog

Work found in use, not yet scheduled into a phase. Newest first. Move an item to
`backlog-done.md` with the commit when it lands.

## Queued after the module split of state.rs / control.rs (operator, 2026-10-07)

- **A typed delay (`D`) should move the live curve like a Ctrl+. step** (0e04f25 counts it as a
  new arrival, so nothing moves): operator answered "yes". Rounding a capture's fractional nudge
  to the 0.1 ms grid with `,` / `.` is fine as is.

## Timing precision (operator, 2026-10-07, after the REW cross-check)

- **Sub-sample sweep arrival, fractional tracking, A − B arrival readout, and a group delay
  that holds below 100 Hz** (the displayed one swings ±30 % there on correct phase):
  `subsample-arrival-group-delay.md`.

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
- **Frame size on slow links**: a remote `tf` frame is ≈ 7.8 KB, ≈ 1.9 Mbit/s per
  measurement at 30 fps. Compacting `validity` saves ≈ 25 % losslessly; compression and
  display quantisation weighed in `docs/design/wire-size.md`. Not needed until a field
  report shows bandwidth, not latency, limiting a remote client.
- **Delay finder full searches run on the transfer job's thread**: every 2 s while tracking,
  about 33 ms on a desktop core, likely 150–250 ms on a Pi; `detect_period` is ~60 % of it.
- **A weighting could reuse the C filter** (A = C + one more section): two biquads per
  sample saved on the SPL path.
- **Pi 4 class: a real Pi 4 B runs the kiosk** (2026-10-07; first built and tested in
  emulation on 2026-10-05, c59ef65). A Pi 4 B netbooted from ai shows pupu's SPL meter
  (`ac2-ui --remote` under cage, rendering on V3D 4.2 to a USB DisplayLink monitor, ~0 % CPU
  idle; a touch steps the SPL view). Not yet: a USB interface on it, frame time (§8.3), NEON
  speed, a measured CPU budget. The emulation notes: a Raspberry Pi
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

- **ac2-ui `measurement_tree_and_delete_choices` on macOS CI**: the `measurement_delete_choices`
  snapshot (the delete dialog over live transfer measurements) differed by 14 573 px once
  (a680dcd's run, a change to ac2d only); passed on rerun. The live curves behind the dialog
  are not pinned.

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

Landed items: `backlog-done.md`.
