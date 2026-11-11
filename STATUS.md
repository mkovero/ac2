# ac2 — status and where to look

Short orientation; read this instead of the whole of `PLAN.md`. Versions live in the code
(`PROTO_VERSION` in `crates/ac2-proto/src/lib.rs`), not here; what landed when is in
`docs/design/backlog-done.md`.

## Where things stand
- Phases 0–6 (1.0 scope) pass their CI criteria; phase 7 (post-1.0 extras) is in progress.
  Per-phase table with CI and hardware results: `PLAN.md` §9.0.
- Hardware verified on Linux: the `pupu` rig (`docs/rigs/pupu.md`), a Pi 4 kiosk client.
  Windows and macOS: tester guides and builds in `testing/windows/`, `testing/macos/`.
- CI on push is Linux only; macOS/Windows run before a release or on dispatch
  (`CLAUDE.md` *Rules*). Rigs run locally built binaries: `docs/rigs/pupu.md` *Deploy log*.
  A session format change on deploy is restored with `ac2 session salvage`.
- Programme metering on the console feed, stages 1–5 landed: input kinds mic / line /
  digital, stereo pairs (mic pairs too: true peak and correlation, no loudness), line
  calibration in dBu (`cal.line`, `session.inputs`); PPM ballistics, true peak, overs, gated
  correlation, ember goniometer (decay 0.1–2 s), `levels.set` / `levels.reset`; BS.1770-5 /
  R128 loudness per line/digital source (EBU Tech 3341/3342 vectors) and `ac2 levels`; LUFS
  bars and the loudness plate. A Programme pane shows **Meters** (true-peak block, LUFS block,
  correlation strip) or **Goniometer** (square, fills the pane), G steps them, a pair opens
  both panes (PROTO 71, session format 35, rig file 7). A session opens without a reference
  or stimulus (SPL-only rigs). Design: `docs/design/programme-meter.md`, V1–V6, §6.1–§6.3.
- Spare inputs on JACK: extra `ac2:in_K` ports after the physical inputs, never
  auto-connected, metered like any input once the operator routes a source into them; the
  daemon re-makes those routes across its own reopens. Docs: `docs/user-guide.md`
  *Metering other JACK sources*.
- The app paces its own frames (`crates/ac2-ui/src/pace.rs`, no vblank wait), so a blanked
  or absent display no longer drops it to 1 fps (verified on ketunkolo; Pi GLES unverified).
- The app hosts no daemon: it starts the `ac2d` beside it (stopped when it quits, Settings ›
  Connection), one `ac2d` per user by a lock, the simulated rig a spawned
  `ac2d --backend fake --ephemeral`; e2e tests with a daemon are the `ac2-e2e` crate. Design:
  `docs/design/ui-spawns-ac2d.md` (N7); the AppImage and the macOS app bundle carry that `ac2d`.
- **3D spectrograph** (`docs/design/spectrograph-3d.md`): v1a + v1b landed
  (PROTO 86, session format 42, rig file 10): a fourth spectrum view (G) drawing the
  spectrograph's history as a skyline of spikes, camera on ←/→, PageUp/PageDown, I/O, Home
  and the mouse, the *ice* colour map, Settings › Display › 3D quality. Skyline goldens are
  their own test binary (`ac2-plot --test skyline`, GL in CI). Bloom (v1b) done. x86 / Apple
  desktops only: on the CM4 (V3D) every test passes but Bare is 8–10× over a frame, so V3D
  has no 3D view (G skips it, a layout's 3D pane draws the 2D spectrograph with a note);
  RTX 2070 Full 1.57 ms GPU at 1080p.
- **Many rigs in one app** (tabs, mixed panes, remote inputs, clocks across PTP
  hosts): `docs/design/multi-rig.md`, decisions N1–N6. Stage 1a (wire and files) landed: a
  daemon keeps its identity in `rig-id`, the hello carries `rig_id` and `name` (`ac2d
  --name` in every mode), every layout pane names its rig (PROTO 72, session format 36, rig
  file 8). Stage 1b landed: the app's state is split into one `RigState` per daemon
  (`AppState.rig` the active one, `background` the others; each owns its link, its events
  and requests carry its `RigId`, `ui.toml` remembers devices per rig), still with one rig.
  Stage 1c landed: Connect… (drop-down or palette) adds a rig, the top-bar drop-down
  switches between them, `ui.toml` remembers the rigs and connects them again at start,
  panes keep their rig across a switch (titles `pupu · …`, *unreachable · retrying* / *not
  connected*), Shift+Esc stops every rig and quit releases them all in parallel. Stage 1d
  landed: the measurement list offers every connected rig's measurements and meters under a
  heading per rig, a row of another rig's dragged onto a pane pins the pane to it (panes have
  no chip), a list row chosen never
  takes another rig's pane, `ui.toml` keeps each pinned pane's rig by id (resolved when that
  rig is held again, any address), and a stored layout reads `ThisRig`/`Rig` panes for the
  rig it belongs to. Stage 2 landed (PROTO 84): the daemon's `clocks` entity (capture clock
  ppm over the last minute, against the disciplined system clock where the backend's stamp
  clock can be read beside it — JACK on Linux, PROTO 85 — else against the computer's own
  clock; the system clock as `adjtimex` states it, *not stated* off Linux), shown per rig on Settings › Connection and by `ac2 status`,
  and a top-bar chip per background rig whose stimulus is armed or playing
  (`▶ kettu −24 dBFS`). Stage 3 landed (PROTO 88, session format 43, rig file 11;
  `docs/design/remote-inputs.md`, timing study `timing-study.md`): a session takes another
  rig's input (`auth admit-rig` on the giving rig, `auth peer-add` and `session open
  --remote-in RIG:IN` on the taking one, or *Add an input from another rig…* in the app),
  streamed raw per device block and joined at a fixed link offset; per remote rig a clock
  relation (*clocks agree within 2 ppm*, *measuring clocks…*, *CLOCKS NOT LOCKED*, *CLOCKS:
  CAN'T TELL*, never *locked*) on the top-bar chip, Settings › Connection › Between rigs and
  `ac2 status`. A transfer function with a remote reference or measurement averages only
  while the clocks agree, else holds its last good result (phase faint) and says why; on
  return its delay is tracked again. Rig acceptance with the operator (§7 there: a cross-rig
  TF on pupu and ketunkolo, then the word clock pull) is open; both rigs run on the Orion's
  word clock / ADAT.
- Open hardware gates (1.0): duplex check and 1 h run on macOS (Windows: done in a VM,
  a real PC and the panel/unplug cases pending, `asio.md` §4c–d), keyboard-only speaker
  tuning per OS, clean install → first measurement < 2 min per OS, signing certificates.

## Pointers by topic
- macOS audio: the `coreaudio` backend over `cahal` (github.com/mkovero/cahal): `docs/design/coreaudio.md`.
- Windows audio: ASIO only over the azo fork, FlexASIO / VB-CABLE in CI: `docs/design/asio.md`.
- Rig monitor (OSC / Art-Net cues from SPL meters kept on the rig): `docs/user-guide.md` *Rig measurements*.
- REW cross-check: `tools/crosscheck`, `docs/rigs/pupu.md`.
- Panes (tiling tree, a measurement's own panes): `PLAN.md` §8.1, `docs/user-guide.md` *Panes: split, close…*.
- Two-system alignment (delay, polarity, summation in an overlap band; Alt+M, `ac2 align`): `docs/design/alignment.md`, `docs/user-guide.md` *Alignment*.
- Power cuts and watchdogs: `docs/design/watchdogs.md`.
- EQ what-if (sets, prediction, console mark, APO text): `docs/design/eq-overlay.md`, `docs/user-guide.md` *EQ what-if*.
- Cursor peak / notch jumps and the mark's Δ / comb readout (Alt+P, Alt+N, Alt+D): `crates/ac2-scene/src/cursor_jump.rs`, `docs/user-guide.md` *Cursor jumps and the mark*.

## Where to look
| need | file |
|---|---|
| what to work on next | `docs/design/backlog.md` (open items only) |
| what already landed | `docs/design/backlog-done.md`, `git log` |
| why a decision was made | `docs/design/open-questions.md` (table at top), `docs/design/<topic>.md` |
| scope / architecture | `PLAN.md` — `grep -n '^#' PLAN.md`, then read one section |
| wire format | `docs/protocol.md` — by section, same way |
| user-facing behaviour, keys | `docs/user-guide.md` — by section |
| rig procedures | `docs/rigs/pupu.md` |
