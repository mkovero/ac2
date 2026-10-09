# ac2 — agent notes

Open-source (MIT) live dual-channel analyzer for PA tuning. Clean-slate successor to `ac`
(`~/src/ac`). Start from `STATUS.md` (where things stand, which file answers what);
`docs/design/open-questions.md` for settled decisions (top table) and open technical questions.

## Build
```
cargo build --workspace
cargo nextest run --workspace        # quick tier (see Tests)
cargo test --workspace --doc         # doctests (nextest skips them)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```
Toolchain pinned in `rust-toolchain.toml`. Edition 2024. Each crate's integration tests are one
binary, `tests/it/` (a module per area): `cargo test -p ac2d --test it leq::`. A test that needs
its process to itself (global allocator, process CPU time) stays a separate file in `tests/`.

## Tests
Tests named `slow_…` need ~5 s+ of real time (Leq windows, recovery backoffs, drift, end-to-end
app runs). Profiles in `.config/nextest.toml`:
- **Quick tier** (default, skips `slow_*`): iteration and pre-commit/pre-push; add
  `cargo test --workspace --doc` when docs changed.
- **Full tier** (`--profile full` + doctests): only when the operator asks or for an all-OS
  release build. CI runs the quick tier on every push; the full tier on an all-OS/release run
  or `gh workflow run ci.yml --ref main -f full=true` (Linux only).
- nextest `-j` = test threads; build jobs `--build-jobs N`. One area: `-p ac2d -E 'test(/^leq::/)'`.

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
| `ac2-traces` | stored traces and their math, text import/export, SPL log files, session files, raw capture files (f32 WAV/RF64 + sidecar) |
| `ac2-paths` | platform config / data dirs, atomic writes (daemon and UI) |
| `ac2-scene` | pure display truth: every displayed number/string, tested headless |
| `ac2-plot` | wgpu renderer for scenes; places pixels, never computes values |
| `ac2-ui` | desktop app (`ac2-ui` binary): reducer, scoped key table, dialogs, can host an embedded daemon; its own code uses no DSP (`tests/it/no_dsp.rs`) |
| `ac2-testkit` | golden vectors from `tools/refgen`, tolerance compare; golden images (feature `image`) |
| `packaging/` | per-OS packaging scripts and icon, run by `.github/workflows/release.yml` |
| `spikes/*` | phase 0 spikes (`audio-duplex`, `gpu-headless`); findings in `docs/design/spike-*.md` |
| `testing/` | per-platform tester guides; release binaries beside them are git-ignored |
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
What an agent reads stays in its context for the rest of the session; keep reads bounded.
- Locate before reading: `grep -n` the symbol, then read about 60 lines around it. Never page
  through a file top to bottom. Big docs (`PLAN.md`, `docs/protocol.md`, `docs/user-guide.md`):
  `grep -n '^#'` first, read one section.
- Iterate with `cargo nextest run -p <crate> <filter>`; quick tier before commit/push (see *Tests*).
  Long commands run in the background (notified on exit) — no `until`/`sleep` polling loops.
- UI changes: assert the `ac2-scene` text first; view a snapshot PNG only for the final look.
- Delegation: one task per agent with the files and functions named in the brief; the agent
  ends when the task is done. Unrelated follow-ups go to a fresh agent, not via SendMessage.

## Audio safety
- Never emit sound on real hardware from automated runs. Tests and spikes output silence on
  real devices unless an explicit `--emit` flag with a typed level is given; agents never
  pass it. Use the fake backend or a JACK dummy server (`jackd -d dummy`) for testing.
- Any emitting code path enforces a global maximum level and fades out on stop.
