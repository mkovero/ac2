# Sessions: working state, named sessions, archives

What the operator saves and opens: the measurements (configurations, applied delays, running
flags), every stored trace with its display edits and slots, and each SPL meter's per-second
log. Not saved: calibrations (the calibration store's), the generator (a loaded session is
always disarmed), the audio device (the audio session). File format:
`ac2_traces::session` (a directory, `session.json` format 24). Wire: `docs/protocol.md`
§3.2 *Sessions*. User guide: *Sessions*.

## Model

```
               file.save (name | daemon path)            file.download_* ──► client file
 working state ───────────────────────────────► named session        (.ac2session)
 (daemon, autosaved) ◄─────────────────────────  (daemon dir)  ◄── file.import ◄── client file
               file.load (name | daemon path | upload)
```

- **Working state**: what the daemon holds now. It is autosaved continuously and restored
  at start, so it survives a restart, crash or reboot. It *belongs to* at most one session:
  `State.working.origin` — a name in the daemon's session directory, a directory on the
  daemon host (local clients), or a file on some client's machine (`client`, the path as
  that client wrote it; only that client can write it again). `None` = untitled.
- **Named session**: a directory in `ac2_paths::session_dir()` on the daemon host. Any
  client, remote included, lists (`file.list`), opens and saves them by name.
- **Archive** (`.ac2session`): a gzip-compressed tar of a session directory — nothing else
  (`ac2-archive`). It is how a session crosses machines: the app on ketunkolo opens one
  from a USB stick into the daemon on pupu, and exports the working state back to the
  stick. `tar tzf` reads it; a session directory copied by hand is the same thing unpacked.

## Unsaved changes

The daemon keeps a hash of the working state's *saved content* — measurement configurations,
delays, trace metadata — taken when the state was last saved or opened,
and compares the current hash after every change: `working.dirty`. Equal content is clean
again (an edit undone by hand). Not counted: running/stopped, the applied delay of a
measurement whose delay is tracking (it moves by itself), SPL logs (they grow
while the meter runs, and their ids are per daemon run). Untitled
state is dirty as soon as it holds anything.

The autosave writes `working.json` (origin, saved hash, saved time) beside its manifest, so
after a restart the header names the same session and "unsaved changes" is still relative
to its last save. An autosave without it (an older daemon's) restores as untitled, dirty
if it holds anything — never silently "saved".

## Nothing silently lost

- `file.new` / `file.load` refuse (`conflict`, detail `unsaved_changes`) while the working
  state is dirty, unless `discard: true`. The app asks *Save / Discard / Cancel*; the CLI
  takes `--discard`.
- Whatever replaced a dirty working state is kept as the **recovery copy** (beside the
  autosave as `<autosave>.replaced`, or in memory without an autosave): `working.recover`
  says what it is; `file.recover` swaps it back in (the state it replaces becomes the
  recovery copy, so recovering twice is a no-op).
- `file.save` onto an existing session other than the working state's origin refuses
  (`conflict`, detail `session_exists`) unless `overwrite: true`; the app asks.

## Transfer

Uploads and downloads are chunked over the control socket (a ctrl message is at most
4 MiB): `file.upload_begin {file, bytes, sha256}` → a transfer id; `file.upload_chunk
{transfer, offset, data}` in order, ≤ 1 MiB each; the SHA-256 is checked when the last byte
arrives. The upload is then used by `file.load {session: {type: upload}}`, `file.salvage`
or `file.import {transfer, name}` (store as a named session without opening it). Download:
`file.download_begin {session}` (none = the working state) packs and hashes it, chunks are
read with `file.download_chunk`, and `file.download_end {transfer, saved_as}` tells the
daemon the client wrote it, which makes that file the working state's origin and clean.

Limits: archive ≤ 64 MiB, unpacked ≤ 512 MiB and ≤ 20 000 entries, at most 4 transfers in
flight per daemon, a transfer idle for 2 min is dropped. Unpacking accepts only regular
files at `session.json`, `session.prev.json`, `traces/<file>`, `spl/<file>` — no links, no
absolute or `..` paths — so an archive cannot write outside its staging directory.

Remote clients still never name a daemon-host path (`refused`); a client's own files only
travel as archives through these commands.

## Old formats

`file.load` of another format version stays `unsupported` with the version named
(`session_version`); there is no migration. `file.salvage {session}` imports the trace
files of such a session (or of a current one) one by one through `trace.import`'s CSV
reader into the working state, and reports per file what was imported and what was refused
(a CSV of another version names it).

`ac2 session salvage` of a folder or file the CLI can read does more, on the client side
(`crates/ac2-cli/src/cmd/salvage`): it reads the old manifest as JSON, builds each
measurement configuration, saved-measurement field and trace edit in the current types —
whole when it fits, otherwise field by field onto this version's default of that kind, with
known moves (a format-23 `fft_len` becomes `resolution: fixed`) — and names every field it
dropped or defaulted. It then issues the ordinary commands (`meas.create`, `meas.colour`,
`meas.layout`, `delay.set`, `delay.track`, `trace.import`, `trace.update` with the restored
owner, math channels last on the restored ids, `meas.start` where the open session captures
the inputs). The daemon's loader keeps reading its own version only; a sweep (its level)
and a math channel are never rebuilt from defaults. A format change that moves a setting
adds an entry to `moved()` in `salvage/read.rs` and a fixture under
`crates/ac2-cli/tests/fixtures/salvage`.

## App

- **Sessions dialog** (Alt+S, the palette, or a click on the session name in the top bar):
  the current session's name and *unsaved changes*; New, Open…, Save, Save as…, Export…,
  Recover previous; **Recents** below, most recent first: the daemon's named sessions
  (from `file.list`, with counts and save time) merged with the files this app opened or
  exported (kept in the app's preferences), each opened with Enter or a tap.
- **Open…** has two sources: *On the daemon* (named sessions) and *This computer* — an
  in-app file browser (no native dialog: the Pi kiosk has none), big rows, directories,
  session folders and `.ac2session` files, with places for Home and removable media
  (`/run/media/$USER/*`, `/media/$USER/*`, `/media/*`, `/mnt/*`). A folder or file chosen
  there is packed if needed and uploaded.
- **Save** writes to the origin (a named session; a file of this machine is exported over
  again); untitled, or a file another machine wrote, asks Save as. **Save as** names a
  session on the daemon; **Export…** chooses a folder and file name on this computer.
- A refused open of an old-format session offers *Import its traces*.
- The embedded daemon (real audio) autosaves like `ac2d`, in its own directory
  (`<data>/autosave-app`, so a stand-alone `ac2d` on the same machine never shares it);
  the simulated rig stays in memory. Quitting with an embedded daemon and unsaved changes
  asks Save / Quit without saving / Cancel; the working state comes back at the next start
  either way.
