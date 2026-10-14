# ac2 — status and where to look

Short orientation; read this instead of the whole of `PLAN.md`. Versions live in the code
(`PROTO_VERSION` in `crates/ac2-proto/src/lib.rs`), not here.

## Where things stand
- Phases 0–6 (1.0 scope) pass their CI criteria; phase 7 (post-1.0 extras) is in progress.
  Per-phase table with CI and hardware results: `PLAN.md` §9.0.
- Hardware verified on Linux: the `pupu` rig (`docs/rigs/pupu.md`), a Pi 4 kiosk client.
  Windows: RME Babyface Pro on its ASIO driver in a VM (1 h duplex, loopback timing;
  `docs/design/asio.md` §4c); tester guide and dev.13 MSI in `testing/windows/`. macOS: no
  audio interface yet; tester guide and dev.13 disk image in `testing/macos/`.
- CI on push is Linux only; macOS/Windows run before a release or on dispatch
  (`CLAUDE.md` *Rules*). Rigs run locally built binaries: last deploy 70655493 (PROTO 51) on pupu,
  ketunkolo and the Pi (`docs/rigs/pupu.md`, *Deploy log*). A session format change on deploy is restored
  with `ac2 session salvage`.
- macOS: the direct Core Audio backend (`coreaudio`, the HAL through `cahal`: its own
  repository github.com/mkovero/cahal, from crates.io) is the only macOS backend (cpal removed,
  `docs/design/coreaudio.md` §11); exact on BlackHole in the all-OS CI run (`all_os=true`).
  Open on real hardware: the tester round (`testing/macos/`, Test 6: stated vs measured
  latency, replug, rate change, streams).
- Newest features: drag a measurement-list row onto a pane (an edge drop splits, Shift-drop
  compares on a transfer pane) and Ctrl+Z / Ctrl+Shift+Z undo pane edits (`docs/user-guide.md`
  *Dragging a row onto a pane*, *Undo and redo*); T on an SPL pane takes the limit text
  (OK / NEAR / OVER, stay ≤, limit), the 10 dB lines and the scale like a graph's grid; Ctrl+S
  saves the session. A health meter in the top bar after the session line (daemon host CPU,
  this computer's with a remote daemon, ac2d's callback load, xruns, data rate, fps;
  `docs/user-guide.md`, the paragraph after the driver-change one; health in the keepalive,
  `docs/protocol.md`). Green in the all-OS CI run on 00bae469.
  Per-measurement Resolution (1/12 … 1/96 octave) for transfer and sweep
  with a grey resolution marker where the estimate is coarser than the grid (`docs/user-guide.md`
  *Resolution and the resolution marker*); mic curve applied to sweeps; settable spectrum/RTA
  averaging; multi-resolution spectrum (683 ms windows at LF … 21 ms at HF, window changes
  marked: `docs/design/multires-spectrum.md`), shown as tone or density (dBFS/Hz; the
  multi-resolution default, so broadband sound reads as on a fixed FFT). Band Leq per
  1/3-octave band (STM 545/2015): `docs/design/band-leq.md`.
- REW cross-check (`tools/crosscheck`, `docs/rigs/pupu.md`): electrical and Genelec at −30 dBFS
  agree with REW and numpy (FAIL 0 on 8b87f49).
- Panes are a tiling tree (one pane at first start, N split, Shift+N turns a split, Alt+←/→ or
  a dragged gap sizes a pane, Q close, Tab / Shift+Tab step the measurement tree's rows into
  the focused pane, G / Shift+G step a pane's views, the IR's linear / log / ETC views among
  them); the focused pane shows any measurement picked, changing kind: `PLAN.md` §8.1,
  `docs/user-guide.md` *Panes: split, close…*. A measurement can carry its own panes (in the
  session, decision P1); only a measurement row brings them, a live-curve, math or trace row
  goes into the focused pane alone (*Layouts follow the measurement*).
- Open hardware gates (1.0): duplex check and 1 h run on macOS (Windows: done in a VM,
  a real PC and the panel/unplug cases pending, `asio.md` §4c–d), keyboard-only speaker
  tuning per OS, clean install → first measurement < 2 min per OS, signing certificates.
- Windows audio is ASIO only (decision W1): the backend over the azo fork is in every Windows
  build and WASAPI is gone; devices without a driver use FlexASIO, and with no ASIO driver the
  device list says so. Passes under Wine (`tools/wine-asio`); the Windows CI audio pass is
  FlexASIO on VB-CABLE (all-OS runs). RME's driver (Babyface Pro in a Windows 11 VM, USB host
  passthrough) passes the 1 h silent run and loopback timing (4160 samples measured, 4159
  stated); its ADAT clock loss is silent to the host (`docs/design/asio.md` §4c).

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
