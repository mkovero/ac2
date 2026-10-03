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

`PROTO_VERSION = 4`. Every ctrl message of every version is a map containing `v` (u16) and
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
| `session.devices` | — | `backends` | |
| `session.preview` | `backend: BackendKind`, `device: DeviceId` | `preview` | |
| `session.preview_stop` | — | `ack` | |
| `session.detect_loopback` | `lease_token`, `backend`, `device`, `output: u16`, `level: Dbfs \| nil` | `loopback_detection` | L |
| `session.open` | `config: SessionConfig` | `session` | |
| `session.close` | — | `ack` | |
| `session.status` | — | `session` | |
| `session.inputs` | `inputs: [InputSetup]` (upserted by channel) | `inputs` (the whole setup) | |
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
| `delay.find` | `meas`, `band: FinderBand`, `observation: Seconds \| nil` | `delay_finding` | |
| `delay.insert` | `meas`, `pick: first_arrival \| strongest \| ranked{index}` | `measurement` | |
| `delay.set` | `meas`, `delay: Seconds` | `measurement` | |
| `delay.track` | `meas`, `enabled` | `measurement` | |
| `trace.capture` | `meas`, `name`, `slot` (1…9 \| nil) | `trace` | |
| `trace.list` | — | `traces` | |
| `trace.get` | `trace` | `trace_data` | |
| `trace.update` | `trace`, `edit: TraceEdit` (full replacement) | `trace` | |
| `trace.delete` | `trace` | `ack` | |
| `trace.average` | `traces`, `method`, `reference`, `name` | `trace` | |
| `trace.math` | `a`, `b`, `op: magnitude_difference \| complex_division`, `name` | `trace` | |
| `trace.import` | `file_name`, `format: ac2_csv \| analyzer_text \| auto`, `role: trace \| target`, `content: bin` | `trace` | |
| `trace.export` | `trace`, `format: ac2_csv` | `export` (`file_name`, `content: bin`) | |
| `cal.spl` | `input`, `mic`, `calibrator_level: DbSpl`, `calibrator_freq: Hz` | `calibration` | |
| `cal.mic_curve` | `input`, `mic`, `action: import{file_name, content: bin} \| clear` | `calibration` (`import`), `ack` (`clear`) | |
| `cal.list` | — | `calibrations` | |
| `cal.delete` | `key: CalKey`, `part: sensitivity \| mic_curve \| all` | `ack` | |
| `spl.log_start` | `meas`, `interval: Seconds` | `spl_log` | |
| `spl.log_stop` | `meas` | `spl_log` | |
| `ir.capture` | `lease_token`, `input`, `sweep: EssSpec`, `name` | `trace` | L (held for the capture) |
| `state.snapshot` | — | `snapshot` | |
| `state.since` | `rev` | `events` or `resync_required` | |
| `grid.get` | `grid_id` | `grid` | |
| `file.save` | `session: SessionRef` | `session_file` | |
| `file.load` | `session: SessionRef` | `session_file`; loads disarmed, no owner, new epoch | |
| `file.list` | — | `sessions` | |

Rules (Q6): `firing` requires `armed`; arming does not emit. `gen.set` carries the full
desired state and refreshes the lease. Refresh at least every 0.5 s; expiry 1.5 s after
the last refresh fades out (20 ms), disarms and clears the owner. `gen.acquire{force}`
stops and disarms before handing over. Every acquire, force, arm, fire, set, stop, release
and expiry is a `generator` event naming the client (`last_action.client`; for `expiry` the
owner whose lease expired).

#### Measurements (`meas.*`)

`meas.update` replaces the configuration and restarts a running job (averages start over),
except when a transfer or spectrum measurement's configuration differs only in `smoothing`
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
  frequency (`1/f`); DC and bins narrower than the kernel pass through, and bins below the
  floor (non-finite level) are gaps the kernel never crosses. A smoothed bin is no longer
  the tone level of that bin: frames say so (`SpecMeta.smoothing`) and clients label it.

#### Devices, preview and loopback detection (`session.*`)

`session.devices` answers `[BackendInfo]`, one per backend the daemon offers, the one it was
started on first: `kind: BackendKind` (`jack` \| `cpal` \| `fake`), `description` (for the
operator), `availability` (tagged by `type`: `available` \| `unavailable` {`reason`}: why,
in plain words with the remedy, e.g. `No JACK server: start JACK (e.g. `jackd -d alsa`) or
use PipeWire`) and `devices: [DeviceInfo]` (empty while unavailable). A daemon started on
real audio offers its platform's backend (`jack` on Linux, `cpal` on macOS and Windows);
one started on the simulated rig offers only that (a simulated device never stands in for
a missing real one).

`DeviceInfo`: `backend`, `host`, `id`, `name` (display name), `input` / `output`:
`DirectionInfo | nil`, `duplex_clock`, `index`, `notes`. `DirectionInfo`: `max_channels`,
`rates_hz` ([{min, max}]), `buffer_frames` ({min, max} \| nil), `default_rate_hz`,
`default_buffer_frames` (u32 \| nil), `channel_names` ([string], one per channel, \| nil
where the backend does not name channels: JACK gives the port alias or short name, cpal
nothing).

`SessionConfig`: `backend: BackendKind | nil` (nil = the default backend), `input_device`,
`output_device` (`DeviceSelector`: `default` \| `id` {`id`}), `input_channels` ([u16],
zero-based), `output_channels` (u16, a count), `sample_rate_hz`, `buffer_frames`,
`loopback` ({`output`, `input`} \| nil). `OpenSession` adds `backend` (the one used),
`input_device`, `output_device`, `sample_rate_hz`, `buffer_frames`, `clock`, `opened_at`.

While a session is open the daemon meters every captured input on `session/levels` (§5.1)
whether or not a measurement runs: per-interval sample peak, 300 ms integrated RMS and clip
(held 1 s), at most 30 frames per second, published only while subscribed.

**Preview.** `session.preview` opens a device for capture only — no output stream exists,
so nothing can be emitted — and publishes meters of every input on `session/preview`
(meta names the device; `audio_sample` counts from the preview's start, `session_epoch` is
the epoch it opened in). Reply `Preview`: `backend`, `device`, `channels` (inputs metered,
`0 .. channels`), `sample_rate_hz`, `expires_in_ms`. There is one preview per daemon;
naming the same device again renews it, another device replaces it. It closes on
`session.preview_stop`, `session.open`, `session.detect_loopback`, or when not renewed
within `expires_in_ms` (5 s). Opening it may fail where the host allows a device only one
stream (`unsupported` / `not_found` with the host's reason).

**Loopback detection.** `session.detect_loopback` needs the stimulus lease (`lease_required`
otherwise) and an explicit `level` (`refused` when nil; there is no default level); a level
above the global ceiling, or a request while the stimulus is armed or firing, is `refused`.
The daemon closes the preview, opens the device with every input and `output + 1` outputs,
plays a 0.5 s pink-noise burst band-limited to 100 Hz – 10 kHz at `level` (RMS) on `output`
only — faded in and out (20 ms), under the global ceiling and the output path's peak limit —
then closes the stream. Each input's capture is cross-correlated with the burst as emitted
(over delays 0 … 0.5 s). Reply `LoopbackDetection`: `backend`, `device`, `output`, `level`,
`ranked: [LoopbackCandidate]` (every input, best first: by normalised correlation, the
earlier arrival first among equally good ones), `loopback: u16 | nil` (the first-ranked
input when its |correlation| ≥ 0.8, else nil), `clock`. `LoopbackCandidate`: `input`,
`delay: Seconds`, `delay_samples: Samples`, `correlation` (−1 … 1), `gain: Db | nil` (nil:
silent input). Delays are exact on a `single_callback` clock; otherwise they share an
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
- `DelayArrival`: `delay: Seconds`, `delay_samples` (f64, fractional), `level: Db` (re the
  strongest), `phase: Degrees`, `uncertainty_samples` (f64, 1 σ), `misfit` (f64),
  `refined` (bool).
- `DelayConfidence` (nil where the finder refused before reaching it): `psr_db`,
  `psr_acq_db`, `band_snr_db` (Db), `excited_fraction` (0…1), `uncertainty_samples` (of
  the rule pick), `pulse_width_samples`, `period` (Samples).

`DelayBand` is `FinderBand` without `auto` (the band actually analysed). `AmbiguityReason`:
`borderline_level`, `close_arrivals`, `merged_lobe`, `outside_refinement`.
`NoEstimateReason` (tagged by `type`): `no_reference`, `no_signal`,
`observation_too_short` (also: not enough audio captured yet), `insufficient_overlap`,
`insufficient_excitation`, `periodic_excitation` {`period`: Samples}, `low_psr`,
`low_precision`, `peak_at_search_edge`, `low_band_snr`. A refusal is a successful reply
with a `no_estimate` outcome, never an error; errors are kept for requests that cannot run
(not a running transfer measurement, invalid band or observation).

Every finding is stored as the measurement's `delay.last_finding`. `delay.insert` applies
it: `first_arrival` takes the accepted first arrival or, when ambiguous, the pre-selected
`ranked[0]`; `strongest` the strongest arrival; `ranked{index}` an entry of the ambiguous
list. Inserting from a `no_estimate` finding is `refused`. `delay.set` (an explicit operator
value) clears `last_finding`; a delay tracking moves keeps it.

`DelayState` (a transfer measurement's `delay`): `applied: Seconds`, `applied_samples`,
`tracking` (the operator's switch), `awaiting_pick`, `last_finding: DelayFinding | nil`. An
`ambiguous` finding sets `awaiting_pick` (decision 1c): tracking is paused — it moves nothing
— until the operator resolves it with `delay.insert` or `delay.set`, or runs `delay.find`
again (a new finding that is not ambiguous clears it).

#### Traces (`trace.*`)

A trace's metadata (`TraceMeta`, §4.1) is mirrored state; its columns leave the daemon only
through `trace.get` (`TraceData`: `meta`, `mag_db`, `phase_deg` (nil = magnitude only),
`coherence` (nil = unknown), column order = the trace's grid, NaN = no value), `trace.export`
and `file.save`. Columns are stored as measured: offset, polarity, nudge and smoothing are
display edits and are never applied to the stored data.

- **Smoothing.** `TraceEdit.smoothing` (`Smoothing` \| nil; transfer and spectrum traces
  only — any other kind is `invalid`) is applied by the daemon when it serves `trace.get`,
  with the live job's kernel (a spectrum has no phase: its power is smoothed in either
  `mode`; a spectrum capture starts with `magnitude`); coherence is never smoothed.
  `trace.export` and `file.save` write the unsmoothed columns (the setting is listed in the
  CSV header), and `trace.average` / `trace.math` combine unsmoothed columns. A capture
  starts with the smoothing its measurement had; an average or A − B starts with the
  smoothing its inputs share (nil when they differ).

- `trace.capture` stores the measurement's newest published `tf`, `spec` or `rta` frame —
  what clients were shown, for a `tf` or `spec` frame before its display smoothing — with
  columns
  whose validity mask is set stored as NaN.
  It needs a result in the current session epoch (`invalid` otherwise: not running, no
  frame yet, SPL measurement). Metadata: `kind` (`TraceKind`, tagged by `type`: `transfer`,
  `target`, `spectrum` {`scale`}, `rta` {`scale`}), `source.captured` {`meas`, `meas_name`,
  `epoch`, `at_sample`}, `delay` (the delay the DSP used), `depth` (transfer),
  `cal` (spectrum / RTA: the calibration the measurement used, picked by the calibration
  matching rules below — its `key` names another mic or input when it was not this mic's;
  transfer functions are ratios and always `uncalibrated`), `mic` (the input setup's mic
  name and the mic curve applied, nil without a mic name), `created_at`.
- **Slots.** `TraceEdit.slot` (1…9 or nil). A slot holds at most one trace: capturing or
  updating into a slot clears it on the trace that held it (a `trace` event for that one
  too).
- **Lock.** `trace.update` on a locked trace may change only `visible`, `order`, `slot` and
  `locked` (not `smoothing`); `trace.delete` of a locked trace is `refused`.
- **Time base (decisions 8a / 8b).** Captured traces share the time base of their session
  epoch; every other source (`imported`, `average`, `math`) is independent.
- `trace.average`: ≥ 2 distinct traces of one kind (no targets). Transfer: `power` (RMS
  magnitude, phase of the complex mean), `complex`, `coherence_weighted` (weight
  γ²/(1 − γ²), γ² capped at 0.999); every input's phase is re-referred to `reference`
  (`DelayReference`: `trace` {`trace`} = that input's measured delay, or `fixed` {`delay`})
  before combining, and the result's `delay` is that reference. Phase methods need every
  input captured in the same epoch (`invalid` otherwise); a power average without a shared
  time base, or with an input without phase, keeps the magnitude only. Spectrum / RTA:
  `power` only, all on one grid. A column is valid only where every input is. The result is
  on the first trace's grid (others resampled).
- `trace.math`: `magnitude_difference` = A − B in dB (no phase); `complex_division` = A / B
  with B's phase re-referred to A's delay when both are captured in one epoch (otherwise
  each keeps its own alignment). Transfer and target traces combine with each other (B is
  resampled onto A's grid); spectra / RTA only with their own kind on one grid, magnitude
  only. Result kind `transfer`, source `math`, independent.
- `trace.import`: the file text is parsed by `format` (§7.1) and, unless it is an ac2 CSV
  on a known grid, resampled onto a log grid (48 points per octave, 96 when the file is
  denser) — magnitude and coherence linear over log frequency, phase unwrapped first.
  `role: target` keeps the magnitude only and makes `kind: target`. The name is the ac2
  header's `name`, else the file name without extension. A refused file is `invalid` with
  `detail: {type: import, line (1-based) | nil, problem: ImportProblem}`.
  `ImportProblem`: `not_text`, `no_data`, `bad_number`, `column_count`, `too_few_columns`,
  `not_ascending`, `out_of_range`, `too_many_rows` (> 65536), `bad_header`,
  `bad_coherence`.

#### Sessions (`file.*`)

`SessionRef` (tagged by `type`): `name` {`name`} — a directory in the daemon's session
directory (letters, digits, space, `-`, `_`, `.`; not starting with `.`) — or `path`
{`path`} — an absolute directory on the daemon host, accepted from local transports only
(`refused` in network mode). `SessionFile`: `name`, `path`, `saved_at`, `measurements`,
`traces`.

- `file.save` writes measurement configurations (applied delay, tracking, running,
  frozen) and every trace with metadata, edits, slots and columns (format §7.2). Never
  generator state; calibrations belong to the calibration store.
- `file.load` checks the whole session first (a refusal changes nothing), then stops and
  disarms the generator and drops its owner (the old lease is gone), deletes every
  measurement and trace, starts a new session epoch newer than every epoch recorded in the
  loaded traces (an open stream reopens without generator routes), and recreates the
  measurements (same ids, restarted when they were running) and traces (same ids). Errors:
  `not_found`; `invalid` (not a session, bad name, damaged files); `unsupported` with
  `detail: {type: session_version, found, supported}` for another format version.
- `file.list`: the sessions in the session directory, by name.
#### Calibration (`cal.*`, `session.inputs`)

Design: `docs/design/q7-calibration.md`. A calibration entry is keyed by the open session's
capture device, the input channel and the mic name (`CalKey`), so `cal.spl` and
`cal.mic_curve` need an open session.

- `cal.spl` reads the input's broadband RMS (uncorrected, τ = 1 s) and stores `spl: SplCal`
  {`sensitivity`: Db (dB SPL of 0 dBFS), `calibrator_level`, `calibrator_freq`,
  `measured`: Dbfs, `calibrated_at`} on the entry, keeping its mic curve. It is `refused`
  below −80 dBFS and while the level is not steady (0.2 s and 1 s readings differ by more
  than 0.05 dB).
- `cal.mic_curve` `import` parses a magnitude file (frequency, gain dB, further columns
  ignored; text lines skipped; whitespace / comma or semicolon + decimal-comma separated)
  and stores `mic_curve: MicCurveRef` {`name`, `file_name`, `content_hash` (FNV-1a 64 hex),
  `points`, `f_lo`, `f_hi`, `imported_at`} on the entry; the points stay in the daemon. A
  refused file is `invalid` with `detail: {type: mic_curve_file, line: u32 | nil, reason}`,
  `reason` one of `too_few_points`, `too_many_points`, `bad_number`, `missing_gain`,
  `non_positive_frequency`, `non_finite`, `gain_out_of_range` (|gain| > 40 dB),
  `not_ascending`. `clear` removes the curve; an entry holding neither a calibration nor a
  curve is deleted.
- Both set the input's mic name to `mic` (the name is typed once, at calibration time).
- `cal.delete` removes from the entry `key` (any device; no open session needed) its
  sensitivity calibration (`sensitivity`), its curve (`mic_curve`) or both (`all`); an entry
  left with neither is deleted (`calibration` event `deleted`). `not_found` when there is no
  such entry or it does not hold the part named. The input setup is not changed.
- `InputSetup` = {`channel`, `mic`: string | nil, `mic_curve`: bool (on/off of the curve,
  decision 7c)}. Mic names are 1–64 characters.
- When the daemon's calibration store file cannot be read it is never written: `cal.spl`,
  `cal.mic_curve`, `cal.delete`, `cal.list` and `session.inputs` are `refused` with
  `detail: {type: cal_store, path, reason}`.

Which calibration a measurement uses (shown as `CalStatus` in `spl`, `rta` and `spec`
frames): the entry of device + input + the input's mic → `verified`; else the newest one
on the same device + input (another mic, or no mic name set), else the newest one for the
same mic elsewhere → `other_mic_or_input`; else `uncalibrated` (dBFS). `CalStatus` is
tagged by `type`: `uncalibrated` | `verified` {`calibrated_at`} | `other_mic_or_input`
{`calibrated_at`}; the age is `capture_wall_ns − calibrated_at`. The mic curve follows the
mic name (that entry's curve, else the newest curve for the same mic) and applies while
the input's `mic_curve` is on; it is normalised to 0 dB at the calibrator frequency in use
(1 kHz uncalibrated).

#### Averaging depth

`TransferConfig.depth: DepthPolicy` (tagged by `type`): `equal_confidence` (every MTW stage
reaches the same effective-average count) or `fast_lf` {`max_settle_s`: Seconds > 0} (no
decimated stage averages over a longer span; those stages show a higher coherence floor).

### 3.3 Reply bodies

`{type, value}` with `type` one of: `ack` (`{rev}`), `welcome`, `backends`, `preview`,
`loopback_detection`, `session`,
`lease`, `generator`, `measurement`, `delay_finding`, `trace`, `traces`, `trace_data`,
`export`, `calibration`, `calibrations`, `inputs`, `spl_log`, `snapshot`, `events`,
`grid`, `session_file`, `sessions`.

### 3.4 Errors

`code` is one of `invalid`, `not_found`, `conflict`, `lease_required`, `lease_held`,
`refused` (safety: level ceiling, firing unarmed, …), `resync_required`, `unsupported`,
`internal`, `version_mismatch`.

`detail` (optional, tagged by `type`): `conflict` {rev}, `lease_held` {owner},
`version` {daemon, client}, `resync` {oldest}, `import` {line, problem} (§3.2 traces),
`session_version` {found, supported} (§3.2 sessions), `mic_curve_file` {line, reason} and
`cal_store` {path, reason} (§3.2 calibration).

## 4. State and events

### 4.1 Entities

The mirrored `State` holds: `session` (`epoch`, `open: OpenSession | nil`),
`measurements` (`id`, `config`, `config_rev`, `running`, `frozen`, `delay`, `grid_id`),
`traces` (`TraceMeta`: `id`, `edit` {name, color, visible, locked, order, offset,
polarity, delay_nudge, slot, smoothing}, `kind`, `source` {captured | imported | average |
math | ir_capture}, `grid_id`, `delay`, `depth`, `cal`, `mic`, `created_at`), `generator` (`owner`,
`armed`, `firing`, `settings`, `ceiling`, `last_action`), `calibrations` (`CalEntry`:
`key` {device, channel, mic}, `spl`: SplCal | nil, `mic_curve`: MicCurveRef | nil),
`inputs` ([InputSetup], sorted by channel), `spl_logs`, `timing` (`TimingStatus`: `epoch`,
`state` {no_stimulus | acquiring | locked{offset} | jumped{from, to} | lost}, `last_lock`,
`drift`, `internal_reference`).

### 4.2 Snapshot and events

`state.snapshot` → `{state, rev, daemon_incarnation, session_epoch}`.

An event is `{rev, kind, payload}`. `kind` is one of `session`, `measurement`, `trace`,
`generator`, `calibration`, `inputs`, `spl_log`, `timing`. `payload` is the entity's
full new value (`inputs`: the whole list); for keyed entities (`measurement`, `trace`,
`calibration`, `spl_log`) it is `{type: "set", value: <entity>}` or `{type: "deleted", value: <key>}`.
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
| `session/levels` | input meters of every input of the open session |
| `session/preview` | input meters of every input of the previewed device |
| `timing` | loopback timing monitor |
| `evt` | state events (§4.2) |
| `ka` | keepalive, every 250 ms |

`<meas>` is the decimal measurement id without sign or leading zeros; a topic has exactly
one spelling. Prefixes: `d/` (all measurement streams), `d/<meas>/` (one measurement —
the trailing slash keeps `d/1/` from matching `d/12/…`), `session/` (both input-meter
topics). Longest topic: 32 bytes.

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
| `kind` | FrameKind | `tf`, `ir`, `rta`, `spec`, `spl`, `levels`, `session_levels`, `preview_levels`, `timing`, `ka`; equals the topic and the `meta` key |
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
| `rta` | `level`: dbfs or db_spl (band power), `validity`: bitmask | `fraction`, `weighting`, `scale`, `cal`, `mic_curve` |
| `spec` | `level`: dbfs or db_spl (tone level; smoothed when `smoothing` is set), `validity`: bitmask | `window`, `scale`, `cal`, `mic_curve`, `smoothing` |
| `spl` | none (n = 0) | `scale`, `weighting`, `time_weighting`, `peak_weighting`, `level`, `lmax`, `lmin`, `leq`, `lpeak`, `duration`, `cal`, `mic_curve` |
| `levels` | `peak`: dbfs, `rms`: dbfs, `clip`: bitmask | `channels` (device input per column; length n) |
| `session_levels` | `peak`: dbfs, `rms`: dbfs, `clip`: bitmask | `channels` (device input per column; length n) |
| `preview_levels` | `peak`: dbfs, `rms`: dbfs, `clip`: bitmask | `backend`, `device`, `channels` (device input per column; length n) |
| `timing` | none | `status` (TimingStatus), `window` {capture_start, offset, psr, loopback, stimulus} |
| `ka` | none | `rev`, `daemon_wall_ns`, `timing` (TimingState), `generator` {owner, armed, firing} |

`cal` is a `CalStatus` (§3.2, calibration); `mic_curve` says the mic curve was applied:
subtracted from `mag` (tf, measurement input only; phase untouched) or `level` (spec per
bin, rta per band as a log-frequency power average), or, for `spl`, run as a minimum-phase
filter before frequency weighting — never on `lpeak`, which stays uncorrected.

The unit of `level` must match `meta.scale`. A required array missing, an array listed
twice or one that does not belong to the kind refuses the frame.

### 5.5 Bitmasks

Undefined bits refuse the frame.

`validity` (0 = valid): `THINNED` 1, `OUT_OF_BAND` 2, `SETTLING` 4, `NO_REFERENCE` 8,
`NO_MEASUREMENT` 16, `PROTECTED` 32, `BELOW_FLOOR` 64, `INSUFFICIENT_RESOLUTION` 128,
`ABOVE_NYQUIST` 256.

`protection`: `NO_REFERENCE` 1, `NO_SIGNAL` 2, `CLIP` 4, `WEAK_REFERENCE` 8,
`DISCONTINUITY` 16 (averages restarted after a stream gap), `CHECK_ROUTING` 32 (reference
and measurement identical or near-perfectly correlated at zero lag, or reference silent while
the measurement has signal: the inputs look mis-patched).

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

## 7. Files

### 7.1 Trace text (`trace.import` / `trace.export`)

**ac2 CSV** (`ac2_csv`, what `trace.export` writes): the first line is exactly
`# ac2 trace export v1` (another version is `bad_header`); then `# key: value` lines with
every metadata field (`name`, `kind`, `source`, `time_base`, `delay_ms`,
`delay_nudge_ms`, `polarity`, `offset_db`, `smoothing` (display only, not applied),
`depth`, `cal`, `mic`,
`created_ns`, `note`, `grid` as the JSON `GridDef`); then the header
`freq_hz,mag_db[,phase_deg][,coherence]` and one row per grid column. Values are written in
their shortest exact form and gaps as `nan`, so an export re-imports bit for bit onto the
grid named in its header. Import reads `name`, `kind` and `grid`.

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
<dir>/session.json                  manifest
<dir>/traces/<generation>-<id>.csv  one ac2 CSV per trace
```

`session.json`: `{format: "ac2-session", version: 3, saved_at, measurements:
[{id, config: MeasConfig, running, frozen, delay: {applied, tracking} | null}], traces:
[{meta: TraceMeta, grid: GridDef, file}]}` (JSON, field names as in this document). A save
writes the trace files of a new generation first, then replaces `session.json` atomically
(temporary file + rename), then removes older generations: a reader sees the old session
or the new one, never a mix. `format` and `version` are read first; any other version is
refused (no migration). Trace files hold the unsmoothed columns; each trace's display
smoothing is its `meta.edit.smoothing` (older versions — version 1 transfer captures could
hold smoothed columns, version 2 named smoothing modes `power` / `complex` and had no
spectrum smoothing — are refused). A directory that holds other files is never written into.

## 8. Cross-language fixtures

`fixtures/protocol/` holds Rust-encoded (`rust_*.bin`) and Python-encoded (`py_*.bin`)
messages plus expected values (`expected/*.json`). See `tools/protocol/README.md`.

## 9. Discovery (mDNS)

A daemon in network mode (`ac2d --listen tcp://…`) advertises one DNS-SD service over mDNS
(RFC 6762 / 6763) unless started with `--no-mdns`. Local modes never advertise.

| field | value |
|---|---|
| service type | `_ac2._tcp.local.` |
| instance name | the rig name (`ac2d --name`, default `ac2 on <hostname>`); dots and control characters replaced by `-`, at most 63 bytes |
| port (SRV) | the ctrl (ROUTER) port; the data (XPUB) port is ctrl + 1 |
| addresses | A / AAAA records of the listening interface (all interfaces for `0.0.0.0`) |

TXT record (all values UTF-8 strings):

| key | meaning |
|---|---|
| `txtvers` | layout of this record, `1`. A reader that does not know the value ignores the advert. |
| `name` | the rig name, as above |
| `v` | daemon version (`ac2d --version`) |
| `proto` | `PROTO_VERSION` the daemon speaks (§2) |
| `fp` | fingerprint of the daemon's CURVE server key: the first 10 bytes of SHA-256 over the 32 raw key bytes as five dash-separated groups of four lowercase hex digits (`1a2b-3c4d-5e6f-7a8b-9c0d`) |

The advert carries no key and grants nothing. A client connects only with a server key it
pinned beforehand (`ac2 auth pair`, after comparing the fingerprint with the one the daemon
host shows), and CURVE fails the handshake when the daemon does not hold that key. Clients
may use `fp` to pick which pinned key belongs to an advert and to warn when a known host
advertises a different fingerprint; they must not pin a key based on an advert alone.
