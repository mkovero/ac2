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
- Open hardware gates (1.0): duplex check and 1 h run on macOS (Windows: done in a VM,
  a real PC and the panel/unplug cases pending, `asio.md` §4c–d), keyboard-only speaker
  tuning per OS, clean install → first measurement < 2 min per OS, signing certificates.

## Pointers by topic
- macOS audio: the `coreaudio` backend over `cahal` (github.com/mkovero/cahal): `docs/design/coreaudio.md`.
- Windows audio: ASIO only over the azo fork, FlexASIO / VB-CABLE in CI: `docs/design/asio.md`.
- Rig monitor (OSC / Art-Net cues from SPL meters kept on the rig): `docs/user-guide.md` *Rig measurements*.
- REW cross-check (numbers, and the `visual` stage: ac2's drawing against REW's own graph image — transfer, group delay, impulse, RTA; the Leq strip ac2-only — `tools/plotcheck`): `tools/crosscheck`, `docs/rigs/pupu.md`.
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
