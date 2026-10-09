# ac2 — status and where to look

Short orientation; read this instead of the whole of `PLAN.md`. Versions live in the code
(`PROTO_VERSION` in `crates/ac2-proto/src/lib.rs`), not here.

## Where things stand
- Phases 0–6 (1.0 scope) pass their CI criteria; phase 7 (post-1.0 extras) is in progress.
  Per-phase table with CI and hardware results: `PLAN.md` §9.0.
- Hardware verified on Linux only: the `pupu` rig (`docs/rigs/pupu.md`), a Pi 4 kiosk
  client. macOS: a tester runs the disk image (`testing/macos/`); Windows: MSI in a VM,
  tester guide and dev.10 MSI in `testing/windows/`.
- CI on push is Linux only; macOS/Windows run before a release or on dispatch
  (`CLAUDE.md` *Rules*). Rigs run locally built binaries: last deploy 08200c3 on pupu,
  ketunkolo and the Pi (`docs/rigs/pupu.md`, *Deploy of 2416a06, then 511efb0*).
- Newest feature: band Leq per 1/3-octave band (STM 545/2015), `docs/design/band-leq.md`;
  its open items are in `docs/design/backlog.md`.
- Panes are a tiling tree (one pane at first start, N split, Q close, Tab puts the next
  measurement in the focused pane, G steps every pane's views, IR a transfer view); the
  focused pane shows any measurement picked (Tab, the list, its chip's list), changing kind: `PLAN.md` §8.1, `docs/user-guide.md` *Panes: split, close…*.
- Open hardware gates: duplex check (`ac2 selftest duplex`) and 1 h run on macOS/Windows, keyboard-only speaker
  tuning per OS, clean install → first measurement < 2 min per OS, signing certificates.

## Where to look
| need | file |
|---|---|
| what to work on next | `docs/design/backlog.md` (open items only), `docs/design/ui-backlog.md` |
| what already landed | `docs/design/backlog-done.md`, `git log` |
| why a decision was made | `docs/design/open-questions.md` (table at top), `docs/design/<topic>.md` |
| scope / architecture | `PLAN.md` — `grep -n '^#' PLAN.md`, then read one section |
| wire format | `docs/protocol.md` — by section, same way |
| user-facing behaviour, keys | `docs/user-guide.md` — by section |
| rig procedures | `docs/rigs/pupu.md` |
