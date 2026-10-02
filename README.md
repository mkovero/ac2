# ac2

Open-source live dual-channel FFT analyzer for PA system tuning — Linux (JACK), macOS,
Windows. Rust core → daemon → CLI / GPU UI over ZeroMQ, locally or across the network.

Status: phase 0 (foundations). See [PLAN.md](PLAN.md).

## Files

Every ac2 program finds its files through one helper (`crates/ac2-paths`), in the platform's
own directories:

| what | Linux | macOS | Windows |
|---|---|---|---|
| config: calibrations (`calibrations.json`), UI preferences (`ui.toml`), key bindings (`keys.toml`), daemon network keys | `~/.config/ac2` | `~/Library/Application Support/ac2` | `%APPDATA%\ac2\config` |
| data: saved sessions (`sessions/<name>/`) | `~/.local/share/ac2` | `~/Library/Application Support/ac2` | `%APPDATA%\ac2\data` |

`XDG_CONFIG_HOME` / `XDG_DATA_HOME` apply on Linux. `AC2_CONFIG_DIR` moves the config
directory, `AC2_SESSION_DIR` the sessions, `ac2d --cal-store PATH` the calibration store.
Calibrations describe the machine's hardware and stay in the config directory; sessions are
the operator's documents and live in the data directory.

License: MIT.
