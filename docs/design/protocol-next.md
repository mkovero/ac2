# Protocol next: edit tokens, roles, big state, frame meta, log keys

Status: decided, not built. Everything here lands under one protocol version
(`PROTO_VERSION` 104 → 105) and one bump of each file format it touches; there is no reader
for the shapes it replaces. File version numbers below are counted from `main` at 5c95a4a3;
a branch that bumps one of them first moves these up by one.

## Problem

Several parts of the protocol carry two jobs in one field or have no way out past a limit:

- A measurement's `config_rev` is both the token frames carry ("the configuration this
  frame was computed under") and the edit token `meas.update` is guarded by. A display-only
  change commits without moving it, so a concurrent whole-config write silently undoes it.
  Every delay step bumps it and resends the whole measurement, EQ sets included.
- `expect_rev` compares against the store's rev, which telemetry moves several times a
  second; it guards nothing in practice.
- Every authorized network client holds full control of the rig.
- On Windows the local pair is loopback TCP: any local process of any user is the operator.
- A snapshot over 4 MiB is refused and the client never syncs; an event over 1 MiB goes as
  `resync_required` but is still held for replay.
- Uploads are held in memory and capped at 64 MiB, below what a download of the same
  session can be.
- Per-frame values sit in static frame metadata, so whole kinds never go as deltas, and
  per-source loudness in `meta` overflows the 8 KiB header on a large interface.
- SPL and band logs are keyed by wall time, which a boot before the clock is set or an NTP
  step makes overlap or run backwards.

## Design

### 1. Measurement edit token and the delay entity

**Edit token.** `Measurement` gains `edit_rev: Rev`: the store rev of the last commit of
this measurement entity, whatever changed (configuration, display, name, colour, offset,
layout, EQ sets, running). `Store::commit` stamps it on the `measurement` set before it
clones the change into the event and the replay queue, so no command path can forget it, and
every reply that carries a measurement reads it back from the store after the commit (never
the clone made before it), so the caller's next `base` is current. `meas.update.base` is
compared against `edit_rev`; a mismatch is `conflict` whose detail (`stale_meas`) carries the
current `edit_rev`. The commands that change one field (`meas.colour`, `meas.offset`,
`meas.layout`, `meas.eq` other than `put`) take no base and are last write wins; they still
move `edit_rev`, so a whole-config write prepared before them is refused and re-read rather
than replayed over them.

Every `base` taken today from `config_rev` moves to `edit_rev`: in the daemon the internal
`meas_update` call (`control/walks.rs`) and the `MeasUpdate` that `control/leq.rs` executes
for a band transfer; in the CLI `cmd/leq.rs`, `cmd/osc.rs`, `cmd/bands.rs`, `cmd/math.rs`,
`cmd/meas.rs`; in the app `leq_dialog.rs` (construction and `LeqDialog::rebase`),
`state/queries.rs` and `state/stimulus.rs`. The paths that compare frame stamps keep comparing
`config_rev`: in the app `conn/pacing.rs`, `state/glide.rs`, `state/walk_bar.rs`,
`state/leq.rs`, and the bins-to-`tf` match in `ac2-scene` `tf/bins.rs`.

Because every commit of the measurement moves `edit_rev`, a second edit built from the mirror
before the first one's event arrives (a key repeat, an offset drag followed by a configuration
key) would be refused. The app keeps one edit in flight per measurement: the next edit waits
for the reply of the one before and takes its `base` from the `edit_rev` that reply carries,
not from the mirror. A `stale_meas` refusal then means another client or the daemon committed
the measurement in between; the app shows it and drops that measurement's queued edits.

**`config_rev`** keeps exactly one meaning: the rev of the last commit that changed what the
measurement's job computes from its configuration. It no longer moves for a delay change.
Display-only and owner-only edits leave it, as today.

**Delay entity.** Delay state leaves `Measurement` (the field `delay` goes) and becomes:

| field | type | meaning |
|---|---|---|
| `meas` | `MeasId` | key; a transfer measurement |
| `rev` | `Rev` | rev of the commit that last moved `applied` or `nudged`; for a delay created with its measurement, that measurement's `config_rev` |
| `applied` | `Seconds` | as today |
| `applied_samples` | `f64` | as today |
| `nudged` | `Seconds` | as today |
| `nudged_samples` | `f64` | as today |
| `tracking` | `bool` | as today |
| `awaiting_pick` | `bool` | as today |

It is fixed in size (about 80 bytes of msgpack), so a tracking step costs one small event.
State gains `delays: Vec<Delay>` sorted by `meas`; the event kind is `delay`
(`Patch<Delay, MeasId>`). The last finding, up to 16 candidates, changes only on
`delay.find`, `delay.set` (typed) and a cross-rig relock, so it becomes a keyed entity of its
own: `DelayFinding` gains `meas: MeasId`, state gains `delay_findings: Vec<DelayFinding>`,
event kind `delay_finding`, deleted when a typed value drops it. A finder result that pauses
tracking commits two events at consecutive revs: `delay` (`awaiting_pick`) then
`delay_finding`.

`delay.insert`, `delay.set`, `delay.nudge` and `delay.track` reply with the new `delay` body
(`ReplyBody::Delay`); `delay.find` keeps its `delay_finding` reply. A transfer measurement
coming into being commits `measurement` then `delay`; one going away (or ceasing to be a
transfer) commits deletes of `delay_finding`, `delay` and `measurement` (or the measurement's
new set) in that order. That covers `meas.create`, `meas.delete`, `meas.update` changing
kind to or from transfer, and `file.new` / `file.load`, which commit each measurement's
entities in the same order. Between the two events a client may hold a transfer measurement
without a delay, or a delay whose measurement is gone; mirror and app read a missing delay as
zero and not settled, and ignore a delay without its measurement.

A delay created with its measurement (`meas.create`, a kind change to transfer, a load) takes
`rev` = that measurement's `config_rev`, so `max(m.config_rev, delay.rev)` is the same before
and after the `delay` commit, and a job started ahead of both commits (as `meas.update` starts
a running measurement's job before it commits) stamps a value that settles. `rev` moves only
when `applied` or `nudged` move; `tracking` and `awaiting_pick` commit `delay` without moving
it.

**Saved changes and recordings.** `delay` is a saved change as `measurement` is: it marks the
working session changed (the unsaved-changes `conflict` on `file.new` / `file.load`) and
reaches the autosave. `delay_finding` is not: a finding is never saved and a load starts
without one. The raw-capture timeline gains `TimelineChange::Delay(Patch<Delay, MeasId>)`
beside `Measurement`, whose value no longer holds the delay; findings are not in the timeline.
The timeline is a record, never read back as tokens: the measurement in it carries `edit_rev`
as committed and nothing compares it.

**Frames.** The header field stays `config_rev`; its value is the newer of the measurement's
`config_rev` and its delay's `rev` that the frame was computed under. Both are store revs, so
one comparison per entity tells a client what a frame reflects: the configuration when
`stamp.config_rev ≥ m.config_rev`, the delay when `stamp.config_rev ≥ delay.rev`. A job
starts with `max(m.config_rev, delay.rev)` (`job_ctx`, for every start: `meas.start`, a
stream reopen, a file load), and `SetDelay` carries the delay's rev as it carries `rev`
today. The daemon's own equality test of a settled capture (`settled` in
`control/traces.rs`) becomes `stamp.config_rev == max(m.config_rev, delay.rev)` and reads
`awaiting_pick` from the delay entity. `leq_reported` (`control/leq.rs`) is SPL only and
keeps comparing `config_rev`. A transfer's `bins/<meas>/<stage>` frames carry the stamp of
the `tf` emit they belong to (`jobs/bins.rs`), so the same rule holds for them.

**Mirror and session file.** `apply_change` gains `Delay` and `DelayFinding` upserts keyed
by `meas`; deletes arrive as events. The saved measurement already keeps its delay apart
(`SavedDelay`), and `edit_rev` is never saved in a session: a load commits every entity
afresh.

### 2. Retiring `expect_rev`; `base` on `meas.eq put`

`Request.expect_rev` goes, with `Client::call_expect`, the `expect_rev` argument of
`call_warned` / `call_with`, and `Command::is_mutation`. Dedup today stores the reply of
every mutation and of the reads that act, `retry` in `control/request.rs` (built on
`is_mutation` and a list of its own). `pn-edit` replaces both with one exhaustive
`Command::is_read`, true for the pure reads only: `hello`, `state.snapshot`, `state.since`,
`grid.get`, `stream.status`, `trace.list`, `trace.get`, `dynamics.table`,
`dynamics.envelope`, `cal.list`, `spl.log_get`, `spl.history_get`, `spl.band_log_get`,
`file.list`, `rec.list`, `plan.list`, `server.info`, `source.state`,
`file.download_chunk`. Their replies are not stored and a retry executes them again.
Everything else keeps its stored reply, including the reads that act: `stream.devices`
(drivers load), `stream.preview`, `stream.preview_stop`, `stream.control_panel`,
`stream.detect_loopback`, `delay.find`, `trace.export`, `file.upload_begin`,
`file.upload_chunk`, `file.import`, `file.download_begin`. So a retried upload chunk, import,
device scan or loopback burst is answered from the store, never run twice. The behaviour is
today's; only the predicate is new.

`ErrorCode::Conflict` stays: it is the answer to a stale `base` (`meas.update`, the `rig.rev`
commands, `meas.eq put`) and to unsaved changes on `file.new` / `file.load`.

`EqEdit::Put` gains `base: u32`, the set's `rev` the edit was made on. A mismatch is
`conflict` with detail `eq_conflict {set: EqSetId, rev: u32}`. `trace.update`,
`stream.inputs`, `walk.update` and `plan.save` stay whole-value writes and are documented as
last write wins.

### 3. View-only role

**Roles.** `ClientRole` is an enum tagged by `type`: `operator`, `viewer`, `rig {rig_id:
RigId}`. It replaces `AuthorizedClient.rig: Option<RigId>` with `role: ClientRole`.

**Where it lives.** Every line of the authorized-clients file names its role as a third
field: `operator`, `view` or `rig=<16 hex>`. A line without one, or with anything else, is
malformed: as today it is dropped with its reason logged and the file rewritten without it,
so a lost or truncated field locks that client out rather than granting it control. The
one-shot deploy converter adds `operator` to today's two-field lines.

`auth.authorize` (today `server.authorize`) takes `role: ClientRole`, typed and required;
`auth.admit_rig` stays the only way to `rig`, and `auth.authorize` refuses `rig`
(`invalid`), and refuses a name that is admitted as a rig (`invalid`), so a rig key never
becomes an app; the rig is revoked with `auth.revoke_rig` first. Authorizing a name already
present otherwise replaces its key and role, as a name given again replaces its key and kind
today, and drops the old key's connections when either changed; there is no `auth.set_role`.

**Demotion and revocation.** Demoting an operator to viewer does what `auth.revoke` does to
what it holds: its generator lease is released and its stimulus stopped at once (fading out),
with the sweep or dynamics run it started; its transfers are dropped (§5). It is refused its
next command outside the viewer set. So a viewer never holds the generator lease, and no
lease path needs a role check.

**What each role may send.** `Command::allowed(&self, role: &ClientRole) -> bool`, one
exhaustive match without a wildcard, replaces the hand-written rig gate and the source-state
gate in `request.rs`; a new command must decide for every role. An operator may send every
command except `source.state`. A rig may send exactly `hello` and `source.state`. A viewer
may send exactly:

`hello`, `state.snapshot`, `state.since`, `grid.get`, `stream.status`, `trace.list`,
`trace.get`, `trace.export`, `dynamics.table`, `dynamics.envelope`, `cal.list`,
`spl.log_get`, `spl.history_get`, `spl.band_log_get`, `file.list`, `rec.list`, `plan.list`,
`server.info`, `gen.stop`, `file.download_begin` (with `session` nil: the working session
only), `file.download_chunk`, `file.download_end` (with `saved_as` nil: a viewer's export
never changes `working.origin`).

`gen.stop` stays universal: anyone who hears sound may stop it. It does all it does for an
operator, so a viewer's stop also aborts a running sweep or dynamics run and disarms the
generator (open question 8). A viewer holds at most one transfer at a time (`trace.export`
or a download; a snapshot transfer aside). Everything else is refused, including the
commands that read but act on the rig: `stream.devices` (drivers load), `stream.preview`,
`stream.preview_stop`, `stream.control_panel`, `stream.detect_loopback`, `delay.find`,
`file.upload_*`, `file.import`. On the data socket a viewer may subscribe to every topic
except `audio/<channel>` and `source/levels` (rigs only, as today, the gate in `peers.rs`),
`bins/<meas>/<stage>` included, and sends credit and keyframe requests as anyone.

**Refusal.** A viewer's refusal is `refused` with detail `view_only {}`; a rig's keeps its
message. Both are checked in `request.rs` before dedup admission, so a refused request stores
nothing.

**State.** `welcome` gains `role: ClientRole`: the app greys its controls from it.
`server.info`'s `authorized[]` shows each client's role. A role change drops the key's
connections as a revoke does, and a command still in flight is checked against the new role
(a viewer's is refused `view_only`); the client reconnects and learns its role from
`welcome`.

**Local pair.** The local endpoints (ipc on every platform, §8) are always `operator`: the
OS user is the boundary. `server.network` and the `SessionRef.path` forms stay reserved to
them, as today. `auth.*` is not: a network operator may authorize, demote and revoke clients
and admit or revoke rigs, as it may today; a viewer and a rig may send none of it.

### 4. Oversize snapshots and events

**Snapshot.** `state.snapshot` encodes its `StateSnapshot`; at or under `MAX_CTRL_BYTES` it
replies `snapshot` as today. Over it, the encoded bytes are held as a download and the reply
is `snapshot_transfer {rev: Rev, transfer: Transfer}`, where `transfer.file_name` is
`state-<rev>.msgpack` and the bytes are the msgpack `StateSnapshot` the inline reply would
have carried. The client fetches with `file.download_chunk`, checks `sha256`, ends with
`file.download_end` (nil `saved_as`) and hands the decoded snapshot to the mirror, which
already buffers events newer than the snapshot it awaits. The fetch runs inside `run_sync`;
it publishes the connection's liveness after every chunk, so a slow link's fetch is not read
as daemon silence.

A snapshot transfer stays in memory, outside every transfer quota (§5, and today's
`MAX_TRANSFERS`). Its bytes are shared by every client that asked at the same rev; a client
holds at most two, a third `state.snapshot` replaces the older, and each is dropped 30 s after
its last chunk (or its reply, if never fetched). `state.snapshot` is a pure read and a retry
runs it again; holding two keeps the first reply's transfer valid when a retry ran it twice
across a rev change. Memory is the distinct revs asked for in 30 s times the state's size, in
practice one.

`state.since` whose reply would exceed `MAX_CTRL_BYTES` answers `resync_required`; the client
takes a snapshot, which goes the transfer path if it must. No entity is capped.

**Events.** An event over `MAX_EVENT_BYTES` is committed (state moved, rev taken) and every
connection taking events is sent `resync_required` with its next message; the snapshot that follows
holds the value, as today. The event is no longer pushed to the store's replay queue: the
store records its rev as not replayable, so a `state.since` spanning it answers
`resync_required` rather than holding an event no client can take. A maximal finished
`sim.run` is the known case; it is rare and the cost is one snapshot per client.

### 5. Uploads spooled to disk

- **Spool.** An upload is written chunk by chunk to `<sessions>/.incoming/up-<transfer>.part`
  and hashed as it arrives; chunks must arrive in order. A chunk whose `offset` equals the
  bytes received is appended; one whose `offset + len` equals the bytes received and whose
  bytes match the tail already written replies `ack` and changes nothing (a retry past the
  dedup window); anything else is `invalid`. The last chunk checks `sha256`. Downloads are
  packed to `.incoming/down-<transfer>.ac2session` the same way, so no transfer is held in
  memory except a snapshot. The staging directory is cleared at start, as today.
- **Cap.** `ac2_archive::MAX_ARCHIVE_BYTES` becomes 1 GiB and `MAX_UNPACKED_BYTES` 4 GiB, one
  limit for both directions. `file.upload_begin` refuses (`refused`) when the session
  directory's file system has less free space than `bytes` plus 1 GiB; the unpack checks
  again against the archive's declared unpacked size, so no upload fills the disk the logs
  and the autosave write to. `file.download_begin` and `trace.export` need only the bytes
  they are about to pack plus 64 MiB, checked before packing: a meter whose card is nearly
  full can still have its working session taken off, the case a download is most needed.
- **Quota.** At most 2 transfers per peer (name and key, or the local connection, as dedup
  keys them; two connections with one key share them), 1 for a viewer, and 8 per daemon;
  over either is `refused` with the count in the message.
- **`file.upload_abort {transfer: TransferId}`** replies `ack`, deletes the spool file and
  frees the slot. A download ends with `file.download_end` as today. A `trace.import`
  refused with `pick_channel` or `bad_options` keeps its upload; the client retries the
  import on the same `transfer` id, and sends `file.upload_abort` when the operator cancels.
- **Ending.** A peer's transfers are dropped, spool files deleted, after 2 minutes without a
  chunk, on `file.upload_abort`, and when the peer is revoked or demoted. The daemon has no
  signal for a ctrl connection's end, so the idle timeout is what frees a vanished client's
  slots.
- **Dynamics tables.** A `trace.import` of a dynamics export is bounded by the upload cap and
  the dynamics reader's own table limits, which follow the largest run `dynamics.run` can
  make; the 65 536-row trace cap applies to curve tables only.
- **Deleting.** `rec.delete {recording: RecordingRef}` (refused while that recording is
  written or replayed), `file.delete {name: String}` and `file.rename {name: String, to:
  String}` for saved sessions (by name only; refused for the session a file job is using;
  renaming the session `working.origin` names updates the origin).

### 6. Per-frame metadata and per-source arrays

**Arrays of their own length.** `ArrayDesc` gains `len: u32`, required: the element count
of that array. `n` stays the frame's column count; curve arrays have `len = n`, per-pair and
per-source arrays their own count. The decoder refuses a frame unless every array's `len`
equals the count its name belongs to (`n`, `meta.pairs.len()` or `meta.sources.len()`), every
`len ≤ MAX_N`, and the payload is Σ `len` × element size; only then is any buffer sized from
it. Every `array_size(d.elem, n)` (`frame/wire.rs` encode, delta encode, delta decode and
header size; `frame/decode.rs`) becomes `array_size(d.elem, d.len)`.

**Levels (`stream_levels`, `preview_levels`, `source_levels`).** Per input (`len` = inputs):
`ppm`, `max`, `true_peak`, `overs`, `peak`, `rms`, `clip` as today; `ppm` and `max` are
declared `dbtp` under `momentary` / `short_term` and `dbfs` under a peak detector. Per pair
(`len` = `pairs.len()`): `pair_corr` (`f32`, unit `correlation`, NaN while a side is under the
gate), `pair_quiet` (`u32`, the raw `Elem::U32`, unit `side`: 0 none, 1 left, 2 right;
`Elem::U8` is the coherence coding and is not used). Per loudness source (`len` = sources):
`lufs_m`, `lufs_s`, `lufs_m_max`, `lufs_s_max` (`f32`, unit `lufs`, −∞ as today), `lufs_i`
(`f32`, `lufs`, NaN for none), `lra` (`f32`, `lu`, NaN for none), `tp_max` (`f32`, `dbtp`),
`loud_s` (`f32`, `seconds`, metered time since the reset). New units: `correlation`, `side`,
`lufs`, `lu`. `meta` keeps only what changes on a configuration change: `channels`,
`ballistic`, `pairs: Vec<Pair>` (left, right) and `sources: Vec<LoudnessSource>`, each source
with its `since: WallNs` (it moves on a reset, which may go as a keyframe). The header no
longer grows per source by more than a pair of channel numbers. `source_levels` carries the
same arrays; its meta keeps `sample_rate_hz` and the timing `mark` beside the fields above.

**Delta meta.** `PerFrame` covers every kind whose metadata moves every frame; a delta's
`meta` slot carries:

| kind | per-frame part |
|---|---|
| `tf` | `filled`, `settling` (today), `delay`, `nudged`, `math.align: Option<AlignState>` |
| `bins` | `blocks`, `eff_avg` (today), `delay`, `nudged` |
| `spl` | as today |
| `spec` | `ages: Vec<Option<Seconds>>` (one per band of `bands`), `harmonics` |
| `stream_gonio`, `preview_gonio` | `gains: Vec<f32>` (dBFS, one per pair) |
| `stream_levels`, `preview_levels` | nil (everything moved into arrays) |
| `source_levels` | `mark` (today) |

Everything else in those kinds' meta (`TfMeta.bins` included) is static; a change of it
still goes as a keyframe.
`leq` and `band_leq` stay keyframe-only.

**Delta stamp.** The delta header carries `config_rev` and `config_applied_at` after
`protection`, and the decoder takes them from the delta instead of the base. `fits` no
longer compares them, so a delay step (which moves `stamp.config_rev`, §1) continues the
chain with the new `delay` / `nudged` in the per-frame meta; a configuration change still
breaks it through `grid_id`, `n`, the arrays or the static meta. The delta header ends
`meta, crc` as the keyframe does.

### 7. SPL log v3 and band log v2

**Key.** A log row is keyed by `second: u64`, a per-log index that only rises. Within a
*run* (one stretch of a meter's job on one open stream) the index advances by the seconds the
audio clock counted, so a capture gap shows as missing indices and a wall-clock step changes
nothing. Each run has a number, `run: u32`, rising within the log. An empty log starts at
`second` 0, `run` 0.

**Boot clock.** Every distance below is taken on a clock that counts suspend, never on
`Instant` (which stops during suspend on Linux and macOS): `CLOCK_BOOTTIME` on Linux,
`mach_continuous_time` on macOS, `QueryInterruptTime` on Windows. A laptop that sleeps for
hours and reopens its stream in the same process sees hours, not seconds. Inside a run the
job compares it with the audio clock at each row: when the boot clock advanced more than 2 s
further than the audio clock since the row before (a suspend or stall the backend did not
report as a discontinuity), the run ends there and a new run starts with the gap the boot
clock measured.

**High-water mark.** Beside each log the daemon keeps `<log>.mark` (atomic write), holding the
highest `run` started and an anchor `(boot id, run, index, boot-clock seconds)`, where the
index is reserved ahead of the last row served. The boot id is the kernel's
(`/proc/sys/kernel/random/boot_id`) on Linux; on macOS and Windows each daemon process
writes a fresh random one, so a new process counts as a new boot there. The anchor is written
at each run start and at each reservation (index + 600); a clean stop writes it exact, at the
last row served. Within a run the boot-clock reading of any row follows from the anchor and
the row's index, so the last row's reading is known without a column for it. The mark
travels with the log (session folder, salvage).

The log's write thread writes the mark, never the job thread or a holder of the shared log
lock, since an fsynced atomic write would stall the once-a-second row. The job asks for the
next reservation a minute before the index reaches the stored mark, and serves a run's first
row only once the write thread has written that run's anchor or reported that it could not.
So a `(run, second)` a client was served is never reused by a different row, whether or not
the row reached the disk.

- **Missing, unreadable or below the log.** A mark that is absent, does not parse, or lies
  below the log's last row leaves the count of rows served but never written unknown. The
  meter starts anyway; the log takes a new `started_at` (so every client drops the rows it
  cached, *Wire* below), the run starts at `last + 1` with gap `unknown`, and the fault is
  logged.
- **Failed write.** A mark that cannot be written (a full or read-only card) stops nothing:
  the meter keeps logging and serving, the uniqueness above is given up until a write
  succeeds, and the fault is logged once per failing stretch.

**Across runs.** Within one daemon process a new run's first index is `last + d`, where
`last` is the last index issued and `d` the whole seconds the boot clock counted between that
row and the run's start; the gap is `known`. The process knows what it served, so a stream
reopen neither jumps the index nor writes the mark. The first run of a new process starts at
`max(mark + 1, last + d)`, where `last` is the log's last row and `d` is taken from:

- the boot clock, when the mark's boot id is the current one: the anchor gives the last row's
  reading; the gap is `known`;
- across boots, the wall-clock distance when both that row's wall time and now are trusted
  (below); the gap is `known`;
- otherwise, or when the distance is ≤ 0, `d` = 1 and the gap is `unknown`.

When `mark + 1` wins over a known `last + d` (after a crash, which left a reservation), the
index overstates the gap by at most the reservation; that only ages old rows out of the
windows sooner and never shows a level as more recent than it is, so the gap stays `known`.
Retention stays by row count.

**Wall time as annotation.** Each row keeps `start_ns` (and `start_utc`) as read, and a
`clock` column: `synced` (the OS held the clock synchronised), `unsynced`, `derived`, or
`not_stated` (the OS says nothing: macOS, Windows, a failed `adjtimex`). `synced`, `derived`
and `not_stated` are trusted for the distance across boots; a desktop OS keeps its clock by
network time without saying so, and a standalone meter runs on Linux, which states it. When
the clock becomes synchronised during a run, the rows of that run logged unsynced are
re-stamped from the boot-clock distance to the sync point and marked `derived`. A wall-clock
step in a run changes the annotation of later rows only. Rows re-stamped this way were
already served as `unsynced` under the same `started_at`: a client that holds `unsynced` rows
of a run and is served a `synced` or `derived` row of that run fetches the run again.

**Windows.** Rolling windows rebuild from the rows whose index lies within the window before
the current index, never across an unknown gap: after a power cut with an unset clock the
windows start over from the first second of the new run. A stream reopen in the same process
(device recovery, configuration change) is a known gap of the boot-clock seconds it took, so
the windows carry over it as they do today, and a suspend ages them by its length. A rebuilt
window never presents a level of unknown age as recent, which is what keeps a standalone
meter from showing a stuck value after a cut. Peak judgement on restart takes the newest
`PEAK_HOLD_S` indices under the same rule.

**Zone.** `Rig` gains `zone: TimeZone`, where `TimeZone` is `{name: Option<String>,
utc_offset_s: i32}`, read from the OS only: the IANA name when the host has one, nil when the
OS gives only an offset (a POSIX `TZ` rule), and the offset in force now. The daemon reads it
at start and once a minute, and commits `rig` when either part changed (a DST change, a host
reconfigured). Both CSV headers gain `# zone: <name or ±hh:mm>`; band log rows gain
`utc_offset_s`, so a logged `period` can be checked afterwards. Re-stamping a row to
`derived` does not re-judge its `period` or change its `utc_offset_s`.

**Band period without a trusted clock.** Day and night limits are judged in that zone by the
clock as it stands, except while the clock is `unsynced` and the host states no RTC. Then the
time of day is not known: a meter with no RTC and no network, cut every night, restarts its
clock from `fake-hwclock`'s last save and falls behind by each night's off time. In that
state the band period and its limit verdict read "can't tell", in the rows (`period` empty,
`BandLogRow.period` and the band frame meta's `period` / `period_after_horizon` nil) and on
the meter; the levels are logged and shown as ever. Rows re-stamped to `derived` keep their
"can't tell". `Clocks` gains `rtc: bool`, which the daemon reads at start: on Linux, true
when some `/sys/class/rtc/rtc*/hctosys` reads 1 (the kernel set the system clock from that
RTC at boot); on macOS and Windows always true, since every host they run on keeps its clock
in a battery-backed RTC.

**Columns.** SPL log v3:
`second,run,start_utc,start_ns,clock,gap,measured_s,unit,laeq_1s,lceq_1s,lzeq_1s,lcpeak_1s,lafmax_1s,sensitivity_db,position_db,position_peak_db`
where `gap` is `known` or `unknown` on a run's first row and empty on the others, with
headers `# ac2 spl log v3`, `# meas`, `# name`, `# input`, `# mic`, `# zone` and `# started:
<utc>`. Band log v2 adds the same `second`, `run`, `clock` and `gap` columns, and
`utc_offset_s` before `period`.

**Wire.** `SplLogRow` gains `second: u64`, `run: u32`, `clock: LogClock` (`synced`,
`unsynced`, `derived`, `not_stated`), `gap: Option<LogGap>` (`known`, `unknown`);
`BandLogRow` the same plus `utc_offset_s: i32`, and its `period` becomes
`Option<BandPeriod>` (nil: can't tell). `SplLogPage` gains `started_at: WallNs`, so a
downloader that sees it change mid-download restarts. `HistorySpan::Range` takes `from: u64,
to: u64` as second indices and keeps `step_s`; its buckets are the whole multiples of `step_s`
on the index (`second / step_s`), not on Unix time. `SplHistory` gains `second: Vec<u64>`
beside `at`, and `log_start` / `log_until` become `first_second` / `last_second:
Option<u64>` with the wall annotations beside them. `spl.history_get` has one cap,
`SplHistory::MAX_POINTS = 14 400`, for `Newest.seconds` and for a range's bucket count, and
refuses (`invalid`) above it; `MAX_SECONDS` goes.

**Salvage.** `file.salvage` also reads `spl/*.csv` and `*.bands.csv` of the current log
versions in a session of another format version. Each recovered log becomes a stopped SPL
measurement of session scope, named `<name> (salvaged)`, default configuration on the
logged input, holding the log as its current log. `Salvage` gains `spl_logs: Vec<MeasId>`;
a log of another version is reported in `refused` and stays on disk as plain CSV.

### 8. Local pair on ipc everywhere

`Listen::default_local` returns `ipc://<runtime dir>\ctrl.sock` and `data.sock` on every
platform; `LOCAL_PORT` goes. The runtime directory on Windows is `%LOCALAPPDATA%\ac2\run`
(per-user ACL by inheritance), kept short because `sun_path` holds 108 bytes. `ac2-zmq`'s
build defines `ZMQ_HAVE_IPC` and `ZMQ_HAVE_STRUCT_SOCKADDR_UN` for Windows targets if
`zeromq-src` does not, and `links_libzmq_4_3_5_with_curve` asserts `libzmq_has("ipc")` on
every platform. Windows 10 1803 is the floor (AF_UNIX). `Listen::validate` refuses loopback
TCP as a local endpoint. There is no second local transport: the Wine runs
(`tools/wine-asio`) use ipc if CI shows Wine's AF_UNIX works, else they run the daemon in
network mode.

### 9. Mechanical and small changes

| old | new | crates |
|---|---|---|
| `session.devices`, `.preview`, `.preview_stop`, `.control_panel`, `.detect_loopback`, `.open`, `.close`, `.status`, `.inputs`, `.outputs`, `.replay` | `stream.*` (same suffixes) | proto, ac2d, client, cli, ui, e2e, tools/protocol |
| `State.session`, event kind `session`, `Session`, `SessionConfig`, `SessionEpoch`, `session_epoch` (header, welcome, snapshot) | `State.stream`, `stream`, `Stream`, `StreamConfig`, `StreamEpoch`, `stream_epoch` | same + traces (sidecar) |
| `StandingSession`, `StandingRig.session`, `CantTellReason::NoSession` (`no_session`), `SourceSession`, `SourceState.session`, `RemoteAudio::NoSession` (`RemoteInputStatus.audio` `no_session`) | `StandingStream`, `StandingRig.stream`, `NoStream` (`no_stream`), `SourceStream`, `SourceState.stream`, `RemoteAudio::NoStream` (`no_stream`): one thing, one name | proto, ac2d, scene, ui, cli |
| frame kinds `session_levels`, `session_gonio`; topics `session/levels`, `session/gonio`, `session/preview`, prefix `session/` | `stream_levels`, `stream_gonio`; `stream/levels`, `stream/gonio`, `stream/preview` (the `stream/` input-meters prefix covers the dialog preview), `stream/` | proto, ac2d, client, ui, plot |
| `Result` as `{"Ok"}` / `{"Err"}` | `{"ok"}` / `{"err"}` (snake_case); `CtrlError::MissingVersion` never reaches the wire: a request without `v` is answered `version_mismatch` | proto, tools/protocol |
| tag keys `kind` (`InputKind`, `Surface`), `state` (`Recovery`), `shape` (`TargetCurve`), `status` (`BandTransferBand`) | `type` | proto, traces, ac2d (calstore, plans), ui |
| `TraceEdit.color` | `colour` | proto, ui, cli |
| `cal.spl.input`, `cal.spl_electrical.input`, `cal.curve_import.input` | `channel: u16` (every `cal.*` names the input by `channel`, as `CalKey`; CLI flags stay `--input`, the operator's word) | proto, ac2d, cli, ui |
| `rig.unpair.rig` | `rig_id: RigId` | proto, ac2d, cli, ui |
| `server.authorize`, `server.revoke` | `auth.authorize`, `auth.revoke` (the CLI keeps `ac2 server authorize`, beside `server info`, which lists the refused keys it takes) | proto, ac2d, cli, ui |
| `rig.monitor_start {base, on}` | `rig.monitor_at_start {base, on}` | proto, ac2d, cli, ui |
| `sample_rate: Hz` / `u32` (sweep info, `ir` meta, dynamics, trace, sidecar), `BinsMeta.rate`, `AudioHeader.rate` (`audio/<channel>`), `EqRealisation::Digital.rate`, `EqShape::Biquad.rate` | `sample_rate_hz`; trace export v7, whose reader takes a v6-or-older JSON header line's `sample_rate` / `lo` / `hi` as `sample_rate_hz` / `lo_hz` / `hi_hz` (open question 1) | proto, traces, ac2d, ui, scene |
| `AlignBand.lo/hi`, `FreqRange.lo/hi`, `BinRange.lo/hi` | `lo_hz`, `hi_hz` (`LevelRange.lo/hi`, a dB axis of the layout, and `MicCurveRef.f_lo/f_hi` stay) | proto, core conv, ui, scene |
| `NotLockedReason::OtherRate {theirs, ours}` | `{theirs_hz, ours_hz}` | proto, ac2d, scene |
| `sim` event setting `State.sim_map` | event kind `sim_map: Option<SimState>` (`State::set_sim` goes) | proto, ac2d, client |
| delta header `…, crc, meta` | `…, config_rev, config_applied_at, …, meta, crc` (§6); `pn-shapes` swaps the tail to `meta, crc`, `pn-frames` adds the stamp fields | proto, tools/protocol |
| `RigPeer.data` | dropped; data endpoint is ctrl port + 1 (`RigPeer::data_endpoint`; `rig.pair` refuses a `ctrl` that is not `tcp://host:port` with port 1 … 65534) | proto, ac2d, ui, cli, rig.json |
| `trace.get` dynamics envelopes as points | parallel `f32` arrays as `dynamics.envelope`: `DynamicsData.envelopes` per input, per step a `DynamicsEdges {start, end}` of `DynamicsEdgeCurve`; `DynamicsPoint` and `DynamicsEdge` go; the CSV export keeps its envelope table | proto, ac2d, traces |
| `ErrorDetail::Version {daemon, client}` | dropped (the refusal's own `v` is the daemon's version); client decode of it dropped | proto, client |
| `stream.detect_loopback.level: Dbfs \| nil` | `level: Dbfs` | proto, ac2d, cli, ui |
| `sim.run` / `sim.fit` busy `invalid` | `refused` | ac2d |
| `#[serde(default)]` on `CalEntry.loading`, `PaneLayout.levels`, `.plates` | required | proto |
| `ppm`, `max` declared `dbfs` under loudness ballistics | `dbtp` (§6) | ac2d jobs/meters |
| `WallNs`, `RigId`, `GridId` in JSON files | decimal strings when `is_human_readable()`, integers in msgpack | proto units |
| run `number` = highest stored + 1 | per-measurement `next_run: u32` in the session file, never lowered | ac2d, traces |
| loopback burst: no state | a `generator` event as it starts, `firing` true for its length, `last_action.action = loopback_burst` (new `GenAction::LoopbackBurst`); `firing` false after | proto, ac2d, ui |
| a stream reopen silently ends a recording | the reply of the command that caused it carries warning `recording_ended {recording: RecordingRef}` (`file.load`, `file.new`, `stream.open`, `stream.close`) | proto, ac2d, cli, ui |
| `DmxSet {channel, value}` | `+ snap: bool` (jump, no fade) | proto, ac2d artnet, ui |
| `artnet.test` blanks other channels | merged over the cue frame, highest wins | ac2d artnet |
| `cal.line` refuses outside +4 … +30 dBu | refuses outside −40 … +50 dBu; warning `line_level_unusual {full_scale: Dbu}` outside +4 … +30 | proto, ac2d, ui |
| — | `rec.delete`, `file.delete`, `file.rename`, `file.upload_abort` (§5) | proto, ac2d, cli, ui |
| — | one `MAX_POINTS` (§7) | proto, ac2d |

The envelope discriminators stay: `op` / `args` on a command, `kind` on an event and a frame.
They name the message, not a variant of a data type.

## Outside this bump

These change nothing on the wire or in a stored format and land on their own branches:

- **Dedup (L-03).** Pure reads are not stored; the reply of a mutation and of a read that
  acts is (`retry` in `control/request.rs`). `pn-edit` turns that into `is_read` (§2).
- **Oversize events (L-02, events).** An event over `MAX_EVENT_BYTES` is not sent; every
  event connection is sent `resync_required` ahead of its next event, credit answer or
  keepalive; §4 adds that its rev is not replayable.
- **Event byte bound (L-10).** Events kept for replay are bounded by bytes as well as count;
  a `state.since` reaching past what the bound kept answers `resync_required`, as for count.
- **`ZMQ_MAXMSGSIZE` and the epoch (L-12).** Every socket sets `ZMQ_MAXMSGSIZE` to its
  message bound; the stream epoch is incremented checked, and a session whose traces
  record an epoch at or past half the range refuses to load.
- **Runtime directory (T-02, Unix).** `Instance::lock` creates the directory, sets it to 0700
  itself, then `lstat`s it: a directory, not a symlink, owned by the effective uid, no group
  or other bits. The daemon refuses to start only when it cannot set the mode or the
  directory is not the user's, so a systemd `RuntimeDirectory=` created at 0755 still starts.
  `ac2-client` and the app's spawner checking the same before connecting is still to do,
  on that branch. Deploy check per host
  anyway: units that create the directory set `RuntimeDirectoryMode=0700`.
- **T-04** a `SessionRef.path` is mirrored to other clients as its base name only; **T-07**
  network off stops a network client's stimulus at once; **D-05** a client resends on `ka`
  silence; **`meas.create`** checks the name as `meas.update` does.
- **S-11.** Device open, enumeration, preview and the reopen after a file load run off the
  ctrl loop; their reply follows when the work ends, and leases keep being refreshed.
- **Art-Net and sACN (Q-01).** Art-Net binds UDP 6454 and answers ArtPoll; sACN (E1.31)
  output beside it. Its wire part (a target type and its command) goes in the PROTO of that
  branch, not this one.
- **ISO 226:2023 (Q-06).** Equal-loudness contours from Table 1 and Formula (1); valid to
  90 phon up to 4 kHz and 80 phon from 5 kHz, informative above.

## Implementation order

One integration branch, `proto-next`, takes each feature branch in turn; each is built and
tested alone (quick tier) before the next starts. The first branch bumps `PROTO_VERSION` to
105; later branches refresh `fixtures/protocol/WIRE_LOCK` under 105. Each stored format is
bumped once (table below), by the first branch that changes its shape; a later branch that
changes it again does so under the same number and extends the converter. Nothing deploys
until the last has landed.

1. `pn-stream`: `session.*` to `stream.*` alone (largest diff, no logic), with the standing
   names (`StandingSession`, `NoSession`, `SourceSession`); `jobs/meters.rs` builds both the `stream_levels`
   and the `source_levels` meta.
2. `pn-names`: the pure renames of §9 (tag keys, `colour`, `channel`, `rig_id`, `auth.*`,
   `rig.monitor_at_start`, unit suffixes with `OtherRate`'s `theirs_hz` / `ours_hz`,
   snake_case `Result`). `TransferResolution` is
   already tagged `type`, and the `sample_rate_hz` of `SourceLevelsMeta` and the standing
   entries already carry the suffix; `BinsMeta.rate` joins the sample-rate renames.
3. `pn-shapes`: the §9 rows that change a shape but not frames or files: `sim_map`, dynamics
   envelopes as arrays, `RigPeer`, the `Version` detail, required `level`, busy `refused`,
   required `serde` fields.
4. `pn-files`: string integers in JSON files and `next_run`.
5. `pn-edit`: §1, §2 (`edit_rev`, delay entity, `expect_rev` gone, `eq put` base); bins
   frames take the `tf` stamp already, so they follow without a change of their own.
6. `pn-roles`: §3 (needs the `auth.*` names of 2). The data socket's rig gate already
   keeps `source/levels` and `audio/<channel>` from every non-rig, a viewer included.
7. `pn-ipc`: §8; then an all-OS CI run (`-f all_os=true`) and the Wine AF_UNIX proof.
8. `pn-big`: §4 and the one history cap; snapshot transfers exempt from `MAX_TRANSFERS`.
9. `pn-spool`: §5; then an all-OS CI run for the free-space query per OS.
10. `pn-frames`: §6 with the `dbtp` tagging and the delta header,
    `crates/ac2d/tests/wire_size.rs` rerun. `PerFrame` (`frame/wire.rs`) already carries
    `bins` and `source_levels` and its match is exhaustive; this step adds `delay` /
    `nudged` to `bins` and gives `source_levels` the levels arrays.
11. `pn-logs`: §7; then an all-OS CI run for the boot clock, boot id, RTC, zone and
    `adjtimex` code per OS.
12. `pn-misc`: `DmxSet.snap`, the `artnet.test` merge, the `cal.line` range, the loopback
    burst event, `recording_ended`.

## Versions and deploy

Every serde writer of the workspace was walked; stored formats that change shape:

| format | from → to | why | deploy action |
|---|---|---|---|
| `PROTO_VERSION` | 104 → 105 | everything above | daemon, CLI, apps together |
| session format (`ac2-traces::session::VERSION`) | 49 → 50 | stream names, tag keys, `lo_hz` / `hi_hz`, `sample_rate_hz`, `next_run`, string integers, log versions and marks | autosave of 49 set aside at start (its logs stay plain CSV); save or export first; old saved sessions reach 105 by `file.salvage` |
| `working.json` (autosave folder) | unversioned | `saved_at`, `replaced_at` as strings | set aside with its autosave |
| rig meter logs (`rig_logs`) | session format 49 → 50 | as the session | not read back; the meters start new logs, old CSV stays on disk |
| raw capture sidecar | 3 → 4 | stream names, `sample_rate_hz`, string integers, measurement without `delay` and `TimelineChange::Delay` (§1) | old captures not replayed; keep the WAV |
| SPL log | v2 → v3 | §7 | none (inside sessions) |
| band log | v1 → v2 | §7 | none (inside sessions) |
| calibration store | 4 → 5 | `InputKind` tag, string `calibrated_at`, required `loading` | converter (adds `loading` where missing) |
| rig settings (`rig.json`) | 11 → 12 | `RigPeer`, `DmxSet.snap`, stream config names, tag keys, unit suffixes, required layout fields, string integers | converter |
| authorized clients | grammar | role on every line (§3) | converter adds `operator` |
| plan files (`plans/*.json`) | unversioned → `version: 1` | `Surface` tag `type` | converter |
| trace export | v6 → v7 | sweep info, `ir` header and dynamics info carry `sample_rate_hz`, `lo_hz` / `hi_hz`, `type` tags | none (open question 1) |
| network settings | 2, unchanged | no affected type | — |
| `ui.toml` | unchanged | its own file types; rig ids already hex strings | — |

CLI `--json` output and selftest reports follow the wire types and are not read back.

At deploy every host stops the old daemon, then: saves or exports any session it wants to
keep; runs `tools/proto-next/convert.py` once, which rewrites `rig.json`, the calibration
store, the authorized-clients file and the plan files into their new shapes and is deleted
after the deploy (no reader of old shapes enters the daemon); checks the runtime directory's
unit setting; installs daemon, CLI and apps together, since no client speaks an older PROTO
and 105 both. A standalone meter's files must be converted before its first start under 105,
or it boots without its meter and refuses every network client.

## Open questions

1. **Trace export v6.** Exports are the operator's archive; the reader reads v1–v5.
   Recommendation: v6 stays readable on its own versioned path, as the older ones are.
2. **Re-stamping on disk.** Rows already appended to the autosave's log file before the sync
   keep `unsynced` there until the next whole write. Recommendation: accept it; the next
   generation carries `derived`.
3. **Zone name.** `chrono` gives no IANA name. Recommendation: `iana-time-zone`, already in
   `chrono`'s `clock` tree, as a direct dependency of `ac2d`.
4. **Salvaged logs as measurements.** The header holds no window configuration.
   Recommendation: default configuration, as §7; the operator re-applies limits to judge.
5. **Windows ipc.** Whether `zeromq-src` 0.3.6 compiles libzmq's Windows ipc path cleanly
   with the defines. Recommendation: prove it in `pn-ipc` on CI first.
6. **Frame header name.** `config_rev` now covers the delay too. Recommendation: keep the
   name; the protocol text states the rule.
7. **`not_stated` as trusted** across boots. Recommendation: accept it; the alternative
   marks every restart on macOS and Windows `unknown`, and no meter runs there.
8. **A viewer's `gen.stop`.** Stopping the generator also aborts a running sweep or dynamics
   run and disarms the generator, for whoever sends it (§3). Recommendation: keep it; a
   viewer who hears sound it should not may end all of it, and the operator re-arms.

## Finding map

| id | section |
|---|---|
| S-04, S-06 | §2 |
| S-05, S-07 | §1 |
| T-01 | §3 |
| L-01, L-02 (replay) | §4 |
| L-06, L-08, T-05, L-09 | §5 |
| D-01, L-04, C-08e | §6 |
| F-01, F-02, F-03, L-11 | §7 |
| T-02 (Windows) | §8 |
| N-01 … N-12, N-15, S-08, D-06, C-05, F-04, F-07, Q-02, Q-03, Q-05, T-06, S-10 | §9 |
| L-02 (notice), L-03, L-10, L-12, T-02 (Unix), T-04, T-07, L-07, D-05, S-11, Q-01, Q-06, `meas.create` name | outside this bump |
