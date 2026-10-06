# Audio that stops: detection and recovery

Status: implemented (protocol 21). Wire format: `docs/protocol.md` §4.1.1.

## What happened

On the pupu rig an RME Fireface 400 was reset; FireWire re-created the device, jackd kept
the old ALSA card open and hung. The daemon's JACK client then got no callbacks and no
error: the session stayed "open", measurements stayed "running", nothing was logged, and
the app only showed STALE frames, minutes old. When jackd was stopped, the host ended the
stream, the daemon tried one reopen ("No JACK server") and gave up with the session closed;
after jackd was restarted by hand a manual `ac2 session open` was needed
(`docs/rigs/pupu.md`, *Wiring and FF400 reset recovery*).

Two faults: a stream that stops without an error was not noticed, and a stream that ended
was not brought back.

## Detecting a stop

The capture fan-out already wakes every hand-off and knows when it last popped a block. A
session's audio is **stopped** when either

- the host ends the stream (JACK's shutdown callback, cpal's `StreamInvalidated` /
  `DeviceNotAvailable`; `EventSnapshot::ended`) — cause `host_ended`; or
- no block arrived for `max(1 s, 20 periods)` while the backend said nothing — cause
  `not_delivering`.

Why that bound: a device on its own clock delivers one block per period, so 20 missing in
a row is not a scheduling hiccup (an overloaded machine late by a few periods catches up
in a burst). The 1 s floor keeps short periods (2.7 ms at 128 frames / 48 kHz) from calling
a page fault an outage; for long ones (8192 frames / 44.1 kHz) 20 periods is 3.7 s, still
seconds. A stream whose blocks a test steps by hand (`Delivery::Stepped`, the fake's manual
drive) is never judged: its pauses are the test's. Backends state this in
`Negotiated::delivery`.

The fan-out reports the stop once (`ControlMsg::AudioStopped` with the wall time of the
last block, or of the open if none came) and the control thread never trusts that stream
again: blocks that arrive late cannot splice onto the audio before the gap.

## What a stop does

1. The stream is closed **on a thread of its own** (`ac2d-close`). Closing a JACK client
   deactivates it, which waits for the server; a hung server never answers. The control
   thread does not wait (it serves every client and the keepalives), and the audio path is
   not involved. Wherever else the control thread closes a stream (session close, a device
   change, shutdown) it waits at most 2 s, then lets the close finish on its thread.
2. Jobs stop, the sweep in progress is aborted, a raw recording ends (`audio_stopped`:
   the file ends with the last audio received; what comes after the reopen is never spliced
   onto it), the session's published frames are cleared. Measurements keep `running`: they are paused, not stopped, and restart with
   fresh averages when the audio is back (nothing averages across the gap; the sync
   contract's discontinuity rule, applied at session scope).
3. The generator is **disarmed** at once (principle 9: a reconnect never leaves outputs
   armed), with an audit entry by the daemon.
4. `session.stopped` is committed: `since`, `cause`, `recovery`. `session.open` keeps the
   session as last opened: the operator did not close it.

## Reopening

The same configuration (devices, channels, rate, buffer, loopback, generator routes) is
reopened attempt after attempt, each on its own thread (`ac2d-reopen`), because opening a
client of a hung server can block as well. The first attempt starts at once (after waiting
up to 2 s for the old stream to close, so a healthy host never has two clients of one
device); failures back off 1, 2, 4, 8, 16, then every 30 s — a device that comes straight
back is found within a second or two, and a rig left waiting for hours tries twice a minute.
There is no limit: the attempts end when one succeeds or a client sends `session.close` or
`session.open` (an attempt still running then closes what it opens). A session file loaded
meanwhile keeps the recovery, in the load's epoch.

A long wait is watched: once a failed attempt's wait is 4 s or more, the daemon looks at the
device every second (`Backend::probe`, on a thread of its own like the attempts) and starts
the next attempt at once when the device **comes back** — absent → present, or present
under a new generation — so a server restarted during a 30 s wait is reopened within about
a second, not up to 30 s later. The look opens nothing: JACK checks the server's socket
(`jack_<server>_<uid>_0` in `$JACK_TMPDIR` or `/dev/shm`, else PipeWire's), its inode and
change time as the generation (a restarted server makes a new one); cpal lists the host's
devices; a replay has no cheaper look (`Unknown`: the attempts alone). Only a change starts
an early attempt: a device that looks present all along (a stale socket, a hung server)
keeps the backoff, never one attempt per look; the first look comes right after the failed
attempt, so a device that returns at once is still seen arriving.

Each failure is logged once per backoff step (at the cap, again when the error changes or
every 10 attempts); the state carries `recovery: waiting {attempt, error, next_at}` with
the backend's own words, which say what it waits for ("No JACK server: start JACK …").
Starting jackd is not the daemon's job: the server is the operator's.

On success the session gets a **new epoch** (`stopped: nil`), the running measurements
restart as after a device change, the generator stays disarmed, and every client resyncs
by itself from the epoch change. An SPL meter's per-second log has no rows for the outage:
the run shows it as gap time (`LeqRun.gaps`, "offline" in the caption), never a splice.

A device change whose immediate reopen fails (`device_changed`) enters the same recovery
instead of closing the session.

## What the operator sees

- Banner (fault, right under DAEMON NOT RESPONDING, replacing STALE):
  `AUDIO STOPPED · device not delivering since 20:36`, detail
  `attempt 3 failed: No JACK server: start JACK … · next in 8 s · measurements paused`;
  an attempt with no answer for 5 s says so (`attempt 1 has had no answer from the audio
  host for 12 s`). In a narrow pane the headline drops its parts from the end
  (`AUDIO STOPPED`), like every banner, and never leaves its row.
- Curves and readouts under it: dimmed, tagged `audio stopped` (legend) and `AUDIO
  STOPPED` (SPL readout, Leq view) instead of a STALE age — the banner has the time.
- Top bar: the session item in fault colour, `audio stopped · reopening (attempt 3, next in
  8 s)`.
- `ac2 status`: `audio        STOPPED: device not delivering since 20:36; attempt 3 failed:
  … · next in 8 s`; `--json` carries `session.stopped`.

All texts come from `ac2_scene::audio`.

## Testing without hardware

The fake backend simulates both outages (`FakeBackend::stall`, `vanish`, `restore`, each
optionally timed): a stall stops callbacks without an error and makes opens wait (a hung
server); a vanish ends running streams through the host and refuses opens as unavailable.
A stream running when an outage began never delivers again, as a client of a reset device.
Tests, all from an empty daemon on the fake: stall → AUDIO STOPPED within the bound, the
control thread answering while an attempt hangs, the device back → same measurements, new
epoch, generator disarmed, SPL log gap (`crates/ac2d/tests/recovery.rs`); vanish → reopen
after the device returns; the device back in the middle of a 30 s wait → reopened within
2.5 s, with no early attempt while it stays away; `ac2 status` (`crates/ac2-cli/tests/recovery_rig.rs`); the app's
banner appears and clears without user action (`crates/ac2-ui/tests/embedded.rs`).

## Open

Only the rig can show how libjack behaves against the hung server: whether
`jack_client_open` returns (libjack's own timeout) or blocks until the server dies, and
how long the old client's close takes after `jackd` is killed. Both are off the control
thread either way. `session.devices` still enumerates on the control thread and would wait
on a hung server for as long as libjack's client open does.
