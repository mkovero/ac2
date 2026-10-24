# Backlog

Work found in use, not yet scheduled into a phase. Newest first. When an item lands, move it
to `backlog-done.md` as one line with the commit.

## Many rigs in one app (design `multi-rig.md`)

- **Not started.** Stages: (1) `AppState` split + id namespacing by rig, top-bar rig drop-down,
  slots carrying their rig, `PaneSource {RigRef, PaneContent}` in `Measurement.layout`
  (PROTO + session bump), P1 storage rule, arm line names the rig, Shift+Escape stops every
  rig; (2) clock reporting, other-rig stimulus chips; (3) remote inputs, only after a
  backend-timestamp and input-latency study on pupu + ketunkolo.

## Programme metering (design `programme-meter.md`)

- **Stages 1 (7823115d) and 2 landed.** Stage 2: PPM ballistics against the IEC 60268-10
  burst table (rows and ac2's acceptance in the note §3; the standard's own tolerances still
  unread), true peak, overs, per-side gated correlation, ember goniometer, Programme pane,
  rig-level `levels.set` (Settings › Rig monitor and `ac2 levels`). Its deviations from the
  note are in §8. **Stage 3 (loudness) open.**
- **Programme pane polish.** No drag of an Inputs row onto a pane yet (P and the pane's list
  open one); the goniometer frames arrive with the session's input meters whether or not a
  Programme pane shows them (`session/` prefix), a separate subscription would spare the
  bandwidth on a rig with many pairs; *both quiet* never shows (the wire names one quiet
  side, the left first).

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
- **Audio continuity is not in `ac2 status`**: the xrun count since the session opened, the
  callback load and host CPU reach clients in the keepalive and the app's top bar, but the
  CLI prints none of them, and capture discontinuities are still only logged (`capture
  discontinuity at sample …`). The keepalive's health plus a discontinuity count (and the
  last one's time) in `status` / `--json` would make the 24 h check (`docs/rigs/pupu.md`) one
  command. Memory over time is likewise only a `/proc` sample.
- **Core Audio backend** (`coreaudio.md`, decision H1): stages (a)–(c) built: `coreaudio`
  is the only macOS backend (cpal removed), exact on BlackHole CI (§12, *Settled on CI*),
  Audio MIDI Setup behind P, only the used input streams on. Not yet verified with an audio
  interface. Next: the tester round on the next build (`testing/macos/README.md`, Tests 1–5
  and Test 6: stated vs measured latency, replug object id, rate change, stream usage on a
  multi-stream interface).
- **Per-cycle out − in check against the stated latency in `coreaudio_host`**: a VM overload
  put one impulse 256 frames early; that block was flagged `XRUN | DISCONTINUITY`
  (`coreaudio.md` §12, *Settled on CI*). Today only `coreaudio-loop` sets such round trips
  apart; the engine should check each cycle's output placement against the stated latency
  and flag a cycle that differs.
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
  PATH fine; the app ran the simulated rig.
- **ASIO on real hardware**: RME's driver with a Babyface Pro in a Windows 11 VM (USB host
  passthrough) passes the 1 h silent duplex and loopback timing (`asio.md` §4c). Open: the
  driver's control panel during a run, unplugging the device, another program holding the
  driver, and a tester on a real PC (§4d, `testing/windows/README.md`, dev.13 MSI).
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
- **ac2-ui `embedded::calibration::mic_curves_imported_and_switched_in_the_input_setup`**:
  failed once on its first try in a full workspace run (passed on retry); 10/10 alone.
- **macOS CI "ac2 on BlackHole (Core Audio)"**: `selftest duplex` failed once with
  `config_changed` count 1 (run 39364032668, 1ee6a366); BlackHole reported a configuration
  change mid-run, the levels were exact. Seen once. Also once: "delay finder read −481.0
  samples, expected 480 ± 1" (run 39468724964, 3310ea85; passed on rerun): the right size with
  the opposite sign, as if reference and measurement swapped for that run. Two oddities on
  BlackHole in one day: watch for a third.
- **ac2d `osc::slow_a_tone_over_the_limit_lights_over_and_back_ok` on macOS CI**: the
  `/w/ok` cue carried 87.0004 against the test's `< 87.0` (run 39383093276); the ok edge
  sits at the limit's hysteresis, so the bound or the edge's comparison is off by rounding.
  Seen once.
- **ac2-cli `json::trace::trace_commands_json` on Windows CI**: `session save-as` exited 3,
  "daemon did not answer `file.save` (3 attempts)" (run 39387937301, c7ba3b04); passed on
  rerun. Seen once.
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
- **Per-device latency offset** (audit 2026-10-20 F4e): a user-entered latency per device for
  a two-device rig without a loopback reference. Parked: the loopback measures that latency,
  and an external reference on the input device needs none; the PA rigs here use loopback.
