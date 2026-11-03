# The app spawns ac2d: no daemon inside the app

Status: built (stages 1 and 3; stage 2, packaging, separately). Operator decision 2026-11-03: "ui is ui and daemon is daemon".
The app hosts no daemon in its process: it connects to this computer's `ac2d`, starts the
bundled `ac2d` as a child when none serves, and by default stops that child when it quits.
Decisions D1 … D12 are listed at the end.

## Why the embedded daemon goes

- **Two daemons per machine.** The embedded one keeps its own identity (`app-rig-id`,
  `ac2-paths/src/lib.rs:100-104`) and autosave (`autosave-app`, `:127-131`) so it can sit
  beside the machine's `ac2d`; both can open one interface, and the app needs `<host> · ac2d`
  / `<host> · in this app` (`state/rigs.rs:604-614`, `ac2-scene/src/chrome.rs:157-163`) to
  tell them apart.
- **The app links the DSP.** Default feature `embedded` (`crates/ac2-ui/Cargo.toml`) pulls
  `ac2d` and through it `ac2-core`, `ac2-audio`, `ac2-roomsim`; `tests/it/no_dsp.rs:50-72`
  can only check that UI code does not use them directly.
- **Two start paths.** `embedded.rs:205-257` rebuilds most of `ac2d/src/main.rs:155-189`
  with small drifts (name "this computer", no `rig_logs`, `recording_dir`, `systemd_notify`).
- **A forgotten daemon holds the interface** (Windows ASIO above all): hence D7's default.

## 1. Which ac2d the app reaches (D1–D4)

**D1. Start-up** (replaces `main.rs:181-218`):

1. `--remote HOST` or `--ctrl/--data`: that daemon only, **no spawn**, even with no local
   `ac2d`. The kiosks (`ac2-ui --remote 127.0.0.1`, docs/rigs/pupu.md) are unchanged.
2. `--connect`: the dialog (§2); a row may spawn.
3. Remembered rigs (`ui.toml`) reconnect; a `reach = "local"` one runs *ensure local* (D4).
   Network rigs never spawn: only-remote rigs last time, no local daemon now.
4. Nothing given or remembered: *ensure local*.

**D2. The binary**: `ac2d` beside `std::env::current_exe()` only — no `PATH` fallback as in
`ac2 daemon start` (`ac2-cli/src/cmd/daemon.rs:172-186`): pre-1.0 the wire has no
compatibility, so a different version on `PATH` must not be picked up. Not found: the error
names the expected path. Dev builds find `target/<profile>/ac2d` after `cargo build -p ac2d`.

**D3. One ac2d per user, enforced by the daemon.** Nothing stops a second instance today:
`Daemon::start` binds at once (`ac2d/src/lib.rs:393-397`). On Unix libzmq's ipc listener
unlinks an existing socket path before binding, so a second `ac2d` silently takes over
`ctrl.sock` while the first keeps the interface (libzmq behaviour as read; stage 1 proves it
with a test). On Windows the second fails on `127.0.0.1:47820` (`AddressInUse`), but only
after rotating `ac2d.log` and opening audio. Fix (stage 1): every `ac2d` except `--ephemeral` takes an exclusive
`std::fs::File::try_lock` (stable on the pinned 1.95) on `runtime_dir()/ac2d.lock`
(`ac2-client/src/endpoint.rs`), on every OS, before log, audio or sockets, whatever it
listens on: one daemon per user owns the shared cal store, `rig.json` and autosave. Held →
exit code 3 (`ac2d::instance::exit::ANOTHER_RUNNING`, *another ac2d is running for this
user*), no file touched; 2 is a setup error, 1 fatal. The holder writes the pid file and
`runtime_dir()/ac2d.endpoints`: line 1 `ac2d: listening ctrl=… data=…` (the
`--print-endpoints` line), line 2 `mode=local|network`. A wildcard `tcp://0.0.0.0:P` in it is
dialled on loopback (`Endpoints::dialable`). `--ephemeral` (explicit `--ctrl/--data`
required) takes no lock and writes neither file: the simulated rig and tests. The app's
child writes the pid file, so **`ac2 daemon stop` also stops the app's child** (the app sees
it exit, D9).

**D4. Ensure local = spawn and connect at once; no running-probe.** The app spawns the
bundled `ac2d` (no flags: platform backend, default local endpoints, `rig-id`, `autosave`,
the shared cal store / `rig.json` / plans / rig logs / recordings) and starts the link to
the default endpoints at the same time. The lock decides: the child serves, or exits 3 and
the app reads `ac2d.endpoints` (polled up to 10 s: the winner writes it once bound) and
reaches the winner (a service, a hand-started `ac2d`, another app's child); the app does not
own it (D8). A winner on the default endpoints is already the link's target; on others the
link is restarted there. A **network-mode winner** is not connected to: a client needs its
pinned key, which the operator copies from that daemon (`ac2 daemon pair`), so the app opens
the connect dialog with *another ac2d is running for this user, in network mode on …: pair
with it, then connect to …* and the address typed in. Pairing from the app on its own would
mean trusting the first key the port answers with; the existing pairing flow already does
this right, so the app reports and prefills rather than growing a second one. The link's first attempt may miss a daemon still binding and
retries (`RETRY_EVERY` 2 s, `conn/mod.rs:410-411`). No socket-file test decides anything:
`local_daemon_running` (`connect.rs:176-190`, only the dialog's *running* detail now) and the
CLI's `probe` (`daemon.rs:140-147`) become a short-timeout hello instead of a
`ctrl.sock` existence test (the file survives a crash). Settings › Connection **Start ac2d
when none is running**, default on: off (hosts running `ac2d` as a service, e.g.
ketunkolo), ensure local only connects and the link keeps retrying. With it on and a service
unit starting later, the unit loses the lock: `packaging/linux/ac2d.service` gets
`SuccessExitStatus=3`, so that loss is an orderly inactive unit, not a restart loop into
systemd's start limit. A spawn that fails before serving (no binary, exit ≠ 3) opens the
connect dialog with the error; never a silent switch to the simulated rig.

*First run without a JACK server* (or ASIO with no driver): `JackBackend::new` does not fail
(`ac2-audio` reports `AudioError::Unavailable` per device instead), so `ac2d` starts and
serves; the app connects, the Sessions dialog opens (no audio I/O), and the Audio I/O page
shows the backend's reason (*no JACK server answered …*). Tested with the fake backend
reporting a device unavailable; never against a real server.

*Two apps on one machine*: app A spawned and owns `ac2d`; app B's spawn exits 3 and B
connects to A's child as not its own. A quitting (stop on) stops the daemon under B: B's
link shows disconnected with D9's toast and retries; B does not respawn on its own.
*Connect a rig… › This computer* in B spawns again (B then owns it). Documented in the guide.

## 2. The connect dialog (D5)

`Choice::Embedded` / `Entry::Embedded` go (`connect.rs:22-39`, titles and details `:53-90`,
`entries` `:293-298`). Rows:

- **This computer** (`Entry::Local`), *running* / *not running; starts when chosen*: runs
  *ensure local*. It replaces both *Local daemon* ("start it with `ac2 daemon start`") and
  *This computer's audio (JACK/ASIO/Core Audio)*; preselected (today the embedded row is,
  `:237-248`).
- **Simulated rig (no audio)** (`Entry::Simulated`): D6. Network rigs: unchanged.

`ConnectSetup.backends` (`app.rs:61-67`), `embedded_backends()` (`main.rs:162-167`) and
`EmbeddedBackend` (`embedded.rs:64-104`) go: the backend is the daemon's (Audio I/O page).
`--embedded` / `--backend` (`main.rs:51-57`) become `--backend NAME`: ensure local with
`ac2d --backend NAME` (`fake` → the simulated rig instead). If the daemon reached is one
already running, its backend (`session.devices`, the default listed first) is compared with
NAME once the device list has arrived (no verdict before it); a different one is refused
(*ac2d on this computer runs on jack, not asio*), never used in its place. CI's FlexASIO smoke (`.github/workflows/ci.yml:265-286`) becomes
`ac2-ui --backend asio --no-discovery --smoke`.

## 3. The simulated rig is an ac2d on the fake backend (D6)

The fake backend is in every `ac2d` build: `BackendChoice::Fake` has no feature or cfg gate
(`ac2d/src/backend.rs:162-164`), and release smoke tests already run `ac2d --backend fake`
(`release.yml:151, 244`). Missing is a mode that keeps nothing, as the embedded fake does
(`embedded.rs:229-239, 260-268`):

- `ac2d --ephemeral`: `RigIdSource::Ephemeral`; no autosave, cal store, `rig.json`, plan
  dir, rig logs or recordings; nothing read or written in config/data dirs; log
  `ac2d-sim.log`. (`main.rs` today always sets `rig_id` to `File` and every store `Some`.)
- Spawned on every OS as `ac2d --backend fake --ephemeral --name "Simulated rig"
  --ctrl tcp://127.0.0.1:0 --data tcp://127.0.0.1:0 --print-endpoints --stop-on-stdin-eof`:
  `--ephemeral`, so no lock, pid or endpoints file (D3) and nothing on disk but its log; the child
  prints its bound endpoints as one stdout line (`ac2d: listening ctrl=… data=…`) that the
  app reads before connecting. Stage 1 checks that `Listen::Local` validation
  (`ac2d/src/config.rs:386-404`) accepts port 0.
- The demo (session open, "demo" started) stays a client step: `set_up_demo`,
  `demo_session`, `demo_measurement` (`embedded.rs:33-62, 272-311`) move to `src/sim.rs`.
- Never remembered (as `app.rs:421-423`); always stopped when the app quits or its rig
  closes, whatever D7 says — it holds no interface and its measurements live only in it.
  One per app: picking it again reads *Simulated rig is already a rig here*.

## 4. Lifetime of a spawned ac2d (D7–D9)

**D7. Settings › Connection: *Stop ac2d when the app quits*, default on**; `ui.toml`
`[daemon] stop_on_quit` (and `start_local`, D4), written only when off; detail *the ac2d
this app started; one started otherwise keeps running*. Read at each spawn: it decides how
the child is spawned (D8), so a change applies from the next spawn (the row says so). With
it on, *Close <rig>* of an owned real-audio rig also stops its `ac2d`. Inside an AppImage
(`$APPIMAGE` set) it is forced on and the row reads *always on: the AppImage's files go away
when the app quits*, since a detached child would outlive the mount it runs from. Settings
rows (←/→): no key, so no keytip; user-guide rows in the same change.

**D8. Ownership and stop: an inherited stdin pipe.** No wire change. With D7 on, the child
is spawned with `Stdio::piped()` stdin and `--stop-on-stdin-eof`: a thread in `ac2d` reads
stdin, and EOF (or a read error) calls `Stopper::stop()` (`ac2d/src/lib.rs:181-195`), the
signal handler's path, so `Control::shutdown` (`control.rs:797-815`) runs: sweep aborted,
recording ended, output stopped through the gate's fade, session closed, file jobs and
autosave flushed, rig logs finished. The app owns an `ac2d` only while it holds that
`std::process::Child` and its stdin write end and `try_wait()` says running; a daemon found
running, or a child that exited 3, is never stopped. Quit, in `main.rs:296-302`'s order:
window closes → every link stops its stimulus and releases its lease (`close_all_links`,
`app.rs:694-706`, `QUIT_GRACE` 1 s) → the write end is dropped → wait ≤ 10 s from that drop
(as `daemon.rs:314-320`), then `Child::kill` and a log line; all after the window is gone.
The kill is safe for the data: the autosave writes its files first and renames the manifest
into place (`autosave.rs:15-19`), so a kill mid-write keeps the last good autosave and loses
only the final one. An app crash on any OS closes the pipe as well, so the child stops in
order with no `--owner-pid`, `pre_exec` or job object. A *hung* app (alive, not answering)
keeps the pipe open and so the child too; accepted, as the operator ends a hung app and that
closes the pipe. `process_group(0)` (Unix) keeps a terminal's Ctrl-C on the app; the app
quitting then ends the child. On Windows both binaries are console-subsystem (no
`windows_subsystem` attribute), so an owned child sharing the app's console would get its
`CTRL_CLOSE_EVENT` and both be killed about 5 s later, before any orderly stop: the owned
child is spawned with `CREATE_NO_WINDOW` (its own hidden console). With D7 off, the child is
spawned detached as `ac2 daemon start` does (`daemon.rs:208-224`: stdin null,
`process_group(0)`; `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`) and keeps running. `ac2 daemon stop` stays
as it is: on Windows it already sends `GenStop` and `SessionClose` before `taskkill /F`
(`daemon.rs:307-313`), so stimulus fades and the stream closes, though the autosave flush is
skipped. The *unsaved changes* question (`AppOptions.embedded_autosave`, `app.rs:47-49`)
becomes: the app will stop an owned daemon that does not autosave (the simulated rig).

**D9. The child dies while the app runs**: `try_wait()` each frame pump; a toast *ac2d
stopped (exit N); its log: <log dir>/ac2d.log*; the rig shows disconnected and the link
retries, so a service or hand-started `ac2d` is picked up. No automatic respawn (a driver
fault would loop holding the interface); *Connect a rig… › This computer* starts it again.
For that exit code to mean anything, stage 1 makes a fatal end exit 1: today
`ControlMsg::Fatal` (sent by the io thread, `io.rs:131, 142`) breaks the control loop
(`control.rs:770-773`), `Handle::wait` returns, and `main.rs:203-208` returns `SUCCESS`.

**Output.** Child stderr to null; stdout piped only for D6's line, else null; `ac2d` writes
its own `ac2d.log` (`main.rs:104-114`, previous run `.log.1`; the lock comes first, so a
loser never rotates it). The app removes `JOURNAL_STREAM` from the child's environment, else
a unit-started app makes the child skip its log file while stderr is null. Lines once in
`ac2-ui.log` are now in `ac2d.log`; the guide's *send the log* names both.

## 5. Packaging

| package | ac2d beside the app today | change |
|---|---|---|
| Linux tarball | yes, `bin/` (`packaging/linux/package.sh:42-45`) | `ac2d.service`: `SuccessExitStatus=3` |
| Linux AppImage | **no**: only `usr/bin/ac2-ui` (`package.sh:76-82`) | add `usr/bin/ac2d` (stage 2); header comment `:5` ("with its embedded daemon") in stage 3 |
| macOS `.app` | **no**: `Contents/MacOS` has only `ac2-ui` (`packaging/macos/package.sh:37-38`); `ac2d` is in the DMG's `bin/` (`:43`) | also `Contents/MacOS/ac2d`, signed with `entitlements.plist` (audio input, hardened runtime) **before** the bundle (`:56-58`); `bin/ac2d` stays for CLI / launchd |
| Windows MSI / zip | yes, `INSTALLFOLDER` (`packaging/windows/ac2.wxs:27-35`), zip `release.yml:228` | none |

**macOS microphone (unverified; needs a tester or CI).** TCC attributes a process to its
*responsible* process; a helper an app bundle starts from its own `Contents/MacOS` is
expected to count as the app (prompt names ac2, uses `NSMicrophoneUsageDescription`,
`Info.plist:31`; grant covers the child). To confirm on hardware: ad-hoc-signed bundles, and
a detached child (D7 off) outliving the app. The launchd plist comment
(`io.github.mkovero.ac2d.plist:11-13`, "or let the app host its own daemon") becomes "or
start ac2-ui, which starts ac2d for you".

## 6. What goes, what changes

- `crates/ac2-ui/src/embedded.rs` deleted (setup to `sim.rs`). New `src/spawn.rs`: find
  (D2), spawn and ensure local (D4, D6), ownership and stop (D8), watch (D9) — `std::process`
  plus `ac2-client`, no new dependency, no DSP.
- `app.rs`: `embedded` (`:205`) / `embedded_slot` (`:221-222`) → `owned: Vec<(slot,
  OwnedDaemon)>`; `link_embedded` (`:267, 277`, `RigState`), `embedded_target`
  (`:929-932`), `start_rig`'s `Choice::Embedded` arm (`:409-427`) go; `close_rigs`
  (`:650-666`) drops an owned daemon's pipe after its link per D7.
- `Cargo.toml`: feature `embedded` and optional `ac2d` go (no features left). `no_dsp.rs`
  keeps one test (no `ac2d`/DSP on normal+build edges); `only_the_embedded_daemon_brings_dsp_in`
  goes; `tests/it/main.rs:7-12` loses its cfgs; CLAUDE.md's UI-only command drops
  `--no-default-features`.
- **D10 names**: a locally reached rig reads just the host name (**ketunkolo**), no
  `· ac2d` suffix; this supersedes pane-curves D8's suffix — with one real daemon per user
  (D3) there is nothing on this host to tell apart. `same_host_rig` (`chrome.rs:157-163`)
  and its test (`:179-180`) go; `label_of` (`state/rigs.rs:604-614`) returns the host. The
  simulated rig reads its hello name *Simulated rig*; network rigs their hello name.
- **D11 `ui.toml`**: `RigAddrFile::Embedded` (`prefs/rig_file.rs:17-33, 41, 67`) and
  `RigAddr::Embedded` (`prefs/mod.rs:319-333`) go. Dropping the variant alone would make an
  old `reach = "embedded"` table fail the parse and set the **whole file** aside as damaged
  (`prefs/file.rs:943-967`); so `rigs` is read per entry (as `lenient_layout`): such an entry
  is dropped with a log warning and one notice, *the app's own daemon is gone; this
  computer's ac2d is connected instead*; the next save omits it. Device/role keys under the
  old `app-rig-id` stay, naming no rig.
- **D12 files on disk**: `ac2_paths::app_rig_id` / `app_autosave_dir` and callers go;
  `<config>/app-rig-id` and `<data>/autosave-app` are neither read nor deleted. The guide
  says to open `autosave-app` (a session directory) once from Sessions if needed.
- `ServerMode::Embedded` (`ac2-proto/src/model/state.rs:214`) and `Listen::Inproc` stay for
  now: `ac2d`'s own tests run daemons in-process (`ac2d/tests/it/common/mod.rs`).

## 7. Tests

- The embedded e2e tests — 22 of the 26 files under `crates/ac2-ui/tests/it/embedded/` call
  `start_embedded*` (85 calls; `grep -rn start_embedded crates/ac2-ui/tests`), 73 tests with
  `tests/it/connect.rs` — move to a tests-only crate `crates/ac2-e2e` (`tests/it/`, deps
  `ac2-ui` + `ac2d`) with the `Driver` harness. One helper `sim_daemon(Setup)` runs
  `ac2d::Daemon::start` on the fake backend over `Listen::Local` (loopback tcp, port 0):
  call sites change mechanically and use the shipped transport. Afterwards
  `cargo nextest run -p ac2-ui` builds no `ac2d` and no audio crate; `ac2-core` and
  `ac2-traces` stay dev-dependencies (test oracle), so the DSP crate is still compiled for
  the UI tests, as today with `--no-default-features`. A dev-dep on `ac2d` in `ac2-ui`
  instead would add the daemon and audio crates to every UI test build (dev-deps cannot be
  optional).
- Spawn tests need the binary (`CARGO_BIN_EXE_ac2d`), so they live in `ac2d/tests/it/spawn.rs`:
  lock refusal (exit 3, log not rotated, pid file untouched); `--ephemeral` writes no
  lock or pid file; a stale socket after `kill -9` no longer blocks; stdin EOF ends the
  daemon in order (autosave complete, rig logs finished); a kill mid-autosave-write leaves
  a loadable autosave; a fatal end exits 1;
  `--ephemeral` writes nothing under temp `XDG_*` dirs; port 0 with `--print-endpoints`.
  The app's ownership rule (D8) and ensure-local outcomes (D4) are tested in `ac2-ui` with
  a stub child (`spawn/tests.rs`: the test binary re-run as the child). One `ac2-e2e` test
  starts the simulated rig from the connect dialog with the built `ac2d` copied beside the
  test binary (skipped when `ac2d` is not built or there is no GPU adapter).
- Fake backend only (Audio safety).

## Stages

Each lands alone, quick tier green, with its user-guide rows.

1. **ac2d** (daemon and CLI, no wire change), done: lock, pid file and `ac2d.endpoints` on
   every `ac2d` but `--ephemeral` (D3). The lock code does `create_dir_all(runtime_dir())`
   itself (0700 on Unix)
   on every OS, then `File::try_lock`, the file held in `main` for the daemon's life;
   `WouldBlock` → exit 3 before log rotation, any other error → exit 2 naming the path,
   never running unlocked. `--ephemeral`, `--print-endpoints`, `--stop-on-stdin-eof` (its
   help: for a parent holding a pipe; EOF within 1 s of start is logged at error level, as
   that means stdin was never a pipe; `ac2 daemon start` never passes it); fatal → exit 1;
   the CLI's `probe` a short-timeout hello; `ac2 daemon start` reads a child exit 3 as
   *another ac2d is running for this user*; `ac2d.service` `SuccessExitStatus=3`;
   `spawn.rs` tests.
2. **Packaging**: `ac2d` in the AppImage and `Contents/MacOS` (nested signing), launchd
   comment; all-OS dispatch; a macOS tester checks TCC (§5).
3. **The app spawns, the embedded daemon goes** (app only, one change), done: `spawn.rs`
   (`ac2-ui/src/spawn.rs`, the children the app watches in `app/daemons.rs`), D1, D2,
   D4–D11 (the owned Windows child with `CREATE_NO_WINDOW`), both Settings rows, dialog
   rows, `--backend`, the CI FlexASIO step, `embedded.rs`, feature, lenient `rigs`, D12,
   host-name labels, tests to `ac2-e2e`, `no_dsp.rs`, the AppImage header comment. Deploy:
   on hosts running `ac2d` as a service (ketunkolo), set *Start ac2d when none is running*
   off before the first app start after the upgrade (docs/rigs/pupu.md gets the line at
   deploy). Docs:
   user-guide (`:76-81, 146, 159-160, 233, 2343`), install.md (`:55` `pw-jack ac2-ui
   --embedded`, `:76` AppImage), protocol.md (`:49-55, 353`), sessions.md `:121-124`,
   settings.md `:102, 113`, multi-rig.md, asio.md `:232, 403, 480`, coreaudio.md `:246,
   512-544`, pane-curves.md §4 (D8 suffix superseded), STATUS.md `:32`, open-questions N1/P2,
   CLAUDE.md (crate map, UI-only command).

## Open questions

1. **macOS TCC** (§5): grant attributed to ac2.app for a `Contents/MacOS` child, signed and
   ad-hoc, and after the app quits?
2. **`ServerMode::Embedded` / `Listen::Inproc`**: remove once `ac2d`'s tests use loopback
   tcp, or keep as test-only transports?

## Row for open-questions.md

N7, in the multi-rig series: it settles which daemons the app holds and retires N1's
embedded-daemon clause.

| N7 | **ui is ui, daemon is daemon** ([ui-spawns-ac2d.md](ui-spawns-ac2d.md)). The app hosts no daemon: at start it spawns the `ac2d` beside its executable (never one on `PATH`) and connects at once; one `ac2d` per user is a lock every `ac2d` but `--ephemeral` takes; a loser exits 3 and the app reaches the winner named in `ac2d.endpoints` without owning it (a network-mode winner: the connect dialog says to pair with it, its address typed in). `--remote` / `--ctrl` never spawn. Settings › Connection: *Start ac2d when none is running* (default on; off on service hosts; `ac2d.service` has `SuccessExitStatus=3`) and *Stop ac2d when the app quits* (default on, forced on in the AppImage): the child then holds an inherited stdin pipe and stops in order (fade, autosave flush) when the app quits or dies; off, it is spawned detached and keeps running. An `ac2d` the app did not start is never stopped; `ac2 daemon stop` stops the app's child too. *Simulated rig* is a spawned `ac2d --backend fake --ephemeral` on loopback port 0, always stopped with the app. A local rig reads the host name (`ketunkolo`), superseding pane-curves D8's `· ac2d` suffix. `reach = "embedded"` rigs are dropped from `ui.toml` on load with a notice; `app-rig-id` / `autosave-app` unused. No wire change. Supersedes N1's embedded clause. (operator, 2026-11-03: "ui is ui and daemon is daemon") |

## Decisions (proposed under the operator's 2026-11-03 rule)

D1 `--remote`/`--ctrl` connect only; else ensure local; network-only remembered rigs spawn
nothing. D2 the binary beside the app, never `PATH`. D3 one `ac2d` per user: lock + pid
file only on default local endpoints, in `runtime_dir()`, every OS; exit 3; `ac2 daemon
stop` also stops the app's child. D4 ensure local = spawn and connect at once, no probe;
*Start ac2d when none is running* (default on); `SuccessExitStatus=3`. D5 dialog rows *This
computer*, *Simulated rig*, network rigs; `--backend NAME` compared with a running
daemon's. D6 simulated rig = `ac2d --backend fake --ephemeral` on loopback port 0 with
`--print-endpoints`, nothing on disk. D7 *Stop ac2d when the app quits*, default on,
applies from the next spawn, forced on in the AppImage. D8 ownership = the child and its
stdin pipe; EOF → orderly shutdown; no wire change. D9 child death: toast, no respawn;
fatal exits 1. D10 local rig reads the host name. D11 `reach = "embedded"` dropped on load
with a notice. D12 `app-rig-id` / `autosave-app` left on disk, unused.
