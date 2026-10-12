# Backlog

Work found in use, not yet scheduled into a phase. Newest first. When an item lands, move it
to `backlog-done.md` as one line with the commit.

## Band Leq (design `band-leq.md`)

- **The daemon drops a frame that fails to encode without a word.** With 22 band windows the
  `band_leq` frame meta passed the old 1 KiB header cap and no band frame arrived, with
  nothing in the log (cap now 8 KiB, `MAX_HEADER_BYTES`). Log once per stream and job when an
  encode or send fails.
- **Predicted window only from a preset**: its length and day/night LAeq limits are not
  settable in the dialog or `spl bands set`.
- **No per-band warn margin beyond the window's own**; a range added with "+ range…" copies
  one margin to every band.
- **+ / − are mouse-only in the dialog**; keys are Insert / Shift+Insert / Delete. Arrows in
  the Band column shift a little with the label width ("31.5 Hz" vs "20 Hz"), as in the Leq rows.
- **Transfer step**: per-band numbers are a text wall (a table would read better); typing an
  estimated attenuation is CLI-only (`ac2 spl bands estimate`).
- **Band history view** (per-band Leq over time from the band log); recorder file-time marks
  for transfer spans; spans of a fast-pace replay.
- **`ac2-core` comments still say "dwelling"** (`band_leq.rs`; never shown to users, the UI
  names the operator's place).

## Protocol tidy-up

- **`EssSpec.fade_in` is mostly dead on the wire.** The sweep starts below the asked band at
  a rising level, and that extension is the fade-in; the field may still act only where no
  lower start fits. Drop it from `EssSpec` or state where it applies, in the same
  `PROTO_VERSION` bump as the next sweep wire change.

## Performance and platforms

Targets: laptops with integrated GPUs on battery, and a Pi 4 class daemon for SPL (PLAN
§12 decision 4). Measured numbers: PLAN §9.0.

- **ac2d holds 666 MB with four transfer measurements** (54 MB with SPL + spectrum only;
  phase 1 run on pupu, `docs/rigs/pupu.md`). Stable over the hour, so not a leak, but large
  for a Pi 4 class daemon: account for the MTW ladders' buffers (and the 65536-point
  spectrum) per job.
- **Audio continuity is not in `ac2 status`**: xruns and capture discontinuities are only
  logged (`capture discontinuity at sample …`). Counters since the session opened (and the
  last one's time) in `status` / `--json` would make the 24 h check (`docs/rigs/pupu.md`) one
  command. Memory over time is likewise only a `/proc` sample.
- **macOS not verified with an audio interface**: a tester has the dev.10 disk image and
  `testing/macos/README.md`. Results pending.
- **Core Audio backend** (`coreaudio.md`, decision H1): build `cahal` and
  `ac2-audio/src/coreaudio_host/` beside cpal with the BlackHole exact-index loopback check
  (stage a), make it the macOS default (b), then remove cpal and `BackendKind::Cpal` (c).
- **App Nap**: the macOS hand-off timer is not coalesced (kqueue `NOTE_CRITICAL`), but a
  hidden app on battery may still be App Napped, throttling a daemon the app hosts. Needs an
  activity assertion from the UI while a session is open.
- **The UI redraws the whole window per data frame**: under a software renderer, 4 live
  measurements cost ≈ 1.7 cores, SPL only ≈ 1.3 (egui has no damage regions). Not measured
  on a real laptop iGPU (PLAN §8.3 frame-time target).
- **No flow control to slow clients**: a Wi-Fi client can lag up to the 48-frame send queue
  (`DATA_SNDHWM`); client-paced credit is designed in `docs/design/flow-control.md`, not
  implemented (wire change).
- **Frame size on slow links**: frames are packed (`docs/design/wire-slim.md`): a `tf` frame
  ≈ 2.4 KB, a heavy tuning session ≈ 3.2 Mbit/s at 60 fps. Further cuts listed there.
- **Delay finder full searches run on the transfer job's thread**: every 2 s while tracking,
  about 33 ms on a desktop core, likely 150–250 ms on a Pi; `detect_period` is ~60 % of it.
- **A weighting could reuse the C filter** (A = C + one more section): two biquads per
  sample saved on the SPL path.
- **Pi 4 B**: runs the kiosk (`ac2-ui --remote` under cage on V3D, ~0 % CPU idle; image built
  outside this repo). The aarch64 suites pass under qemu-user. Open, needs the board: a USB
  interface on it, NEON speed of the RTA's 4-band groups and the f32 mic-curve FIR, frame
  time (§8.3), SD-card writes, a measured CPU budget; then an HW gate in PLAN.
- **`ac2-zmq`'s test binary does not link with GNU ld**: `-lsodium` comes before the static
  `libzmq.a` on its link line, so ld drops it (`undefined reference to sodium_init`). Hidden
  on x86_64 Linux (rustc links with rust-lld); seen cross-building for aarch64 and natively on
  a Pi. `crates/ac2-zmq/build.rs` should emit the libraries in dependency order (zmq, then
  sodium); `-fuse-ld=lld` works around it.

## Electrical calibration (`q7-calibration.md` §11)

- **Stored traces do not record the method**: `CalState::Calibrated` names the key and the
  sensitivity, not whether it was electrical or its uncertainty; trace captions say `cal`
  either way. Adding it is a session-format change.
- **Clipping refusal is not exercised end to end**: the fake rig's generator ceiling keeps
  the input below full scale; the clip tracking is covered only by reading.
- **No guided in-line tone**: the operator starts the 1 kHz tone (own source or the
  generator); the dialog could offer to arm a 1 kHz sine under the usual ceiling.

## Windows (2026-10-03, operator's Windows 11 VM, release build of 6c122d5)

- MSI installed cleanly (unsigned: SmartScreen "More info → Run anyway"), Start-menu entry and
  PATH fine; the app ran the simulated rig. Real audio on Windows (now ASIO only) is still untested;
  `testing/windows/README.md` (dev.10 MSI) is the tester guide for it.
- ~~**No stimulus on a WASAPI capture endpoint**~~ — done: Settings › Audio has an
  **Output** device row (default: the input device when it has outputs, else the system
  default output), `ac2 session open --out-device`, and loopback detection plays on the
  output device. The tester guide now chooses the playback endpoint there.
- ~~**Sluggish without a GPU**~~ — done: a CPU adapter (type `Cpu`, or WARP / llvmpipe /
  lavapipe / SwiftShader by name; `ac2_plot::adapter::classify`) paces data and animation
  frames to 30 fps, drops egui's popup/window shadows (no MSAA or blur exists to skip) and
  toasts "software rendering: reduced frame rate" once. A dense 1080p frame is ≈ 20 ms on
  llvmpipe (prepare_bench), so the cap halves the drawing from saturated to ≈ 60 %.

## Flaky tests

- **ac2-ui `measurement_tree_and_delete_choices` on macOS CI**: the `measurement_delete_choices`
  snapshot differed by 14 573 px once (a680dcd); passed on rerun. The live curves behind the
  dialog are not pinned.
- **ac2d `remote_stimulus` "timed out waiting for the tone"**: once in a full workspace run
  at load ~30; passed 4/4 alone.
- **ac2d `traces::capture::capture_average_math_export_import`, unaligned capture at 301.6 Hz:
  −6.37 dB (want −6.02 ± 0.3)**, once, at load ~40. Unloaded −0.002 ± 0.028 dB (300 runs,
  max 0.10); 200 runs at load 40–130 did not repeat it; no capture-discontinuity warning.
  Next: log `eff_avg` and the block count the low-frequency stage averaged when the band
  check fails.

## Parked

- **Genelec 1083 on pupu: 2–6 kHz dip of −5…−9.5 dB** (`docs/rigs/pupu.md`). Same within
  ±1.2 dB at 102 and 94 cm, no distortion above 0.15 % with it, confirmed in the direct sound
  by an independent JACK + numpy capture: most likely woofer–tweeter interference off the
  summing axis. Left: heights through the reference axis, 15–30° off axis, rear-panel
  settings, listening position;
  `ac2 ir capture --ref 2 --mic 1 --out 1,2 --level -50dbfs --duration 6s --repeats 2`.
