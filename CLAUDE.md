# ac2 — agent notes

Open-source (MIT) live dual-channel analyzer for PA tuning. Clean-slate successor to `ac`
(`~/src/ac`). Read `PLAN.md` for scope and architecture; `docs/design/open-questions.md`
for settled decisions (top table) and open technical questions.

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
| `ac2-core` | pure DSP/acoustics math; no I/O, no threads, `forbid(unsafe_code)` |
| `ac2-audio` | backend trait + capabilities, sample-indexed blocks; jack, cpal, fake |
| `ac2-proto` | typed protocol: commands, events, frame headers, version |
| `ac2-zmq` | the only crate linking libzmq: safe typed sockets, CURVE behind `SecureContext` (ZAP handler first) |
| `ac2-client` | async client |
| `ac2-discovery` | mDNS advert (`_ac2._tcp`, network mode only) and browse; names rigs, never trusts them |
| `ac2d` | daemon (`ac2d` binary) |
| `ac2-cli` | CLI (`ac2` binary) |
| `ac2-traces` | stored traces: capture columns, average / A−B, text import/export, session files |
| `ac2-paths` | where files live (platform config / data dirs) and atomic writes; shared by the daemon and the UI |
| `ac2-scene` | pure display truth: every displayed number/string, tested headless |
| `ac2-plot` | wgpu renderer for scenes; places pixels, never computes values |
| `ac2-ui` | desktop app |
| `ac2-testkit` | golden vectors from `tools/refgen`, tolerance compare; golden images (feature `image`) |
| `packaging/` | per-OS packaging scripts and icon, run by `.github/workflows/release.yml` |
| `spikes/*` | phase 0 throwaway spikes; findings in `docs/design/spike-*.md` |

## Rules
- No compatibility shims, no stringly-typed modes/commands (serde enums), no trait default
  returning `Ok` for an unsupported operation.
- Comments explain why the math/physics requires something — never issue numbers or history.
- `ac` is a source of ideas and test material, never code to copy file-for-file, and never
  the oracle for expected results (use `tools/refgen` / analytic models).
- Put a new dependency in the crate's own `Cargo.toml`; don't edit `[workspace.dependencies]`
  in parallel branches.
- Audio callback: no allocation, locks or syscalls.

## Audio safety
- Never emit sound on real hardware from automated runs. Tests and spikes output silence on
  real devices unless an explicit `--emit` flag with a typed level is given; agents never
  pass it. Use the fake backend or a JACK dummy server (`jackd -d dummy`) for testing.
- Any emitting code path enforces a global maximum level and fades out on stop.
