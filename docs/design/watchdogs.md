# Watchdogs and failing loud

Status: implemented (`ac2-paths` `notify.rs` and `log.rs`, `ac2d` `control.rs` and
`autosave/spl_files.rs`, `ac2-ui` `app.rs`). Wire format: `docs/protocol.md` §7.3
(`autosave.log_failures`).

## Why

A value frozen on a screen at front of house reads as a valid measurement. A process that
has crashed is restarted and is obvious; a process that is half alive (a dead thread, a
hung loop, a GPU present that never returns, a log that silently stopped growing) is not.
So every failure that can be turned into a restart is turned into one, and every failure
that cannot (a full disk) is shown to the operator until it is gone.

## What each process pings

Both programs speak systemd's notify protocol (`READY=1`, `WATCHDOG=1`, `STOPPING=1`) as
datagrams to `$NOTIFY_SOCKET`, path or abstract (`@`). Linux only; with `NOTIFY_SOCKET`
unset (a desktop session, macOS, Windows) the notifier does nothing. `WATCHDOG_USEC` sets
the ping interval: a ping goes out at most every quarter of it, so a ping that also waits up
to the same again for the next tick or frame still lands within half the timeout, and a
60 fps UI does not make a syscall per frame. `WATCHDOG_PID` is not checked: each program is
its unit's main process (a wrapper script `exec`s it), and neither starts other programs
that could inherit the variables.

| process | `READY=1` | `WATCHDOG=1` | `STOPPING=1` |
|---|---|---|---|
| `ac2d` | control thread, before its loop: sockets bound, autosave restored, rig monitor started | the control loop's keepalive tick (250 ms) | first thing in the control loop's shutdown |
| `ac2-ui` | the first frame after one was presented | each frame after the first presented one | — (the unit stops it) |

The pings come from the loop they vouch for, never from a side thread: a stuck control
loop (a blocking call, a deadlock) or a UI whose reducer, renderer or GPU present hangs
stops pinging, and systemd kills and restarts it. eframe runs a frame's `ui` pass only after
the previous frame was painted and presented on the same thread, so a ping at the start of
a pass proves the previous present returned. An idle UI repaints at least every second
(`Notifier::max_idle`, shorter for a watchdog under 4 s) while a watchdog is set, so a
healthy UI with nothing changing still pings.

A daemon embedded in the app never notifies (`DaemonConfig::systemd_notify` is set by the
`ac2d` program only): its pings would vouch for the UI's unit while the UI hung.

A UI that does not draw because its output is off (a Wayland compositor stops frame
callbacks for a blanked output) stops pinging too and is restarted. The kiosk keeps its
display on; a host that blanks it must not set a watchdog on the UI.

## Unit settings

| unit | settings |
|---|---|
| `ac2d.service` (user unit) | `Type=notify`, `WatchdogSec=10`, `Restart=always` |
| `ac2-ui.service` (the Pi kiosk's user unit; `ExecStart` a wrapper that `exec`s `ac2-ui`) | `Type=notify`, `WatchdogSec=5`, `NotifyAccess=main`, `TimeoutStartSec` generous (the first frame waits for GPU init), `Restart=always` |

The kiosk runs `ac2-ui` as its own user unit rather than as cage's child: a child of a
`PAMName=login` service lives in the session's scope, where the service's watchdog,
`NotifyAccess` and `MemoryMax` do not reach it. cage's client only runs
`systemctl --user start --wait ac2-ui.service`.

## panic = "abort"

Release builds abort on a panic (`[profile.release] panic = "abort"`): a panicking thread
takes its process down and the unit restarts it, instead of leaving the rest running with a
dead thread (the UI's link thread, the daemon's autosave writer). A panic hook
(`ac2_paths::log::install_panic_hook`) logs the thread, place and a backtrace through the
tracing subscriber first, so the log file says why. Tests and debug builds still unwind.

What changes for code that catches panics: the ZAP handler's `catch_unwind` (`ac2-zmq`) and
the libjack callback wrapper (`ac2-audio` `jack_host.rs`) no longer catch anything in a
release build; a panic there aborts, which is the intent: the process restarts with a clean
authenticator or audio client rather than carrying on after one went wrong.

## Disk failures reach clients

The autosave's own write failure was already the `autosave` entity's `failed{reason}`. An
SPL meter's log append or rewrite that fails is now too: the write thread reports each log's
first failure and its recovery to the control thread, which keeps
`autosave.log_failures` (`{meas, log, reason, since}`, one per failing file) and commits it as
an ordinary `autosave` event. A full disk reads `disk full`. The app shows **LOG NOT SAVED ·
reason** on every pane (a warning under the measurement warnings: the data on screen is
valid, its copy on disk is not); `ac2 status` and `ac2 session status` print a line per log.
The entry goes when the file is written again (retried every 30 s) or its meter goes.

A rewrite's temporary file is removed when the write fails, so a full disk is not filled
further by the retries. The app's `ui.toml` stays dirty when its write fails and is tried
again every 30 s (one notification per run of failures).

The log files themselves (`ac2d.log`, `ac2-ui.log`) are capped at 16 MiB in a run: past it the
file moves to `.1` and a fresh one starts, so a repeating warning in a weeks-long run cannot
fill the disk the autosave needs.

## Power cuts

A standalone meter is switched off at the wall every night for years: any state file can
come back zero-length, torn half-way, full of NUL blocks (a filesystem that committed the
size before the data) or garbage. Neither program may refuse to start or panic on one.

Writes: every state file is written to a temporary file in the same directory, synced,
renamed over the target, and the directory synced (`ac2_paths::write_private_atomic` /
`write_atomic`, `ac2_traces::session::write_atomic`, the SPL log rewrite, the client key
files). Appended SPL and band logs (the session's and the rig meters') are synced every 5
min and at stop.

A cut append is not damage. What it leaves past the last whole row (a row torn anywhere,
mid-number or mid-field; zero blocks a file system extended the file with, possibly after a
torn row and with newlines among them; bytes that are not text) is the tail: every whole row
before it is read into the meter's history, a warning names the bytes left out, and the
tail is cut off before the next append, so no zeros stay mid-file. Only a log whose header
is unreadable is damage. A whole line of text that is not a row was written so and is an
error.

Reads, when a file is damaged:

| file | what happens |
|---|---|
| `rig.json`, a section unreadable (a meter, the outputs, the ceiling…) | set aside as `rig.json.damaged-<unix s>`; that section (or meter) alone is taken from the previous copy where it reads there, else left out; every other section is read and the file written again; the reason names each section lost |
| `rig.json`, `calibrations.json`, not readable at all | set aside as `<name>.damaged-<unix s>`; the previous copy (`<name>.prev`, the file before its last write) is read instead, else defaults; the store stays writable |
| autosave `session.json` | set aside; `session.prev.json` restored; both damaged: the directory is set aside and the daemon starts empty |
| a trace, or an SPL / band log without its header | that file alone is moved to `<autosave>.damaged-files/`; the trace is left out, the meter starts a new log or goes on without its band history |
| rig meters' log manifest | as the autosave; the rig meters (from `rig.json`) run either way |
| `working.json` | set aside; the session reads as untitled |
| network `server.key` | set aside, a new pair generated (clients must pin it again) |
| `authorized_clients` | the lines that parse are kept and the file rewritten with them, the original set aside |
| client `client.key` / `known_servers` (ac2-client) | a damaged pair is set aside and a new one made; a torn pin line is skipped |
| `ui.toml` | set aside, defaults; a notice in the app |

Each damage is logged as an error and, in the daemon, listed in `rig.damaged_files` until
the next start: the app shows FILE DAMAGED · what (an info banner, last), `ac2 status` a
`damaged` line per file. Another build's file (a newer format version) is not damage: it is
set aside under its version (`.v<N>`) as before. Set-aside files are never deleted by the
programs.
