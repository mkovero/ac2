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
- **Next: many rigs in one app** (tabs, mixed panes, remote inputs, clocks across PTP hosts):
  `docs/design/multi-rig.md`, decisions N1–N6. Stage 1a (wire and files) landed: a daemon
  keeps its identity in `rig-id` (`app-rig-id` for the app's embedded daemon), the hello
  carries `rig_id` and `name` (`ac2d --name` in every mode), every layout pane names its
  rig (PROTO 72, session format 36, rig file 8). Stage 1b landed: the app's state is split
  into one `RigState` per daemon (`AppState.rig` the active one, `background` the others;
  each owns its link, its events and requests carry its `RigId`, `ui.toml` remembers
  devices per rig), still with one rig. Next: 1c, many rigs (*Stages*).
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
