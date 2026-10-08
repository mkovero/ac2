# ac2 — agent notes

Open-source (MIT) live dual-channel analyzer for PA tuning. Clean-slate successor to `ac`
(`~/src/ac`). Start from `STATUS.md` (where things stand, which file answers what);
`docs/design/open-questions.md` for settled decisions (top table) and open technical questions.

## Build
```
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```
Toolchain pinned in `rust-toolchain.toml`. Edition 2024.

## Crate map
| crate | role |
|---|---|
| `ac2-core` | pure DSP/acoustics math (MTW, delay finder, spectrum/RTA, SPL, rolling Leq, sweep, mic curves, generator); no I/O, no threads, `forbid(unsafe_code)` |
| `ac2-audio` | backend trait + capabilities, sample-indexed blocks; jack (Linux only), cpal (macOS, Windows), fake, replay (a recording as a capture-only device) |
| `ac2-proto` | typed protocol: commands, events, frame headers, version |
| `ac2-zmq` | the only crate linking libzmq: safe typed sockets, CURVE behind `SecureContext` (ZAP handler first) |
| `ac2-client` | async client |
| `ac2-discovery` | mDNS advert (`_ac2._tcp`, network mode only) and browse; names rigs, never trusts them |
| `ac2d` | daemon (`ac2d` binary): session, jobs, state, calibration store, autosave, SPL log and Leq history |
| `ac2-cli` | CLI (`ac2` binary) |
| `ac2-traces` | stored traces: capture columns, average / A−B, smoothing, mic curve after capture, text import/export, per-second SPL log files, session files, raw capture files (f32 WAV/RF64 + sidecar) |
| `ac2-paths` | where files live (platform config / data dirs) and atomic writes; shared by the daemon and the UI |
| `ac2-scene` | pure display truth: every displayed number/string, tested headless |
| `ac2-plot` | wgpu renderer for scenes; places pixels, never computes values |
| `ac2-ui` | desktop app (`ac2-ui` binary): reducer, scoped key table, dialogs, can host an embedded daemon; its own code uses no DSP (`tests/no_dsp.rs`) |
| `ac2-testkit` | golden vectors from `tools/refgen`, tolerance compare; golden images (feature `image`) |
| `packaging/` | per-OS packaging scripts and icon, run by `.github/workflows/release.yml` |
| `spikes/*` | phase 0 throwaway spikes (`audio-duplex`, `gpu-headless`; the ZMQ spike became `ac2-zmq`); findings in `docs/design/spike-*.md` |
| `testing/` | per-platform tester guides (`testing/macos/README.md`, `testing/windows/README.md`); release binaries placed beside them are git-ignored |
| `tools/` | `refgen` (golden vectors), `protocol` (Python cross-language fixtures), `release` (smoke scripts), `experiments` |

## Rules
- No compatibility shims, no stringly-typed modes/commands (serde enums), no trait default
  returning `Ok` for an unsupported operation.
- Comments explain why the math/physics requires something — never issue numbers or history.
- `ac` is a source of ideas and test material, never code to copy file-for-file, and never
  the oracle for expected results (use `tools/refgen` / analytic models).
- Put a new dependency in the crate's own `Cargo.toml`; don't edit `[workspace.dependencies]`
  in parallel branches.
- Audio callback: no allocation, locks or syscalls.
- CI must stay green: after pushing, check the run (`gh run list --branch main`). Pushes run
  Linux only; macOS/Windows run before each release or on demand
  (`gh workflow run ci.yml --ref <branch> -f all_os=true`) — dispatch that after touching
  platform code (audio backends, paths, packaging, `cfg(windows|macos)`).
- Any change to what goes on the wire bumps `PROTO_VERSION` (pre-1.0: no compatibility);
  `fixtures/protocol/WIRE_LOCK` and its test enforce it.

## Context budget
Every turn re-sends the whole context, so what an agent reads stays paid for until it ends.
- Locate before reading: `grep -n` the symbol, then read about 60 lines around it. Never page
  through a file top to bottom. Big docs (`PLAN.md`, `docs/protocol.md`, `docs/user-guide.md`):
  `grep -n '^#'` first, read one section.
- Iterate with `cargo test -p <crate> <filter>`; the full workspace run once, before commit.
  Long commands run in the background (notified on exit) — no `until`/`sleep` polling loops.
- UI changes: assert the `ac2-scene` text first; view a snapshot PNG only for the final look.
- Delegation: one task per agent with the files and functions named in the brief; the agent
  ends when the task is done. Unrelated follow-ups go to a fresh agent, not via SendMessage.

## Audio safety
- Never emit sound on real hardware from automated runs. Tests and spikes output silence on
  real devices unless an explicit `--emit` flag with a typed level is given; agents never
  pass it. Use the fake backend or a JACK dummy server (`jackd -d dummy`) for testing.
- Any emitting code path enforces a global maximum level and fades out on stop.
