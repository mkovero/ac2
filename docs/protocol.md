# ac2 protocol (version 1)

Normative description of the wire protocol between `ac2d` and its clients. The Rust types
in `crates/ac2-proto` are the implementation; `crates/ac2-proto/tests/doc_parity.rs` fails
when a command, reply body, event kind, error code, frame header field, frame kind, array
name or unit exists in the code but not here. Background: PLAN.md §6,
`docs/design/q2-q5-q6-protocol.md`, `docs/design/spike-zmq-curve.md`.

The protocol is transport-agnostic bytes. Under ZeroMQ, ctrl is DEALER ↔ ROUTER (one
message frame per request or reply) and data is XPUB/SUB (multipart).

## 1. Encoding rules

- Ctrl messages, events and frame headers are **msgpack maps with named fields**.
  Field order carries no meaning.
- Unknown fields are refused everywhere (strict in both directions). Fields are never
  defaulted when missing, except that an absent optional field reads as `nil`.
- Enums are snake_case strings (`"db_spl"`) when they carry no data. Enums with data are
  maps tagged by `type` (`{"type": "sine", "freq": 1000.0}`) unless stated otherwise.
- Physical quantities are floats (f64) in the unit their type names: `Hz`, `Seconds`, `Db`
  (ratio), `Dbfs` (0 dBFS = RMS of a full-scale sine), `DbSpl`, `Degrees`. Sample counts
  (`Samples`) are signed integers; sample indices and `WallNs` (Unix ns, daemon clock) are
  u64.
- Opaque bytes (`lease_token`, file content) are msgpack `bin`, never base64. A lease
  token is 16 bytes (u128, big-endian).
- Std `Result` values (the reply `result`) are `{"Ok": …}` or `{"Err": …}`.

## 2. Version and hello

`PROTO_VERSION = 1`. Every ctrl message of every version is a map containing `v` (u16) and
`id` (u64); that is the only layout fixed across versions. A receiver reads those two
fields first:

- `v` missing → refused (`MissingVersion`); it is never assumed.
- `v` ≠ own version → refused. The daemon answers with a reply at its own version whose
  `result` is `Err{code: version_mismatch, detail: {type: version, daemon, client}}`. A
  client that receives a reply with another `v` treats it as the same refusal without
  decoding the body.
- There is no negotiation, no range of accepted versions and no compatibility layer.

The first request on a connection is `hello`; its reply `welcome` carries `client_id`
(the identity the daemon bound to the connection: the ZAP User-Id under CURVE, otherwise a
daemon-assigned id), `daemon_incarnation`, `session_epoch` and `rev`.

Frame headers carry `v` too; a frame at another version is refused.

## 3. Ctrl

### 3.1 Request and reply

```
Request { v: u16, id: u64, cmd: {op: "<name>", args?: {…}}, expect_rev: u64 | nil }
Reply   { v: u16, id: u64, result: {"Ok": {type: "<body>", value?: …}} | {"Err": ProtoError} }
ProtoError { code: ErrorCode, msg: string, detail: ErrorDetail | nil }
```

- `args` is absent for commands without arguments.
- `id` is unique per client within the dedup window (last 256 ids, 30 s). A retried id
  returns the stored reply and never executes twice.
- `expect_rev` applies to mutations: if state has moved past it the reply is `conflict`
  with `detail: {type: conflict, rev}` (the current rev). Commits are serial.
- Largest ctrl message: 4 MiB, checked before parsing.

### 3.2 Commands

Lease column: **L** = `lease_token` required (Q6).

| op | args | reply body | lease |
|---|---|---|---|
| `hello` | `client` | `welcome` | |
| `session.devices` | — | `devices` | |
| `session.open` | `config: SessionConfig` | `session` | |
| `session.close` | — | `ack` | |
| `session.status` | — | `session` | |
| `gen.acquire` | `force: bool` | `lease` (`lease_token`, `expires_in_ms`) | |
| `gen.set` | `lease_token`, `desired: {settings, armed, firing}` | `generator` | L |
| `gen.refresh` | `lease_token` | `lease` | L |
| `gen.release` | `lease_token` | `ack` | L |
| `gen.stop` | — | `ack` | universal |
| `meas.create` | `config: MeasConfig` | `measurement` | |
| `meas.update` | `meas`, `config` | `measurement` | |
| `meas.delete` | `meas` | `ack` | |
| `meas.start` | `meas` | `measurement` | |
| `meas.stop` | `meas` | `measurement` | |
| `meas.freeze` | `meas`, `frozen` | `measurement` | |
| `meas.reset` | `meas` | `ack` | |
| `delay.find` | `meas` | `delay_finding` | |
| `delay.insert` | `meas`, `pick: first_arrival \| strongest \| candidate{index}` | `measurement` | |
| `delay.set` | `meas`, `delay: Seconds` | `measurement` | |
| `delay.track` | `meas`, `enabled` | `measurement` | |
| `trace.capture` | `meas`, `name` | `trace` | |
| `trace.list` | — | `traces` | |
| `trace.get` | `trace` | `trace_data` | |
| `trace.update` | `trace`, `edit: TraceEdit` (full replacement) | `trace` | |
| `trace.delete` | `trace` | `ack` | |
| `trace.average` | `traces`, `method`, `reference`, `name` | `trace` | |
| `trace.math` | `a`, `b`, `op: magnitude_difference \| complex_division`, `name` | `trace` | |
| `trace.import` | `file_name`, `format`, `content: bin` | `trace` | |
| `trace.export` | `trace`, `format` | `export` (`file_name`, `content: bin`) | |
| `cal.spl` | `input`, `mic`, `calibrator_level: DbSpl`, `calibrator_freq: Hz` | `calibration` | |
| `cal.mic_curve` | `input`, `action: assign{name, provenance, points} \| bypass{bypassed} \| clear` | `mic_curve` | |
| `cal.list` | — | `calibrations` | |
| `spl.log_start` | `meas`, `interval: Seconds` | `spl_log` | |
| `spl.log_stop` | `meas` | `spl_log` | |
| `ir.capture` | `lease_token`, `input`, `sweep: EssSpec`, `name` | `trace` | L (held for the capture) |
| `state.snapshot` | — | `snapshot` | |
| `state.since` | `rev` | `events` or `resync_required` | |
| `grid.get` | `grid_id` | `grid` | |
| `file.save` | `path` (daemon host) | `ack` | |
| `file.load` | `path` (daemon host) | `ack`; loads disarmed, no owner | |

Rules (Q6): `firing` requires `armed`; arming does not emit. `gen.set` carries the full
desired state and refreshes the lease. Refresh at least every 0.5 s; expiry 1.5 s after
the last refresh fades out (20 ms), disarms and clears the owner. `gen.acquire{force}`
stops and disarms before handing over. Every acquire, force, arm, fire, set, stop, release
and expiry is a `generator` event naming the client.

### 3.3 Reply bodies

`{type, value}` with `type` one of: `ack` (`{rev}`), `welcome`, `devices`, `session`,
`lease`, `generator`, `measurement`, `delay_finding`, `trace`, `traces`, `trace_data`,
`export`, `calibration`, `calibrations`, `mic_curve` (value or nil after `clear`),
`spl_log`, `snapshot`, `events`, `grid`.

### 3.4 Errors

`code` is one of `invalid`, `not_found`, `conflict`, `lease_required`, `lease_held`,
`refused` (safety: level ceiling, firing unarmed, …), `resync_required`, `unsupported`,
`internal`, `version_mismatch`.

`detail` (optional, tagged by `type`): `conflict` {rev}, `lease_held` {owner},
`version` {daemon, client}, `resync` {oldest}.

## 4. State and events

### 4.1 Entities

The mirrored `State` holds: `session` (`epoch`, `open: OpenSession | nil`),
`measurements` (`id`, `config`, `config_rev`, `running`, `frozen`, `delay`, `grid_id`),
`traces` (`TraceMeta`: `id`, `edit` {name, color, visible, locked, order, offset,
polarity, delay_nudge}, `source` {captured | imported | average | math | ir_capture},
`grid_id`, `delay`, `smoothing`, `cal`, `mic`, `created_at`), `generator` (`owner`,
`armed`, `firing`, `settings`, `ceiling`, `last_action`), `calibrations` (`CalEntry`:
`key` {device, channel, mic}, `sensitivity`, `calibrator_level`, `calibrator_freq`,
`measured`, `calibrated_at`), `mic_curves`, `spl_logs`, `timing` (`TimingStatus`: `epoch`,
`state` {no_stimulus | acquiring | locked{offset} | jumped{from, to} | lost}, `last_lock`,
`drift`, `internal_reference`).

### 4.2 Snapshot and events

`state.snapshot` → `{state, rev, daemon_incarnation, session_epoch}`.

An event is `{rev, kind, payload}`. `kind` is one of `session`, `measurement`, `trace`,
`generator`, `calibration`, `mic_curve`, `spl_log`, `timing`. `payload` is the entity's
full new value; for keyed entities (`measurement`, `trace`, `calibration`, `mic_curve`,
`spl_log`) it is `{type: "set", value: <entity>}` or `{type: "deleted", value: <key>}`.
Applying an event is assignment. Events travel on the data socket as
`[b"evt"][msgpack event]` and in `state.since` replies. Largest event: 1 MiB.

### 4.3 Sync procedure (client)

1. Subscribe to `evt` and `ka`; buffer events.
2. Wait for the first `ka` (subscription is live).
3. `state.snapshot` → rev R. Apply buffered events with rev > R in order, then live ones.
4. Event with rev > last + 1 → `state.since(last)`; `resync_required` → back to 3.
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
| `d/<meas>/levels` | input meters of the measurement's channels |
| `timing` | loopback timing monitor |
| `evt` | state events (§4.2) |
| `ka` | keepalive, every 250 ms |

`<meas>` is the decimal measurement id without sign or leading zeros; a topic has exactly
one spelling. Prefixes: `d/` (all measurement streams), `d/<meas>/` (one measurement —
the trailing slash keeps `d/1/` from matching `d/12/…`). Longest topic: 32 bytes.

### 5.2 Layout

```
part 0   topic (UTF-8)
part 1   header (msgpack map, ≤ 1024 bytes)
part 2…  arrays: each exactly n × 4 bytes, little-endian; f32 or u32 per header
```

Columns are in grid order. Invalid values are NaN; where the reason matters a `validity`
bitmask array says why.

### 5.3 Header fields

| field | type | meaning |
|---|---|---|
| `v` | u16 | protocol version |
| `kind` | FrameKind | `tf`, `ir`, `rta`, `spec`, `spl`, `levels`, `timing`, `ka`; equals the topic and the `meta` key |
| `seq` | u64 | per topic per incarnation; clients keep the max per topic when draining |
| `audio_sample` | u64 | session sample index (origin = session open) of the newest sample in the frame |
| `session_epoch` | u32 | frames from older epochs are discarded |
| `daemon_incarnation` | u64 | random per daemon start |
| `config_rev` | u64 | rev of the config the DSP actually used (`ka`: current state rev) |
| `config_applied_at` | u64 | sample index from which that config took effect |
| `capture_wall_ns` | u64 | daemon wall clock of the newest sample (Unix ns); frame age = now + offset − this |
| `grid_id` | u64 \| nil | column grid (`tf`, `rta`, `spec`); nil otherwise |
| `protection` | u32 | protection bitmask (§5.5) |
| `n` | u32 | elements per array, ≤ 65536 |
| `arrays` | [{`name`, `unit`, `elem`}] | one descriptor per array part, in part order |
| `meta` | {kind: {…}} | per-kind metadata (§5.4) |

`elem` is `f32` or `u32`. `unit` is one of `db`, `dbfs`, `db_spl`, `deg`, `coherence`
(γ², 0…1), `count`, `full_scale` (linear, full scale = 1), `bitmask` (always `u32`).

### 5.4 Kinds

| kind | arrays (name: unit) | meta |
|---|---|---|
| `tf` | `mag`: db, `phase`: deg, `coh`: coherence, `eff_avg`: count (optional — presence = listed), `validity`: bitmask | `delay`, `frozen`, `smoothing`, `mic_curve` |
| `ir` | `ir_linear`: full_scale, `ir_etc`: db (optional) | `sample_rate`, `t0`, `dt`, `inserted_delay`; point i at `t0 + i·dt` |
| `rta` | `level`: dbfs or db_spl (band power), `validity`: bitmask | `fraction`, `weighting`, `scale` |
| `spec` | `level`: dbfs or db_spl (tone level), `validity`: bitmask | `window`, `scale` |
| `spl` | none (n = 0) | `scale`, `weighting`, `time_weighting`, `peak_weighting`, `level`, `lmax`, `lmin`, `leq`, `lpeak`, `duration` |
| `levels` | `peak`: dbfs, `rms`: dbfs, `clip`: bitmask | `channels` (device input per column; length n) |
| `timing` | none | `status` (TimingStatus), `window` {capture_start, offset, psr, loopback, stimulus} |
| `ka` | none | `rev`, `daemon_wall_ns`, `timing` (TimingState), `generator` {owner, armed, firing} |

The unit of `level` must match `meta.scale`. A required array missing, an array listed
twice or one that does not belong to the kind refuses the frame.

### 5.5 Bitmasks

Undefined bits refuse the frame.

`validity` (0 = valid): `THINNED` 1, `OUT_OF_BAND` 2, `SETTLING` 4, `NO_REFERENCE` 8,
`NO_MEASUREMENT` 16, `PROTECTED` 32, `BELOW_FLOOR` 64, `INSUFFICIENT_RESOLUTION` 128,
`ABOVE_NYQUIST` 256.

`protection`: `NO_REFERENCE` 1, `NO_SIGNAL` 2, `CLIP` 4, `WEAK_REFERENCE` 8,
`DISCONTINUITY` 16 (averages restarted after a stream gap).

`clip`: `CLIP` 1 (clipped in this interval), `HELD` 2 (indicator held).

### 5.6 Bounds (checked before decoding)

In order: 2 … 10 parts; total ≤ 2 MiB; topic valid; header ≤ 1024 bytes; then after the
bounded header parse: `v`, `n` ≤ 65536, array count = parts − 2, every array part exactly
n × 4 bytes, `kind` = topic = `meta` key, then the per-kind array schema and bitmask bits.
A malformed frame is dropped and counted by the client; decoders never panic.

### 5.7 Sizes

A 480-column `tf` frame: topic 6 B, header ≈ 390 B, four arrays (mag, phase, coh,
validity) of 1920 B — ≈ 8.1 KB; ≈ 10.0 KB with `eff_avg`. At 60 fps ≈ 0.5–0.6 MB/s per
measurement locally, half that remote at 30 fps.

## 6. Grids

`grid.get(grid_id)` returns a `GridDef`, valid for the incarnation:

- `{type: "log", ppo, k_min, k_max}`: columns `1000 · 2^(k/ppo)` Hz, `k = k_min…k_max`.
- `{type: "iec_bands", fraction, centres: [Hz]}`: exact IEC mid-band frequencies.
- `{type: "linear", fs, n}`: FFT bins `k · fs / n`, `k = 0…n/2`.

`grid_id` = FNV-1a 64 (offset 0xcbf29ce484222325, prime 0x100000001b3) over canonical
bytes: tag byte (1 log, 2 iec_bands, 3 linear), then little-endian fields — log: `ppo`
u32, `k_min` i32, `k_max` i32; iec_bands: band designator b u32 (1, 3, 6, 12, 24), centre
count u32, each centre f64; linear: `fs` f64, `n` u32. Example: log 48/−240/239 (480
columns) = `0x79ec3d16ae0e94d0`.

## 7. Cross-language fixtures

`fixtures/protocol/` holds Rust-encoded (`rust_*.bin`) and Python-encoded (`py_*.bin`)
messages plus expected values (`expected/*.json`). See `tools/protocol/README.md`.
