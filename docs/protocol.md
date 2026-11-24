# ac2 protocol

Normative description of the wire protocol between `ac2d` and its clients. The Rust types
in `crates/ac2-proto` are the implementation; `crates/ac2-proto/tests/it/doc_parity.rs` checks
one direction, code to document: it fails when a name of a type it lists (the type's variants
and fields, one level deep) is missing here, and it checks `PROTO_VERSION`, `MAX_HEADER_BYTES`,
`MAX_N` and the log grid id. A name written here that the code lacks is not caught. Background: PLAN.md §6,
`docs/design/q2-q5-q6-protocol.md`, `docs/design/spike-zmq-curve.md`.

The protocol is transport-agnostic bytes. Under ZeroMQ, ctrl is DEALER ↔ ROUTER (one
message frame per request or reply) and data is DEALER ↔ ROUTER too: the client subscribes
and returns credit on its data DEALER, the daemon sends multipart frames, events and
keepalives to each connection as its credit allows (§5.8).

## 1. Encoding rules

- Ctrl messages and events are **msgpack maps with named fields**; field order carries no
  meaning. Frame headers are the exception: **msgpack arrays**, positional (§5.3), since a
  frame goes out tens of times a second with a payload often smaller than its field names.
  Frame arrays are packed binary, not msgpack (§5.2).
- Unknown fields are refused everywhere (strict in both directions). Fields are never
  defaulted when missing, except that an absent optional field reads as `nil`.
- Enums are snake_case strings (`"db_spl"`) when they carry no data. Enums with data are
  maps tagged by `type` (`{"type": "sine", "freq": 1000.0}`) unless stated otherwise. A
  variant without data of an enum tagged by `type` is `{type: x}` (`[x]` in a positional
  frame header).
- Physical quantities are floats (f64) in the unit their type names: `Hz`, `Seconds`, `Db`
  (ratio), `Dbfs` (0 dBFS = RMS of a full-scale sine), `Dbtp` (true peak re full scale),
  `Lufs` (programme loudness), `DbSpl`, `Dbu` (dB re 0.7746 V RMS), `Volts` (RMS),
  `MvPerPa` (mic sensitivity), `Ohms`, `Phon`, `Degrees`. Sample counts (`Samples`) are
  signed integers; sample indices and `WallNs` (Unix ns, daemon clock) are u64.
- Indices on the wire (channels, inputs, outputs, steps, points) are zero-based except
  `DynamicsStatus.step` (1-based; 0 is before the first burst) and the 1-based indices in
  CSV files; counters (`repeat`, `pass`) and line numbers start at 1.
- Opaque bytes (`lease_token`, file content) are msgpack `bin`, never base64. A lease
  token is 16 bytes (u128, big-endian).
- Std `Result` values (the reply `result`) are `{"Ok": …}` or `{"Err": …}`.

## 2. Version and hello

`PROTO_VERSION = 102`. Every ctrl message of every version is a map containing `v` (u16) and
`id` (u64); that is the only layout fixed across versions. A receiver reads those two
fields first:

- `v` missing → refused as `version_mismatch` with `detail.client` 0; it is never assumed.
- `v` ≠ own version → refused. The daemon answers with a reply at its own version whose
  `result` is `Err{code: version_mismatch, detail: {type: version, daemon, client}}`. A
  client that receives a reply with another `v` treats it as the same refusal without
  decoding the body.
- There is no negotiation, no range of accepted versions and no compatibility layer.
- An envelope (`v`, `id`) that does not parse is dropped without a reply. An envelope that
  parses with a body that does not decode is answered `invalid`. A request without `id` is
  answered as id 0.

The first request on a connection is `hello` {`client`: string, the client software's
name}; its reply `welcome` carries `server` (`ac2d <version> (build <id>)`), `client_id`
(`ClientId`, a string: the identity the daemon bound to the connection: the ZAP User-Id under CURVE, otherwise a
daemon-assigned id), `daemon_incarnation`, `session_epoch`, `rev`, `rig_id` and `name`.
A `hello` that is not the first request gets another `welcome`.

`rig_id` (u64, never 0) is the daemon's identity: random, made once and kept in a file
(`rig-id` in the ac2 config directory, 16 lowercase hex digits and a newline), the same across reconnects,
restarts, renames, address changes and a move between local and network mode, in every
mode. It is not the CURVE key, which a local daemon does not have. A file that cannot be
read is set aside (`damaged_files`, kind `rig_id`) and a new id made, so a client sees a
new rig, never takes it for another. An ephemeral daemon (`ac2d --ephemeral`, the app's
simulated rig) has a new id every start.
`name` is the rig's name (`ac2d --name`, default `ac2 on <hostname>`), display only and read anew on each connect; a network-mode
daemon advertises it over mDNS.

Frame headers carry `v` too; a frame at another version is refused.

## 3. Ctrl

### 3.1 Request and reply

```
Request { v: u16, id: u64, cmd: {op: "<name>", args?: {…}}, expect_rev: u64 | nil }
Reply   { v: u16, id: u64, result: {"Ok": {type: "<body>", value?: …}} | {"Err": ProtoError},
          warnings: [Warning] }
ProtoError { code: ErrorCode, msg: string, detail: ErrorDetail | nil }
Warning { type: "<name>", … }
```

- `warnings` lists what a command that succeeded did that the caller should know about;
  it is empty for almost every reply, and always present. `type` is one of
  `rig_monitor_blind` {meas: [MeasId]} (`session.close` while rig measurements run) and
  `rig_monitor_moved` {meas: [MeasId]} (`session.open` of another backend or input device
  while they run). A client shows them to the operator; they change nothing in `result`.

- `args` is absent for commands without arguments.
- `id` is unique per client within the dedup window. The window is keyed by (authorized
  name, key) on the network pair and by connection on the local pair, so a local retry
  across a reconnect executes again. A client starts its ids at a random 63-bit value,
  since processes sharing one key share one window. A copy of an id sent while the request
  still runs (a panel, the delay finder, loopback detection) runs nothing: when the request
  completes, the reply goes to every copy, on whichever connection each arrived. A mutation
  or a read that acts (`session.devices`, `session.preview`, `session.preview_stop`,
  `session.control_panel`, `session.detect_loopback`, `delay.find`, `trace.export`,
  `file.upload_begin`, `file.upload_chunk`, `file.import`, `file.download_begin`) executes
  once per id, whichever command it carries: its reply is stored, the last 256 stored
  replies each kept 30 s, and a copy sent after the reply gets the stored reply. Any other
  read is not stored: a copy of it sent after the reply runs again. Requests refused before
  execution (protocol version, undecodable, revoked key) are not remembered. Revoking a
  client forgets its stored replies and running ids.
- `session.control_panel`, `delay.find`, `session.detect_loopback` and the file jobs
  (`file.list`, `file.save`, `file.load`, `file.new`, `file.recover`, `file.salvage`,
  `file.import`, `file.download_begin`) run beside the ctrl loop: other requests are
  answered while they run.
- `expect_rev` applies to mutations: if state has moved past it the reply is `conflict`
  with `detail: {type: conflict, rev}` (the current rev). Commits are serial.
- Largest ctrl message: 4 MiB, checked before parsing. A reply that would be larger is
  sent as `too_large` instead (a client discards an oversize message unread, so it would
  only time out); a larger request is answered `too_large` to its `id` (read from its
  envelope alone), and a client refuses to send one. What can be that large goes another
  way: `trace.export` as a download, `trace.import` from an upload, `dynamics.table` for a
  dynamics trace's step table.
- Every receiving socket bounds a message part before buffering it (`ZMQ_MAXMSGSIZE`): the
  largest legal part plus 64 KiB of slack (CURVE's framing; a request a little over 4 MiB is
  still read and answered `too_large`). A part past that closes the connection that carried
  it, unanswered. The bounds: ctrl 4 MiB both ways; a data request 4 KiB; a frame or event
  part toward a client or rig 2 MiB.

### 3.2 Commands

Lease column: **L** = `lease_token` required (Q6); **non-mutating** = the command commits no
state change (`expect_rev` does not apply). Every command not marked non-mutating is a
mutation.

| op | args | reply body | lease |
|---|---|---|---|
| `hello` | `client` | `welcome` | non-mutating |
| `session.devices` | — | `backends` | non-mutating |
| `session.preview` | `backend: BackendKind`, `device: DeviceId`, `inputs: [PreviewInput]` ({`channel`, `kind: InputKind`}), `pairs: [Pair]` | `preview` | non-mutating |
| `session.preview_stop` | — | `ack` | non-mutating |
| `session.control_panel` | `backend: BackendKind`, `device: DeviceId` | `ack` | non-mutating |
| `session.detect_loopback` | `lease_token`, `backend`, `input_device`, `output_device`, `output: u16`, `level: Dbfs \| nil` | `loopback_detection` | L, non-mutating |
| `session.open` | `config: SessionConfig` | `session` | |
| `session.close` | — | `ack` | |
| `session.status` | — | `session` | non-mutating |
| `session.inputs` | `inputs: [InputSetup]` (upserted by channel), `pairs: [Pair]` (the whole list) | `inputs` (`inputs`: the whole setup, `pairs`) | |
| `session.outputs` | `outputs: [OutputSetup]` (upserted by channel; `label: nil` clears) | `outputs` (every label) | |
| `gen.acquire` | `force: bool` | `lease` (`lease_token`, `expires_in_ms`) | |
| `gen.set` | `lease_token`, `desired: {settings, armed, firing}` | `generator` | L |
| `gen.refresh` | `lease_token` | `lease` | L |
| `gen.release` | `lease_token` | `ack` | L |
| `gen.stop` | — | `ack` | universal |
| `gen.ceiling` | `ceiling: Dbfs`, `confirm_raise: bool` | `generator` | any client |
| `osc.target` | `base: Rev` (`rig.rev`), `addr: "ip:port" \| nil` | `ack` (`rig` event) | |
| `osc.test` | `msg: OscMessage` | `ack` (`rig` event: the sent count) | |
| `rig.monitor_start` | `base: Rev` (`rig.rev`), `on: bool` | `ack` (`rig` event) | |
| `artnet.target` | `base: Rev` (`rig.rev`), `target: ArtnetTarget \| nil` | `ack` (`rig` event) | |
| `artnet.test` | `channels: [DmxSet]` | `ack` (`rig` event: the sent count) | |
| `artnet.fade` | `base: Rev` (`rig.rev`), `seconds: f64` | `ack` (`rig` event) | |
| `levels.set` | `base: Rev` (`rig.rev`), `ballistic: Ballistic \| nil`, `correlation_window: CorrelationWindow \| nil`, `over_threshold: Dbtp \| nil`, `correlation_gate: Dbfs \| nil`, `loudness_target: Lufs \| nil` | `ack` (`rig` event) | |
| `levels.reset` | `channels: [u16]` (empty: every captured input) | `ack` | |
| `meas.create` | `config: MeasConfig` | `measurement` | |
| `meas.update` | `meas`, `base: Rev` (the `config_rev` the edit was made on), `config` | `measurement` | |
| `meas.colour` | `meas`, `colour: ColourFamily \| nil` | `measurement` | |
| `meas.layout` | `meas`, `layout: PaneLayout \| nil` | `measurement` | |
| `meas.offset` | `meas`, `offset: Db` | `measurement` | |
| `meas.eq` | `meas`, `edit: EqEdit` | `measurement` | |
| `meas.delete` | `meas`, `traces: keep \| delete` (what becomes of the stored traces and math traces it owns) | `ack` | |
| `meas.start` | `meas` | `measurement` | |
| `meas.stop` | `meas` | `measurement` | |
| `meas.reset` | `meas` | `ack` | |
| `delay.find` | `meas`, `band: FinderBand`, `observation: Seconds \| nil` | `delay_finding` | non-mutating |
| `delay.insert` | `meas`, `pick: first_arrival \| strongest \| ranked{index}` | `measurement` | |
| `delay.set` | `meas`, `delay: Seconds` | `measurement` | |
| `delay.nudge` | `meas`, `by: Seconds` | `measurement` | |
| `delay.track` | `meas`, `enabled` | `measurement` | |
| `trace.capture` | `meas`, `name`, `slot` (1…9 \| nil) | `trace` | |
| `trace.list` | — | `traces` | non-mutating |
| `trace.get` | `trace` | `trace_data` | non-mutating |
| `trace.update` | `trace`, `edit: TraceEdit` (full replacement) | `trace` | |
| `trace.delete` | `trace` | `ack` | |
| `trace.average` | `traces`, `method`, `reference`, `name` | `trace` | |
| `trace.import` | `transfer` (a complete upload, used up), `format: ac2_csv \| analyzer_text \| ir_wav \| auto`, `role: trace \| target`, `ir: IrImport \| nil`, `owner: TraceOwner` | `trace` | |
| `trace.target` | `curve: TargetCurve`, `name`, `owner: TraceOwner` | `trace` | |
| `trace.export` | `trace`, `format: ac2_csv \| ir_wav` | `transfer` (`file_name` the suggested name, `bytes`, `sha256`): fetch with `file.download_chunk`, close with `file.download_end` | non-mutating |
| `trace.mic_curve` | `trace`, `curve: MicCurveId \| nil` (nil removes) | `trace` | |
| `cal.spl` | `input`, `mic`, `calibrator_level: DbSpl`, `calibrator_freq: Hz` | `calibration` | |
| `cal.spl_electrical` | `input`, `mic`, `connection: in_line \| injected`, `volts: Volts`, `freq: Hz`, `mic_sensitivity: MvPerPa \| nil`, `loading: MicLoading \| nil`, `uncertainty: Db \| nil`, `replace_acoustic: bool` | `calibration` | |
| `cal.curve_import` | `mic`, `label: string \| nil`, `file_name`, `content: bin`, `input: u16 \| nil` | `mic` | |
| `cal.curve_rename` | `curve: MicCurveId`, `label` | `mic` | |
| `cal.curve_delete` | `curve: MicCurveId` | `ack` | |
| `cal.line` | `channel`, `volts: Volts` | `line_calibration` | |
| `cal.list` | — | `calibrations` (`calibrations`, `line_calibrations`, `mics`) | non-mutating |
| `cal.delete` | `key: CalRef` (`type`: `spl` {device, channel, mic} \| `line` {device, channel}) | `ack` | |
| `plan.save` | `plan: PlanSpec` | `plan` | |
| `plan.list` | — | `plans` | non-mutating |
| `plan.delete` | `name` | `ack` | |
| `walk.new` | `name`, `plan` (name), `kind: WalkKind` (`live_transfer` \| `sweep`), `meas: [MeasId]`, `points: [WalkPointSpec]` ({`label`, `at: [x, y, z]`}) | `walk` | |
| `walk.update` | `walk: WalkId`, `name`, `points: [WalkPointSpec]` | `walk` | |
| `walk.goto` | `walk`, `step: u16` (zero-based) | `walk` | |
| `walk.capture` | `walk` | `walk` | |
| `walk.skip` | `walk`, `point: u16` (zero-based) | `walk` | |
| `walk.redo` | `walk`, `point: u16` | `walk` | |
| `walk.swap` | `walk`, `points: [u16]` | `walk` | |
| `walk.delete` | `walk` | `ack` | |
| `sim.run` | `request: SimRequest` | `sim` | |
| `sim.fit` | `request: SimFitRequest` | `sim` | |
| `sim.map` | `request: SimMapRequest` | `sim` | |
| `sim.cancel` | — | `sim` | |
| `spl.log_get` | `meas`, `log: SplLogWhich`, `from: u64`, `max: u32` | `spl_log_page` | non-mutating |
| `spl.log_new` | `meas` | `ack` | |
| `spl.history_get` | `meas`, `span: HistorySpan` | `spl_history` | non-mutating |
| `spl.band_transfer` | `meas`, `foh`, `at_place: BandLevelSource`, `background: BandLevelSource \| nil`, `place: str` | `measurement` (as `meas.update`) | |
| `spl.band_log_get` | `meas`, `from`, `until: WallNs`, `step: u32 \| nil` | `spl_band_log` | non-mutating |
| `sweep.run` | `lease_token`, `meas` (a sweep measurement), `name: string \| nil` | `sweep` (the run as started) | L (held for the run), armed |
| `dynamics.run` | `lease_token`, `meas` (a dynamics measurement), `name: string \| nil`, `rests_from: TraceId \| nil` | `dynamics` (the run as started) | L (held for the run), armed |
| `dynamics.table` | `trace` (a dynamics trace, else `invalid`) | `dynamics_table` (`DynamicsTable`: `info`, `inputs`, as in `DynamicsData` without `envelopes`) | non-mutating |
| `dynamics.envelope` | `trace` (a dynamics trace), `input: u16` (a measured input, zero-based), `step: u32` (zero-based); else `invalid` | `dynamics_envelope` (`DynamicsEnvelope`: `input`, `step`, `start`, `end`, each edge a `DynamicsEdgeCurve` of parallel f32 arrays {`t`, `gr_db`, `h_db`: four arrays, H2…H5, NaN where blind}) | non-mutating |
| `state.snapshot` | — | `snapshot` | non-mutating |
| `state.since` | `rev` | `events` or `resync_required` | non-mutating |
| `grid.get` | `grid_id` | `grid` | non-mutating |
| `file.save` | `session: SessionRef`, `overwrite: bool` | `session_file`; the working state belongs to it, clean | |
| `file.load` | `session: SessionRef`, `discard: bool` | `session_file`; loads disarmed, no owner, new epoch | |
| `file.new` | `discard: bool` | `ack`; every session measurement and trace deleted (rig measurements stay), untitled | |
| `file.recover` | — | `ack`; swaps in the replaced working state | |
| `file.salvage` | `session: SessionRef` | `salvage` | |
| `file.list` | — | `sessions` | non-mutating |
| `file.upload_begin` | `file` (its path on the client), `bytes: u64`, `sha256: string` | `transfer` | non-mutating |
| `file.upload_chunk` | `transfer`, `offset: u64`, `data: bin` | `ack` | non-mutating |
| `file.import` | `transfer`, `name`, `overwrite: bool` | `session_file` | non-mutating (writes a file, commits no state) |
| `file.download_begin` | `session: SessionRef \| nil` (nil: the working state) | `transfer` | non-mutating |
| `file.download_chunk` | `transfer`, `offset: u64` | `chunk` (bin) | non-mutating |
| `file.download_end` | `transfer`, `saved_as: string \| nil` | `ack` | |
| `rec.start` | `request: RecordRequest` | `recording` (the run as started) | |
| `rec.stop` | — | `recording` (the run, finished) | |
| `rec.list` | — | `recordings` | non-mutating |
| `session.replay` | `recording: RecordingRef`, `pace: realtime \| fast` | `session` | |
| `server.info` | — | `server` | non-mutating |
| `server.network` | `enabled: bool`, `bind: NetBind \| nil`, `port: u16 \| nil` | `server` | local clients only |
| `server.authorize` | `name`, `key` (Z85) | `server` | |
| `server.revoke` | `name` | `server` | |
| `auth.admit_rig` | `key` (Z85), `rig_id: RigId`, `name` | `server` | |
| `auth.revoke_rig` | `rig_id: RigId` | `server` | |
| `rig.pair` | `peer: RigPeer` | `ack` | |
| `rig.unpair` | `rig: RigId` | `ack` | |
| `source.state` | — | `source_state` | non-mutating; with `hello`, the only command an admitted rig may send |

`session.status` returns `state.session`. `trace.list` returns `state.traces`. `plan.list`
returns `state.plans`. `gen.refresh` checks the lease, moves its deadline out by the lease
expiry and, while the generator fires, keeps the output gate open until that deadline.
`delay.track` sets `delay.tracking` on a transfer measurement and tells its job.

```
GeneratorDesired  { settings: GeneratorSettings, armed: bool, firing: bool }
GeneratorSettings { signal: Signal, level: Dbfs, band: BandLimit | nil, outputs: [u16] }
Signal = {type: "white"} | {type: "pink"} | {type: "periodic_pink", period: Samples}
       | {type: "sine", freq: Hz} | {type: "ess", sweep: EssSpec}
BandLimit { highpass: Hz | nil, lowpass: Hz | nil, order: FilterOrder }
```

`GeneratorDesired` is `gen.set`'s `desired`. `level` is RMS; `band` applies to noise only;
`outputs` are zero-based output channels; a `periodic_pink` `period` is a power of two.

Rules (Q6): `firing` requires `armed`; arming does not emit. `gen.set` carries the full
desired state and refreshes the lease. Refresh at least every 0.5 s; expiry 1.5 s after
the last refresh fades out (20 ms), disarms and clears the owner. `gen.acquire{force}`
stops and disarms before handing over. A lease belongs to the instance holding its token,
not to the key: an acquire by the current owner's key (a reconnect, or a second process
with the same key) also fades out, disarms and invalidates the old token, so a new lease
always starts disarmed. A transport hiccup keeps the lease only by refreshing with the
held token. Turning the network off (`server.network`) ends a network client's lease at
once the same way, with an `expiry` event: that client can no longer refresh or stop it.
Every acquire, force, arm, fire, set, stop, release
and expiry is a `generator` event naming the client (`last_action.client`; for `expiry` the
owner whose lease expired). The output path enforces the deadline itself; if it mutes on an
expired deadline before the control side noticed, the daemon disarms with the same `expiry`
event, so the state never says firing while the output is silent.

**System max level (`gen.ceiling`).** `generator.ceiling` (dBFS RMS) is the system
maximum: `gen.set`, `sweep.run`, `dynamics.run` and `session.detect_loopback` above it are
`refused`, and the output path limits every sample to 6 × the ceiling's RMS in amplitude
(+15.6 dB of crest; at most full scale) on the running stream at once; generators already
refuse a level whose own crest would pass it. `generator.ceiling_bound` is the hard upper bound fixed at
daemon start (`ac2d --max-level`); `gen.ceiling` above it, above 0 dBFS or not finite is
`invalid`. Any client may change it (no lease):

- **Lowering** applies at once. A stimulus armed or playing at a level above the new
  maximum, and a sweep or dynamics run with any level above it, is stopped and disarmed
  (faded out, 20 ms): stopping is unambiguous where a quietly lowered level would leave the owner's level and the
  measurement's level disagreeing. One at or below the new maximum carries on untouched.
- **Raising** needs `confirm_raise: true` (`refused` without), and is `refused` while the
  generator is armed or firing, a sweep runs or a loopback detection plays.
- Each change is a `generator` event whose `last_action` is `ceiling_lowered` or
  `ceiling_raised` naming the client, and a line in the daemon's audit log. The daemon keeps
  the value in its rig settings file (atomic write) and starts with it; a value above a
  later, lower `--max-level` comes up at that bound. A change that cannot be written is
  `internal` and leaves the maximum as it was.

**Output labels (`session.outputs`).** `OutputSetup` = {`channel`: u16 (zero-based), `label`: string |
nil}. Labels name the rig's outputs (`Main L`, `Sub`) for every client; they are kept with
the rig settings (not in sessions) by channel number, whatever the device. A label is 1–32
characters, no control characters, no surrounding space (`invalid` otherwise); `nil` clears
it. `state.outputs` lists the labelled outputs, sorted by channel.

**Rig monitor, OSC and Art-Net cues (`osc.*`, `artnet.*`, `rig.*`).** The `rig` entity (`Rig`) holds what belongs
to the daemon's machine rather than to a session: `rev` (the rev of its last settings change),
`osc_target` (`"ip:port"` \| nil: where OSC cues go; an IP literal, never a name to look up),
`artnet_target` (`ArtnetTarget` \| nil: where the Art-Net universe goes), `artnet_fade` (f64
seconds, 0–60: how long the DMX channels take to move to a new cue frame; 0 jumps; 1 until
set), `monitor_at_start` (at start the daemon reopens the last session's device capture-only so the
rig measurements meter with no client connected; the generator stays disarmed), `levels`
(`LevelsSettings` {`ballistic`: `Ballistic`, `correlation_window`: `CorrelationWindow`,
`over_threshold`: Dbtp, `correlation_gate`: Dbfs, `loudness_target`: Lufs}: the programme meters' settings, below), `osc` and
`artnet` (each an `OutputStatus` {`sent`: packets since the daemon started, u64,
`last_error`: str \| nil, the latest send error as the operating system said it}), `dmx`
([`DmxSet`]: the cue universe's channels above 0, in channel order, whether or not a target is
set), `cues` ([`RigCue`] {`meas`, `subject`:
`CueSubject`, `state`: `CueState`}: every cued subject's state as last sent) and
`damaged_files` ([`DamagedFile`] {`kind`: `StateFile`, `path`, `set_aside_as`: str \| nil,
`reason`, `at`: `WallNs`}: state files found damaged at this start, set aside and replaced
by their previous copy or a default; empty on a sound start, kept until the daemon restarts).
`StateFile` is `autosave`, `trace`, `spl_log`, `band_log`, `working`, `rig_settings`,
`rig_logs`, `calibrations`, `server_key`, `authorized_clients`, `rig_id` or
`network_settings`. `CueSubject` is
tagged by `type`: `meter` (the whole meter: the worst state of its limits), `window`
{duration, weighting}, `peak` {quantity}, `band` {duration, weighting, nominal},
`predicted`. `CueState` is `ok` (under the limit, or none), `near`, `over` or `fault` (the rig
cannot judge: no audio, device lost, not calibrated).
`osc.target`, `artnet.target`, `artnet.fade`, `rig.monitor_start` and `levels.set` carry the `rig.rev` they were made on; another is
`conflict` with `detail: conflict {rev}` (the current one). `osc.test` sends one message to
the target now (setup); `invalid` without a target, `internal` with the operating system's
reason when the send fails; `append_level` appends 0 dB. `osc.target` refuses an unspecified
address or port 0 (`invalid`), writes the rig settings file before it takes effect, and sends
the current state of every cue to the new target at once. `artnet.target` likewise refuses
an unspecified address or port 0, persists, and sends the current frame at once.
`artnet.fade` sets `rig.artnet_fade` and persists it; `seconds` outside 0–60 or not finite is
`invalid`. It applies from the next change of the cue frame (a fade in progress keeps its
length).
**Programme meters (`levels.*`).** `rig.levels` holds what the input meters of §5.4
(`session_levels`, `preview_levels`) read beyond peak and RMS: `ballistic` is what the bars
read — a peak detector in `ppm`: `true_peak` (BS.1770 Annex 2, 4× oversampled; the
default), `type1` (IEC 60268-10 Type I quasi-peak), `type2` (Type II) or `vu` (IEC 60268-17;
offered on a `line` input with a line calibration only — elsewhere `ppm` carries the true
peak and the row says so); or a loudness window of each bar's source: `momentary` (BS.1770
momentary, 400 ms, LUFS) or `short_term` (3 s), read from the frame's `loudness` — under
these two every input's `ppm` and `max` carry its true peak and true-peak hold (dBTP), so a
frame stays self-describing, and an input without a source (a mic, a partner not captured, a
rate outside the meter's) reads its true peak; `correlation_window` is the correlator's exponential window, `short` (0.3 s) or `long`
(1 s, the default); `over_threshold` (dBTP, default 0; −20 … +6) is the true peak above which
an over is counted (a sustained clip counts once: the next over only after the true peak fell
1 dB below the threshold); `correlation_gate` (dBFS RMS of a side, default −60; −120 … 0) is
the level below which a pair's side reads *quiet* and no correlation is given;
`loudness_target` (LUFS, default −23, the R128 programme level; −36 … −5) is the level a
loudness bar is drawn around (its span is target −18 … target +9 LU) and lit above, the rig's
so that two clients colour one programme alike. The daemon runs every detector and every
source's loudness all the time and reports the chosen ballistic, so `levels.set` reads a
settled value at once; it keeps the fields given (nil keeps a field), persists the rig
settings first, and refuses a threshold, gate or target outside its range or not finite
(`invalid`).
`levels.reset` clears the max hold of every ballistic and the over count of `channels`
(device inputs; empty = every captured input) on the session meters and the preview alike,
and resets every loudness source with an input in `channels` (integrated, range, the maxima,
`duration` and `since` restart; momentary and short-term are signal state and carry on);
nothing in the state changes, the next meter frame carries the cleared values.
`artnet.test` sends one frame holding `channels` (every other channel 0) now, to find a
fixture's channels; the keepalive repeats it for 5 s, then the cue frame comes back (at once
when a cue changes state meanwhile); `invalid` without a target or with a channel listed
twice, `internal` when the send fails.
Cue rules: a state change is sent at once, and every cued subject's current message is sent
again every second, so a lost datagram heals within a second and a receiver can treat
silence (a few seconds) as a fault. A meter whose job is not running (no session, the device
lost and being reopened, the meter stopped, or no judgement from it for 3 s: a stalled
device) is `fault` on every cue, with no level appended;
a running meter's subjects follow its judgements (`no_limit` → `ok`, `not_calibrated` →
`fault`), from its first judged second (a cue added while it runs keeps its last state until
then). The meter cue takes the worst state of all its subjects, cued or not, in the order
`over` > `fault` > `near` > `ok`: an overload is a fact about the sound and is never masked by
a limit that cannot be judged. `append_level` appends the level the state was judged on (the
worst subject's for the meter cue), when one is known. A cue that goes away (its measurement
deleted, a session measurement replaced by a load, the cue removed by `meas.update`, the daemon stopping cleanly)
sends its `ok` message once and stops holding its levels. Only subjects with a cue are listed in `rig.cues` or sent; a cue
edit keeps the window's state and history (it changes what is sent, not what is judged).
`rig.osc` and `rig.artnet` are committed when `last_error` is set or cleared, and otherwise at
most every 10 s while the sent counter moves (`rig.cues` and `rig.dmx` changes are committed at
once).
Art-Net: ac2 owns one universe and sends all 512 channels as ArtDmx (OpCode 0x5000, protocol
14, `Sequence` 0) from an ephemeral UDP port to `artnet_target`. Each channel is the highest
level any cue's current state lists for it (HTP), 0 where none lists it. When the cue frame
changes, every channel fades from the level last output to its new level over `artnet_fade`,
evenly in perceived brightness: with p = (v/255)^(1/2.2), p moves linearly from start to end
and each frame carries round(255·p^2.2); a frame goes out every 25 ms while fading, the last
one exactly the cue frame. A change during a fade starts a new full fade from the levels
output at that moment. With `artnet_fade` 0, the first frame to a target, a test frame, the
cue frame returning after a test and the clean-stop frame, the frame is sent at once with no
fade. Otherwise the current frame is sent again every second while a target is set (nodes
drop a source silent for about 4 s). Fade frames count in `rig.artnet.sent` under the same
10 s commit rule. `Sequence` stays 0 (no reordering check): every frame is
the whole universe and is resent within a second, so a reordered datagram heals by itself, and
a restarted daemon is never ignored by a node that remembers a higher sequence. Mix ac2's
universe with a lighting desk at the receiver (an Art-Net input merged HTP, or a node's HTP
merge). A daemon stopping cleanly sends one frame of every cue's `ok` levels.
With `monitor_at_start` on, each `session.open` that succeeds while rig measurements exist is
recorded in the rig settings (turning it on with a session open records that one). A daemon
started with it on and a recorded session opens that device itself, after restoring the
autosave: the same inputs, rate and buffer, no output channels and no loopback, so the
generator has nothing to play on (`gen.set` is `invalid`) until a client opens a session of
its own. A failed open is logged and tried again on the audio recovery's backoff (1 s
doubling to 30 s) until it opens or a client opens or closes a session. `session.close`, and
a `session.open` of another backend or input device, are allowed while rig measurements
run; the daemon logs a warning and the reply carries one (§3.1): `rig_monitor_blind` (the rig
meters stop metering) or `rig_monitor_moved` (they meter the new device's inputs), each
listing the rig measurements (`meas`).
A `Cue` = {`ok`, `near`, `over`, `fault`: `CueAction` \| nil} says what each state does;
`CueAction` = {`osc`: `OscMessage` \| nil (sent on entering the state and repeated), `dmx`:
[`DmxSet`] (the levels the state holds while it is current)}. `DmxSet` = {`channel`: u16
1–512, `value`: u8}; a channel outside 1–512 fails to decode. `ArtnetTarget` = {`addr`:
`"ip:port"` (a node's address or a subnet's broadcast address; Art-Net's port is 6454),
`universe`: u16 0–32767, the 15-bit Port-Address: net (bits 14–8) · sub-net (7–4) ·
universe (3–0); above 32767 fails to decode}. The cue sits on each limit (`LeqWindow.cue`, `PeakLimit.cue`, `BandWindow.cue`,
`PredictedWindow.cue`) and on the meter (`SplConfig.cue`). `OscMessage` = {`path`: str (a
leading `/`, then printable ASCII without space or any of `#*,?[]{}`, at most 128 bytes;
anything else fails to decode), `args`: [`OscArg`], `append_level`: bool (append the judged
level, dB SPL, as a trailing float)}; `OscArg` = {`type`: `int` \| `float` \| `str`, `value`:
i32 \| f32 \| str}.

**Server (`server.*`).** `server.info` → `ServerInfo` {`local`, `network`, `interfaces`,
`toggle`, `recording_dir`}. `local`: `LocalServer` (tagged by `type`) \| nil: `embedded` (in its
client's process; only test daemons, the app spawns `ac2d`) \| `local` {`ctrl`, `data`} (the
pair served for the daemon's whole life, this machine only; the OS user is the trust
boundary); nil to a network client, which has no use for this host's local endpoints.
`network`: `NetworkServer` {`setting` (`NetSettingView`, tagged by `type`: `host`
{`setting`: `NetSetting`} to a client of the local pair \| `peer` {`setting`: `off` \| `on`
\| `fixed`} to a network client, which is told neither where the rig binds nor whether
the setting is kept), `status` (`NetStatusView`, tagged by `type`: `host` {`status`:
`NetStatus`} to a client of the local pair \| `peer` {`status`: `PeerNetStatus`} to a
network client; `NetStatus`: `off` \| `listening` {`ctrl`, `data`} \| `waiting` {`reason`} \|
`failed` {`reason`}; `reason` `NetReason`: `address_in_use` \| `interface_missing` \|
`address_missing` \| `zap_failed` \| `no_server_key` \| `not_handed_over` \| `other`
{`detail`}; `PeerNetStatus` is the same with `reason` a plain `NetReasonKind` string, `other`
without its `detail`: the OS's words about the host stay on it), `server_key` (Z85) \| nil (made
at the first turn-on), `fingerprint` \| nil, `advertised_as`: string \| nil (the mDNS name),
`authorized`: [`AuthorizedClient` {`name`, `key`, `fingerprint`, `rig`: RigId \| nil}],
`refused`: [`RefusedKey` {`key`: string \| nil (nil: not CURVE), `fingerprint`: string \|
nil, `address`, `count`, `last_at`: WallNs}] \| nil, newest first, at most 32; nil to a
network client, which gets the authorized names and public keys (pairing shows them) but
not who else knocked, or from where}. `toggle` (`NetToggle`, tagged by `type`, per asker):
`allowed` \| `fixed` (`ac2d --listen`) \| `unavailable` {`reason`: `in_process` \|
`no_key_store` (no key files, so no server key) \| `not_local` (the asker is a network
client)}: whether `server.network` from this client may switch the network, the same
answer the command gives it (one function decides both). `NetSetting` is tagged by `type`: `off` {`kept`: `NetChoice` \| nil
(where it was last on, where a turn-on with nil `bind` and `port` goes; nil until it was
first on), `saved`} \| `on` {`choice`: `NetChoice`, `saved`} \| `fixed`
{`listen`: the `ac2d --listen` address as given; nothing is kept, the command line is the
setting}; `saved` (`NetSaved`: `saved` \| `not_saved` {`reason`: `store_read_only` \|
`unreadable` {`detail`} (left as it is, never written over, until a restart reads it) \|
`damaged_in_place` (could not be set aside; each change tries again before it writes) \|
`no_store` \| `write_failed` {`detail`}}) says whether it outlives a restart. `NetChoice`
{`bind`: `NetBind`, `port` (ctrl; data on the next)}. `NetBind` is tagged
by `type`: `all` (every IPv4 interface) \| `interface` {`name`} (resolved at each bind: its
first IPv4 address, else its first global IPv6 one) \| `addr` {`addr`: the address as
text}. `interfaces`: [`NetInterface` {`name`, `addrs`: [address as text], `up`}], the
host's interfaces read at each `server.info`, loopback left out; empty to a network
client. `recording_dir`: string \| nil (where `rec.start`
records on the daemon host). `server.authorize` adds `key` under `name` to the
authorized-clients file (atomic write) and to the running handshake check; `server.revoke`
removes `name`. A revoked or replaced key's requests are `refused`, its data connection is
dropped at once (its subscriptions and audio end) and it cannot connect again. A connection
is bound to the key it authenticated with, so authorizing a different key under a revoked
name does not let a connection made with the old key act again: the rule above holds for
the old key. Both answer the new
`server` info, with the network on or off (the key list is kept either way); an `embedded`
daemon, which has no key files, answers `unsupported`. A name is 1–64 characters
without whitespace. A name given again replaces its key (and kind): the connections of the
key it had are dropped by the same rule. `invalid` for a key already under another name or
a malformed key; `not_found` for revoking an unknown name; `refused` for revoking the
requesting client's own name (that would lock the operator out). Any authorized client may use them: every authorized client is equally
trusted.

`server.network` {`enabled`, `bind`, `port`} turns the network listener on or off while the
daemon runs; a nil `bind` or `port` keeps the one set before (at first `all` and 47820, data
on the next port). Turning on keeps its choice; turning off keeps the one it was on at (or a
`bind` or `port` given with it) as `kept`, in `network.json` too; a plain turn-off checks
nothing it keeps. Only a client of the local pair may send it (`refused` over the network,
`toggle` `not_local`: a remote app could otherwise cut itself and every other one off); `refused` when `ac2d
--listen` fixed the network for the daemon's life (`setting` `fixed`), `unsupported` on an
`embedded` daemon or one without key files; a port of 0 or 65535 (data needs the next
one) or one the local endpoints use, and an empty interface name or one with control
characters, are `invalid`. The first turn-on makes the server key; when it cannot be made
the turn-on is `refused` with the reason and the setting is left as it was. Turning on binds a new CURVE pair beside the local one, the
session untouched: an open session is served to the new pair's clients at once. Turning
off sends every network data connection the `network_off` notice (§5.8), then closes the
pair: their subscriptions end and a lease they held runs out as on any lost connection
(fading out). Turning on at another bind or port moves the pair. The daemon keeps the
setting in `network.json` beside its other settings (`saved`); a damaged file, one of
another version, or one `server.network` would refuse, is set aside (`damaged_files`, kind `network_settings`) and the network
starts off. Answers the new `server` info; `State.server` follows.

**Remote inputs (`auth.admit_rig`, `auth.revoke_rig`, `rig.pair`, `rig.unpair`,
`source.state`).** A rig takes another rig's
inputs over a link of its own: it connects to the other rig's ctrl and data endpoints under
CURVE with its own server key pair as the client key. The other rig must first admit that
key with `auth.admit_rig` {`key`, `rig_id`, `name`}: an authorized-clients entry marked as
a rig (`AuthorizedClient.rig` = the admitted rig's id; nil for an operator's client).
Admitting a name already present replaces its entry, and admitting the rig's own id is
`invalid`; a replaced or `server.revoke`d key follows the revoke rule above (requests
`refused`, data connection and audio dropped at once, no reconnect). `auth.revoke_rig` {`rig_id`} removes
every entry admitted as that rig, and only those: an operator's client authorized under the
same name stays (`not_found` when no entry is that rig's). An
admitted rig may send `hello` and `source.state` and nothing else (`refused`), and is sent
only `audio/<channel>` frames (§5.1) and the §5.8 notices (`network_off`, `rejoin`); no
`ka`. `source.state` → `SourceState` {`rig`, `name`,
`clocks` (`Clocks`, with its `stamp_map`), `session`: `SourceSession` \| nil
{`incarnation` (`StreamIncarnation` {`daemon`, `epoch`}), `sample_rate_hz`,
`buffer_frames`, `replaying`, `capture_stamps` (whether its backend stamps capture
times), `inputs`: [`SourceInput` {`channel`, `setup`: `InputSetup` \| nil,
`cal_status`, `calibration`: `CalEntry` \| nil, `curve`: `SourceCurve` \| nil
{`reference`: `MicCurveRef`, `freq`: [f64], `gain_db`: [f64]}}]}}: all of the session an
admitted rig sees. `rig.pair` {`peer`: `RigPeer` {`rig`, `name`, `ctrl`, `data`, `key` (the
other rig's server key, Z85)}} adds one rig this rig takes inputs from, in place of an entry
for the same rig; a name another paired rig holds is `invalid` (unpair that rig first: an
entry is found by its name, and replacing it would drop that rig and its feed); `rig.unpair` {`rig`} removes that rig's entry
(`not_found` when there is none). Either leaves the other entries as they are, kept in the rig
settings; the links follow at once. A running session's joined feed starts over only when
an entry it takes inputs from changed or went; pairing or unpairing another rig leaves it
running. A rig whose network was never turned
on has no key yet, so its links stay `no_server_key`; once it has one they link with the
network off too (a link is an outgoing connection).

The stream carries every output of the session: arming routes the generator to
`settings.outputs` without reopening the stream (same session epoch; running measurements,
and on JACK every port connection, stay). Re-routing while firing fades out, switches and
fades back in. On JACK the daemon connects the outputs the operator chose for the stimulus
(the generator's outputs and the session's loopback output), and only those, to the
physical playback ports of the same number as the device listing names them (output 1 →
the first playback port), removes the connections it made for outputs no longer chosen,
never touches connections made by anyone else, and connects again after any reopen.

#### Measurements (`meas.*`)

`meas.colour` sets the colour family a measurement's curves are drawn in
(`ColourFamily`: `{type: hue, index}` with `index` 0 … 7, or `{type: neutral}` (grey);
nil: the family its id gives). It is `Measurement.colour`, kept in sessions and the
autosave; the job, its averages and `config_rev` are untouched. Families are the display's:
clients take chosen families first and give the others the rest by id. `invalid` for a
math trace (drawn in its owner's colours) and for an index past 7.

`meas.layout` sets the panes a client shows while the measurement is selected, or clears
them (nil). It is `Measurement.layout`, kept in sessions and the autosave (and so in
session archives); the job, its averages and `config_rev` are untouched; `meas.update`
keeps it and `meas.delete` takes it with the measurement. Last write wins: a client puts a
layout it receives for the measurement it has selected on screen and never sends it back.
A `PaneLayout` is a tree: `{type: pane, pane: LayoutPane}` or `{type: split, axis: row |
column, ratio, a: PaneLayout, b: PaneLayout}` (`row`: `a` left; `column`: `a` on top;
`ratio`: the share `a` takes, strictly between 0 and 1). `LayoutPane` is {`kind`,
`transfer_view`, `spectrum_view`, `spectrum_camera`, `spl_view`, `sweep_view`,
`programme_view`, `dynamics`, `chrome`, `content`, `levels`, `plates`}
(snake_case enums, `ac2_proto::model::layout`): its kind (`transfer`, `spectrum`, `spl`,
`distortion`, `programme`, `walk`, `dynamics`), the view of each kind it may be turned into (`spectrum_view`:
`spectrum`, `spectrum_spectrograph`, `spectrograph` or `spectrograph_3d`; `programme_view`:
`meters` or `goniometer`; a goniometer on a pane that meters no pair draws its meters), the
Dynamics pane's view and picks (`dynamics`, required: {`view`: `steps` \| `envelopes` \|
`release` \| `table`; `a`, `b`: a `DynamicsSide`, `{type: default}`, `{type: off}` (B only;
A off is `invalid`) or `{type: curve, rig, trace, input}`; `step`: {`level` dBFS, `burst` s},
finite with the burst above 0, or nil for the middle level and longest burst; `metric`: the
Steps column, `gr_end`, `attack_tau`, `attack_2db`, `settle_0_5db`, `overshoot`,
`release_tau`, `recovery_2db`, `recovery_0_5db`, `thd_attack`, `thd_settled`,
`thd_release`, `thump_start`, `thump_end` or `drift`; `thd`: Envelopes draw THD; `all_levels`:
Release draws every level of A}), where
its 3D spectrograph is seen from (`spectrum_camera`, required: {`yaw_deg` i8 within ±80,
`pitch_deg` u8 within 2 … 75, `zoom_step` i8 within −8 … 4}; a camera outside a range is
refused with `invalid`, never clamped: a rule of `spectrum_camera` alone, other fields state
their own), what it
shows (`content`, required, a tagged map, each reference carrying the rig whose id it is), the
level axis ranges the operator set on it (`levels`: {`transfer`, `spectrum_dbfs`,
`spectrum_spl`, `distortion`}, each {`lo`, `hi`} dB or nil, nil being framed by the client
when the pane first shows a curve) and where its readout plates sit (`plates`: {`legend`,
`readout`, `mark`, `peaks`, `harmonics`, `loudness`}, each {`at`, `scale`}; a `plates` map
holds all six; a pane kind without a plate keeps its slot at the default; `harmonics` is the spectrum's reading
of the generator's sine (§3.2 *Harmonics of the generator's sine*); `loudness` is the Programme pane's loudness
readout, docked beside or above the bars and sized from the room it gets while `at` is nil
(its `scale` then only sizes it once floated): `at` nil for the client's own placement or
[x, y], each in 0 … 1, the plate's top-left corner as a fraction of the room the plot leaves
around the plate, so a dropped plate stays inside its plot at any pane size; `scale` the
plate's text size, 1 being the theme's).

`PaneContent` is one of:

| `type` | fields | on |
|---|---|---|
| `nothing` | — | any pane: nothing picked |
| `curves` | `entries: [PaneEntry]`, 1 … 33, the lead first | `transfer`, `spectrum`, `distortion`, `dynamics` |
| `meter` | `rig`, `meas` | `spl`: its one meter |
| `inputs` | `rig`, `inputs: [u16]`, 1 … 8 zero-based device inputs | `programme` |
| `pair` | `rig`, `left: u16`, `right: u16` | `programme`: the stereo pair it meters |
| `walk` | `rig`, `walk: WalkId` | `walk` |

A `PaneEntry` is {`rig`, `what`}; `what` is `{type: group, meas}` (a measurement's group: its
live curve, its math traces, its shown stored traces, what is filed under it now and later),
`{type: live, meas}` (one live curve, a measurement's or a math trace's) or `{type: trace,
trace}` (one stored trace). The lead names the pane and owns its readouts; the entries after
it are drawn behind it, nearest first, tagged as compared. `rig` is `{type: this_rig}`, the
daemon storing the layout, or `{type: rig, id: RigId, name}`, another rig by its id (never
its CURVE key), whose ids are that daemon's, kept opaque here and resolved by the client;
`name` is for the operator while that rig is absent.

`invalid` for a ratio out of range, for more than 16 panes, for a level range not finite
with `hi` above `lo`, for a plate `at` outside 0 … 1 or a `scale` not finite and positive,
for content on a pane of another kind (the table's last column), for `curves` with no entry,
more than 33 or one twice, for inputs none, more than eight or one twice, for a pair of one
input, and for a `live` or `trace` entry of this rig that a `group` entry in the same pane
already draws (the trace filed under that measurement, its live curve or a math trace it
owns). Ids of this rig are not looked up when stored: deleting a measurement, a trace or a
walk takes its entries out of every layout (an emptied pane becomes `nothing`; one more
`measurement` change per layout it was in), and so does loading a session. Another rig's
entries are kept as they are. The focused pane, a maximised or full-screen pane and the
window are each client's; a session file's layout that is not valid is dropped on load.

`meas.offset` sets the display offset of the measurement's live curves, in dB: display
only (its frames carry the measured levels), drawn in every pane that
draws the curve. It is `Measurement.offset`, kept in sessions and the
autosave; the job, its averages and `config_rev` are untouched. `invalid` unless finite and
within ±200 dB; a session file's that is not is 0 on load.

`meas.eq` edits the measurement's EQ what-if sets (`Measurement.eq: [EqSet]`, transfer and
sweep measurements; `docs/design/eq-overlay.md`); the job and `config_rev` are untouched.
`EqEdit` (tagged by `type`): `new` {`name` \| nil (the next free `EQ n`), `realisation` \|
nil (digital at the session rate), `peak_q` \| nil (`half_gain`), `sections`} — also
predicts the live curve —, `put` {`set`, `body`: `EqBody`} (replaces what the set holds,
`rev` + 1), `delete` {`set`} (its filter curve and predictions go with it), `mark` {`set`,
`clear`} (snapshots the set as `in_console`, or clears it), `predict` {`set`, `curve`:
`Operand` (`meas` = this measurement's live curve, or `trace` = one of its transfer
traces), `on`} (creates or deletes the math trace `curve × EQ`), `copy` {`set`, `to`}.
`EqSet` = {`id`: `EqSetId`, `rev`, `body`, `in_console`: `EqSnapshot` \| nil, `trace`: the
set's filter curve, a locked transfer trace with `source.eq` {`meas`, `set`, `rev`} whose
columns the daemon recomputes on every edit and load (`rev`: the set's rev they are of; a
client refetches the data when it changes)}. `EqBody` = {`name`, `realisation`
(`digital` {`rate`} \| `analog`), `peak_q`: `PeakQ` (`half_gain` \| `minus3db_from_peak` \|
`band_pass`, the default reading of a peaking section's Q), `enabled`, `sections`:
[`EqSection` {`enabled`, `shape`: `EqShape`}]}; `EqSnapshot` = {`set`, `rev`, `body`}.
`EqShape` (tagged by `type`): `peaking` {`f`, `gain`, `bw`, `q_measured`: `PeakQ` \| nil},
`low_shelf` / `high_shelf` {`f`, `gain`, `order`: `first` \| `second` {`slope`:
`ShelfSlope`}, `f_at`: `midpoint` \| `corner`}, `high_pass` / `low_pass` {`f`,
`alignment`: `butterworth` \| `linkwitz_riley` \| `bessel` {`norm`: `phase` \| `delay` \|
`minus3db`}, `order`}, `high_pass_q` / `low_pass_q` {`f`, `q`}, `notch` / `band_pass` {`f`,
`bw`}, `all_pass_1` {`f`}, `all_pass_2` {`f`, `q`}, `gain` {`gain`}, `polarity`, `delay`
{`seconds`}, `biquad` {`b0`, `b1`, `b2`, `a1`, `a2`, `rate`}, `response` {`trace`}.
`Bandwidth`: `q` {`value`} \| `octaves` {`n`}; `ShelfSlope`: `q` {`value`} \| `s` {`value`} \|
`octaves` {`n`}. `invalid` with the section's number and reason for a refused section (f at
or above fs/2, Q outside 0 … 100, gain beyond ±30 dB, an order the alignment lacks, an
unstable biquad, a `response` naming no transfer trace…), for more than 8 sets or 64
sections, and for a `predict` curve not of this measurement. A capture records the marked
snapshots of its measurement in `TraceMeta.eq_in`; a math capture of a prediction records
the set as it was (its columns contain it).

```
MeasConfig     { name: string, kind: MeasKind, scope: MeasScope }
MeasKind = {type: "transfer", config: TransferConfig} | {type: "spectrum", config: SpectrumConfig}
         | {type: "rta", config: RtaConfig} | {type: "spl", config: SplConfig}
         | {type: "math", config: MathConfig} | {type: "sweep", config: SweepConfig}
         | {type: "dynamics", config: DynamicsConfig}
TransferConfig { reference_input: u16, measurement_input: u16, averaging: TfAveraging,
                 resolution: Resolution, smoothing: Smoothing | nil, depth: DepthPolicy,
                 lf_window: LfWindow, peak: PeakMode }
SpectrumConfig { input: u16, resolution: SpecResolution, window: Window,
                 averaging: SpecAveraging, smoothing: SmoothingFraction | nil,
                 level: SpecLevel, display: SpectrumDisplay }
RtaConfig      { input: u16, fraction: BandFraction, f_lo: Hz, f_hi: Hz,
                 weighting: Weighting, averaging: SpecAveraging, display: SpectrumDisplay }
```

`MeasConfig` = {`name`, `kind`, `scope`: `MeasScope`}. `scope` is `session` (the working
session's: saved in session files, replaced by a load) or `rig` (the rig's, SPL meters only:
kept with the daemon's rig settings, never in a session file; a load, a new session, a salvage
or the autosave restore leaves it running with its log, Leq history and judgements, and a
restarted daemon brings it back with its id). A `meas.update` changing `scope` moves the
measurement between them under the same id; a change to a rig measurement is refused
(`refused`) when the rig settings cannot be written. Session and rig measurements share one id
counter; a loaded session file whose ids rig measurements hold has those measurements (and
every reference to them) numbered past the counter. Its SPL log is kept apart from the
autosave (`ac2d --rig-logs`) and carries on after a restart. Traces are always the session's:
a stored trace or math trace filed under a rig measurement is saved in a session file without an
owner (as an import), and a load replaces it like any other.

`meas.update` carries `base`, the measurement's `config_rev` the edit was made on. Another
`config_rev` (a client changed it since) is `conflict` with `detail: conflict {rev}` (the
current `config_rev`): the client re-reads the measurement rather than overwrite that edit.

`meas.update` replaces the configuration and restarts a running job (averages start over;
a spectrum's new `resolution` (§3.2 *Spectrum resolution*) or `window` included: frames of
the new `config_rev` carry the new grid's `grid_id`; a spectrum named by a math trace
keeps its `resolution`, `refused` otherwise), except when a transfer or spectrum measurement's configuration differs only in `smoothing`
(and the name): display smoothing then changes in place, averaging goes on, and the first
`tf` / `spec` frame with the new `config_rev` carries it (`TfMeta.smoothing`,
`SpecMeta.smoothing`). Smoothing is fractional-octave (`SmoothingFraction`: `third` \|
`sixth` \| `twelfth` \| `twenty_fourth` \| `forty_eighth`; Hann kernel in log frequency, 1.5×
the nominal width so its noise bandwidth is the nominal one) and applies to transfer
functions and narrowband spectra; RTA bands already are fractional-octave.

- **Transfer** (`TransferConfig.smoothing`: `Smoothing` \| nil; `Smoothing`: `fraction`,
  `mode` `magnitude` \| `magnitude_phase`). Magnitude is power-averaged on the log grid;
  `magnitude_phase` (what the front ends set) also averages the phase, unwrapped within each
  run of valid columns. Unwrapping follows the smaller step between neighbouring columns, so
  a residual delay `τ` with `τ·Δf > ½` between columns (48 ppo: `τ·f > 34`, e.g. 3.4 ms at
  10 kHz) cannot be followed: set the delay first. `magnitude` keeps the measured phase.
- **Spectrum** (`SpectrumConfig.smoothing`: `SmoothingFraction` \| nil). Bin power is
  averaged over the kernel on the linear FFT bins, each bin weighted by its width in log
  frequency (`1/f`); DC and bins narrower than the kernel pass through, and bins without
  power (no finite level) are gaps the kernel never crosses. Smoothing runs on every bin,
  before the bins are gathered into the frame's display columns (§5.4). A smoothed bin is
  no longer the tone level of that bin: frames say so (`SpecMeta.smoothing`) and clients
  label it. A density (`level: density`, below) is smoothed as a density: the kernel
  averages power per hertz.
- **Spectrum and RTA display** (`SpectrumConfig.display`, `RtaConfig.display`:
  `SpectrumDisplay`): how clients draw the measurement, kept with it so every client draws
  it the same and a session file restores it. `style` (`SpectrumStyle`: `line` \| `fill` \|
  `bars`; narrowband curves asked for `bars` draw as `line`), `peak` (`PeakMode`: `off` \|
  `hold` \| `track`: with `hold` clients draw a peak-hold curve they keep themselves, with
  `track` they mark and list the curve's highest peaks and deepest dips, found by
  themselves, and draw no hold), `spectrograph_span_s` (u32: 10 \| 30 \| 60
  \| 120, `invalid` otherwise) and `spectrograph_colours` (`Colormap`: `viridis` \| `magma`
  \| `inferno` \| `cividis` \| `grey` \| `ice`). A configuration that differs only in `display`
  (and the name) changes nothing the daemon computes: the job, its averages and the
  measurement's `config_rev` stay as they are. A math trace on the spectrum pane draws
  with its owner's display.
- **Transfer peak mode** (`TransferConfig.peak`: `PeakMode`, as a spectrum's
  `display.peak`): what clients draw over the live magnitude, the hold and the marks their
  own. A configuration that differs only in `peak` (and the name) changes nothing the
  daemon computes: the job, its averages and the measurement's `config_rev` stay as they
  are. A math trace on the transfer pane draws with its owner's mode.
- **SPL** (`SplConfig`): any change on the same `input` — `weighting`, `time_weighting`,
  `peak_weighting`, the Leq windows and peak limits (`leq: LeqConfig`), the measuring-position
  correction (`position`), the name — applies in place; the
  meter, its log and its windows go on (§3.2, SPL log). The meter runs every frequency
  weighting with every time weighting (and the peak with C and Z) all the time and reports
  the configured ones, so the first `spl` frame with the new `config_rev` reads the new
  weighting settled, and its `lmax`, `lmin`, `leq`, `lpeak` and `duration` cover the same
  interval as before the change (since the meter started or `meas.reset`). A new `input`
  restarts the meter.
- **Math trace** (`math`, `MathConfig`: `owner`, `domain`, `expr`, `reference`,
  `smoothing`). Design: `docs/design/math-traces.md`. `owner` (`TraceOwner`, below): the
  measurement it is listed under — front ends give it the one selected when it was made —
  or `imported`; its captures are filed under the same owner. A math trace or a
  measurement that does not exist as owner is `invalid` / `not_found`. A result the daemon computes from operands named
  by id — live measurements and stored traces — and publishes like a measurement of its
  `domain`'s kind: `transfer` (`tf`, combined as complex values), `spectrum` (`spec`, tone
  levels on one bin grid) or `rta` (`rta`, band powers on one band layout). Its `grid_id`
  is the published grid (a transfer result's is known before it runs; a spectrum's is the
  display grid of its operands' FFT, its capture every bin).
  - `expr` (`MathExpr`, tagged by `type`): `binary` {`a`, `op`, `b`}, `average` {`of`:
    2 … 64 distinct operands when created (`meas.update` may leave fewer, and the frames
    then carry no average, `FEW_OPERANDS`), `method` `power` \| `complex` \| `coherence_weighted`} or
    `align` {`a`, `b`, `band`: `AlignBand` {`lo`, `hi`} (Hz, 0 < `lo` < `hi`), `max_delay`
    (Seconds, 0 < … ≤ 1)}. An operand (`Operand`, tagged by `type`) is `meas` {`meas`},
    `trace` {`trace`} or `eq` {`meas`, `set`} (an EQ set: `B` of a transfer `multiply`
    only; the set's response, or its change `H_now / H_in` when `A` recorded the set in
    console — `TraceMeta.eq_in`, or the set's `in_console` for a live `A`).
  - `align` (transfer only; `docs/design/alignment.md`): the delay and polarity that make
    two systems add in their overlap `band`. Its frame's `math.align` holds the fit
    (`AlignState`, §5.4) and the frame's `align_*` arrays the per-column curves; its `tf`
    curve is the aligned sum `A + B` with the fit applied to the delayed one, phase
    referred to A's delay, coherence none. Needs phase and one time base, as a sum does.
  - `op` (`MathOp`): `divide` (A ÷ B: A relative to B), `multiply` (A × B: the cascade),
    `add` (transfer: the complex sum, what A and B sum to acoustically; levels: the power
    sum `10·lg(10^{a/10} + 10^{b/10})`), `subtract` (transfer: the complex difference;
    levels: the level difference in dB). Levels average on `power` only. A level's
    `multiply` / `divide` is a level through a filter (or with it removed): B is a stored
    `transfer` trace (a captured transfer function or an imported impulse response) and the
    result is A ± B's magnitude in dB per column, phase none. On a spectrum B's magnitude is
    interpolated at each bin; on an RTA each band takes the power mean of B's columns inside
    the band edges (the gain pink noise sees through the filter; B interpolated at the
    centre when no column falls inside, NaN outside B's range). B's scale, mic curve and
    time base do not matter; a live B is `invalid`.
  - `reference` (`MathReference`, tagged by `type`): `operand` {`operand`} = the delay that
    operand was measured with (a live one's newest; kept while it is left out), or `fixed`
    {`delay`}: what the phase of a transfer sum, difference or average is referred to.
  - `smoothing` (`Smoothing` \| nil): transfer and spectrum domains (`invalid` for RTA),
    applied to the result; the operands are combined unsmoothed. Only `smoothing` changes
    in place; any other change restarts the channel.
  - Transfer phase and time base (decision 8a): every operand is converted from its own
    inserted delay, so operands of one session epoch (live ones, and captures of that
    epoch) combine with their relative arrival. A ratio states `delay` 0 (its phase keeps
    A's arrival relative to B), a cascade `τa + τb`; neither needs a shared time base, and
    without one its phase is of each operand's own alignment (`PhaseBasis`
    `own_alignments`). A sum, difference, `complex` or `coherence_weighted` average needs
    phase in every operand and one time base: `invalid` at create, and at run time an
    operand of another time base is left out as `mismatch`. A `power` average across time
    bases keeps the magnitude only (`no_phase`), as does a ratio or cascade with an
    operand without phase (a target: allowed in `divide` and `multiply` only).
  - Coherence: `divide` and `multiply` carry the lower γ² of the two operands per column; a
    sum or difference none (NaN); an average the plain mean of its operands' — display
    masks, never estimates.
  - Whenever a frame is due the daemon asks every live operand for its current unsmoothed
    result (as `trace.capture` does) and combines it with the stored operands' columns
    (their mic curve baked in, resampled onto a transfer result's grid; spectra and RTA
    bands must share one grid). A live operand stopped, without a usable result, whose
    frame carries `CLIP`, `NO_REFERENCE`, `CHECK_ROUTING` or `NO_SIGNAL`, or that does not
    combine (another scale, grid or time base) is left out of that frame; without both
    operands of a `binary` expression, or two of an `average`, every column is NaN with
    `FEW_OPERANDS`. A column has a value only where every included operand has one
    (otherwise its validity is the union of theirs). The frame's `math` meta says which
    operands went in and what the phase is relative to (§5.4).
  - Refused at create / update: an operand that does not exist (`not_found`), of another
    kind than the domain, a math trace as an operand, live transfer operands on
    different grids, `a` = `b`, a duplicate in an average (`invalid`). While a math trace
    names a measurement, `meas.delete` of it (unless the math trace goes with it: owned by it,
    `traces: delete`) and a `meas.update` that changes its kind,
    grid, FFT length or band layout are `refused`; `trace.delete` of a named trace is
    `refused`; `trace.mic_curve` on it restarts the math trace with the corrected columns.
    `meas.reset` of a math trace is `invalid` (reset its operands).

- **Sweep measurement** (`sweep`, `SweepConfig`: `reference_input`, `measurement_inputs`
  ([u16], 1…16 mics recorded by the same play, each its own trace; none twice, none the
  reference), `outputs` ([u16], the speaker's and the loopback's), `level: Dbfs` (typed, no default),
  `sweep: EssSpec`, `repeats` (1…8), `gate: Seconds | nil`, `tail: Seconds | nil`,
  `lf_harmonics: LfHarmonics`, `resolution: Resolution`; §3.2 *Sweep runs*). Settings
  only: it publishes no stream, `running` stays false, and `meas.start` / `meas.stop` /
  `meas.reset` of it are `invalid`. `sweep.run` plays it (below); each run is
  a stored `sweep` trace it owns. `meas.update` changes the settings for the next run.
  `invalid` at create / update: no measurement input or more than 16, one listed twice or
  equal to the reference, outputs empty or repeated, repeats outside
  1…8, a non-finite or positive level, a gate ≤ 0, a tail above 20 s, sweep parameters the
  generator refuses at the session's rate (checked again at every run, as are the
  session's inputs and outputs and the ceiling).

- **Dynamics measurement** (`dynamics`, `DynamicsConfig`: `reference_input`,
  `measurement_inputs` ([u16], 1…8, each a part of the same trace; none twice, none the
  reference), `outputs` ([u16]), `carrier: Hz` (≥ 20 Hz; snapped at every run to a whole-sample
  period, §3.2 *Dynamics runs*), `onset: DynamicsOnset` (`zero` \| `peak`), `pilot: Dbfs` (the
  rest level), `bursts: [Dbfs]` (1…8, each ≤ 0 and above the pilot), `durations: [Seconds]`
  (1…16, each > 0 and ≤ 10 s; front ends send 1, 3, 10, 30, 100, 300 ms, 1, 3, 10 s), `rest_max:
  Seconds` (2…30 s; front ends send 30 s)). Settings only, as a sweep measurement: no stream,
  `meas.start` / `meas.stop` / `meas.reset` of it are `invalid`; `dynamics.run` plays it and
  each run is one stored `dynamics` trace it owns. `invalid` at create / update: settings
  outside those bounds. A burst above the ceiling is accepted at create / update (and in a
  loaded session), as a sweep's level is: `dynamics.run` refuses it against the ceiling in
  force when it plays.

#### Ownership (`TraceOwner`) and `meas.delete`

Design: `docs/design/measurement-tree.md`. Every stored trace has an owner,
`TraceEdit.owner` (`TraceOwner`, tagged by `type`: `meas` {`meas`} or `imported`), and so
does every math trace (`MathConfig.owner`). A capture is owned by the measurement it came
from, a math capture by the math trace's owner, a sweep run by its sweep measurement, an
average by the owner its inputs share (else `imported`), an import and a generated target
by the `owner` its command names. Moving a
trace is a `trace.update` with another `owner` (allowed on a locked trace); moving a math
trace a `meas.update` with another `MathConfig.owner`. An owner must be an existing
measurement other than a math trace (`not_found` / `invalid`).

`meas.delete {meas, traces}` deletes the measurement and, by `traces` (`OwnedTraces`):
`keep` — its stored traces and math traces stay, their owner becomes `imported` (one `trace` /
`measurement` event each); `delete` — they are deleted with it (locked traces included:
the operator chose to). Refused (`refused`) while a math trace that stays names the
measurement or a trace deleted with it as an operand, and while a run of the measurement
plays or analyses.

#### SPL log and Leq windows (`spl.log_get`, `spl.log_new`, `leq` frames)

Design: `docs/design/leq.md`. `LeqConfig` = {`windows`: [`LeqWindow`] (at most 8, in
display order), `horizon`: Seconds (1 s … 1 h, whole seconds; front ends send 60), `peaks`:
`PeakLimits` {`lcpeak`, `lafmax`: `PeakLimit` \| nil}}. `LeqWindow` =
{`duration`: Seconds (1 s … 24 h, whole seconds), `weighting`: `a` \| `c` \| `z`, `limit`:
DbSpl \| nil, `warn_margin`: Db ≥ 0 (front ends send 3), `cue`: `Cue` \| nil}. `PeakLimit` = {`limit`:
DbSpl, `warn_margin`: Db ≥ 0, `cue`: `Cue` \| nil}. `SplConfig.cue`: `Cue` \| nil, the
meter's own cue (§3.2 *Rig monitor, OSC and Art-Net cues*). `SplConfig.position`: `PositionCorrection` \| nil = {`level`: Db, `peak`: Db, each
within ±30}: the measuring-position correction (`docs/design/leq.md`, *Measuring-position
correction*), added to every level the meter reports while calibrated — `level` to the
energy levels (the time-weighted levels, Leq, the windows, headroom, run totals, LAFmax),
`peak` to the peak levels (`lpeak`, LCpeak) — and to what limits are judged on; uncalibrated
meters ignore it. Front ends create meters with LAeq over 1, 5, 10, 30 and 60 min, no limits
and no correction. A configuration outside these bounds is `invalid` at `meas.create` /
`meas.update`.

Every running SPL meter integrates its input (after the mic curve, when on) into one-second
blocks of A-, C- and Z-weighted energy on a grid of whole seconds from its first sample;
lost samples (a capture discontinuity) move the grid on without energy or measured time.
Each second is a log row, `SplLogRow` = {`start`: WallNs, `measured`: Seconds (< 1 next to
a gap), `laeq`, `lceq`, `lzeq`: Dbfs over the measured time, `lcpeak`, `lafmax`: Dbfs, the
second's highest C-weighted peak and A-weighted Fast level (both kept through a `meas.reset`),
`sensitivity`: Db \| nil (dB SPL of 0 dBFS in force), `position`:
`PositionCorrection` \| nil (the correction in force while calibrated; never included in the
row's levels, which are what was measured)}. The log belongs to the meter: it survives stopping and starting,
device reopens, config changes and daemon restarts (session files and the autosave carry
it, §7.2), keeps the newest 48 h, and is kept after `meas.reset` (a display
operation). `spl.log_get` returns `SplLogPage` {`meas`, `from`, `total`,
`rows`}: rows are numbered from the meter's first logged second; `from` says where the
returned rows start (later than asked when older rows were dropped), `total` is one past
the newest row, at most 20000 rows per reply. `invalid` for a measurement that is not an
SPL meter. `log` (`SplLogWhich`: `current` \| `previous`) picks the log the meter is writing
or the one `spl.log_new` ended last; `not_found` when there is no previous log.

`spl.log_new` ends the meter's log and starts an empty one: the windows, their judgements,
the alarms, the run clock and the total start over; the windows, limits and horizon are
kept. Rows of the new log are numbered from 0 again. The ended log is kept as the
`previous` log (in memory: until the next `spl.log_new` of the meter, the meter's deletion
or a daemon restart; session files and the autosave carry only the current log), so a
client exports it after the reset without losing the seconds in between. `invalid` for a
measurement that is not an SPL meter.

A window of N seconds covers the newest N seconds of time, measured or not; its Leq is over
the measured time in it (never extrapolated; `elapsed` < N while it fills after the log's
first second, `measured` < `elapsed` flags it incomplete). Windows rebuild from the log by
wall time when the meter's job restarts. A limit is judged only while the meter reads dB SPL
(a sensitivity calibration applies), at 0.1 dB resolution. A full window (`elapsed` = N) is
over when the rounded Leq is above the limit, near when within `warn_margin` below it or at
it. A filling window is judged on its budget, the limit as a mean square `P` times its
measured time `M` plus the `r = N − elapsed` seconds left (`docs/design/leq.md`, *Judging a
filling window*): `least` = `10·lg(E / (M + r))` is the Leq it ends at if the rest is
silent, and it is over only when the rounded `least` is above the limit (the energy `E` has
spent the budget: a certainty); else, with the Leq so far above the limit it is near and
`ON_COURSE` (at the same mean power the full window ends over it; `over_in` = `(P·(M + r) −
E) / (E / M)` s until the budget is spent), within the margin near, else ok. Once full
`least` is the Leq and the two rules agree. Headroom: a window filling for at least the
horizon (`r ≥ h`, `h` = horizon) allows the steady level that, held until it is full, spends
its budget exactly: `(P·(M + r) − E) / r`. Otherwise, with `K = N − horizon` newest seconds
staying in the window, energy `E_K` and measured time `M_K`, the steady level allowed for
the horizon is `(P·(M_K + h) − E_K) / h` (the limit itself when `K ≤ 0`). Clients floor it
to 0.1 dB; when it is ≤ 0 the window cannot recover within the horizon and `recover` gives
the time to recover playing at the limit.

States have hysteresis (`docs/design/leq.md`, *Hysteresis*): a judgement rises at once and
is lowered only when the rounded Leq is 0.3 dB under the state's boundary (the limit for
`over`, the limit less `warn_margin` for `near`) or has been under it for 10 s in a row;
until then the frame and the entity keep the higher state.

A peak limit is judged each second on the highest `lcpeak` (`lafmax`) of the newest 10
seconds (`LeqPeak::HOLD_S`), with the position correction's `peak` (`level`) and the
sensitivity added: over when its rounded value is above the limit, near within
`warn_margin` below it or at it, else ok; the hold is its dwell (one second over keeps it
over for 10 s). The `leq` frame's `lcpeak` and `lafmax` (`LeqPeak` \| nil: nil without that
limit) carry {`level`: f64, that held value in the frame's `scale` (NaN before anything was
measured), `judgement`: `LeqJudgement`}.

`spl.history_get` returns `SplHistory` {`meas`, `windows`: [`LeqWindow`] (the meter's, in
configuration order), `scale`: `LevelScale`, `step_s`: u32, `log_start`, `log_until`: WallNs
\| nil, `at`: [WallNs], `leq`: [[f32]], `over`: [[bool]]}: each window over a span of the
meter's current log, as its `leq` frames carried them. `span` (`HistorySpan`, tagged by
`type`) is `newest` {`seconds`: u32}, the newest `seconds` (at most 14400, 4 h; more is
clamped) a point a second, or `range` {`from`, `to`: WallNs, `step_s`: u32}, the seconds
ending after `from` and at or before `to`, anywhere in the log. With `step_s` 1 each second
is a point; with more, each point is a bucket of `step_s` whole seconds of Unix time (a
second belongs to the bucket its end falls in; `at` is the bucket's end, a multiple of
`step_s` s), `leq[w][k]` window `w`'s **highest** Leq in the bucket and `over[w][k]` true
when it was over in any second of it — a limit is about the worst moment. A range of more
than 4000 points (`SplHistory::MAX_POINTS`: `(to − from) / step_s`, rounded up), `step_s`
0 or `to` ≤ `from` is `invalid`. `step_s` in the reply is 1 for `newest`. `at` is the end of
each second (bucket), oldest first; a second without a row has no entry, as no frame was
sent for it, and a bucket without any second neither. `leq[w][k]` is in `scale` (NaN when
nothing was measured in the window), `over[w][k]` whether the window was over its limit
then. `log_start` and `log_until` are the start of the log's oldest kept second and the end
of its newest (nil for an empty log), whatever the span: a client knows the whole extent it
can ask for. The daemon replays the log as the meter's job computed it
(`docs/design/leq.md`, *The history strip*), from the rows of the span and the longest
window before it: a second lost while running is a gap in the windows, a stretch without
rows a restart whose windows were refilled from the rows in their span; each second judged
with the window's limit, the row's sensitivity and its position correction, a filling
window on its budget, with the hysteresis above (begun part way through a log, a state held
from before the first row read can be released up to 10 s early; the seconds before the
longest window holds only rows read are left out, unless the span starts at the log's first
row). `leq` includes the row's position correction. Only seconds after the last change of
unit within what was replayed (a calibration, or a change of the position correction) are
returned; `scale` is theirs. The windows are the meter's current ones, also for seconds
logged before they were set. Empty `at` for an empty log or a span without rows. `invalid`
for a measurement that is not an SPL meter. A client that was not connected (an app
restarted) draws the history from `newest` and continues with the frames; a client looking
back over the log asks `range` at its screen's resolution.

##### Downloading a meter's whole log

A client that gathers a meter's log for investigation (another machine, a phone viewer)
reads it with three non-mutating commands, none of which disturbs the meter:

1. **Every second, exactly:** `spl.log_get` with `log: current`, `from: 0`, `max: 20000`,
   then again with `from` = the reply's `from` + its number of rows, until that reaches the
   reply's `total` (or a page is empty). The first reply's `from` is the oldest row still
   kept (rows older than 48 h are dropped, so it is above 0 on a long log). New seconds
   keep arriving while paging: `total` grows, and the last page ends at the newest. These
   rows are the record (§7.4 writes them as CSV, as `ac2 spl leq export` does); the log
   `spl.log_new` ended last is read the same way with `log: previous`.
2. **Band levels:** `spl.band_log_get` over `[from, until)` with `step: 1` returns each
   logged second's 28 band levels, at most 3600 rows a reply: walk the span an hour at a
   time.
3. **The windows as the meter judged them:** `spl.history_get` with `range` over
   [`log_start`, `log_until`] (from any first reply) at a `step_s` that keeps it within
   4000 points, or in slices at `step_s` 1, gives each window's Leq and over-limit state
   without replaying the log client-side.

The meter's `spl_log` entity (§4.1) changes when a window's judgement changes (each window's
`LeqWindowState` {`duration`, `weighting`, `judgement`, `since`}; `LeqJudgement`: `no_limit`
\| `not_calibrated` \| `ok` \| `near` \| `over`) or a peak limit's (`peaks`: `PeakStates`
{`lcpeak`, `lafmax`: `LeqPeakState` {`judgement`, `since`}}, `no_limit` without that limit),
when the windows change and when the log starts (`started_at`). Going over and recovering
append a `LeqAlarm` {`at`, `subject`: `AlarmSubject` (`window` {`duration`, `weighting`} \|
`peak` {`quantity`: `lcpeak` \| `lafmax`} \| `band` {`duration`, `weighting`, `nominal`: Hz, the
band window's band} \| `predicted` (the band meter's predicted LAeq at the transfer's
place)), `kind`: `over` \| `recovered`, `level` (the
window's Leq or the peak limit's held level, with the correction), `limit`, `position`: Db
\| nil (the correction included in `level`)} to `alarms` (the newest 100).

The `leq` frame's `run` (`LeqRun` \| nil before the log's first second) is the log as a
whole, every second: `started_at` (WallNs of its oldest kept second), `until` (WallNs of the
end of its newest second), `measured` and `gaps` (Seconds: time measured between them, and
time not measured — capture gaps, the meter or the daemon stopped; a gap is never
silence), `trimmed` (bool: the log is at its 48 h retention, so older seconds were or may
have been dropped and `started_at` is the oldest kept), and `laeq`, `lceq`, `lzeq` (f64 in
the frame's `scale`: the energy average over all the measured time, exactly from the
seconds' energies; NaN before anything was measured). The run clock is `until −
started_at`; it carries on across app and daemon restarts as the log does.

#### Band meter (`SplConfig.bands`, `spl.band_transfer`, `spl.band_log_get`, `band_leq` frames)

Design: `docs/design/band-leq.md`. `SplConfig.bands`: `BandLeqConfig` \| nil (nil: no band
meter) = {`windows`: [`BandWindow`] (at most 64, none twice with the same band, duration
and weighting),
`predicted`: `PredictedWindow` \| nil, `correction`: `BandCorrection` {`impulse`: `none` \|
`plus5` \| `plus10`, `tonal`: `none` \| `plus3` \| `plus6`} (STM 545/2015 §13, summed, applied
to the seconds from when it is set), `transfer`: `BandTransferSet` \| nil}. `BandWindow` =
{`band`: Hz (the nominal centre of one 1/3-octave band 20 Hz … 10 kHz: the band shown,
judged and alarmed; every band is integrated and logged whatever the windows), `duration`:
Seconds (whole seconds, 1 s … 24 h), `weighting`: `a` \| `c` \| `z` (the weighting at the
band's exact mid-band frequency added to its unweighted level), `limit`: DbSpl \| nil,
`day_offset`: Db \| nil (nil: `limit` holds day and night; else `limit` is the night's,
22:00–07:00 in the daemon host's time zone, and 07:00–22:00 it is `limit` plus this), `warn_margin`: Db ≥ 0,
`cue`: `Cue` \| nil}; the
limit is at the transfer's place with a transfer, else at the mic. A range of bands is a
front-end convenience that adds one window per band; nothing range-shaped is on the wire. `PredictedWindow` = {`duration`:
Seconds, `day`, `night`: DbSpl \| nil, `warn_margin`: Db, `cue`: `Cue` \| nil} (the A-weighted level predicted at
the place, judged only with a transfer). `BandTransferSet` = {`place`: str (the operator's
name of the place the limits are for, 1 … 40 characters, `receiving room` unless named;
every line naming the place uses it), `measured_at`: WallNs, `origin`: `TransferOrigin`
(`measured`: by `spl.band_transfer`; `estimated`: typed by the operator through
`meas.update`, each band `unchecked` at the guessed attenuation or `missing`), `bands`:
[`BandTransferBand`; 28] (20 Hz … 10 kHz)}; `BandTransferBand` (tagged by `status`):
`unchecked` {`attenuation`: Db} (no background measured), `clean` {`attenuation`} (≥ 10 dB
over the background), `corrected` {`attenuation`, `margin`: Db} (3 … 10 dB over it, the
background subtracted), `unusable` {`at_least`: Db} (< 3 dB over it: a bound), `missing`. A
configuration outside these bounds is `invalid` at `meas.create` / `meas.update`. The presets
(`ac2_proto::model::BandLeqPreset`: STM 545/2015 low frequencies, eleven LZeq 60 min windows
20 … 200 Hz, night 74 … 32 dB, day 5 dB higher, predicted LAeq 60 min ≤ 25 dB at night; living room,
the same eleven windows without limits, predicted day 35, night 30 dB) replace the
windows and the predicted window and keep the correction and the transfer; they
are filled in by the front ends, the daemon sees the values.

A running SPL meter with a band meter filters its input after the mic curve, unweighted,
through the 1/3-octave bank and integrates each band per second on the meter's own second
grid (a gap moves it on without energy). The seconds are logged (§7.4, band log) unweighted
with the period of their local start, the correction and the sensitivity in force; each
band window holds every second's energy of its own bands with its weighting and
correction. Without a transfer the limits are judged at the mic as they are; with one each
band's limit at the mic is the place's limit plus the attenuation (the bound for an
unusable band; a missing band has no limit), and the LAeq at the place is predicted from
every second: each band less its attenuation, A-weighted at the band centre, summed (the
estimate from the measured bands; `at_most` with the unusable bands at their bound). A
window with a `day_offset` is judged by the night limits while it holds a night second, else by the
day limits; headroom against the set in force once the horizon (the meter's `leq.horizon`)
has passed. Judging, filling windows, headroom and hysteresis are the Leq windows' (above),
on dB SPL only (the sensitivity; the position correction is not applied to the band meter).
A band of a window going over or recovering, and the predicted LAeq, append an alarm as a
window does. The windows rebuild from the band log by wall time when the job restarts and
when the configuration changes (not for a change of the correction or the margins alone);
`spl.log_new` starts them over.

`spl.band_transfer` computes a transfer FOH → `place` and stores it in the meter's
`bands.transfer` (then applied as `meas.update`, whose reply it returns; `invalid` for a
`place` outside 1 … 40 characters). `foh`, `at_place`
and `background` are each a `BandLevelSource` (tagged by `type`): `log` {`meas`, `from`,
`until`: WallNs} — the energy average dB SPL of the band seconds of SPL meter `meas`'s current
log starting in [`from`, `until`) (the same meter moved, or another meter), without the
correction; `invalid` when none were logged there or one was uncalibrated — or `levels`
{`levels`: [DbSpl \| nil; 28]} (typed, or read from a text file of `<Hz> <dB>` lines; nil:
not measured). Per band: FOH and the place of the same steady test signal at the same level
(not necessarily at the same time or on synchronised clocks), attenuation = FOH − place,
with the background rules above. `invalid` for a meter without a band meter; for two `log`
spans of the same meter that overlap (one mic cannot be at FOH and at the place, or hear
the signal and the silence, at once); and for a `log` span of a meter without a band meter
and nothing logged there (the message says to turn it on and calibrate).

`spl.band_log_get` reads a span of the band log back: `spl_band_log` = `SplBandLog`
{`meas`, `from`, `until`, `step`, `average`: `BandLogAverage` {`seconds`: u32 (logged in
the span), `measured`: Seconds, `uncalibrated`: u32 (seconds without a sensitivity),
`levels`: [DbSpl \| nil; 28] \| nil (the energy average a `log` source gives the transfer;
nil unless every second is calibrated and something was measured)}, `rows`: [`BandLogSecond`
{`start`: WallNs, `measured`: Seconds, `period`, `correction`: Db, `sensitivity`: Db \| nil,
`levels`: [f64 \| nil; 28] (dB SPL with a sensitivity, else dBFS; without the correction)}]}.
`rows` holds every `step`-th logged second (nil: none, the average only). The reply is
bounded: more than 3600 rows is `invalid`, naming the step that fits; `step` 0 and a span
that ends before it starts are `invalid`; a meter that is not an SPL meter `invalid`, an
unknown one `not_found`. A replayed recording's meter logs at the replay's wall time: at
`realtime` pace file second t is the replay's start + t, so its spans are addressable; at
`fast` pace they are not.

The `band_leq` frame (§5.4), once a second while subscribed: one column per window, in
the configuration's order (n = the windows): `leq` (in `scale`, weighted, correction included; NaN before anything was
measured), `limit` (at the mic; NaN: none), `allowed` (headroom; NaN: none), `recover`
(Seconds; NaN: none), `leq_flags` (§5.5, the Leq windows' bits: the judgement and on course).
Meta: `scale` (`db_spl` once calibrated; limits are judged only then), `cal`, `mic_curve`,
`horizon`, `correction` (Db in force), `limits_from` (`BandLimitPlace`: `at_mic` (no
transfer: the limits as typed) \| `transferred` \| `estimated` (an estimated transfer)),
`windows`: [`BandWindowState` {`band`: Hz, `duration`, `weighting`, `elapsed`, `measured`
(Seconds), `period`, `period_after_horizon` (`BandPeriod`: `day` \| `night`)}] one per
column, in the configuration's order (the front end groups and ranks them), `predicted`: `PredictedLeq` {`duration`, `estimate`, `at_most`:
f64 (dB SPL; NaN uncalibrated), `limit`: DbSpl \| nil, `judgement`} \| nil without a transfer
or a predicted window.

#### Devices, preview and loopback detection (`session.*`)

`session.devices` answers `[BackendInfo]`, one per backend the daemon offers, the one it was
started on first: `kind: BackendKind` (`jack` \| `coreaudio` \| `asio` \| `fake`; `replay` is the
kind of a replay session and is never listed), `description` (for the
operator), `availability` (tagged by `type`: `available` \| `unavailable` {`reason`}: why,
in plain words with the remedy, e.g. `No JACK server: start JACK (e.g. `jackd -d alsa`) or
use PipeWire`) and `devices: [DeviceInfo]` (empty while unavailable). A daemon started on
real audio offers its platform's backend (`jack` on Linux, `coreaudio` on macOS, `asio`
on Windows; with no ASIO driver installed, `asio` is `unavailable` with a reason naming
FlexASIO);
one started on the simulated rig offers only that (a simulated device never stands in for
a missing real one).

`DeviceInfo`: `backend`, `host`, `id`, `name` (display name), `input` / `output`:
`DirectionInfo | nil`, `duplex_clock`, `index`, `clock_sources` ([`ClockSource`]: `name`,
`current` (bool); the clocks the driver can run on, e.g. `Internal`, `Word Clock`, `ADAT`,
the one in use marked; ASIO only, empty elsewhere; read-only, set in the driver's control
panel), `notes`. `DirectionInfo`: `max_channels`,
`rates_hz` ([{min, max}]), `buffer_frames` ({min, max} \| nil), `default_rate_hz`,
`default_buffer_frames` (u32 \| nil), `channel_names` ([string], one per channel, \| nil
where the backend does not name channels: JACK gives the port alias or short name, ASIO
the driver's channel name, Core Audio the device's channel name where it has one), `system_default` (bool: the host's default device for this direction), `spare_channels` (u16: spare inputs a session can
open beyond `max_channels`: ports tied to no physical channel, which the operator connects
other sources to with the host's own tools; spare `k` (zero-based) is input channel
`max_channels + k`; JACK and the simulated rig only, 0 elsewhere and for outputs). A host
whose devices are single-direction lists an interface (or a built-in mic and speakers) as an
input-only and an output-only device, each with `duplex_clock: unknown`.

`SessionConfig`: `backend: BackendKind | nil` (nil = the default backend), `input_device`,
`output_device` (`DeviceSelector`: `default` \| `id` {`id`}), `input_channels` ([u16],
zero-based; spare inputs listed like any other), `spare_inputs` (u16: spare inputs the
session opens; input channels from the device's `max_channels` up to `max_channels +
spare_inputs` may then be captured; more than `spare_channels`, or a channel past them,
is `unsupported`), `output_channels` (u16, a count), `sample_rate_hz`, `buffer_frames`,
`loopback` ({`output`, `input`} \| nil), `remote_inputs` ([`RemoteInput` {`slot`, `rig`,
`rig_name`, `channel` (the other rig's device input)}]: each is input channel
`REMOTE_BASE + slot`, `REMOTE_BASE` = `0x8000`, listed in `input_channels` like a local
one; at least one local input is required, the loopback input must be local, slots and
rig/channel pairs are unique, an input of this rig is `invalid` ("that input is on this
rig"), a remote rig at another rate or replaying is `refused`), `link_offset` (s \| nil:
how long the joined feed waits for remote audio; nil = two remote periods, one of this
rig's and 2 ms, at most 0.5 s). A remote input carries the session's measurements like a
local one, except calibration (`refused`: set on the rig the input is on); a recording
takes it from the joined feed, its gaps marked; a transfer function with one channel on a remote rig averages only while the two
rigs' clocks agree (`remote.transfers`, §4.1). `OpenSession` adds `backend` (the one used),
`input_device`, `output_device`, `sample_rate_hz`, `buffer_frames`, `clock`, `opened_at`,
`replay` (`ReplayInfo` \| nil: the recording a replay session plays, §3.2 raw capture
files), `clock_source` (string \| nil: the clock source the device ran on when opened,
ASIO), `driver_change` (`DriverChange` \| nil: {`from_rate_hz`, `from_buffer_frames`},
set when the session reopened after its driver reset itself at another rate or buffer
size, §4.1.1; nil after a `session.open`), `driver_reset` (`DriverReset` \| nil:
{`reason`: `DriverResetReason`, `output_stopped`: bool, the generator was emitting and its
output was cut}, set when the session reopened after a `driver_reset` stop, kept through
later recoveries, nil after a `session.open`), `stated_latency` (`StatedLatency` \| nil:
{`input_frames`, `output_frames`: the driver's `getLatencies` at the open buffer size,
`driver_buffering`: {`input_frames`, `output_frames`} \| nil, the ASIO 2.3 buffering inside
the driver, part of the two, nil when the driver does not report it}; ASIO only),
`spare_ports` ([{`input`: u16, `port`: string}]: each spare input captured and the full
name of its port in the host's graph, e.g. `ac2:in_19`; empty without spares). JACK ports
are named by input channel (`in_19` is input 19). On its own reopen (a new `session.open`
on the same daemon, a device change) the daemon reads what is connected to the spare
ports before closing and connects it again to the new ones; after the JACK server or the
daemon restarts it cannot, so the operator keeps such routes in a patchbay. `session.open` with `backend: replay` is `invalid`: recordings open with
`session.replay`. Input and output may be two devices of one backend; an output device
without outputs (or an `output_device: default` the host has none for) is `not_found`
when `output_channels > 0`. Two devices open with `clock: unknown`: they may drift, which
the loopback monitor measures while a stimulus plays (`timing.drift`, §4.1).

While a session is open the daemon meters every captured input on `session/levels` (§5.1)
whether or not a measurement runs: per-interval sample peak, 300 ms integrated RMS and clip
(held 1 s), and the programme meters of `rig.levels` (the chosen ballistic's reading, the
true peak, the max hold and the over count per input; the correlation per declared pair whose
both inputs are captured), at most 30 frames per second, published only while subscribed.
Beside them `session/gonio` carries each such pair's goniometer points (§5.4
`session_gonio`) at the same rate.

**Driver control panel.** `session.control_panel` opens the named device's driver
settings window on the daemon's machine (ASIO's `controlPanel`, where pro interfaces set
rate, buffer size, clock and routing), on the driver's own COM thread. While a session runs
on that driver the call goes to it; with no session the driver is loaded and stays loaded
(listed in full, every other ASIO driver by name only) until a session or another panel
needs the process's one driver. A session open on another ASIO driver makes it
`unsupported`. On `coreaudio` it brings up Audio MIDI Setup (where a Core Audio device's
rate, format and clock source are set) for a device that is present, `not_found`
for one that is not. The reply follows the driver's answer, or a short wait while a modal
panel stays open (`ack`, after 1.2 s); a backend without a panel is `unsupported`. A change there may make
the driver reset the session's stream (§4.1.1, `driver_reset`).

**Preview.** `session.preview` opens a device for capture only — no output stream exists,
so nothing can be emitted — and publishes meters of every input on `session/preview`
(meta names the device; `audio_sample` counts from the preview's start, `session_epoch` is
the epoch it opened in), with the same programme meters as the session's over the kinds and
pairs declared, and the pairs' goniometer points on `preview/gonio`. Reply `Preview`: `backend`, `device`, `channels` (inputs metered,
`0 .. channels`), `sample_rate_hz`, `expires_in_ms`. There is one preview per daemon, and
the last caller wins: naming the same device again renews it, another device replaces it,
whichever client asks. It closes on
`session.preview_stop`, `session.open`, `session.detect_loopback`, or when not renewed
within `expires_in_ms` (5 s). Opening it may fail where the host allows a device only one
stream (`unsupported` / `not_found` with the host's reason).

**Loopback detection.** `session.detect_loopback` needs the stimulus lease (`lease_required`
otherwise) and an explicit `level` (`refused` when nil; there is no default level); a level
above the global ceiling, or a request while the stimulus is armed or firing, is `refused`.
It is allowed with a session open. The daemon closes the preview, opens `input_device` with every input and `output_device`
with `output + 1` outputs (the same id for one device),
plays a 0.5 s pink-noise burst band-limited to 100 Hz – 10 kHz at `level` (RMS) on `output`
only — faded in and out (20 ms), under the global ceiling and the output path's peak limit —
then closes the stream. Each input's capture is cross-correlated with the burst as emitted
(over delays 0 … 0.5 s). Reply `LoopbackDetection`: `backend`, `input_device`, `output_device`, `output`, `level`,
`ranked: [LoopbackCandidate]` (every input, best first: by normalised correlation, the
earlier arrival first among equally good ones), `loopback: u16 | nil` (the first-ranked
input when its |correlation| ≥ 0.8, else nil), `clock`. `LoopbackCandidate`: `input`,
`delay: Seconds`, `delay_samples: Samples`, `correlation` (−1 … 1), `gain: Db | nil` (nil:
silent input). `clock` (`ClockRelation`) is `single_callback` (capture and playback in one callback, on
one clock) or `unknown` (they may drift). Delays are exact on a `single_callback` clock;
otherwise they share an
unknown offset (the ranking is unaffected). The burst is audited in the daemon log; it is
not generator state.

#### Delay finder (`delay.find`, `delay.insert`)

`FinderBand` (tagged by `type`): `full` (2–16 kHz), `mid` (300 Hz–3 kHz), `sub`
(20–120 Hz), `custom` {`lo_hz`, `hi_hz`}, `auto` (full → mid → sub, the first band not
refused). `observation` is the measurement block length; nil uses the audio captured so far,
up to the band's default (full 0.25 s, mid 0.5 s, sub and auto 4 s). In the sub band (and a
custom band below 150 Hz) it must be 2, 4 or 8 s; anywhere it is at most 8 s. A finder run
needs live audio: the measurement must be running.

The reply `DelayFinding`:

- `outcome: DelayOutcome`, `confidence: DelayConfidence`, `band: DelayBand`,
  `observation: Seconds` (block analysed), `candidates: [DelayArrival]` (every candidate,
  by delay, ≤ 16), `found_at: WallNs`.
- `DelayOutcome` (tagged by `type`): `accepted` {`first`, `strongest`} | `ambiguous`
  {`reasons: [AmbiguityReason]`, `ranked: [DelayArrival]` (≤ 3, the rule pick first),
  `strongest`} | `no_estimate` {`reasons: [NoEstimateReason]`}.
- `DelayArrival`: `delay`, `delay_samples` (f64, fractional), `level` (dB re the strongest),
  `phase`, `uncertainty_samples` (1 σ), `misfit`, `refined`.
- `DelayConfidence` (nil where the finder refused before reaching it): `psr_db`,
  `psr_acq_db`, `band_snr_db` (Db), `excited_fraction` (0…1), `uncertainty_samples` (of
  the rule pick), `pulse_width_samples`, `period` (Samples).

`DelayBand` is `FinderBand` without `auto` (the band actually analysed). `AmbiguityReason`:
`borderline_level`, `close_arrivals`, `merged_lobe`, `outside_refinement`.
`NoEstimateReason` (tagged by `type`): `no_reference`, `no_signal`,
`observation_too_short` (also: not enough audio captured yet), `insufficient_overlap`,
`insufficient_excitation`, `periodic_excitation` {`period`: Samples}, `low_psr`,
`low_precision`, `peak_at_search_edge`, `low_band_snr`, `generator_stopped`. The last comes
from the daemon, not the finder: the generator stopped within the audio a find would read
(the observation plus the ±1 s search span), so the finder does not run, and the refusal
is in the reply only (`delay.last_finding`, an awaited pick and tracking stay as they were).
After the generator starts, a find reads only the audio from 0.1 s after the start; too
little of it yet is `observation_too_short`. A refusal is a successful reply
with a `no_estimate` outcome, never an error; errors are kept for requests that cannot run
(not a running transfer measurement, invalid band or observation).

Every finding is stored as the measurement's `delay.last_finding`. `delay.insert` applies
it: `first_arrival` takes the accepted first arrival or, when ambiguous, the pre-selected
`ranked[0]`; `strongest` the strongest arrival; `ranked{index}` an entry of the ambiguous
list. Inserting from a `no_estimate` finding is `refused`. `delay.set` (an explicit operator
value) clears `last_finding`; a delay tracking moves keeps it. `delay.nudge` moves the
applied delay by `by` (either sign, fractions of a sample allowed) and keeps
`last_finding` (it refines that delay); like `delay.insert` and `delay.set` it resolves
`awaiting_pick`. A measurement has one delay, `applied`; the daemon keeps its offset from
the arrival apart as `nudged`: the applied delay is the *arrival* plus `nudged`. `delay.insert` sets a new arrival (`nudged` 0); `delay.set`
keeps the arrival and sets `nudged` to the value's distance from it, and `delay.nudge` adds
its step to `nudged` (both refused beyond ±10 s from the arrival); tracking compares the
finder's estimates with the arrival and moves it, keeping `nudged`. A view's shared time
base refers the live curve to the arrival, so a step moves that curve alone, like a
trace's `delay_nudge` (`docs/design/delay-no-resettle.md`, *What the keys mean on the
view*). A capture records the applied delay as `TraceMeta.delay` and `nudged` as its
`edit.delay_nudge` (the trace's offset from that arrival). Delays are not rounded to whole
samples: the finder's fractional estimate is inserted as found, and the delay in samples is
kept to 10⁻⁶ sample, so whole 0.1 ms steps (9.6 samples at 96 kHz) out and back return
exactly to the start. A change of the
delay does not restart the transfer function: each analysis stage keeps its averages,
turned to the new delay, while the change is small next to its window, and only the other
stages show `settling` again (`docs/design/delay-no-resettle.md`).

`DelayState` (a transfer measurement's `delay`): `applied: Seconds`, `applied_samples`
(f64: samples at the session rate, fraction included), `nudged: Seconds` and
`nudged_samples` (f64; the offset of `applied` from the arrival, which `delay.nudge` steps
and `delay.set` move),
`tracking` (the operator's switch), `awaiting_pick`, `last_finding: DelayFinding | nil`. An
`ambiguous` finding sets `awaiting_pick` (decision 1c): tracking is paused — it moves nothing
— until the operator resolves it with `delay.insert` or `delay.set`, or runs `delay.find`
again (a new finding that is not ambiguous clears it).

#### Traces (`trace.*`)

A trace's metadata (`TraceMeta`, §4.1) is mirrored state; its columns leave the daemon only
through `trace.get` (`TraceData`: `meta`, `mag_db`, `phase_deg` (nil = magnitude only),
`coherence` (nil = unknown), `sweep` (`SweepData` of a sweep trace, else nil), `dynamics`
(`DynamicsData` of a dynamics trace, else nil), `ir` (`TransferIr` of a transfer trace
captured with one, else nil), column order = the trace's grid, NaN = no value),
`trace.export` and `file.save`. A dynamics trace's `mag_db` is one NaN column at its
carrier; its `phase_deg`, `coherence`, `sweep` and `ir` are nil. Columns are stored as
measured: offset, polarity, nudge, smoothing and a mic curve applied after capture are
display edits and are never applied to the stored data.

- **Smoothing.** `TraceEdit.smoothing` (`Smoothing` \| nil; transfer and spectrum traces
  only — any other kind is `invalid`) is applied by the daemon when it serves `trace.get`,
  with the live job's kernel (a spectrum has no phase: its power is smoothed in either
  `mode`; a spectrum capture starts with `magnitude`); coherence is never smoothed.
  `trace.export` and `file.save` write the unsmoothed columns (the setting is listed in the
  CSV header), and `trace.average` and math traces combine unsmoothed columns. A capture
  starts with the smoothing its measurement had; an average starts with the smoothing its
  inputs share (nil when they differ).
- **Mic curve after capture.** `trace.mic_curve {trace, curve}` puts the named curve of the
  mic library (`MicCurveId` {`mic`, `label`}) on a stored trace, or removes the applied one
  (`curve: nil`; `not_found` when there is none). It is recorded as `TraceMeta.mic_curve` (`TraceMicCurve`: `mic`, `curve:
  MicCurveRef`, `f_norm: Hz`) and is a display edit: `trace.get` subtracts the curve,
  normalised to 0 dB at `f_norm`, from the magnitude after the smoothing — per column on
  log and linear grids, as the band power average on IEC bands, phase and coherence never —
  and corrects a sweep's distortion (order n at f by c(f) − c(n·f), floors alike, THD
  re-summed from the corrected orders; the impulse response untouched). `f_norm` is the
  normalisation frequency of the trace's sensitivity calibration (the calibrator's; 1 kHz
  for an electrical one), else of the newest sensitivity calibration of the mic, else 1 kHz. The daemon keeps the curve's points with the trace, so a later change to the
  calibration store does not change it. `trace.export` writes the uncorrected columns and
  names the curve in its `# mic:` line; `trace.average` and math traces combine corrected
  columns, and an average's `mic.curve` names the curve its columns now carry. Refused:
  a trace whose `mic.curve` is set (captured with the curve in its columns: a second
  correction would count it twice) and a locked trace (`refused`), a target (`invalid`),
  a curve not in the mic library (`not_found`).

- `trace.capture` stores the measurement's current `tf`, `spec` or `rta` result, formed
  for the capture whether or not anyone subscribes — the result its next frame carries
  (never older than a frame a client has seen), for a `tf` or `spec` result before its
  display smoothing, for a `spec` result with every FFT bin (grid `linear`) where the live
  frame has display columns (grid `log_bins`) — with columns whose validity mask is set
  stored as NaN.
  A transfer capture also keeps the impulse response its `ir` frame would carry, formed
  from the same state: `TraceData.ir` (`TransferIr` {`sample_rate`, `ir`: `TraceIr`},
  `t0` re the trace's `delay`), every sample (one FFT length of the full-rate stage: 4096 points, ±42.7 ms at 48 kHz). It is nil
  when the measurement has no IR yet (no block accepted with a reference); traces made by
  `trace.average`, math traces, imports of other formats and targets have none.
  It needs a result in the current session epoch (`invalid` otherwise: not running, no
  frame yet, SPL measurement). Metadata: `kind` (`TraceKind`, tagged by `type`: `transfer`,
  `target`, `spectrum` {`scale`, `level`: `SpecLevel`}, `rta` {`scale`}), `source.captured` {`meas`, `meas_name`,
  `epoch`, `at_sample`}, `edit.owner` the measurement, `delay` (the delay the DSP used),
  `depth` (transfer),
  `cal` (spectrum / RTA: the calibration the measurement used, picked by the calibration
  matching rules below — its `key` names another mic or input when it was not this mic's;
  transfer functions are ratios and always `uncalibrated`), `mic` (the input setup's mic
  name and the mic curve applied to the captured columns as its full `MicCurveRef` — label,
  file, content hash —, nil without a mic name; a sweep
  names no curve, its analysis works on the raw recordings), `mic_curve` (nil: see
  *Mic curve after capture*), `created_at`. A math trace captures as its domain's kind
  (`transfer`, `spectrum` {`scale`, `level`}, `rta` {`scale`}) with `source.math` {`meas`,
  `meas_name`, `epoch`, `at_sample`, `expr`, `operands`: [`NamedOperand` {`operand`,
  `name`}] — the operands that went into the captured result, named as at capture —,
  `phase`: `PhaseBasis`}, `delay` its stated delay, `depth` and `mic` nil,
  `uncalibrated`; `refused` when the expression lacks its operands at that moment.
- **Slots.** `TraceEdit.slot` (1…9 or nil). A slot holds at most one trace: capturing or
  updating into a slot clears it on the trace that held it (a `trace` event for that one
  too).
- **Last write wins.** `trace.update` replaces the whole `TraceEdit` and carries no
  `base` (`expect_rev` is the only guard): of two clients' edits, the later one stands whole.
- **Lock.** `trace.update` on a locked trace may change only `visible`, `order`, `slot`,
  `owner` and `locked` (not `smoothing`); `trace.delete` and `trace.mic_curve` of a locked trace are
  `refused`.
- **Time base (decisions 8a / 8b).** Captured traces (`captured`, `sweep`) share the
  time base of their session epoch, and so does a `math` capture whose `phase` is
  `shared_time_base` and whose expression is a sum, difference or average (a ratio or a
  cascade is relative, in no time base); every other source (`imported`, `generated`,
  `average`, other `math`) is independent.
- `trace.average`: ≥ 2 distinct traces of one kind (no targets; a tone and a density
  spectrum are different kinds). Transfer: `power` (RMS
  magnitude, phase of the complex mean), `complex`, `coherence_weighted` (weight
  γ²/(1 − γ²), γ² capped at 0.999); every input's phase is re-referred to `reference`
  (`DelayReference`: `trace` {`trace`} = that input's measured delay, or `fixed` {`delay`})
  before combining, and the result's `delay` is that reference. Phase methods need every
  input captured in the same epoch (`invalid` otherwise); a power average without a shared
  time base, or with an input without phase, keeps the magnitude only. Spectrum / RTA:
  `power` only, all on one grid. A column is valid only where every input is. The result is
  on the first trace's grid (others resampled).
- `trace.import`: a WAV / RF64 file (`format: ir_wav`, or `auto` on a `RIFF` / `RF64`
  header) is an impulse response (§7.6); `ir` (`IrImport`) says how to read it and is
  `invalid` (problem `bad_options`) on a text file or with `role: target`. Otherwise the
  file text is parsed by `format` (§7.1) and, unless it is an ac2 CSV
  on a known grid, resampled onto a log grid (48 points per octave, 96 when the file is
  denser) — magnitude and coherence linear over log frequency, phase unwrapped first.
  `role: target` keeps the magnitude only and makes `kind: target`. The name is the ac2
  header's `name`, else the file name without extension; `delay` is the ac2 header's
  `delay_ms` (0 for other files). An ac2 sweep export with its analysis facts and impulse
  response imports as a `sweep` trace. `source.imported` {`file_name`, `format`, `notes`,
  `ir_file`}:
  `notes` lists what the file held that the trace does not keep (`ImportNote`:
  `sweep_without_analysis` — a v1 sweep export, imported as its transfer function with the
  distortion dropped; `sweep_off_grid` — distortion is never resampled;
  `mic_curve_not_applied` — the export named a mic curve applied after capture, the
  columns are without it; `ir_truncated` — an impulse response longer than 65536 samples,
  cut around its peak). `ir_file` (`IrFile` \| nil) are the facts of an imported impulse
  response (§7.6). The trace is listed under `owner` (`TraceOwner`: a measurement, or
  `imported`): a target loaded for the system being tuned belongs with that system's
  measurement; `not_found` for a measurement that does not exist, `invalid` for a math
  trace (math owns no traces). A refused file is `invalid` with
  `detail: {type: import, line (1-based) | nil, problem: ImportProblem}`.
  `ImportProblem`: `not_text`, `no_data`, `bad_number`, `column_count`, `too_few_columns`,
  `not_ascending`, `out_of_range`, `too_many_rows` (> 65536, IR rows too; a dynamics trace's envelopes > 6 144 000),
  `bad_header`, `bad_coherence`, `not_wav` (not a readable WAV / RF64 file),
  `pick_channel` (a multi-channel file without `ir.channel`, or a channel it lacks),
  `no_impulse` (every sample zero or not finite, fewer than 2), `bad_options` (above, or
  a gate not finite, `pre` < 0 or `post` ≤ 0).
- `trace.target`: a target curve the daemon generates instead of reading a file, as a
  `kind: target` trace under `owner` (as `trace.import`'s), named `name`, `source.generated` {`curve`}, `delay`
  0. `TargetCurve` (tagged by `shape`): `flat` (0 dB); `tilt` {`slope_db_per_oct`,
  `pivot: Hz`, `lf_shelf: LfShelf | nil`} — `slope · log₂(f / pivot)` dB plus, with
  `LfShelf` {`gain: Db`, `corner: Hz`}, the cookbook second-order low shelf at slope S = 1
  with half its gain at `corner` (analog prototype); both on the 1/48-octave log grid from
  20 Hz to 20 kHz; `loudness_shift` {`from: Phon`, `to: Phon`} — how the perceived tonal
  balance of a programme balanced at `from` changes when played at `to`: the difference of
  the two ISO 226:2003 equal-loudness-level contours, each referred to its own 1 kHz value
  (`C_from − C_to`, 0 dB at 1 kHz), defined at the standard's 29 one-third-octave
  frequencies (20 Hz … 12.5 kHz) and on the log grid between them linear over log frequency.
  It describes perception, not a response to equalise to. `invalid` outside slope ±6 dB/oct,
  pivot 20 Hz … 20 kHz, shelf gain ±20 dB, corner 20 … 1000 Hz, loudness 20 … 100 phon, or
  `from` = `to`.
- `trace.export` `format: ir_wav`: the trace's impulse response (a transfer trace's `ir`,
  else a sweep's) as a mono 32-bit float WAV at its sample rate, every stored sample as
  stored (an imported file's samples come back bit-exact; `file_name` `<name>.wav`). A trace
  without one, or with one stored decimated (one point per several samples, longer than
  65536), is `invalid`.

#### Spatial walks (`plan.*`, `walk.*`)

`docs/design/spatial-walk.md`. A **plan** (`PlanSpec`: `name`, `outline` [[x, y]],
`ceiling` m \| nil, `zones` [{`name`, `outline`, `ear_height`, `weight` \| nil, `seats` \|
nil}], `markers` [{`name`, `kind` speaker \| foh \| point, `at` [x, y, z], `heading_deg` \|
nil}], `surfaces` [{`surface`, `material`}]; metres) lives in the daemon's plan library,
one `<name>.json` per plan in its data directory (`plans/`), mirrored as `State.plans`.

- `plan.save` checks the plan (`ac2_room::plan::validate`: simple counter-clockwise
  polygons, zones inside the outline, markers inside the room …) and replaces the one of
  that name; `invalid` with the reason, or for a name that is no file name (letters,
  digits, space, `-`, `_`, `.`, not starting with `.`); `refused` for a name another plan
  holds in another case. Last write wins: `plan.save` carries no `base` (`expect_rev` is
  the only guard). `plan.delete` is `not_found` for a name the library lacks; the session's
  copies stay.
- `walk.new` starts a walk (`WalkState`: `id`, `name`, `plan`, `kind` live_transfer \|
  sweep, `mics` [`WalkMic` {`meas`, `input`}], `points` [{`label`, `at`, `step: u16`, `mic:
  WalkMic`, `state` {`type`: pending \| captured {trace, context} \| skipped}}], `step: u16 |
  nil`, `average: MeasId | nil`, `room: WalkRoom | nil`) through 1 … 64 points (unique
  non-empty labels, finite, `z` ≥ 0). The plan is the session's copy when it holds one, else
  the library's, copied into `State.walk_plans` (so a library edit mid-walk changes
  nothing). A **live** walk's `meas` are 1 … 8 session transfer measurements, one per mic,
  each on its own measurement input: mic *k* is `{meas[k], its measurement_input}`. A
  **sweep** walk's `meas` is one session sweep measurement with at most 8 measurement inputs:
  its mics are `{meas, input}` for each of its `measurement_inputs`, in order. No other walk
  may name a measurement (`invalid` otherwise, `refused` for one another walk names).
- **Steps.** The daemon groups the points into steps that place every mic once
  (`ac2_room::steps::group`): one mic takes the points in walking order; with several, the
  first step puts mic *k* at the *k*-th point and each later step pairs the mics with the
  points left so the summed distance each is carried from its last point is least (fewer
  points than mics: the nearest mics go, the others sit out). A point's `step` and `mic`
  say where it belongs and which mic stands (or stood) there; the walk's `step` is where
  the mics are: each stands at its pending point of that step.
- A **move** — `walk.new`, `walk.goto`, `walk.swap`, the advance after `walk.capture` or
  `walk.skip`, `walk.redo` — resets a live walk's measurements' averaging (`meas.reset`)
  and turns their delay tracking on. A sweep walk's move resets nothing: a sweep has no
  running average or tracking, its run at the step is the capture.
- `walk.capture` captures at every pending point of the current step in one step (all or
  none), each by its point's `mic`, each trace named `<label> · <mic>` (the input's mic,
  else the measurement's name) with `position` set, and `captured_unsettled` when the frame
  was not ready: `filled` < 1, a fault refusing its blocks, a delay change after the
  averages restarted (an older config rev) or an arrival awaiting the operator's pick. It
  goes to the next step with a pending point after the current one (wrapping round), `step`
  nil when none is left. `refused` at a step with nothing pending (redo a point), `invalid`
  when `step` is nil. The point's `context` (`CaptureContext` {`at`, `mic: MicState | nil`
  (`MicState` {`name`: string, `curve`: `MicCurveRef` \| nil}),
  `eq_in: [EqMark {set, rev}]`}) records the same facts its trace records (`created_at`,
  `mic`, `eq_in`), so a client can say when a walk's captures were taken on different days
  or, for one mic, under another mic, mic curve or EQ. Calibration and generator level are
  not recorded: a transfer is a ratio of two inputs, and both cancel out of it. A sweep
  walk refuses `walk.capture` (`refused`): its capture is a run of its sweep measurement.
- **Sweep walk capture.** A `sweep.run` of a sweep walk's measurement while its mics stand
  at a step records only the standing mics' inputs (its `measurement_inputs`), plays under
  the lease and the ceiling as every run, names each trace `<label> · <mic>` (the input's
  mic, else its name as a run names it) with `position` set, and once `done` captures each
  standing point by its trace and goes to the next step with a pending point. A run that is
  refused, fails or is aborted leaves the points pending. While the walk's run plays or is
  analysed, `walk.goto`, `walk.skip`, `walk.redo`, `walk.swap`, `walk.update` and
  `walk.delete` on it are `refused`: the run records the mics where they stood. A run of
  the measurement with every point captured or skipped is an ordinary run.
- `walk.goto` puts the mics at step `step` (`not_found` past the last). `walk.skip` marks
  one point skipped (a captured one's trace becomes `superseded`; `refused` when skipped
  already); when that leaves the current step nothing pending, the walk goes on to the next
  step, else the other mics stay. `walk.redo` makes one point pending again (its trace
  `superseded`) for the mic that captured it (for a mic its step leaves free when a swap
  gave that mic another of the step's points) and goes to its step, where only the mics of
  pending points stand. `walk.swap` moves the mics at `points` (two or more pending points
  of the current step) round by one: each to the next listed point, the last to the first;
  the same exchange applies to the pending points of later steps, so every mic walks on
  from where it now stands (`refused` for a point not pending at the step). `walk.update`
  renames the walk and edits its points by label, grouping them into steps again: a kept
  point keeps its state (a captured one its position and mic too); a captured point cannot
  be taken out (`refused`). Last write wins: `walk.update` carries no `base`
  (`expect_rev` is the only guard).
- The walk's **average** is a power-average math trace of the captured points' traces,
  in point order, owned by the walk's first measurement and named `<walk> · average`: made
  (and started) at the second capture, its `of` updated after every capture, skip and redo
  (as `meas.update`, which restarts it), kept (holding no average) when fewer than two captures are left.
- A sweep walk's **room** (`WalkRoom` {`positions`, `sources` (1), `broadband`, `octave`,
  `third`: [`WalkRoomBand` {`centre: Hz | nil`, `edt`, `t20`, `c80`, `d50`:
  `PositionMean` {`mean`, `min`, `max`, `n`} \| nil}]}, ISO 3382-1) is made from its
  captures' room parameters after every capture, skip and redo: per band (matched by
  mid-band frequency), the arithmetic mean, least and greatest over the positions that
  give the parameter; `n` counts them (a position whose decay refuses it in the band is
  left out), nil when none does. `positions` counts the captures carrying room parameters.
- While a walk names them, its measurements, its average and its points' traces cannot be
  deleted, nor its measurements change kind or measurement inputs (its mics; `refused`). `walk.delete` keeps the traces
  and the average, which becomes an ordinary average of them; the plan copy goes with the
  last walk walking it.

#### Room simulation (`sim.*`)

`docs/design/spatial-walk.md` §7. The daemon simulates a plan with the `ac2-roomsim`
engine (image sources for the early reflections, ray tracing for the late field, per
octave band 125 Hz … 8 kHz) on a worker thread of its own. A simulation is not a
measurement and makes no trace; its latest run or fit is `State.sim`, its latest coverage
map `State.sim_map` (a `sim` event carries either; its `kind` says which), neither saved with
the session. One runs at a time (runs, fits and maps share the worker).

- `sim.run` {`request`: `SimRequest` {`plan`, `sources` [speaker marker names], `receivers`
  {`type`: `walk` {`walk`} \| `points` {`points` [{`label`, `at`}]}}, `settings`}} simulates
  every source at every receiver. `SimSettings` {`image_order` 0 … 4 (front ends send 3),
  `rays_per_band` 1 … 1 000 000 (front ends send 20 000, per source), `budget` s > 0 … 600
  (front ends send 10; the ray tracing's wall time, shared by the sources), `seed` (front ends send 0; the same seed
  gives the same result)}. At most 64 receivers and 64 source × receiver results. The plan
  is the session's copy when it holds one, else the library's. Surfaces carry their
  `material` by the simulator's name; an untagged wall or ceiling is `plaster_on_masonry`,
  an untagged floor `concrete`. Sources are omnidirectional points; air absorption at
  20 °C, 50 % RH. When a `sim.fit` on an identical plan came before, the run uses its
  fitted absorption (`absorption` `fitted`), else the table's (`assumed`).
- `sim.fit` {`request`: `SimFitRequest` {`walk`, `source`, `settings`}} fits the plan's
  absorption to a **sweep** walk's measured T20 (`WalkRoom`), its receivers the walk's
  captured points: one scale per octave band on every surface's absorption, from the
  Eyring scale, Newton steps on log T20 against log scale, fixed seed, stopping when every
  fitted band is within 3 % or after 6 passes (`budget` per pass). A band the walk gives no
  T20 keeps the table's absorption. `not_found` for an unknown walk.
- Both reply `sim` with the started state (`status` running) and refuse `invalid` with
  `detail` `sim` {`reason`: `SimRefusal`}: `outdoor` (no ceiling), `unknown_material`
  {`surface`, `name`}, `plan` {`reason`} (does not validate), `no_speaker` {`name`},
  `outside` {`name`} (a source or receiver not inside the room), `empty`,
  `not_a_sweep_walk`, `no_measured_t20`, `busy` (one is running), and `invalid` for
  settings out of range.
- `sim.map` {`request`: `SimMapRequest` {`plan`, `sources` [speaker marker names; their
  energies add as power], `spacing` m (front ends send 1.0), `band` Hz \| nil (a nominal octave
  centre 125 … 8000; nil: every band summed, broadband), `window` s (0 … 0.5, front ends send 0.05:
  the direct sound and early reflections), `image_order` (0 … 4, front ends send 2)}} maps the
  direct and early energy over the plan's zones, image sources only (no ray tracing): a
  grid of `spacing` squares over the zones' extent, a cell kept when its centre lies in a
  zone (the first that holds it gives its ear height) and inside the room. The result is
  `SimState.map`: `SimMap` {`spacing`, `origin` [x, y] (centre of column 0, row 0), `band`,
  `window`, `cells` [`SimMapCell` {`col`, `row`, `db`}]} with `db` the cell's level re the
  map's loudest cell, ≤ 0, floored at −40. At most 4096 cells: a finer spacing is refused
  `map_too_fine` {`cells`, `max`}; no zone, or no cell centre in one, `no_zones`; `invalid`
  for a band that is not an octave of the simulation, or a spacing or window out of range.
  The kept fit applies as to `sim.run`. A map's `settings.rays_per_band` is 0 and its
  `sources` carry no receivers.
- `sim.cancel` stops the running simulation (`refused` when none runs) and replies `sim`
  with it `cancelled` {`why`: `operator`}. A run also stops `cancelled` when its plan changes
  or is deleted (`plan_changed`; saving it unchanged does not), when the fit's walk is
  deleted (`walk_deleted`) and at `session.close` (`session_closed`).
- Progress: `sim` events with `status` `running` {`stage` `image_sources` \|
  `ray_tracing` \| `analysis`, `done`, `total`, `source` (index), `pass` (fit pass, 1-based; 0 for a run)}, at
  most five a second, then `done`, `cancelled` {`why`} or `failed` {`msg`}.
- **`SimState`** {`id: SimId`, `plan`, `kind` {`type`: `run` \| `fit` {`walk`} \| `map`},
  `walk` (`WalkId` \| nil: the walk whose points are the receivers, for a run at a walk and a
  fit), `settings`,
  `status`, `bands` [Hz] (nominal octave centres; every per-band list follows them),
  `absorption` {`type`: `assumed` \| `fitted` {`walk`, `positions`, `bands` [`SimFitBand`
  {`scale` \| nil (nil: assumed), `measured`, `simulated` (s \| nil), `converged`}]}},
  `sources` [`SimSource` {`name`, `at`, `receivers` [`SimReceiver` {`label`, `at`,
  `arrivals` [`SimArrival` {`time` s, `energy` [per band], `path` [surfaces, empty for the
  direct sound]}] (the first 48 by time), `late` `SimEchogram` {`bin` s, `energy` [per band
  [≤ 128 bins]]} (the ray-traced late energy), `room`: `RoomAcoustics` (ISO 3382-1, octave
  bands and broadband; no third-octave bands)}], `room`: `WalkRoom` \| nil (the mean over
  the receivers)}] (empty until done), `schroeder` Hz \| nil, `valid_above` Hz \| nil (below
  it the room is modal and the simulation not valid), `eyring` [s] (Eyring's T with air),
  `rays_per_band` (traced, least over the sources), `budget_exhausted`, `lost_rays`, `map`
  (`SimMap` \| nil, a map's)}. The
  room parameters are ac2-core's evaluation of an impulse response synthesised from the
  arrivals and the late field, each band from that band's energy alone: like with like
  with a measured response.
- **Kept fit:** the latest finished `sim.fit` is kept by the daemon for later runs and maps
  of the identical plan and mirrored as `State.sim_fit`: `SimFitKept` {`plan`, `walk`,
  `absorption` (`fitted`)} \| nil, carried by `sim_fit` events (nil: dropped, when its walk
  is deleted or its plan changes or is deleted). Not saved with the session.

#### Sweep runs (`sweep.run`)

Design: `docs/design/sweep-distortion.md`. `sweep.run {lease_token, meas, name}` runs the
sweep measurement `meas` (`MeasKind::Sweep`, `SweepConfig` above) with its settings:
`sweep: EssSpec` {`start: Hz`, `end: Hz`, `duration`, `fade_in`, `fade_out`}, `tail`
(silence recorded after each sweep: the room's decay and its noise, at most 20 s; nil or
shorter = the analysis minimum, ≥ 1 s), `resolution` (`Resolution`, the trace's grid, §3.2
*Resolution*), `lf_harmonics` (`LfHarmonics`, the harmonic windows
at the lowest columns, harmonics below about 1 kHz: `standard` puts every order in one shared
window, short enough for H5 — the lowest floor; `fine` puts each order in the longest window
between its neighbours' impulses, with the fundamental and the floor in that window — finer
low-frequency resolution and lower columns (a 5.5 s sweep from 10 Hz: H2 from 10 Hz where
the shared window stops near 20 Hz), a floor
higher by the window's length over the shared one, and a `post_roll` of at least four of the
longest window; it changes the recording, so it is a setting of the measurement). The emitted sweep keeps the rate but starts up to two
octaves below `start` (a whole number of cycles per rate constant, at least 1 Hz) and fades
in up to `start` in place of `fade_in`, so a path's switch-on transient lies below the
analysed band; responses are reported from `start`. `fade_in` applies to a `gen` ESS and to
a sweep run whose start cannot be extended lower (a start of about 1 Hz, the lowest emitted
start); otherwise the onset fade is derived. `name` nil names the trace `Run <number>`.

- Refused (`refused`) above the ceiling, or while the generator is not armed by the caller
  (arm with `gen.set` first: like firing, the run needs it), while it fires, while a
  loopback detection or another sweep runs, or without an open session. `invalid`: not a
  sweep measurement, inputs not captured, outputs not in the session, sweep parameters the
  generator refuses at the session's rate.
- The daemon routes the generator to `outputs`, plays `repeats` synchronised sweeps each
  followed by its silence (`post_roll`, ≥ 1 s and ≥ `tail`), records the reference and every
  measurement input, analyses each against the reference on a job thread and stores one trace
  of `kind: sweep` per measurement input, owned by the measurement (source `sweep` {`meas`,
  `meas_name`, `run`, `number` — one more than the highest run number of the measurement's
  stored runs —, `epoch`, `sweep`, `level`, `repeats`, `lf_harmonics`, `reference_input`,
  `measurement_input` — the trace's own input}, `delay` = its arrival). The traces are named
  the run's `name`; with several inputs each adds ` · ` and its input's name (the mic name,
  else the device's channel name, else `Input N`). Each trace carries its own input's mic and
  active mic curve (`mic`, `mic_curve`), as every measurement on that input does. One input's
  refused analysis fails the whole run (`msg` names the input). While it plays
  the generator is `firing` with the sweep as its settings; once the recording is in it is
  disarmed (`last_action` `stop` by the daemon; the lease stays with its holder), so the next
  sweep or stimulus needs an explicit arm.
- Progress and outcome are the `sweep` entity (§4.1), `SweepRun`: `id`, `meas`, `owner`,
  `name`,
  `reference_input`, `measurement_inputs`, `outputs`, `level`, `sweep`, `sweep_duration`
  (actual, of each emitted sweep), `post_roll`, `repeats`, `gate`, `lf_harmonics`, `resolution`, `started_at`, `status` (`SweepStatus`, tagged by
  `type`): `playing` {`repeat`, 1-based} → `analysing` → `done` {`traces`: one per measurement input, in
  `measurement_inputs` order} or `failed`
  {`reason`, `msg`}. `SweepFailure`: `stopped` (`gen.stop`, `gen.release`, forced takeover),
  `lease_expired`, `session_closed`, `dropout` (audio lost while recording), `no_reference`
  (the reference carries no sweep), `analysis`. A failed run's audio is discarded.
- `trace.get` of a sweep trace: `mag_db` / `phase_deg` are the fundamental's response (mic re
  reference, phase referred to the arrival; NaN outside the sweep), `coherence` nil, and
  `sweep` (`SweepData`): `harmonics` ([`HarmonicCurve` {`order`, `curve`}], H2…H5), `thd`,
  each `DistortionCurve` {`level_db`, `floor_db`} in dB re the fundamental per grid column
  (harmonic k at k·f is reported at the fundamental f; NaN where that order is not measured);
  `reference_harmonics` ([`HarmonicCurve`] \| nil: the reference input's own H2…H5,
  separated against the emitted sweep as the measurement's are, re its own fundamental; nil
  only for a sweep imported from an export written without them);
  `ir` (`TraceIr` {`t0`, `dt` re the arrival (the trace's `delay`), `linear`, `etc_db`}, from H5's window to the
  end of the linear window, at most 16384 points, peak-preserving); `info` (`SweepInfo`:
  `sample_rate`, `rate` (L: harmonic k's impulse at −L·ln k), `duration`, `repeats`,
  `arrival`, `reference_level` (loopback gain), `window_pre`, `window_post`, `gate_pre`,
  `gate`, `floor_margin`, `clipped`); `room` (`RoomAcoustics | nil`, below; nil only for a
  sweep imported from an export written without it); `unresolved` (`SweepUnresolved | nil`,
  §3.2 *Resolution*; nil only for a sweep imported from an export written without it). A distortion point is valid when
  `level_db ≥ floor_db + floor_margin`; otherwise it reads "< floor".
- `RoomAcoustics` (ISO 3382-1 room parameters of the full-rate impulse response, design
  `docs/design/room-metrics.md`): `broadband` (`RoomBand` of the IR as captured), `octave`
  ([`RoomBand`], 63 Hz…8 kHz) and `third` ([`RoomBand`], 50 Hz…10 kHz), each only bands
  whose edges lie inside the sweep's range; `span_end` (`Seconds`, end of the IR analysed re
  the arrival: the end of the silence after the sweep). `RoomBand`: `centre: Hz | nil` (nil =
  broadband), `onset` and `truncation` (`Seconds` re the arrival: the band's trigger and where
  its decay meets the noise), `decay_range` (`Db | nil`: depth of the decay curve there, nil
  without a decay), `edt`,
  `t20`, `t30` (seconds), `c50`, `c80` (dB), `d50` (ratio 0…1), each a `RoomValue` (tagged by
  `type`: `value` {`value`} or `refused` {`reason`}), and `curvature` (`f64 | nil`, percent
  100·(T30/T20 − 1) when both are given). `RoomRefusal` (tagged by `type`): `no_decay`,
  `insufficient_range` {`range`, `needed`} (`Db`; EDT, C50, C80, D50 need 20 dB, T20 35 dB,
  T30 45 dB), `filter_limited` {`bandwidth_decay`} (bandwidth × decay time below 8). A refused
  value is never sent as a number.
- Smoothing, averages and math traces treat a sweep trace as a transfer function (magnitude and
  phase; the distortion stays with the sweep trace).

#### Dynamics runs (`dynamics.run`)

Design: `docs/design/dynamics.md`. `dynamics.run {lease_token, meas, name, rests_from}` plays
the dynamics measurement `meas`'s programme, all in whole carrier periods: a linearity check
(`pilot − 6 dB`, then `pilot`, 2 s each), then for each burst level (outer) and length (inner)
a burst at that level followed by a rest at the pilot, then a 20 ms fade. The carrier is
snapped to the nearest frequency whose period is a whole, even number of samples (a multiple
of 4 for `peak` onset) at the session's rate; every burst starts and ends on an onset point.
A rest ends at the first onset point after the analysis has seen the processor recover
(gain within 0.05 dB of its baseline, at least 2 s), or at `rest_max`, which the source counts
itself; with `rests_from` (a dynamics trace) the rests are those of that trace, replayed, so
an A/B pair plays an identical programme. The generator's level is the highest burst level
(crest factor √2), checked against the ceiling at every run. `name` nil names the trace
`Run <number>`.

- Refused (`refused`) above the ceiling, while the generator is not armed by the caller,
  while it fires, while a loopback detection, a sweep or another dynamics run plays, or
  without an open session. `invalid`: not a dynamics measurement, inputs not captured,
  outputs not in the session, a carrier the session's rate cannot play (period below 12
  samples), `rests_from` not a dynamics trace or with another step count.
- While it plays the generator is `firing` with `sine` {`freq`: the carrier played} at the
  highest level as its settings, and `gen.set` is `refused`; stop, release, lease expiry
  and a closed session abort it with the ordinary fade-out. When the last rest is in, the
  generator is disarmed (as after a sweep) and one trace of `kind: dynamics` is stored, owned
  by the measurement (source `dynamics` {`meas`, `meas_name`, `run`, `number` — one more than
  the highest run number of the measurement's stored runs —, `epoch`}), `cal`
  `uncalibrated`, on a one-column grid at the carrier (`log`, 48 per octave) whose column is
  a gap.
- Progress and outcome are the `dynamics` entity (§4.1), `DynamicsRun`: `id`, `meas`,
  `owner`, `name`, `reference_input`, `measurement_inputs`, `outputs`, `carrier` (played),
  `onset`, `pilot`, `bursts`, `durations` (played: whole periods), `rest_max` (played),
  `rests_from`, `longest` (the longest the run can take: every rest at its longest or
  replayed length), `started_at`, `status` (`DynamicsStatus`, tagged by `type`): `playing`
  {`step` (1-based; 0 before the first burst), `steps`, `phase` (`DynamicsPhase`: `waiting`
  \| `check` \| `burst` \| `rest`)} → `done` {`trace`} or `failed` {`reason`, `msg`}.
  `DynamicsFailure`: `stopped`, `lease_expired`, `session_closed`, `dropout`, `no_reference`
  (the reference carries no programme), `pilot_not_below_threshold` (the gain at the pilot
  differs from the gain 6 dB below it), `mismatch` (the reference's edges do not match the
  programme, or its end never came).
- `trace.get` of a dynamics trace: `dynamics` (`DynamicsData`): `info` (`DynamicsInfo`:
  `sample_rate`, `carrier`, `period` (samples), `onset`, `pilot`, `bursts`, `durations`,
  `rest_max`, `replayed`, `reference_input`, `steps`: [`DynamicsStep` {`level`, `burst`,
  `rest` (as played), `recovered` (false: the rest reached `rest_max`)}]), `inputs`:
  [`DynamicsInput` {`input`, `name`, `delay` (the processor's envelope delay), `g0` (gain at
  the pilot re the reference), `rows`: one `DynamicsRow` per step}], `envelopes`: per input
  (as `inputs`), one [`DynamicsPoint` {`edge` (`start` \| `end`), `t` (s from the edge),
  `gr_db`, `h_db` (H2…H5 re the fundamental, NaN where blind)}] per step. A long run at a
  high carrier holds tens of MB of envelopes, over the ctrl message limit (`too_large`):
  `dynamics.table` reads its step table, `dynamics.envelope` one step's envelopes of one
  input, `trace.export` all of it. `DynamicsRow`: `gr_end`, `gr_settled`
  (nil: no burst at the level settled), `settled`, `attack_tau`, `attack_2db`, `settle_0_5db`,
  `overshoot` ({`level`, `at`, `reference`} \| nil), `release_tau`, `recovery_2db`,
  `recovery_0_5db`, `harmonics` ({`attack`, `settled`, `release`, `rest`}, each {`h`: four
  levels, `thd`}), `thump_start`, `thump_end` ({`peak`, `rms`}), `drift`. A time is a
  `DynamicsTime` (tagged by `type`): `seconds` {`value`}, `within_period`, `cant_tell`,
  `no_gain_reduction`; a level a `DynamicsLevel`: `db` {`value`}, `below_floor`,
  `cant_tell`. Smoothing, averages and math traces refuse a dynamics trace.

#### Sessions (`file.*`)

Design: `docs/design/sessions.md`. The **working state** is the measurements and traces the
daemon holds (autosaved, §7.3); a **session** is a saved copy of it — a directory in the
daemon's session directory, a directory on the daemon host, or a session archive on a
client's machine (`.ac2session`: the directory as a gzip-compressed tar, §7.2).

`SessionRef` (tagged by `type`): `name` {`name`} — a directory in the daemon's session
directory (letters, digits, space, `-`, `_`, `.`; not starting with `.`) — `path`
{`path`} — an absolute directory on the daemon host, accepted from local transports only
(`refused` to a network client) — or `upload` {`transfer`} — a complete upload of this
client's (`file.load`, `file.salvage` only; used up by the command). `SessionFile`:
`name`, `path`, `saved_at`, `measurements`, `traces`.

The `working` entity (`WorkingSession`): `origin` (`SessionOrigin` | nil = untitled:
`name` {name}, `path` {path}, `client` {path} — an archive on a client's machine, by the
last component of the path that client named it: the daemon never keeps a client's
folders), `dirty` (the saved content differs from the origin's last save or open:
measurement configurations, delays — not a tracking delay's applied value —,
trace metadata; not running/stopped or SPL logs;
untitled: holds anything), `saved_at` (when the origin's content was saved), `recover`
(`Recoverable` | nil: `replaced_at`, `origin`, `measurements`, `traces` of the state the
last `file.new` / `file.load` replaced while it was dirty). It survives a daemon restart
with the autosave (§7.3).

- `file.save` writes the session measurements' configurations (applied delay, tracking,
  running; never a rig measurement's) and every trace with metadata, edits, slots and columns (format §7.2). Never
  generator state; calibrations belong to the calibration store. An existing session other
  than the working state's origin: `conflict` with `detail: {type: session_exists, name,
  saved_at}` unless `overwrite`. Afterwards `working.origin` is that session, clean.
- `file.load` with the working state dirty and `discard` false: `conflict` with `detail:
  {type: unsaved_changes, origin}`, nothing changed. Otherwise it checks the whole session
  first (a refusal changes nothing), keeps a dirty working state for `file.recover`, then
  stops and disarms the generator and drops its owner (the old lease is gone), deletes
  every session measurement and every trace (rig measurements stay), starts a new session epoch newer than every epoch recorded
  in the loaded traces (an open stream reopens without generator routes), and recreates
  the measurements (same ids, restarted when they were running) and traces (same ids).
  `working.origin` becomes the session (`client` {file name} for an upload), clean. Errors:
  `not_found`; `invalid` (not a session, bad name, damaged files or archive); `unsupported`
  with `detail: {type: session_version, found, supported}` for another format version.
- `file.new`: guarded as `file.load`; deletes every session measurement and every trace
  (generator disarmed, the audio I/O stays open, rig measurements stay); untitled.
- `file.recover`: swaps the working state with `working.recover` (which then holds the
  state it replaced, so a second recover undoes the first); `not_found` without one.
- `file.salvage`: imports each `traces/*.csv` of the session — any format version — into
  the working state as `trace.import` of an ac2 CSV would (role from the file; a math
  trace's trace becomes an imported trace), file by file. Reply `salvage` {`version`:
  the manifest's version | nil, `imported`: [TraceMeta], `refused`: [{file, reason}]}.
- `file.list`: the sessions in the session directory, by name.
- Disk and archive work (`file.save`, `file.load`, `file.new`, `file.recover`,
  `file.salvage`, `file.import`, `file.download_begin`, `file.list`) runs beside the other commands: lease refreshes,
  `gen.stop` and other clients are answered while it waits on the disk, and its reply
  follows when it is done. One such operation at a time: while one is under way, another
  `file.save` / `file.load` / `file.new` / `file.recover` / `file.salvage` / `file.import` /
  `file.download_begin` is `refused` (nothing changed; ask again once it is answered);
  `file.list` is never refused for it. A save writes the content as it was when it began
  (edits made meanwhile stay unsaved). A load, new or recover whose working state changed
  (content or saved standing) while it read is refused with `conflict`, nothing changed.
  A salvage imports onto the working state as it is when its read is done.
- Archives and trace files travel in chunks of at most `chunk_bytes` (1 MiB). Upload: `file.upload_begin`
  (at most 64 MiB, else `invalid`) → `transfer` {`transfer`, `file_name`, `bytes`, `sha256`,
  `chunk_bytes`}; `file.upload_chunk` in order (`offset` = bytes so far, else `invalid`);
  after the last byte the SHA-256 is checked (`invalid` on a mismatch, the upload dropped).
  Then `file.load` / `file.salvage` with `{type: upload}` or `file.import` (stores it as a
  named session; `session_exists` unless `overwrite`), or `trace.import` (reads it as a
  trace file, named after the last component of the upload's `file` when the file names
  no trace; the upload is used up whether the import succeeds or not). Download: `file.download_begin`
  packs (`file_name` `<name>.ac2session`; an archive over 64 MiB packed is `invalid`, as an
  upload is), `file.download_chunk` reads, and
  `file.download_end` drops it; with `saved_as` after a working-state download, the
  working state's origin becomes `client` {the last component of `saved_as`}, clean if it
  has not changed since the download began. `trace.export` offers its file as such a
  download (no `saved_as`).
  Unpacking accepts regular files at `session.json`,
  `session.prev.json`, `traces/<file>`, `spl/<file>` only, at most 512 MiB and 20 000
  entries. At most 4 transfers per daemon; one idle for 2 min is dropped (`not_found`
  after). A transfer belongs to the client that began it.
- The daemon also keeps an autosave of the same content (§7.3); its status is the
  `autosave` entity.

#### Raw capture files (`rec.*`, `session.replay`)

Design: `docs/design/raw-capture.md`; file format §7.5. A daemon has a recording directory
(`ac2d`: `recordings` in the data directory, `--recordings <dir>`); one without (`ac2d --ephemeral`,
or an in-process test daemon not given one) answers `rec.start` `unsupported`, `rec.list` with
nothing and `session.replay` by name `not_found`.

`RecordRequest`: `inputs` ([u16] session channels, each captured by the open session, in
file channel order; a remote input is recorded from the joined feed, its gaps marked), `name` (string \| nil: a file stem like a session name; nil =
`rec-<UTC date>T<hh-mm-ss>`, made unique), `max_duration` (Seconds, 0 < d ≤ 86 400),
`max_bytes` (u64 \| nil, at least one second of audio).

- `rec.start` refuses (`refused`) without an open session, while another recording runs,
  or when a recording of that name exists (never overwritten); `invalid` for an empty,
  repeated or uncaptured input, a bound out of range, a bad name. The file and sidecar are
  created before the reply; the first captured block after the reply is the file's first
  frame.
- The recording ends with `rec.stop` (everything captured until the request is kept), at
  its bounds, on a write failure (disk full), when the session closes or reopens (device
  or configuration change, `session.open`, `session.replay`, `file.load`) and at daemon
  shutdown; the file and sidecar are always finalised and say why. A daemon that dies while
  recording leaves a sidecar without an end; the next one to start with that directory
  finishes it (`interrupted`) once its audio file has not been written for 30 s, and
  mirrors it as the `recording` entity.
- `rec.stop` without a recording is `refused`.
- Progress and outcome are the `recording` entity (§4.1), `RecordingRun`: `name`, `path`
  (the audio file on the daemon host), `inputs`, `sample_rate_hz`, `session_epoch`,
  `start_sample` (session sample of the file's first frame), `started_at`, `started_by`,
  `frames`, `bytes` (file size), `discontinuities`, `remote_gaps` (`RemoteGaps` {`spans`,
  `frames`}: the remote inputs' marked gaps, frames summed over channels), `max_duration`,
  `max_bytes`, `status` (tagged by `type`: `recording` \| `ended` {`reason`:
  `RecordingEnd`}). `frames`, `bytes`, `discontinuities` and `remote_gaps` are updated about once a second while recording. `RecordingEnd`
  (tagged by `type`): `stopped`, `duration_limit`, `size_limit`, `write_failed` {`msg`},
  `session_closed`, `session_reopened`, `audio_stopped` (the session's audio stopped, §4.1.1;
  the file ends at the last audio received), `daemon_shutdown`, `interrupted`.
- `rec.list` → [`RecordingFile`]: `name`, `path`, `sample_rate_hz`, `inputs`, `frames`,
  `started_at`, `discontinuities`, `remote_gaps`, `end` (`RecordingEnd` \| nil while being
  written),
  oldest first.
- Every committed change of a measurement, the generator, the input setup or a calibration
  while recording goes into the sidecar's timeline at the newest captured sample. Every
  block that does not follow the previous one (xrun, device gap, capture overflow,
  configuration change, or the recorder falling behind the fan-out by more than 10 s of
  audio) goes into its discontinuity list with the samples lost; the file holds only
  captured frames, never filler.

`RecordingRef` (tagged by `type`): `name` {`name`} in the recording directory, or `path`
{`path`}: the absolute path of the `.wav` or `.ac2rec.json` file, local transports only
(`refused` to a network client). `session.replay` closes any session and opens a new epoch on a
`replay` backend: the recorded device's id (so its calibrations apply), the recorded inputs
under their device numbers (an input not recorded cannot be captured), the recorded rate
and period, no outputs (the generator cannot be armed). Running measurements restart on it
as on any reopen; one whose input was not recorded stays stopped. Sample 0 is the file's
first frame; at each recorded discontinuity the sample index jumps by the samples lost and
the block carries the recorded flags (a configuration change replays as a plain
discontinuity), so analyses reset where they reset live. `pace`: `realtime` (one second
per second) or `fast` (as fast as every running measurement takes the audio; nothing is
dropped). The replay starts once the measurements are attached and stops after the last
frame; the session stays open. `ReplayInfo`: `name`, `path`, `frames`, `end_sample` (one
past the last sample index), `pace`, `recorded_start_sample`, `recorded_at`. Errors:
`not_found` (no such recording), `refused` (still being recorded), `invalid` (a sidecar or
audio file this build does not read, or that disagree).
#### Calibration (`cal.*`, `session.inputs`)

Design: `docs/design/q7-calibration.md`. Two stores: **sensitivity calibrations**, keyed by
the open session's capture device, the input channel and the mic name (`CalKey`), so
`cal.spl` needs an open session; and the **mic library**: named curves per mic (`Mic`
{`name`, `curves: [MicCurveRef]`}), which follow the mic name across inputs and devices.
Each input chooses which of its mic's curves applies (`InputSetup.curve`).

- `cal.spl` reads the input's broadband RMS (uncorrected, τ = 1 s) and stores `CalEntry`
  {`key`, `spl: SplCal` {`sensitivity`: Db (dB SPL of 0 dBFS), `method: CalMethod`, `freq`:
  Hz (the tone read), `measured`: Dbfs, `calibrated_at`}} with `method` `acoustic`
  {`calibrator_level`: DbSpl}. It is `refused` below −80 dBFS, when the input clipped in
  the last 2 s, and while the level is not steady (0.2 s and 1 s readings differ by more
  than 0.05 dB). It sets the input's mic name to `mic` (the name is typed once, at
  calibration time). It replaces any calibration of the key, electrical ones included.
- `cal.spl_electrical` reads the level `L` the same way while the operator measures
  `volts` (RMS) at the input, and stores `sensitivity = 20·lg(V_FS / (S · 20 µPa))` with
  `V_FS = volts / 10^(L/20)` (volts at 0 dBFS) and `S` = `mic_sensitivity` (mV/Pa), or, when
  nil, the one value the `stated_sensitivity` of `mic`'s curves states (`invalid` when they
  state none or disagree). With `loading` {`mic_impedance`: Ohms, `input_impedance`: Ohms}
  `S` is the open-circuit sensitivity divided down by the preamp's input,
  `S · Z_in / (Z_in + Z_out)` (`invalid` unless both are finite and above 0); nil: unloaded.
  `method` is `electrical` {`connection`: `in_line` (pins 2–3 with
  the mic connected and powered) | `injected` (a generator in place of the mic), `volts`,
  `full_scale`: Volts (V_FS), `mic_sensitivity`: MvPerPa, `mic_sensitivity_from:
  SensitivitySource` (tagged by `type`: `typed` | `data_sheet` {`label`, `file_name`}),
  `loading`: MicLoading | nil (as given; `mic_sensitivity` stays the open-circuit value and
  `sensitivity` counts the loading), `uncertainty`: Db (± dB; 1 when nil; 0.05–6 accepted)}. `refused` like `cal.spl`,
  and above −3 dBFS or below −70 dBFS (a poor reading); `invalid` for `volts` outside
  0.1 mV–100 V, a mic sensitivity outside 0.1–1000 mV/Pa, or a `freq` outside 20 Hz–20 kHz (any `freq`
  in that range is accepted; the mic curve is normalised at 1 kHz, where data sheets
  state the sensitivity). An existing `acoustic` calibration of the key is `refused`
  unless `replace_acoustic`.
- `cal.curve_import` parses a magnitude file (frequency, gain dB, further columns ignored;
  text lines skipped; whitespace / comma or semicolon + decimal-comma separated) and stores
  it as `mic`'s curve `label` — by default the incidence angle the file's header or name
  states (`90-degree-curve`, `_90Grad`, `0deg` → `90°`, `0°`), else the file stem; a curve
  of that label is replaced. `MicCurveRef` = {`label`, `file_name`, `content_hash` (FNV-1a
  64 hex), `points`, `f_lo`, `f_hi`, `imported_at`, `stated_sensitivity`: f64 mV/Pa | nil
  (as the header states it; used only as `cal.spl_electrical`'s default sensitivity)}; the points stay in the
  daemon. With `input`, that input's mic name becomes `mic`, and the curve becomes its
  active one when it is the mic's only curve. A refused file is `invalid` with `detail:
  {type: mic_curve_file, line: u32 | nil, reason}`, `reason` one of `too_few_points`,
  `too_many_points`, `bad_number`, `missing_gain`, `non_positive_frequency`, `non_finite`,
  `gain_out_of_range` (|gain| > 40 dB), `not_ascending`. Labels are 1–32 characters, not
  `off` / `none`.
- `cal.curve_rename` relabels a curve (`invalid` when the mic has the new label already);
  inputs that chose it follow. `cal.curve_delete` removes it (a mic left without curves is
  deleted, event `mic` `deleted`); inputs that chose it keep the label and show it as not
  stored. `not_found` for an unknown mic or label. Neither needs an open session.
- `cal.line` (`docs/design/programme-meter.md` §1.1) calibrates a `line` input: the
  operator measured `volts` (RMS) of a steady tone at the input; the daemon measures the
  tone's level `L` as `cal.spl_electrical` does (`refused` below −80 dBFS, when the input
  clipped in the last 2 s, while the level is not steady, and when `L` is outside
  −70 … −3 dBFS) and stores `LineCalEntry` {`key: LineCalKey` {device, channel},
  `line: LineCal` {`v_fs`: Volts (`volts / 10^(L/20)`, the volts at 0 dBFS), `volts`,
  `measured`: Dbfs (`L`), `calibrated_at`}} under the open session's input device,
  replacing the input's earlier one. dBu of 0 dBFS is `20·lg(v_fs / 0.7746 V)`; `invalid`
  when it falls outside +4 … +30 dBu (a unit slip) or when the input is a `mic` or
  `digital` one (named by channel).
- `cal.delete` removes the sensitivity (`spl`) or line (`line`) calibration `key` (any
  device; no open session needed); `not_found` when there is none. The input setup is not
  changed.
- `InputSetup` = {`channel`, `kind: InputKind`, `pair`: `left` | `right` | nil}; `InputKind`
  is tagged by `kind`: `mic` {`mic`: string | nil, `curve: CurveChoice`} | `line` |
  `digital` (`docs/design/programme-meter.md` §1: the kind is declared, never inferred, and
  fixes the dB reference; an input with no row is an unnamed `mic`). `CurveChoice` is
  tagged by `type`: `not_chosen` | `off` | `curve` {`label`}. `Pair` = {`left`, `right`,
  `name`: string | nil} (channels). `session.inputs` upserts its rows and replaces the pair
  list (last write wins: it carries no `base`, `expect_rev` is the only guard); the daemon sets every row's `pair` side from the list. It refuses (`invalid`, naming
  the inputs by channel) a mic row whose chosen label is not stored for its mic (unless the
  row is unchanged), a pair of an input with itself, of two kinds, of an input
  in two pairs, or of an input the open session does not capture. The daemon chooses a
  mic's only curve on a row where none is chosen (on every change of the setup or the
  library); with several, the row stays `not_chosen` and no curve applies until the
  operator chooses. A new mic name on a row starts `not_chosen`. Mic and pair names are 1–64
  characters. `cal.spl`, `cal.spl_electrical` and `cal.curve_import` with `input` need a
  `mic` input; an SPL meter (`meas.create` / `meas.start` of a `spl` measurement) needs one
  too and is `invalid` otherwise (*input 3 is a line input; an SPL meter needs a
  microphone*). Transfer, spectrum, RTA and sweeps take any kind.
- When the daemon's calibration store file cannot be read it is never written: `cal.spl`,
  `cal.spl_electrical`, `cal.line`,
  `cal.curve_*`, `cal.delete`, `cal.list` and `session.inputs` are `refused` with `detail:
  {type: cal_store, path, reason}`.

Which calibration a measurement uses (shown as `CalStatus` in `spl`, `rta` and `spec`
frames): the entry of device + input + the input's mic → `verified`; else the newest one
on the same device + input (another mic, or no mic name set), else the newest one for the
same mic elsewhere → `other_mic_or_input`; else `uncalibrated` (dBFS). A `line` input
uses its line calibration on the same device + input → `line` {`calibrated_at`, `dbu_fs`:
Dbu}, else `uncalibrated`; a `digital` input is always `uncalibrated`. `CalStatus` is
tagged by `type`: `uncalibrated` | `verified` {`calibrated_at`, `basis`} |
`other_mic_or_input` {`calibrated_at`, `basis`} | `line` {`calibrated_at`, `dbu_fs`}; the
age is `capture_wall_ns − calibrated_at`. `basis: CalBasis` (tagged by `type`) says what the calibration rests on:
`acoustic` {`calibrator_level`} | `electrical` {`connection`, `mic_sensitivity`,
`data_sheet`: bool, `loading`: MicLoading | nil, `uncertainty`}. The mic curve is the input's chosen curve of its mic —
only that one; none when not chosen, off, or not stored — normalised to 0 dB at the
calibration's frequency in use (the calibrator's; 1 kHz for an electrical calibration and
uncalibrated). The rules are
the pure functions of `ac2_proto::cal`, which clients use to word what is in use.

#### Resolution and unresolved ranges

`TransferConfig.resolution` and `SweepConfig.resolution` (`Resolution`: `twelfth` \|
`twenty_fourth` \| `forty_eighth` \| `ninety_sixth`, 12 … 96 points per octave; front ends
default to `forty_eighth`) set the stored column grid: a transfer function's is fifteen
octaves, 7.8 Hz … 256 kHz (`{type: log, ppo, k_min: −7·ppo, k_max: 8·ppo − 1}`), the same at
every sample rate (columns above the band a session's rate measures are invalid), a sweep's covers its
`start … end`. It is not smoothing (a display edit over these columns). `meas.update` with a
new transfer resolution restarts the job (new columns, averages start over); a sweep's
applies to its next run, and stored traces keep their grid.

`Unresolved` {`resolution`, `ranges`: [`FreqRange` {`lo`, `hi`: Hz}], ascending, disjoint,
on column edges} lists where one estimate's resolution cell — the bandwidth its analysis
window sets — is wider than one column at that resolution, so the columns there are
interpolation, not detail. Transfer: `TfMeta.unresolved` is [`BinRange` {`lo`, `hi`,
`bin_hz`: Hz}] instead (ascending, disjoint, on column edges; empty: all resolved), each
range with the bin spacing of the coarsest MTW stage serving it, split where another stage
takes over (stages switch over where a bin fills a 1/48-octave column, so at 1/96 the band
just above each crossover is unresolved too; at 96 kHz: below ≈ 135 Hz with 0.98 Hz bins,
≈ 203–406 Hz with 2.9 Hz and ≈ 1.62–3.25 kHz with 23 Hz; with `lf_window: long` the lowest
range ends two octaves lower, with 0.24 Hz bins). There a column holds at most one bin of the
serving stage and many hold none (`THINNED`); in a crossover's blend the deeper stage's bins
fill some of those columns, so the column spacing is not the bin width. A math trace whose
included operands are all live transfers states their ranges, each piece at the coarsest
width among the operands covering it; nil when it includes a stored trace (which keeps no
ranges) or nothing. Sweep
(`SweepData.unresolved`: `SweepUnresolved` {`response`, `harmonics`} \| nil): the response's
cell is 1/(its linear window), set by `gate`; a distortion point's is, on the fundamental's
axis, 1/(k · its harmonic window) for every order k it reports, with `lf_harmonics: fine`
each order's own window where it serves.

#### Spectrum resolution (`SpecResolution`)

`SpectrumConfig.resolution: SpecResolution` (tagged by `type`):

- `fixed` {`fft_len`: a power of two in 64 … 65 536}: one FFT; frames on a `log_bins` grid
  (§6), every column the same bin width `fs / fft_len` and the same window length.
- `multi` {`grid`: `Resolution`}: a multi-resolution spectrum. Six full-rate FFTs of halving
  length, the longest the smallest power of two whose bin is at most 96 000 / 65 536 Hz
  (65 536 points at 88.2/96 kHz, 32 768 at 44.1/48 kHz, 131 072 at 176.4/192 kHz), each
  with its own hop (the daemon's display hop rule: the largest power of two ≤ fs/30,
  clamped to n/8 … n/2) and its own `averaging` (an `fifo` of N frames is N frames of
  every band, so the longest band averages over N times its hop; at most 128 frames,
  `invalid` beyond). Frames are on a `log` grid at `grid`'s points per octave from
  `1000 · 2^−7` Hz (7.8 Hz) to below Nyquist (`k_min = −7·ppo`, `k_max =
  floor(ppo · log2(fs / 2000) − ½)`). Level is the tone level as for `fixed`: band b serves
  alone up to `A_b = 2κ(48)·fs/N_b` (`κ(p) = 1/(2^(1/2p) − 2^(−1/2p))`, so its bin fills
  at most half a 1/48-octave column there), and between `A_b` and `A_{b+1} = 2A_b` the two
  bands' column powers are blended, `(1−w)·P_b + w·P_{b+1}` with `w = f/A_b − 1`. A sine
  reads its level everywhere; broadband noise reads in a bandwidth `ENBW · f / (2κ)` (about
  1/64 octave for Hann) between the first and last anchor, with no step at an anchor, and
  in the band's fixed bin below `A_0` and above `A_5`. A column has no value (NaN) until
  every band behind it has a frame. The design note is `docs/design/multires-spectrum.md`.

A `multi` frame carries `SpecMeta.bands` (`SpecBands` \| nil; nil for `fixed` and math):
`bands` ([`SpecBand`], longest window first) and `unresolved` (`Unresolved`: the columns
whose coarsest contributing bin is wider than a column, at 1/48 only below ≈ 101 Hz at
96 kHz). `SpecBand` {`window`: Seconds (its FFT length / fs), `hop`: Seconds (how often its
columns change), `sole`: Hz (its anchor `A_b`, where it alone serves; the longest band
also alone below it, the shortest above it), `age`: Seconds \| nil (its
newest frame's end before this frame's audio end; nil before its first frame)}. Ages are
exact: every band's frame ends on a multiple of the shortest hop, and a column whose bands
made no new frame since the previous `spec` frame repeats its previous value.

#### Harmonics of the generator's sine (`SpecMeta.harmonics`)

While the generator fires a `sine` (`Generator.firing` with `settings.signal` a sine; not a
sweep, not noise), every live `spec` frame of a spectrum measurement carries
`SpecMeta.harmonics` (`SpecHarmonics`); otherwise, for a math trace, in a captured
frame and before the FFT that is read has a spectrum it is nil. `SpecHarmonics` {`f0`: Hz
(the generator's frequency), `on_bin`: bool (`f0` is a bin centre of the FFT read, so the
window leaks nothing), `reading`: `HarmonicReading`}.

The reading is taken from the averaged power spectrum at full resolution, before display
smoothing and whatever `level` is: every bin of a `fixed` spectrum's FFT; for `multi` the
bins of its longest window (the ladder's first band, 32 768 points at 48 kHz, 65 536 at
96 kHz). A tone's power is the sum over its window's main lobe, corrected for the part of
the window's energy outside it, so a sine between bins reads its level without scalloping.
A mic curve is applied per bin before reading, as the frame's `level` has it.

`HarmonicReading` is tagged by `type`:

- `read` {`fundamental`: f64, the sine's level in the frame's `scale` (dBFS or dB SPL), what
  the spectrum shows for a tone at `f0` in tone-level mode; `harmonics`: [`HarmonicLevel`],
  orders 2, 3, … up to 10 or the last whose lobe lies below fs/2; `thd`: Db \| nil, the
  power of the resolved harmonics re the fundamental, nil when none is resolved; `thd_bound`:
  Db, an upper limit of the distortion: every harmonic listed at the larger of its level
  and its floor; `thd_n`: Db, everything outside the lobes of DC and the fundamental re the
  fundamental; `leakage`: Db \| nil, the part of an off-bin sine's own power its window
  spreads outside its lobe, below which `thd_n` cannot read; nil on a bin}.
  `HarmonicLevel` {`order`: u8, `level`: Db re the fundamental (dBc), `floor`: Db re the
  fundamental, what the harmonic's lobe would hold from the noise or the fundamental's
  skirt around it, `resolved`: bool, `level` stands 6 dB above `floor`}. A harmonic that
  is not resolved has `level` = `floor`: it is below that, by how much is not known. No
  ratio is NaN or infinite: a power of exactly zero is sent as −400 dB.
- `unreadable` {`reason`: `rectangular_off_bin` (a rectangular window reads a sine only on
  a bin) \| `too_low` (the lobes of DC, the sine and its harmonics leave no bins between
  them at this FFT length) \| `out_of_band` (the sine's lobe reaches fs/2) \|
  `no_fundamental` (the lobe holds no power: digital silence)}.

The average still holds what the input was before the sine started or moved; the reading
settles with it. `ac2 gen sine --lock MEAS` sets a frequency on a bin of a spectrum
measurement (`k · fs / N`, 1 ≤ k < N/2, nearest to the one asked for).

#### Spectrum level (`SpecLevel`)

`SpectrumConfig.level: SpecLevel` is `tone` or `density`; a change restarts the job (its
averaging starts over). Front ends default it by resolution: `tone` for `fixed`, `density`
for `multi`.

- `tone`: folded bin power `F_k = c_k·|X_k|²` scaled by `2 / S₁²` (`S₁ = Σw`): a sine of
  RMS `a` reads `2a²` (`a²` re ½ FS², the mean square of a full-scale sine: 0 dBFS at full
  scale). Units `dbfs` / `db_spl`.
- `density`: power per hertz of noise bandwidth, keeping the same reference. `fixed`:
  `2·F_k / (fs·S₂)` (`S₂ = Σw²`), i.e. tone power over `ENBW · fs / N`; white noise of
  deviation σ reads `4σ²/fs`. `multi`: each band's mean bin over the column, blended as for
  tone, divided by `B(f) = Σ_b w_b · ENBW · fs / N_b` (continuous through the anchors).
  Units `dbfs_per_hz` / `db_spl_per_hz` (a calibrated input: sensitivity and mic curve
  applied as for tone). A sine reads `level − 10·log10(B)` where a column covers one bin.

Frames carry it (`SpecMeta.level`); a fixed density's display columns are the mean of their
bins (§5.4). Captures carry it in their kind (`TraceKind` `spectrum` {`scale`, `level`}).

#### Averaging depth

`TransferConfig.depth: DepthPolicy` (tagged by `type`): `equal_confidence` (every MTW stage
reaches the same effective-average count) or `fast_lf` {`max_settle_s`: Seconds > 0} (no
decimated stage averages over a longer span; those stages show a higher coherence floor).

#### Longest window

`TransferConfig.lf_window: LfWindow`: `standard` (the deepest MTW stage runs at about 4 kHz,
a window of about 1 s: 1/48 octave is resolved from about 68 Hz) or `long` (a further stage
at about 1 kHz serves the columns below about 68 Hz with a window of about 4 s: 1/48 octave
is resolved from about 17 Hz, and those columns settle four times as slowly). `meas.update`
with a new value restarts the job. Captures record it (`TraceMeta.lf_window`, nil for
other kinds).

### 3.3 Reply bodies

`{type, value}` with `type` one of: `ack` (`{rev}`), `welcome`, `backends`, `preview`,
`loopback_detection`, `session`,
`lease`, `generator`, `measurement`, `delay_finding`, `trace`, `traces`, `trace_data`,
`dynamics_table`, `dynamics_envelope`, `calibration`, `line_calibration`, `calibrations`, `mic`, `inputs`, `outputs`, `server`, `source_state`,
`spl_log_page`, `spl_history`, `spl_band_log`,
`snapshot`, `events`,
`grid`, `session_file`, `sessions`, `transfer`, `chunk`, `salvage`, `sweep`, `dynamics`, `recording`,
`recordings`, `plan`, `plans`, `walk`, `sim`.

### 3.4 Errors

`code` is one of `invalid`, `not_found`, `conflict`, `lease_required`, `lease_held`,
`refused` (safety: level ceiling, firing unarmed, …), `resync_required`, `unsupported`,
`internal`, `version_mismatch`, `too_large` (the request, or the reply it would have,
exceeds the 4 MiB ctrl message limit, §3.1).

`detail` (optional, tagged by `type`): `conflict` {rev}, `lease_held` {owner},
`version` {daemon, client}, `resync` {oldest}, `import` {line, problem} (§3.2 traces),
`session_version` {found, supported}, `unsaved_changes` {origin}, `session_exists` {name,
saved_at} (§3.2 sessions), `mic_curve_file` {line, reason} and
`cal_store` {path, reason} (§3.2 calibration), `sim` {reason} (§3.2 room simulation).

## 4. State and events

### 4.1 Entities

The mirrored `State` holds: `session` (`epoch`, `open: OpenSession | nil`, `stopped:
AudioStopped | nil`; §4.1.1),
`measurements` (`id`, `config`, `config_rev`, `running`, `delay`, `grid_id`, `colour`,
`layout`, `offset`, `eq`),
`traces` (`TraceMeta`: `id`, `edit` {name, color, visible, locked, order, offset,
polarity, delay_nudge, slot, smoothing, owner}, `kind`, `source` {captured | imported |
math | eq | average | generated | sweep | dynamics}, `grid_id`, `delay`, `depth`, `lf_window`, `cal`, `mic`, `mic_curve`, `created_at`,
`eq_in` ([`EqSnapshot`]: the EQ sets marked in the console when captured), `position`
(`TracePosition` | nil: a walk's capture names its point `label`, `at: [x, y, z]`, `plan`
and `walk`), `captured_unsettled` (captured before the walk measurement was ready),
`superseded` (a redone or skipped point's capture: kept, out of the walk's average);
`kind` one of
`transfer`, `target`, `spectrum`, `rta`, `sweep`, `dynamics`), `generator` (`owner`, `owner_local` (the owner is a client of the local pair, which has
no name of its own: show it as an app on this rig),
`armed`, `firing`, `settings`, `ceiling`, `ceiling_bound`, `last_action` {`action`: acquire
\| force \| arm \| fire \| set \| stop \| release \| expiry \| ceiling_lowered \|
ceiling_raised, `client`, `at`}), `calibrations` (`CalEntry`:
`key` {device, channel, mic}, `spl`: SplCal), `line_calibrations` ([LineCalEntry]), `mics`
(`Mic`: `name`, `curves` [MicCurveRef]), `inputs` ([InputSetup], sorted by channel), `pairs`
([Pair], sorted by left channel), `outputs` ([OutputSetup], the
labelled outputs, sorted by channel), `spl_logs` (`SplLog` per SPL
meter: `meas`, `started_at`, `windows`, `peaks`, `alarms`; §3.2), `timing` (`TimingStatus`: `epoch`,
`state` {no_stimulus | acquiring | locked{offset} | jumped{from, to} | lost}, `last_lock`,
`drift` (`Drift` | nil: `ppm` output-vs-input clock drift from the loopback offset's slope,
`span` s regressed, `warning` true when output and input are on different clocks, `at`
WallNs of the newest window; kept after the stimulus stops and for the rest of the session;
committed when the warning flips, the span first reaches the judged length or the shown value changes: 1 ppm, 0.1 ppm below 10 ppm while warning), `internal_reference`), `clocks` (`Clocks`, what the host can say about its
clocks and nothing inferred from them: `system` {synced{maxerror_us} | unsynced{maxerror_us} |
not_stated} — the system clock as the OS states it (Linux: `adjtimex` read with no mode bits,
`STA_UNSYNC` clear or set; `maxerror_us` the kernel's error bound, which grows by 500 µs a
second between the discipline daemon's updates, never the clock's offset; macOS and Windows
state nothing), read every 5 s; `capture` (`DeviceClock` | nil while no audio I/O is open:
`device`, `rate` {measuring | measured{ppm, span, against} | synthetic}) — the capture
device's sample clock, a least-squares fit of the backend's block stamps against their
sample index over the last 60 s of audio (segments pooled across gaps), `ppm` positive when
the device runs fast against `against`: `system_clock` when the backend's stamp clock was
read beside the disciplined monotonic clock (Linux `CLOCK_MONOTONIC`) over the same minute
and its rate taken out, `host_crystal` when it could not be (macOS host time, the Windows
performance counter: the figure includes that clock's own error); `measuring` until the
fit (and the stamp clock's, for `system_clock`) spans 54 s, `synthetic` for a replay,
whose stamps are its sample index; read every 5 s of audio; `stamp_map` (`StampMap` \| nil
{`epoch_stamp_ns`, `system_ns`, `ppm`}: the capture stream's stamp clock on the system
clock, `system = system_ns + (stamp − epoch_stamp_ns)·(1 + ppm·10⁻⁶)`; nil while no stream
whose stamp clock can be read beside the system clock is open); committed when any changes),
`remote` (`RemoteState`: `peers` ([RigPeer], as `rig.pair` / `rig.unpair` left them), `rigs`
([`RemoteRig` {`rig`, `name`, `link` {not_paired | no_server_key | connecting | up |
network_off (it sent the `network_off` notice, §5.8, and is not heard since: read until
anything answers there again, a welcome or a failed handshake, or it is followed to another
address; a plain loss reads unreachable) | unreachable{since} | not_admitted |
key_changed | other_rig{found}}, `clock`: `PeerClock`
{`relation` {measuring | agree | not_locked{reason: cross_rate{ppm, over} |
own_clock{rig, ppm}} | cant_tell{reason: system_clock_unsynced{rig} |
system_clock_not_stated{rig} | not_against_system_clock{rig} | no_capture_stamps |
clocks_disagree{by} | no_audio}}, `ppm`, `span`, `last_change`}, `bytes_per_s`, `window`
(bytes of frames the link may have in flight)}], one per rig the session's remote inputs are on),
`inputs` ([`RemoteInputStatus` {`slot`, `audio` {waiting | on_this_rig | no_session |
not_captured | replaying | rate_mismatch{theirs, ours} | starting | streaming}, `anchor`
(nil | system_time | arrival{reason: system_clock | no_capture_stamps | no_stamp_map |
clocks_disagree{by}}: how remote samples are placed against this rig's), `late_frames`,
`late_frames_hour`, `slips`, `last_slip` (samples, signed), `arrival_margin` (s),
`setup`, `cal_status`, `calibration`, `curve` (the source rig's, as it states them)}]),
`link_offset` (s \| nil, the offset in use), `transfers` ([`CrossRigTransfer` {`meas`,
`rig` (the remote rig), `side` {reference | measurement} (which channel is remote),
`state` {running | stopped{why: measuring | not_locked | cant_tell, held}} (only `agree`
averages; `held`: the result from before the failed window stands, else nothing is shown),
`relock` (`Relock` \| nil {`after` (s stopped), `from` (why), `outcome` {tracking |
tracked{delay, was} | ambiguous | find_again}}; said a minute once settled; `ambiguous`
and `find_again` stay until a pick, a typed delay or a find), `slip` (`DelaySlip` \| nil
{`by` (s)}: the remote audio slipped and the delay moved by exactly that; said a minute),
`delay_drift` (s per minute \| nil: the delay drifts above 0.05 ms/min over 2 min),
`late` (remote audio keeps arriving after the link offset: raise it)}], one per transfer
function with exactly one channel on a remote rig); committed when any of it changes; the
clock relation is reported only, never corrected for),
`sweep` (`SweepRun` | nil: the latest `sweep.run` run),
`dynamics` (`DynamicsRun` | nil: the latest `dynamics.run` run),
`autosave` (`Autosave`: `state` {off | saved | pending | failed{reason}}, `saved_at: WallNs |
nil`, `log_failures: [LogWriteFailure]`; see §7.3), `recording` (`RecordingRun` | nil: the latest recording, §3.2), `working`
(`WorkingSession`: the working state's session and unsaved changes, §3.2 sessions), `rig`
(`Rig`: rig settings, OSC and Art-Net sending and cue states, §3.2 *Rig monitor, OSC and Art-Net cues*),
`plans` ([PlanSpec], the daemon's plan library, sorted by name; §3.2 *Spatial walks*),
`walks` ([WalkState]), `walk_plans` ([PlanSpec], the session's copies of the plans its
walks walk), `sim` (`SimState` | nil: the latest room simulation run or fit), `sim_map`
(`SimState` | nil: the latest coverage map) and `sim_fit` (`SimFitKept` | nil: the
absorption fit kept for later runs); none saved with the session (§3.2 *Room simulation*),
`server` (`ServerSummary` {`status`: off \| listening \| waiting \| failed, `ctrl` \| nil
(while listening), `port` \| nil (set to; nil while off), `fingerprint` \| nil}: what every
client sees of the network, so a change made elsewhere shows at once; details in
`server.info`).


#### 4.1.1 Audio stopped and recovery

While a session is open the daemon watches its audio (`docs/design/audio-recovery.md`).
`session.stopped` is set when the stream stops delivering and stays set until the same
configuration is open again or a client closes the session; `session.open` keeps the
session as last opened throughout. `AudioStopped`: `since` (WallNs, the last audio
received, or the open when none came), `cause` (tagged by `type`): `not_delivering`
{`after_ms`: u32, the silence that counts as stopped: max(1 s, 20 periods)} \| `host_ended`
(the audio host ended the stream: server shut down, device removed) \| `driver_reset`
{`reason`: `DriverResetReason`} (the driver reset or its clock changed, ASIO; the backend
cut the output within the block, over 2 ms; `reason`: `reset_request` (its settings
changed, typically in its control panel, or its clock source) \| `resync_request` (its
clock slipped or relocked) \| `buffer_size_change` \| `rate_change` (`sampleRateDidChange`,
also at the same rate) \| `clock_lost` (a rate of 0)) \| `device_changed`
(a device or configuration change whose reopen failed); `recovery` (tagged by `state`):
`opening` {`attempt`: u32 from 1, `started`: WallNs} \| `waiting` {`attempt`, `error`:
string (what the backend said, e.g. that no JACK server runs), `next_at`: WallNs}.

The daemon tears the stopped stream down off its control thread (closing a client of a hung
server may block) and reopens the same configuration attempt after attempt, backing off
1, 2, 4, 8, 16 then every 30 s, until it succeeds or a client sends `session.close` or
`session.open`. Running measurements stay `running` and pause: no frames, nothing averaged
across the gap. When an attempt succeeds the session gets a new epoch with `stopped: nil`,
the same measurements restart from fresh averages (an SPL meter's log shows the outage as
gap time), and the generator is disarmed (it was disarmed when the audio stopped).

After a `driver_reset` the reopen does not ask for the configuration's rate and buffer
size: it takes the driver's current ones, because the operator changed them in the
driver's panel (setting the old rate back would undo that, or fail for good on a driver
following an external clock at the new rate). A configuration that named a rate or
buffer size names the new one from then on; `OpenSession.driver_change` says what it ran
at before, and the measurements restart at the new rate (delays are re-derived in
samples). Every later recovery reopens the configuration as it then stands.

### 4.2 Snapshot and events

`state.snapshot` → `{state, rev, daemon_incarnation, session_epoch}`.

An event is `{rev, kind, payload}`. `kind` is one of `session`, `measurement`, `trace`,
`generator`, `calibration`, `line_calibration`, `mic`, `inputs`, `outputs`, `spl_log`, `timing`, `clocks`, `remote`, `sweep`, `dynamics`, `autosave`, `recording`, `working`, `rig`, `plan`, `walk`, `walk_plan`, `sim`, `sim_fit`, `server`. `payload` is the entity's
full new value (`inputs`: {`inputs`, `pairs`}, both whole lists; `outputs`: the whole list;
`sim`: the `SimState`, which `State.sim` becomes, or `State.sim_map` for a map; `sim_fit`: the `SimFitKept` or nil); for keyed entities (`measurement`, `trace`,
`calibration`, `line_calibration` (keyed by `LineCalKey`), `mic` (keyed by name), `spl_log`, `plan` and `walk_plan` (keyed by name), `walk` (keyed by
`WalkId`)) it is `{type: "set", value: <entity>}` or `{type: "deleted", value: <key>}`.
Applying an event is assignment. Events travel on the data socket as
`[b"evt"][msgpack event]` and in `state.since` replies. Largest event: 1 MiB; one that
would be larger is not sent on the data socket, and every connection taking events is sent
`resync_required` in its place (the snapshot holds the value). On the data
socket they go outside the byte window, within a bound of their own (§5.8): past it a
connection's events are dropped and it is sent `resync_required`.

Every event takes the next rev. A command changing N entities emits N events with
consecutive revs, in order; clients may observe the states between them. `ack.rev` is the
state's rev after the command: its last event's rev, unchanged if it changed nothing.

Reply bodies are not state: a mirror applies events and snapshots only; a reply gives ids
and tells the operator.

### 4.3 Sync procedure (client)

1. On the data DEALER, state the window and subscribe to `evt` and `ka` (§5.8); buffer
   events.
2. Wait for the first `ka`. It is only a hint that the subscription is live: a `ka` also
   goes to a connection not subscribed to it. Steps 4 and 6 repair an event missed before
   the subscription took effect.
3. `state.snapshot` → rev R. Apply buffered events with rev > R in order, then live ones.
4. Event with rev > last + 1 → `state.since(last)`; the `resync_required` notice → back
   to 3.
5. `ka` with another `daemon_incarnation` → drop all state, back to 1.
6. `ka.rev` ahead of the last applied event for > 1 s → `state.since`.

Replay buffer: last 1024 events or 60 s, whichever holds fewer.

## 5. Data frames

### 5.1 Topics

| topic | content |
|---|---|
| `d/<meas>/tf` | transfer function |
| `d/<meas>/ir` | impulse response view |
| `d/<meas>/rta` | fractional-octave RTA |
| `d/<meas>/spec` | narrowband spectrum |
| `d/<meas>/spl` | SPL meter |
| `d/<meas>/leq` | rolling Leq windows of an SPL meter, once a second |
| `d/<meas>/band_leq` | an SPL meter's band meter (1/3-octave band Leq per band window), once a second |
| `d/<meas>/levels` | input meters of the measurement's channels |
| `session/levels` | input meters of every input of the open session |
| `session/preview` | input meters of every input of the previewed device |
| `session/gonio` | goniometer points of every pair of the open session |
| `preview/gonio` | goniometer points of every pair of the previewed device |
| `timing` | loopback timing monitor |
| `evt` | state events (§4.2) |
| `audio/<channel>` | raw capture of device input `<channel>`, to admitted rigs only (below) |
| `ka` | keepalive, every 250 ms to a connection subscribed to it; one a second to one that is not and was sent nothing else in that second (not to admitted rigs) |

A measurement topic is formed and sent only while someone subscribes to it. `tf`, `ir`,
`spec` and `rta` frames come when the result changes (at most at the daemon's publish rate);
`spl` at most 20 times a second (Lmax, Lmin, Lpeak and Leq cover the meter's interval, so
none is lost between frames). An unchanged result (settled, gated, a long hop) is re-sent with a
fresh header every 250 ms, so only a stream without audio goes STALE.

`<meas>` is the decimal measurement id without sign or leading zeros; a topic has exactly
one spelling. A subscription (§5.8) is one topic or one of the prefixes `d/` (all measurement streams), `d/<meas>/` (one measurement —
the trailing slash keeps `d/1/` from matching `d/12/…`), `session/` (both input-meter
topics and the session's goniometer; the preview's goniometer is under `preview/`, so a
client metering alone does not receive its points). Longest topic: 32 bytes.

An `audio/<channel>` frame carries one device block of raw capture to an admitted rig
(§5.9). Only connections whose key was admitted as a rig receive them; anyone else
subscribing to `audio/` receives nothing.

The kind a topic carries (§5.3 `kind`, and the `meta` key):

| topic | kind |
|---|---|
| `d/<meas>/<stream>` | that stream's kind (`tf`, `ir`, `rta`, `spec`, `spl`, `leq`, `band_leq`, `levels`) |
| `session/levels` | `session_levels` |
| `session/preview` | `preview_levels` |
| `session/gonio` | `session_gonio` |
| `preview/gonio` | `preview_gonio` |
| `timing` | `timing` |
| `ka` | `ka` |
| `evt`, `audio/<channel>` | none (not a frame of §5.2) |

### 5.2 Layout

```
part 0   topic (UTF-8)
part 1   body: the msgpack header (≤ 8192 bytes), then, only when the frame has
         arrays, the payload in the same part: deflated (RFC 1951, raw: no zlib or
         gzip wrapper), up to the end of the part
```

A frame goes in one of two forms, the header's second element (`form`):

- **Keyframe** (`form` 0): the full header (§5.3) and the payload in its intra form. It
  needs nothing before it.
- **Delta** (`form` 1): a short header and the payload against the topic's frame of `seq`
  `base`, the frame the receiver applied last. It inherits from that frame everything the
  short header does not carry: kind, incarnation, epoch, `config_rev`, `config_applied_at`,
  `grid_id`, `n`, the arrays and the metadata but for the kind's per-frame fields.

| delta field | type | meaning |
|---|---|---|
| `v` | u16 | protocol version |
| `form` | u8 | 1 |
| `seq` | u64 | this frame's |
| `base` | u64 | `seq` of the frame it applies on |
| `audio_sample` | u64 | as §5.3 |
| `capture_wall_ns` | u64 | as §5.3 |
| `protection` | u32 | as §5.3 |
| `crc` | u32 | CRC of the frame's codes (below) |
| `meta` | […] \| nil | the per-frame metadata: `tf` [filled, settling], `spl` [level, lmax, lmin, leq, lpeak, duration]; nil for every other kind |

The **codes** of a frame are its arrays quantised to their `elem` encoding (below),
concatenated in the header's array order, each code little-endian, a `bfp16` array's
exponent byte first. `crc` is the CRC-32 of the codes (IEEE 802.3, reflected, as zlib's
`crc32`): a keyframe's is checked as it is decoded, a delta's on the codes it makes of its
base. The payload inflates to exactly the codes' size, per array:

| elem | keyframe (intra) | delta |
|---|---|---|
| `i16`, `i16_mag_offset`, `u16` | each code minus the previous column's (the first minus 0), modulo 2¹⁶, as two planes: the low byte of every difference, then the high byte of every difference | each code minus the base's code of the same column, modulo 2¹⁶, as the two planes, not differenced between columns |
| `u8` | each code minus the previous column's (the first minus 0), modulo 2⁸ | each code minus the base's code of the same column, modulo 2⁸ (a held column's γ² repeats, so its byte is 0; NaN in or out is one more byte) |
| `i16` in `deg` | as `i16` | the short way round: (x − b + 18 000) mod 36 000 − 18 000 when both codes are angles (in [−18 000, 18 000)); the NaN code (0x8000) when x is NaN; x − b modulo 2¹⁶ when b is NaN. A frame whose angle array holds any other code goes as a keyframe |
| `bfp16` | the exponent byte, then the mantissas as `i16` intra | the exponent byte, then the mantissas as an `i16` delta if the exponent equals the base's, else as `i16` intra |
| `u32` | as they are | each code XOR the base's |
| `f32`, `i8` | as they are | as they are |

Neighbouring columns of a curve are close, so an intra high plane is mostly 0x00 / 0xff; a
column a deeper analysis stage holds between its hops repeats bit for bit, so its delta is
0. Both deflate to little.

The daemon keeps one chain per topic and connection: the base of a delta is the last frame
of the topic it handed over for that connection. A connection that credit holds back
(§5.8) receives a delta over the longer distance, from the frame it has to the newest;
connections in step share one encode. A frame goes to a connection as a keyframe when it
has no base of the topic yet (its subscription), on its `keyframe` request, after a send
to it that could not complete, at least every 500 ms, when a static field differs from
the base's (another `n`, grid, array set, epoch, config or metadata), and when the delta
would not be smaller than the last keyframe of the topic it was sent. One connection's
keyframes never go to another.

A receiver applies each topic's frames in the order they arrive. A keyframe replaces the
frame held. A delta applies only on the frame of `seq` `base` of its topic and only if the
codes it makes match `crc`; any other delta is refused, never drawn, and the frame held
stays (it reads as STALE once no new frame comes, §5.1). A receiver that held a frame of
the topic and refuses a delta asks for a keyframe of it (`keyframe`, §5.8), at most once
per 250 ms; one that holds none yet waits for the keyframe without asking. A delta never repairs a gap: its keyframe does, within 500 ms or a
round trip after asking.

Columns are in grid order. Invalid values are NaN; where the reason matters a `validity`
bitmask array says why.

Element encodings (`elem`), all little-endian:

| elem | bytes | encoding |
|---|---|---|
| `f32` | 4 | IEEE-754 binary32 |
| `u32` | 4 | unsigned (bitmasks) |
| `i16` | 2 | hundredths of the unit, `round(x × 100)` rounding halves up, clamped to ±32 766; −32 768 = NaN, −32 767 = −∞, 32 767 = +∞. Degrees are wrapped on the hundredths to [−18 000, 18 000), so a phase is in [−180°, 180°) and ±∞ degrees is NaN. Decoded as code ÷ 100 |
| `u16` | 2 | ten-thousandths, `round(x × 10 000)` rounding halves up, clamped to 0 … 10 000; 65 535 = NaN; any other code above 10 000 refuses the frame. Decoded as code ÷ 10 000 |
| `i16_mag_offset` | 2 | an `i16` code minus the `i16` code of the same column's `mag`, modulo 2¹⁶ (as signed: the edge's hundredths of dB from the curve); decoded by adding the `mag` code back, modulo 2¹⁶, then as `i16`. Exact for every pair of codes, NaN and ±∞ included |
| `u8` | 1 | γ² in hundredths rounded down: the largest `k` in 0 … 100 with `k ÷ 100` (as f32) ≤ x, 0 below 0, 100 above 1; 255 = NaN; any other code above 100 refuses the frame. Decoded as code ÷ 100 in f32. A coherence mask in hundredths compares the same on it as on x (0.2996 reads 0.29, below a mask at 0.3); rounding to nearest would lift it over the mask |
| `i8` | 1 | signed; the goniometer's coordinates, −127 … 127 of the pair's auto gain |
| `bfp16` | 2 (+ 1 per array) | block floating point: the array's bytes start with one signed exponent byte `e`, then one `i16` mantissa per element, `round(x × 2^−e)` rounding halves up, clamped to ±32 766, with the `i16` codes of NaN and ±∞. `e` is the smallest integer in [−128, 113] for which every finite \|x\| × 2^−e ≤ 32 765 (from `frexp(peak) = (m, k)`: `k − 14` if `m × 2^15 > 32 765`, else `k − 15`), 0 when no element is finite and non-zero; an exponent above 113 refuses the frame. Decoded as mantissa × 2^e rounded to f32 |

Which arrays use which encoding is fixed by name: `mag`, `phase`, `level` (rta, spec),
`ir_etc` and the `align_*` arrays are `i16`; `range_lo` and `range_hi` are `i16_mag_offset`;
`coh` is `u16`; `range_coh` is `u8`; `ir_linear` is `bfp16`; bitmasks and
counts (`overs`) are `u32`; `gonio_x` and `gonio_y` are `i8`;
everything else (meters, Leq and band Leq windows) is `f32`. Precision: `i16` is within
0.005 dB or 0.005° of the value computed, `u16` within 5·10⁻⁵ of γ² — below the readouts
(0.1 dB, 1°, 0.01) and, for a group delay fitted to the phase over 1/12 octave of
1/48-octave columns, below 13 µs at 20 Hz and 3 µs at 100 Hz, far below the measurement's
own phase noise there. `bfp16` is within 2^(e−1), less than 1/32 765 of the array's peak
(−90.3 dB re peak): a thousandth of a pixel on the linear IR view, 0.03 dB at −40 dB and
0.3 dB at −60 dB on its log view; deeper floors read on `ir_etc`.
Stored traces, captures, exports and math are formed in the daemon from the analysis in
f32, never from frames (design: `docs/design/wire-slim.md`).

### 5.3 Header fields

The keyframe header is an array of these fields in this order; `v` comes first so a frame
of another version is recognised as such before the rest is read, and `form` second. Every struct inside it (array
descriptors, the per-kind metadata of §5.4 and the types they hold) is likewise an array of
its fields in the order listed; an enum with data is an array of its `type` name followed by
its fields (`["verified", calibrated_at, ["acoustic", calibrator_level]]`); the `meta` key
stays a one-entry map `{kind: [...]}`; enums without data stay strings. Nested orders:
`Smoothing` [fraction, mode]; `CalStatus` `uncalibrated` \| `verified` /
`other_mic_or_input` [calibrated_at, basis] \| `line` [calibrated_at, dbu_fs]; `InputKind`
`mic` [mic, curve] \| `line` \| `digital` (tagged by `kind`); `CalBasis` `acoustic` [calibrator_level] \|
`electrical` [connection, mic_sensitivity, data_sheet, loading, uncertainty] (`loading` nil
or [mic_impedance, input_impedance]); `LeqRun` [started_at,
until, measured, gaps, trimmed, laeq, lceq, lzeq]; `LeqPeak` [level, judgement];
`PositionCorrection` [level, peak]; `TimingStatus` [epoch, state, last_lock,
drift, internal_reference]; `TimingState` `no_stimulus` \| `acquiring` \| `locked`
[offset] \| `jumped` [from, to] \| `lost`; `LastLock` [epoch, offset, at_sample, at];
`Drift` [ppm, span, warning, at]. `tools/protocol/ac2proto.py` (`HEADER`, `META`) is the same
layout as code.

| field | type | meaning |
|---|---|---|
| `v` | u16 | protocol version |
| `form` | u8 | 0 (a keyframe; §5.2) |
| `kind` | FrameKind | `tf`, `ir`, `rta`, `spec`, `spl`, `leq`, `band_leq`, `levels`, `session_levels`, `preview_levels`, `session_gonio`, `preview_gonio`, `timing`, `ka`; the kind its topic carries (§5.1) and the `meta` key |
| `seq` | u64 | per topic per incarnation, increasing; not contiguous (a frame replaced before it was sent never goes), so a delta names its base's |
| `audio_sample` | u64 | session sample index (origin = session open) of the newest sample in the frame |
| `session_epoch` | u32 | frames from older epochs are discarded |
| `daemon_incarnation` | u64 | random per daemon start |
| `config_rev` | u64 | rev of the config the DSP actually used (`ka`: current state rev) |
| `config_applied_at` | u64 | sample index from which that config took effect |
| `capture_wall_ns` | u64 | daemon wall clock of the newest sample (Unix ns); frame age = now + offset − this |
| `grid_id` | u64 \| nil | column grid (`tf`, `rta`, `spec`); nil otherwise |
| `protection` | u32 | protection bitmask (§5.5) |
| `n` | u32 | elements per array, ≤ 65536 |
| `arrays` | [[`name`, `unit`, `elem`]] | one descriptor per array, in payload order |
| `meta` | {kind: […]} | per-kind metadata (§5.4), fields in the order listed there |
| `crc` | u32 | CRC-32 of the frame's codes (§5.2) |

`elem` is `f32`, `u32`, `i16`, `i16_mag_offset`, `u16`, `u8`, `i8` or `bfp16` (§5.2), fixed per array name. `unit` is one of `db`, `dbfs`, `dbtp` (dB true peak), `db_spl`, `dbfs_per_hz`, `db_spl_per_hz` (a spectrum density), `deg`, `coherence`
(γ², 0…1), `full_scale` (linear, full scale = 1), `seconds`, `bitmask` (always
`u32`), `count` (an event count, always `u32`), `gonio_unit` (a goniometer coordinate,
always `i8`).

In frame age, `offset` is the daemon's clock minus the client's: the maximum, over a 10 s
window, of a `ka`'s `daemon_wall_ns` minus the local wall time it was received at (the
least delayed keepalive bounds it best). A client reads a topic as STALE when nothing new
has come on it for 1 s (3 s for `leq` and `band_leq`, which come once a second), when its
newest frame's age is over that, or when the daemon stopped responding.

### 5.4 Kinds

| kind | arrays (name: unit) | meta |
|---|---|---|
| `tf` | `mag`: db, `phase`: deg, `coh`: coherence, `validity`: bitmask; an alignment's (all four or none) `align_phase_before`, `align_phase_after`: deg (A − B as measured and with the fit applied), `align_sum_before`, `align_sum_after`: db (the summation per column); an average's range across positions (all three or none) `range_lo`, `range_hi`: db, `range_coh`: coherence | `delay`, `nudged` (Seconds: the part of `delay` `delay.nudge` steps added to the arrival; 0 for a math trace), `smoothing`, `mic_curve`, `mic_range` (`FreqRange` \| nil: the lowest and highest point of that mic curve; outside them the curve is held at its end value, so those columns are not corrected; nil without a curve and for a math trace), `math` (`MathState` \| nil: a math trace's operands, below), `unresolved` ([`BinRange`] \| nil: where the columns are finer than the bins serving them, with their width, §3.2 *Resolution*), `settling` (`FreqRange` \| nil: the frequencies whose averages are still filling, the span served by the analysis stages short of their depth; nil once all are full; a math trace's spans its included live operands'), `filled` (f32, 0…1: how full the averages are, counted in accumulated blocks of the least full analysis stage, so refused blocks add nothing; a math trace's is the least of its included live operands, 1 for traces) |
| `ir` | `ir_linear`: full_scale, `ir_etc`: db (optional) | `sample_rate`, `t0`, `dt`, `inserted_delay`; point i at `t0 + i·dt` |
| `rta` | `level`: dbfs or db_spl (band power), `validity`: bitmask | `fraction`, `weighting`, `scale`, `cal`, `mic_curve`, `mic_range` (as `tf`'s), `math` (`MathState` \| nil) |
| `spec` | `level`: dbfs or db_spl (tone level), dbfs_per_hz or db_spl_per_hz (density; §3.2 *Spectrum level*); smoothed when `smoothing` is set; NaN for no power; on a `log_bins` grid (`fixed`) or a `log` grid (`multi`, §3.2 *Spectrum resolution*): each column the highest level among its bins (tone) or their mean power (density) | `window`, `scale`, `level` (`SpecLevel`), `cal`, `mic_curve`, `mic_range` (as `tf`'s), `smoothing`, `math` (`MathState` \| nil), `bands` (`SpecBands` \| nil), `harmonics` (`SpecHarmonics` \| nil, §3.2 *Harmonics of the generator's sine*) |
| `spl` | none (n = 0) | `scale`, `weighting`, `time_weighting`, `peak_weighting`, `level`, `lmax`, `lmin`, `leq`, `lpeak`, `duration`, `cal`, `mic_curve`, `position` (`PositionCorrection` \| nil: included in the levels) |
| `leq` | one column per window of the meter's configuration (`config_rev`), in its order: `leq`: dbfs or db_spl, `elapsed`: seconds, `measured`: seconds, `allowed`: dbfs or db_spl (headroom; NaN without a judged limit or when it cannot recover), `recover`: seconds (to recover at the limit; NaN unless it cannot within the horizon), `least`: dbfs or db_spl (the Leq the window ends at if the rest is silent; the Leq once full), `over_in`: seconds (until a window `ON_COURSE` spends its budget; else NaN), `leq_flags`: bitmask | `scale`, `cal`, `mic_curve`, `horizon`, `logged` (rows logged so far), `run` (`LeqRun` \| nil, §3.2 SPL log), `lcpeak`, `lafmax` (`LeqPeak` \| nil), `position` (`PositionCorrection` \| nil: included in every level) |
| `band_leq` | one column per band window, in configuration order: `leq`: dbfs or db_spl, `limit`: dbfs or db_spl, `allowed`: dbfs or db_spl, `recover`: seconds, `leq_flags`: bitmask | `scale`, `cal`, `mic_curve`, `horizon`, `correction`, `limits_from`, `windows` ([`BandWindowState`]), `predicted` (`PredictedLeq` \| nil); §3.2 *Band meter* |
| `levels` | `peak`: dbfs, `rms`: dbfs, `clip`: bitmask | `channels` (device input per column; length n) |
| `session_levels` | `peak`: dbfs, `rms`: dbfs, `clip`: bitmask, `ppm`: dbfs (the `ballistic`'s reading; the true peak where VU is chosen but not offered on the input, and under `momentary` / `short_term`), `true_peak`: dbtp, `max`: dbfs (highest `ppm` of the chosen ballistic since `levels.reset`), `overs`: count (since `levels.reset`) | `channels` (device input per column; length n), `ballistic` (`Ballistic`), `pair_levels` ([`PairLevels`] {`left`, `right`, `correlation`: f32 \| nil (−1 … 1 over the rig's window), `quiet`: `left` \| `right` \| nil (the side below the gate; then `correlation` is nil)}: every declared pair whose both inputs are captured), `loudness` ([`SourceLoudness`], below) |
| `preview_levels` | as `session_levels` | `backend`, `device`, `channels` (device input per column; length n), `inputs` ([PreviewInput] as `session.preview` declared them), `pairs` ([Pair]), `ballistic`, `pair_levels` ([`PairLevels`], one per declared pair), `loudness` ([`SourceLoudness`]; a preview's sources never carry into the session) |
| `session_gonio` | `gonio_x`, `gonio_y`: gonio_unit — every pair's 256 points in turn (n = 256 × pairs), `x = (r − l)/√2`, `y = (l + r)/√2` divided by the pair's auto gain (a mono signal is a vertical line, a left-only one leans left), one pair drawn from a random position in each of 256 bins of the frame's samples | `pairs` ([`GonioPairMeta`] {`left`, `right`, `gain_dbfs`: the auto gain, dBFS of the pair magnitude `√(l² + r²)`}) |
| `preview_gonio` | as `session_gonio`, for the previewed device's declared pairs | as `session_gonio` |
| `timing` | none | `status` (TimingStatus), `window` {capture_start, offset, psr, loopback, stimulus} |
| `ka` | none | `rev`, `daemon_wall_ns`, `timing` (TimingState), `generator` {owner, owner_local, armed, firing}, `health` (`DaemonHealth`, below) |

`SourceLoudness` (BS.1770-5 / EBU Tech 3341 and 3342; docs/design/programme-meter.md §6.1):
`source` (`LoudnessSource`: {`type`: `pair`, `left`, `right`} or {`type`: `input`,
`channel`}; zero-based device inputs), `momentary` and `short_term` (LUFS, f64: the last
400 ms and 3 s; −∞ on silence and until a window lies wholly after the source's start or a
capture discontinuity), `integrated` (LUFS \| nil: gated, since the reset; nil until a
block passes the −70 LUFS gate), `range` (LU \| nil: the loudness range; nil until a
short-term value passes its gate; settling while `duration` < 60 s), `max_momentary`,
`max_short_term` (LUFS: −∞ until the first window after the reset), `max_true_peak`
(dBTP, the source's channels; −∞ until the first metered block), `duration` (s of metered
samples since the reset, gaps excluded: what settling reads, independent of the wall
clock), `since` (WallNs: the daemon clock at the reset, for display only). Sources: every
declared pair with both inputs captured and line or digital (stereo, weights 1, 1), then
every captured line or digital input in no pair (weight 1, ascending by channel); none for
a mic input, nor for a pair input whose partner is not captured, nor at a sample rate
outside 44.1 … 192 kHz or not a multiple of 100 Hz. A change of kinds or pairs drops the
sources that changed; new ones start afresh. Nothing is persisted.

`DaemonHealth`: `host` (the daemon's host name as a DNS label, so a client on another
machine names where the figures come from), `cpu` (f64 \| nil: whole-host CPU use over about
the last second, every process on every core, as a share of capacity, 1.0 = every core
busy; nil until sampled twice), `audio` (`AudioHealth` \| nil: nil with no session open).
`AudioHealth`: `load`, `peak` (f64 \| nil: the mean, and the largest single-callback, share
of the block period the daemon's own audio callback took over about the last second;
1.0 = the whole period), `session_load`, `session_peak` (the same since the session
opened), `xruns` (u64: xruns the backend reported since the session opened). The load is
the daemon's callback only, not the whole audio graph (other JACK clients, the driver); a
load is nil when no callback was timed over its span (a replay).

`cal` is a `CalStatus` (§3.2, calibration); `mic_curve` says the mic curve was applied:
subtracted from `mag` (tf, measurement input only; phase untouched) or `level` (spec per
bin, rta per band as a log-frequency power average), or, for `spl`, run as a minimum-phase
filter before frequency weighting — never on `lpeak`, which stays uncorrected.

`MathState`: `operands` [{`operand`: `Operand`, `status`: `OperandStatus`}], one per operand
of the expression in its order, and `phase`: `PhaseBasis` (`shared_time_base`: the phase
keeps the operands' relative arrival, referred to `delay`; `own_alignments`: of each operand
as aligned by its own delay, no shared time base; `no_phase`). `OperandStatus` (tagged by
`type`): `included` (went into this frame), `stopped` (not running), `settling` (running
without a usable result: no valid column yet, or no answer in time), `refused`
{`protection`: the operand's fault flags among `CLIP`, `NO_REFERENCE`, `CHECK_ROUTING`,
`NO_SIGNAL`}, `mismatch` (does not combine with the others: another level scale, spectrum
level (`tone` / `density`), grid or time base). A math trace's own `protection` is always 0: its operands' faults are in their
statuses. `align` (`AlignState` \| nil, an `align` expression's): `outcome` (tagged by
`type`) `fit` {`fit`: `AlignFit`} or `no_fit` {`why`: `AlignNoFit`, tagged by `type`:
`few_operands`, `few_columns` {`found`}, `no_weight`}. `AlignFit`: `best`,
`other_polarity` (\| nil), `runner_up` (\| nil: another delay of the same polarity whose
score is within 0.1 of the best — the band is too narrow to tell), `before`, `after`
(`Summation` {`mean_db`, `worst_db`}: `20·lg(|A+B|/(|A|+|B|))` in the band, its weighted
mean and minimum, before and after the suggestion) and `columns`. `AlignCandidate`:
`delay` (Seconds ≥ 0), `delayed` (`a` \| `b`: the one to delay), `inverted` (its polarity
flipped), `residual_deg` (weighted RMS of the remaining phase difference) and `score`
(weighted mean cosine of it, −1 … 1). `range` (`RangeSpread` \| nil; a transfer average's,
nil for every other expression and for level averages; `docs/design/spatial-walk.md` §10),
tagged by `type`: `too_few` (fewer than 3 positions went in: no band), `envelope`
{`positions`} (3 … 5: lowest to highest level per column) or `percentiles` {`positions`}
(6 on: the 10th to 90th percentile, linear between order statistics at rank `p·(n − 1)`).
Its frame's `range_lo` / `range_hi` arrays are the band's edges, dB, NaN where any included
operand has no level or the average has none, display-smoothed as the curve is, coded as
offsets from the same column's `mag` (`i16_mag_offset`, §5.2);
`range_coh` is the lowest γ² among the included operands that have one (NaN: none has), in
hundredths rounded down (`u8`):
a client leaves a column out of the band where that is below its coherence mask, as it
would blank a single curve there. Live only: a capture of the average keeps no band. A spectrum math result's live frame gathers its bins into display columns like a
spectrum's, each the highest value among its bins (a level difference: the largest in the
column; the mean power for a density result, as a density spectrum's); its capture keeps
every bin. Its `level` is its operands'.

The unit of `level` must match `meta.scale`. A required array missing, an array listed
twice or one that does not belong to the kind refuses the frame.

### 5.5 Bitmasks

Undefined bits refuse the frame.

`validity` (0 = valid): `THINNED` 1, `OUT_OF_BAND` 2, `SETTLING` 4, `NO_REFERENCE` 8,
`NO_MEASUREMENT` 16, `PROTECTED` 32, `BELOW_FLOOR` 64, `INSUFFICIENT_RESOLUTION` 128,
`ABOVE_NYQUIST` 256, `FEW_OPERANDS` 512 (a math trace without the usable operands its
expression needs).

`protection`: `NO_REFERENCE` 1, `NO_SIGNAL` 2, `CLIP` 4, `WEAK_REFERENCE` 8,
`DISCONTINUITY` 16 (averages restarted after a stream gap), `CHECK_ROUTING` 32 (reference
and measurement identical or near-perfectly correlated at zero lag, or reference silent while
the measurement has signal: the inputs look mis-patched).

`clip`: `CLIP` 1 (clipped in this interval), `HELD` 2 (indicator held).

`leq_flags`: `LIMIT` 1 (the window has a limit), `JUDGED` 2 (and it is judged: dB SPL),
`NEAR` 4, `OVER` 8, `CANNOT_RECOVER` 16, `INCOMPLETE` 32 (part of the window not
measured), `ON_COURSE` 64 (filling, with `NEAR`: the Leq so far above the limit). The judgement is `no_limit` without `LIMIT`, `not_calibrated` with `LIMIT` but
not `JUDGED`, else `over`, `near` or `ok`.

### 5.6 Bounds (checked before decoding)

In order: 2 parts; total ≤ 2 MiB; topic valid; `v` and `form` (read on their own); the
header parsed from at most its first 8192 bytes (≤ 8192 bytes); then for a keyframe: `n` ≤ 65536, ≤ 12 arrays, payload bytes after the header exactly when arrays
are listed, the payload inflating to exactly n × Σ element sizes plus one byte per `bfp16`
array (inflation stops there: a
payload that would inflate further is refused, so it cannot be a decompression bomb),
`kind` = the topic's kind (§5.1) = `meta` key, then the per-kind array schema (each array's `elem` as fixed by
its name), `crc`, undefined `u16` codes, `bfp16` exponents and bitmask bits. A delta's
payload is sized by its base: applied, it inflates to exactly the base's codes' size, then
`crc`. A malformed frame is dropped and counted by the client; decoders never panic.

### 5.7 Sizes

Measured on the fake rig (`crates/ac2d/tests/wire_size.rs`, noisy high bands, so dearer
than most real curves); bytes include topic, header and payload, not the transport. The
`spec` rows are at `fixed` resolution:

| frame | as f32 arrays | keyframe | delta |
|---|---|---|---|
| `tf`, 720 columns | | ≈ 2 700 B (header ≈ 180 B) | ≈ 1 350 B |
| `tf` power average with its range band | | ≈ 4 440 B | ≈ 2 800 B |
| `spec`, 65 536 points (897 columns) | 3 696 B | ≈ 1 590 B | ≈ 1 430 B |
| `spec`, 4 096 points (513 columns) | 2 161 B | ≈ 1 025 B | ≈ 930 B |
| `rta`, 1/3 octave (30 bands) | 368 B | ≈ 218 B | ≈ 116 B |
| `band_leq` (11 windows), `leq` (5) | 988 B, 467 B | ≈ 832 B, 379 B | |
| `ir`, 4 096 points (`ir_linear`, `ir_etc`) | 32 896 B | ≈ 13 050 B | ≈ 10 400 B |
| `spl` (no arrays) | 138 B | ≈ 141 B | ≈ 85 B |

A `tf` stream goes out at its analysis hop (≈ 26 frames/s at 48 kHz, whatever the publish
rate), a math trace's whenever an operand changes (up to the publish rate), a default
`spec` with each new spectrum (every `n/8` samples: ≈ 6 per second at 65 536 points),
re-sent every 0.25 s of audio while nothing changes. About 7 % of a `tf` stream's frames go
as keyframes. A session of three transfer functions
and their average, three default spectra and a 4 096-point one, a 1/3-octave RTA, two SPL
meters with band meters and the input meters is ≈ 2.2 Mbit/s at 30 fps and ≈ 2.4 Mbit/s at
60 fps (3.7 with keyframes only); a pane in its IR view adds `ir` at ≈ 30 frames/s,
≈ 4.6 Mbit/s in all at 30 fps and ≈ 4.8 at 60 fps. Encoding and decoding a 720-column `tf`
frame take ≈ 88 and 25 µs on a desktop core.

### 5.8 Flow control

The data socket is a ROUTER on the daemon and one DEALER per client connection. The
client never sets a routing id: libzmq gives every connection its own, so a reconnect is a
new connection, whose subscriptions bring keyframes. Under CURVE the data ROUTER is
authenticated by the same ZAP handler as ctrl.

Client → daemon, one part, a msgpack map `{v, msg}` with `msg` tagged by `type`:

| `type` | fields | meaning |
|---|---|---|
| `window` | `bytes: u32` | the frame bytes this connection takes in flight, clamped to 20 KiB … 256 KiB; 64 KiB until stated |
| `subscribe` | `sub: string` | receive a topic or prefix (§5.1); its topics' newest frames come at once, as keyframes; repeating a held subscription changes nothing |
| `unsubscribe` | `sub: string` | stop receiving it |
| `keyframe` | `topic: Topic` | the next frame of the topic to this connection goes as a keyframe (a chain broke, §5.2); one request per 250 ms is honoured |
| `credit` | `bytes: u32`, `events: u32` | read since the last credit: frame bytes, each frame at least 64, and messages outside the window |

`v` ≠ the daemon's version: the message is dropped. A client sends `window` and its
subscriptions when it connects and again after every reconnect (a completed handshake).

Daemon → client: frames (§5.2), `[evt][event]`, `[ka][frame]` and notices
`[flow][msgpack {v, msg: {type}}]`:

| notice | meaning |
|---|---|
| `resync_required` | events were dropped past the event bound: take a new `state.snapshot` (§4.3) |
| `rejoin` | the daemon holds nothing for this connection (it was dropped as stalled): send `window` and every subscription again |
| `network_off` | the rig's network was turned off (`server.network`): this connection is closed next, sent to network connections only; reconnecting is pointless until it is turned on |

Credit:

- **Frames** are charged their bytes, every part, at least 64. The daemon keeps at most
  the window in flight to a connection; a frame larger than the window goes alone. A frame
  that waits for credit is replaced by its topic's newer one, so credit buys the newest
  frame of each topic, never a backlog: on a link of rate R a connection's standing delay
  is at most window ÷ R, plus its turn in the rotation.
- **Rotation.** A connection with credit is sent its topics with a new frame in turn,
  one frame each, from where its last round stopped, each topic at most at the publish
  rate.
- **Events, keepalives, notices** go at once, outside the window: an event can be neither
  replaced nor dropped, and a keepalive must reach a connection whose window frames fill.
  At most 256 are unacknowledged per connection, and at most 4 MiB of them; past either
  its events are dropped and, once it has room again, the first thing it is sent is
  `resync_required`.
- **Credit on receipt.** A client credits every message it reads, frame or not, whether
  or not it uses it (a refused delta consumed link all the same), at least once per read
  burst and whenever half the window is owed.

A connection is dropped (its subscriptions and chains with it) when a send to it fails
because its connection is gone, or when it holds unacknowledged messages and has sent
nothing for 60 s; one heard from again after that is sent `rejoin`. `rejoin` comes only
after the connection sends something, so a client watches for the completed handshake
(ZMQ socket monitor) and sends `window` and its subscriptions on every reconnect. The data
port is ctrl + 1 (§9).

### 5.9 Raw capture frames (`audio/<channel>`)

An `audio/<channel>` frame is three parts, `[topic][msgpack header][f32 LE samples]`, one
per device block of that channel, never deflated. The header is a msgpack map (it has no
`form`): {`v`, `start_sample` (the source rig's sample index of the first frame),
`frames` (≤ 65536), `flags` (u8: `xrun` 1, the source's backend reported an xrun before
the block; `gap` 2, the block does not continue the one before on the source rig; other
bits refused), `capture_ns` (u64 \| nil: the capture stamp of the first frame on the
source's stamp clock, which its `clocks.stamp_map` puts on system time), `rate`,
`incarnation` (`StreamIncarnation`)}.

Bounds, checked before decoding: three parts; total ≤ 2 MiB; topic valid; header
≤ 8192 bytes, `v` read on its own first; `frames` ≤ 65536; body exactly `frames` × 4
bytes.

Audio is charged to the same byte window as other frames (§5.8: its bytes, every part, at
least 64). Each admitted rig has a FIFO of its own, so a slow rig never holds back the
others. Its frames are never conflated: the FIFO holds up to the window, and past it the
oldest frames are dropped and counted. A frame larger than the window goes alone. The
receiver sees a loss as a jump in `start_sample`.

## 6. Grids

A `grid_id` always names the same `GridDef` (it is a content hash, below). `grid.get(grid_id)`
answers the ids this daemon incarnation has published, else `not_found`:

- `{type: "log", ppo, k_min, k_max}`: columns `1000 · 2^(k/ppo)` Hz, `k = k_min…k_max`.
- `{type: "iec_bands", fraction, centres: [Hz]}`: exact IEC mid-band frequencies.
- `{type: "linear", fs, n}`: FFT bins `k · fs / n`, `k = 0…n/2`.
- `{type: "log_bins", fs, n, ppo}`: the bins of an `n`-point FFT at `fs` gathered into
  display columns (the live `spec` frame; the daemon uses `ppo` 96). With `df = fs / n`
  and `r = 2^(1/ppo)`: bins `k < K = ceil(1 / (r − 1))` are one column each (centre
  `k · df`, edges `(k ± ½)·df`, the lowest clamped at 0); from `e = (K − ½)·df` on, a
  column spans `[e, e·r^j)` for the smallest `j ≥ 1` whose span holds at least one bin
  centre not yet taken (`j = 1` except for rounding), holds the bins `k` with `k · df` in
  it, and is centred at the geometric mean of its edges; the last column ends at
  `(n/2 + ½)·df`. Every bin is in exactly one column. 65 536 points at 48 kHz: 897
  columns for 32 769 bins.

`grid_id` = FNV-1a 64 (offset 0xcbf29ce484222325, prime 0x100000001b3) over canonical
bytes: tag byte (1 log, 2 iec_bands, 3 linear, 4 log_bins), then little-endian fields —
log: `ppo` u32, `k_min` i32, `k_max` i32; iec_bands: band designator b u32 (1, 3, 6, 12,
24), centre count u32, each centre f64; linear: `fs` f64, `n` u32; log_bins: `fs` f64, `n`
u32, `ppo` u32. Example (an example grid, not the transfer grid): log 48/−240/239
(480 columns) = `0x79ec3d16ae0e94d0`.

## 7. Files

What a daemon reads of each artefact; any other version is refused:

| artefact | versions read |
|---|---|
| trace CSV (§7.1) | `v1` … `v6` |
| session directory (§7.2) | manifest version 48 only |
| raw capture sidecar (§7.5) | version 3 only; the protocol types it holds must still decode |
| SPL log (§7.4) | `v2` only |
| band log | `v1` only |
| `file.salvage` | `traces/*.csv` of a session only (each as a trace CSV above) |

### 7.1 Trace text (`trace.import` / `trace.export`)

**ac2 CSV** (`ac2_csv`, what `trace.export` writes): the first line is exactly
`# ac2 trace export v6` (`v5`, `v4`, `v3`, `v2` and `v1` are read too; another version is `bad_header`); then
`# key: value` lines with every metadata field (`name`, `kind`, `source`, `time_base`,
`delay_ms`, `delay_nudge_ms`, `polarity`, `offset_db`, `smoothing` (display only, not
applied), `depth`, `lf_window`, `cal`, `mic` (`name (curve: <label>, …; file …, hash …)`: in the
columns for a capture with a curve, or applied after capture as a display edit, not in the
columns), `mic_curve` (the
JSON `TraceMicCurve`, only when one is applied after capture), `created_ns`, `note`, `grid`
as the JSON `GridDef`); a sweep trace adds `sweep_info` (the JSON `SweepInfo`),
`sweep_ir` (JSON `{t0, dt, points}`), `sweep_unresolved` (the JSON `SweepUnresolved`) and
`room_metrics` (the JSON `RoomAcoustics`); a transfer
trace with an impulse response adds `transfer_ir` (JSON `{sample_rate, t0, dt, points}`). Then the
header
`freq_hz,mag_db[,phase_deg][,coherence]` and one row per grid column; a sweep trace
(`kind: sweep`) adds `h2_db,h2_floor_db,…,h5_db,h5_floor_db,thd_db,thd_floor_db` after
`phase_deg` (dB re the fundamental at the row's fundamental frequency), then the reference's
own `ref_h2_db,ref_h2_floor_db,…,ref_h5_db,ref_h5_floor_db`, and after the
frequency rows its impulse response: the header `t_s,linear,etc_db` and `points` rows
(`t_s` = `t0 + i·dt`, for reading; a transfer trace's impulse response is the same table,
`t_s` re its `delay_ms`), then the room parameters as comment lines for reading
(`# band_hz,edt_s,t20_s,t30_s,c50_db,c80_db,d50,decay_range_db,curvature_pct,onset_s,
truncation_s`, one per band, broadband first; a refused value reads `refused:<why>`).
Values are written in their shortest exact form and gaps
as `nan`, so an export re-imports bit for bit onto the grid named in its header. Import
reads `name`, `kind`, `grid`, `delay_ms` and, for a sweep, `sweep_info`, `sweep_ir`,
`room_metrics`, the distortion columns and the impulse response, for a transfer trace
`transfer_ir` and its impulse response (`sweep_ir` on another kind than `sweep`, or
`transfer_ir` on another than `transfer`, is `bad_header`; as a target, nothing of it is
kept); a v5 sweep export imports without `reference_harmonics`; a v4 sweep export imports without `unresolved`; a v3 export imports as written (no transfer trace in it has an IR); a v2 sweep export imports
as a sweep without room parameters; a v1 sweep export (no `sweep_info`) imports as
its transfer function (note `sweep_without_analysis`).

A dynamics trace (`kind: dynamics`) has no frequency rows: after the common header lines
(without `grid`) come `# dynamics_info:` (the JSON `DynamicsInfo`), `# dynamics_inputs:`
(JSON [{`input`, `name`, `delay`, `g0`}]), then the step table announced by
`# dynamics_steps: {"rows":n}` with the header
`input,step,level_dbfs,burst_s,rest_s,recovered,gr_end_db,gr_settled_db,settled,
attack_tau_s,attack_2db_s,settle_0_5db_s,overshoot_db,overshoot_t_s,overshoot_ref_db,
release_tau_s,recovery_2db_s,recovery_0_5db_s`, then `<w>_h2_db…<w>_h5_db,<w>_thd_db` for
`w` = `attack`, `settled`, `release`, `rest`, then
`thump_start_peak_db,thump_start_rms_db,thump_end_peak_db,thump_end_rms_db,drift_db`; then the
envelopes announced by `# dynamics_envelopes: {"rows":m}` with the header
`input,step,edge,t_s,gr_db,h2_db,h3_db,h4_db,h5_db`. Inputs and steps are 1-based; a time cell
reads seconds, `within_period`, `cant_tell` or `no_gain_reduction`, a level cell dB,
`below_floor` or `cant_tell`; envelope gaps read `nan`. Import restores the trace exactly.
Import takes at most 6 144 000 envelope rows (128 steps × 8 inputs × 6000 points), its own
cap in place of the 65536-row table cap.

**Analyzer text** (`analyzer_text`): columns separated by `,`, `;` (decimal commas
allowed), tabs or spaces; UTF-8 or Latin-1; comment lines starting with `#`, `*`, `;`,
`%`, `!` or `//`; non-numeric lines before the first data row are headers, and a header
naming the columns maps them (freq; mag / SPL / dB / level; phase / deg; coh), otherwise
columns are `freq mag [phase] [coherence]`. Coherence where most values exceed 1 is read as
percent. Frequencies must be positive (0 Hz allowed as the first row) and strictly
ascending, every data row must have the first row's column count, and at least two rows
need a magnitude. `auto` picks ac2 CSV when the first line starts with
`# ac2 trace export`.

### 7.2 Session directory (`file.save` / `file.load`)

```
<dir>/session.json           manifest
<dir>/traces/<id>-<hash>.csv one ac2 CSV per trace (a sweep's whole data included)
<dir>/spl/<name>.csv         one SPL log per SPL meter (§7.4)
<dir>/spl/<name>.bands.csv   its band log, when the meter's band meter logged (§7.4)
```

`session.json`: `{format: "ac2-session", version: 48, saved_at, measurements:
[{id, config: MeasConfig, running, delay: {applied, nudged, tracking} | null, colour:
ColourFamily | null, layout: PaneLayout | null, offset: Db, eq: [EqSet]}], spl_logs:
[{meas, file, bands: file | null}], traces: [{meta: TraceMeta, grid: GridDef, file, mic_curve_points: [[Hz,
dB]] | null}], walks: [WalkState], plans: [PlanSpec], remote_inputs: [RemoteInput]}` (JSON, field names as in this
document; `plans` are the session's copies of the plans its walks walk; `remote_inputs` are
the other rigs' inputs the session took, by slot; `mic_curve_points` are the points of
`meta.mic_curve`, a curve applied after capture). A trace
file is named by its trace id and a 64-bit FNV-1a hash of its content, so it never changes
once written. A save writes the trace files that are not there yet, then replaces
`session.json` atomically (temporary file + rename), then removes the files no manifest
names: a reader sees the old session or the new one, never a mix. An SPL log's
unterminated last line (an append cut short, §7.3) is not read and is not an error.
`format` and `version` are read first; any other version is
refused (no migration; `file.salvage` recovers the traces). Trace files hold the unsmoothed
columns; each trace's display smoothing is its `meta.edit.smoothing`. A directory that
holds other files is never written into.

A session archive (`.ac2session`) is that directory as a gzip-compressed POSIX tar, the
entries at their relative paths (`session.json`, `traces/…`, `spl/…`), regular files only.

### 7.3 Autosave

A daemon started with an autosave directory (`ac2d` by default: `autosave` in the data
directory; `--autosave <dir>`, `--no-autosave`) writes the measurements and traces there in
the §7.2 format whenever they change: after 1.5 s without further changes, at most 10 s
after the first unwritten one, off the control thread, and once more at shutdown. A write
that would not change what is on disk is skipped. `<dir>` is updated in place: a write adds
only the trace files not there yet, renames `session.json` to `session.prev.json` (the
backup) and puts the new manifest in place; files neither manifest names are removed. A
failed or interrupted write therefore leaves the last good manifest.

Each SPL meter's per-second log is a file in `<dir>/spl/` that the daemon appends to: the
new rows every 30 s, synced to the disk every 5 min, when the log ends or the meter goes,
and at shutdown. A growing log is not a change to write (`spl.log_new`, a new meter or a
loaded session is: the manifest names the new log's file). A file is written whole when its
log starts, after a failed append, and once it holds a day of rows past the 48 h retention
(then it keeps the retained rows). A power cut loses at most the last 5 min of a log; a
daemon crash the last 30 s.

At start the daemon loads `<dir>/session.json` (else `session.prev.json`, renaming a
damaged `session.json` to `session.damaged.json`) exactly as `file.load` does — disarmed,
no owner, a new session epoch, no audio I/O opened — logs what it restored, and carries
on appending to the restored logs' files after their last whole line. An autosave of
another session format version is renamed to `<dir>.v<N>`, an unreadable one to
`<dir>.damaged` (a name taken gets the time appended), with a warning; neither is deleted.
An autosave that reads but holds items `file.load` would refuse (a check added since they
were written) costs only those items: each is taken out with what depended on it, kept as a
session of its own in `<dir>.set-aside-<time>`, and reported in `rig.damaged_files` (kind
`autosave`, the reason naming the item); a measurement's traces stay with owner `imported`,
as `meas.delete` with `keep` leaves them. The rest is restored and written back without them.
`--no-restore` starts empty and moves the autosave to `<dir>.unrestored`.

Beside the manifest the autosave keeps `working.json` (`{origin, saved_hash, saved_at}`),
written with each autosave, so `working` (origin, dirty) is the same after a restart; an
autosave without it restores untitled (dirty when it holds anything). The state a
`file.new` / `file.load` replaces while dirty is written whole to `<dir>.replaced` (a
session directory with its own `working.json`) for `file.recover`; without an autosave it
is kept in memory.

The `autosave` entity: `off` (no autosave directory), `saved` (the disk holds the current
state; `saved_at` nil until the first write), `pending` (a change is waiting or being
written), `failed{reason}` (the last write failed; shown until a write succeeds, retried
10 s later). `saved_at` is when the autosave on disk was written (after a restore, when the
restored one was). Status changes are ordinary events, so they bump `rev`.

`log_failures` lists the SPL meters' log files the daemon cannot write at the moment, each
`{meas: MeasId, log: "spl" | "bands", reason, since: WallNs}`: `reason` is short where the
remedy is the same whatever the path (`disk full`, `disk quota exceeded`, `read-only file
system`, `permission denied`), else the file system's message; `since` is the first failed
write of the run. The Leq history in memory is complete; the log on disk misses the seconds
since `since` until a write succeeds (retried every 30 s and with each autosave write), when
the entry goes. An entry also goes with its meter. Each change is an `autosave` event.

### 7.4 SPL log (CSV)

What sessions store and `ac2 spl leq export` writes: the first line is exactly
`# ac2 spl log v2`, then `# key: value` lines (`meas`, `name`, `input` (1-based), `mic`), then the
header
`start_utc,start_ns,measured_s,unit,laeq_1s,lceq_1s,lzeq_1s,lcpeak_1s,lafmax_1s,sensitivity_db,position_db,position_peak_db`
and one row per logged second: ISO 8601 UTC time of the second's start, the same in Unix
ns, the measured time, `dB SPL` or `dBFS`, the five levels in that unit as measured (4
decimals; `-inf` for digital silence), the sensitivity (empty uncalibrated) and the
measuring-position correction in force, energy and peak (empty without one): a corrected
level is the row's level plus the correction, which the row records but never applies.
Reading it back takes `start_ns`, `measured_s`, the levels, the sensitivity and the
correction.

A meter's band log (`<name>.bands.csv`, kept and appended beside its SPL log by sessions and
the autosave, same 48 h retention): first line exactly `# ac2 band log v1`, the same `# key:
value` lines, then the header
`start_utc,start_ns,measured_s,unit,period,correction_db,sensitivity_db,z20hz,z25hz,…,z10000hz`
(28 band columns, `z<nominal>hz`) and one row per second with anything measured: start
(UTC and ns), measured time, `dB SPL` or `dBFS`, `day` or `night` (the limit set of the
second's local start), the §13 correction in force (dB), the sensitivity (empty
uncalibrated; 4 decimals), and the 28 unweighted band Leq in the row's unit to 0.01 dB
(`-inf` without energy), **without** the correction: the rating level is the band level plus
`correction_db`, which the windows apply when they are rebuilt from the log.

### 7.5 Raw capture files (`rec.start`)

Design and replay tolerance: `docs/design/raw-capture.md`. A recording `<name>` is two files
in the recording directory:

- `<name>.wav`: 32-bit IEEE float, little-endian, interleaved, the channels in
  `RecordRequest.inputs` order, exactly the captured values. `WAVE_FORMAT_EXTENSIBLE`
  (float subformat, channel mask 0) with a `fact` chunk; a fixed 116-byte header (`RIFF`,
  `WAVE`, a 28-byte `JUNK` chunk, `fmt ` of 40 bytes, `fact`, `data`). Past 4 GiB the file
  becomes RF64 (EBU Tech 3306): `RIFF` → `RF64`, `JUNK` → `ds64` with the 64-bit sizes, the
  32-bit sizes 0xFFFFFFFF; the audio never moves.
- `<name>.ac2rec.json`: the sidecar, JSON, `format: "ac2-raw-capture"`, `version: 3`
  (another version is refused), `software` {`ac2`, `build`, `protocol`}, `audio`
  {`file`, `sample_rate`, `channels`: [{`input` (session channel), `remote` (`RemoteInput`
  \| nil: whose input a remote channel was), `name`, `mic`, `roles`: [{type: loopback
  \| reference \| measured \| analysed, `measurement`}]}]}, `device` {`backend`,
  `input_device`, `output_device`, `buffer_frames`, `clock`, `session_epoch`, `loopback`},
  `start` and `end.at` (`Mark` {`session_sample`, `wall_ns`, `utc`}), `end` {`at`,
  `frames`, `reason`: RecordingEnd} \| nil while recording, `limits` {`max_duration`,
  `max_bytes`}, `started_by`, `initial` {`measurements`, `generator`, `inputs`,
  `calibrations` (of the recorded inputs on that device)}, `timeline` [{`at_sample`,
  `frame`, `wall_ns`, `change`: {type: measurement \| generator \| inputs \| calibration,
  value}}], `discontinuities` [{`frame` (first file frame after it), `session_sample`,
  `lost_frames`, `estimated`, `causes`: [xrun \| gap \| overflow \| config_change \|
  recorder_behind]}], `remote_gaps` [{`channel` (file channel), `frame`, `session_sample`,
  `frames`, `late`}] (samples another rig never delivered; zeros in the file),
  `remote_slips` [{`channel`, `frame`, `session_sample`, `samples`}] (the remote channel's
  map moved by `samples` from there on). Entity values are the protocol types of `software.protocol` in JSON.

The sidecar is written when recording starts and replaced atomically when it ends. Session
sample of file frame f: `start.session_sample + f + Σ lost_frames` of the discontinuities at
or before f.

A WAV from elsewhere (16-, 24- or 32-bit integer PCM or 32-bit float, e.g. a recorder's)
becomes a recording on the client: `ac2 rec import` (`ac2_traces::raw::import_wav`) writes
its samples as above (integer full scale is ±1.0) with a sidecar of `device.backend`
`replay`, `input_device` = `output_device` = `file:<file name>`, `clock` `unknown`,
`buffer_frames` 0, `start` at the import's wall time and session sample 0, no timeline or
discontinuities, and `end.reason` `stopped`. Calibrations of its replay are keyed by that
device id, so `cal.spl` on a recorded calibrator tone calibrates it.

### 7.6 Impulse response WAV (`ir_wav`)

A RIFF or RF64 WAVE file: PCM 16, 24 or 32 bit or 32-bit float (`WAVE_FORMAT_EXTENSIBLE`
too), any sample rate and channel count. One channel is read: `IrImport.channel`
(0-based; required when the file has several). A file longer than 65536 samples keeps
65536 from 4096 before the peak of `|h|` (note `ir_truncated`).

`IrImport` {`channel: u16 | nil`, `window`, `align`, `resolution`}, nil = defaults:
- `window` (`IrImportWindow`, tagged by `type`): `whole` (default: every sample,
  unwindowed — an exported IR is often windowed already) or `gate` {`pre`, `post`}
  (seconds before and after the peak; half-Hann rise over `pre`, half-Hann fall over the
  last tenth of `post`).
- `align` (`IrAlign`): `peak` (default: time zero is the peak of `|h|`, located between
  samples on the band-limited interpolant; the trace's `delay` is that time re the file's
  first sample, and its phase is the response's own, bulk delay removed) or `start` (time
  zero is the first sample: `delay` 0, the phase carries the file's lead-in).
- `resolution`: the transfer grid of the trace (§3.2 *Resolution*; default `forty_eighth`).

The result is a `transfer` trace: per column the mean over its edges of the zero-padded
FFT's interpolant (power for the magnitude, complex for the phase), NaN above Nyquist; no
coherence. Its `ir` holds every kept sample (`t0` = −the time zero re the first kept
sample, `dt` = 1/fs) and the Hilbert envelope (ETC). `IrFile` {`sample_rate`, `channels`,
`channel`, `frames` (the file's), `kept`, `first` (first kept frame), `peak` (seconds re
the file's first frame), `import` (the `IrImport` used, `channel` filled), `unresolved`
(`Unresolved`: columns narrower than the window's cell `fs / samples`)}.

## 8. Cross-language fixtures

`fixtures/protocol/` holds Rust-encoded (`rust_*.bin`) and Python-encoded (`py_*.bin`)
messages plus expected values (`expected/*.json`). See `tools/protocol/README.md`.

## 9. Discovery (mDNS)

A daemon whose network is on (`ac2d --listen tcp://…` or `server.network`) advertises one
DNS-SD service over mDNS (RFC 6762 / 6763) unless started with `--no-mdns`; it stops when
the network is turned off. Local modes never advertise.

| field | value |
|---|---|
| service type | `_ac2._tcp.local.` |
| instance name | the rig name (`ac2d --name`, default `ac2 on <hostname>`); dots and control characters replaced by `-`, at most 63 bytes |
| port (SRV) | the ctrl (ROUTER) port; the data (ROUTER) port is ctrl + 1 |
| addresses | A / AAAA records of the listening interface (all interfaces for `0.0.0.0`) |

TXT record (all values UTF-8 strings):

| key | meaning |
|---|---|
| `txtvers` | layout of this record, `2`. A reader that does not know the value ignores the advert. |
| `name` | the rig name, as above |
| `v` | daemon version (`ac2d --version`) |
| `proto` | `PROTO_VERSION` the daemon speaks (§2) |
| `fp` | fingerprint of the daemon's CURVE server key: the first 10 bytes of SHA-256 over the 32 raw key bytes as five dash-separated groups of four lowercase hex digits (`1a2b-3c4d-5e6f-7a8b-9c0d`) |
| `key` | the full 40-character Z85 public CURVE server key |

The advert carries no secret and grants nothing. A client connects only with a server key it
pinned after comparing the fingerprint with the one the daemon host shows
(`ac2 auth pair` or a discovered-key pairing dialog), and CURVE fails the handshake when the daemon does not hold that key. Clients
may use `fp` to pick which pinned key belongs to an advert and to warn when a known host
advertises a different fingerprint; they must not pin a key based on an advert alone.
