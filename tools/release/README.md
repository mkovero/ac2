# tools/release

Scripts the release and all-OS CI runs use (`.github/workflows/`); each file's header has its usage.

| file | what it does |
|---|---|
| `check-windows-deps.ps1` | fails if a Windows executable imports the Visual C++ runtime DLLs (the build links the static CRT) |
| `coreaudio-loop/` | standalone crate (not a workspace member, it emits impulses): exact-index check of the Core Audio backend on BlackHole's own loop |
| `fetch-libsodium.sh` | vendors the prebuilt MSVC libsodium for Windows builds and prints its directory for `SODIUM_DIST_DIR` |
| `first-measurement.sh` | times a clean install of the Linux tarball to a first transfer measurement on the simulated rig, in a temp HOME |
| `loc-check.sh` | fails if any `crates/**/*.rs` is over 1000 lines (CI's rustfmt job) |
| `macos-blackhole.sh` | runs the macOS binaries on BlackHole 2ch: device list, duplex self-test, a recorded daemon session, `coreaudio-loop`, a revoked mic grant |
| `windows-asio.ps1` | runs the Windows binaries on FlexASIO over VB-CABLE: ASIO device list, duplex self-test, a recorded daemon session |
