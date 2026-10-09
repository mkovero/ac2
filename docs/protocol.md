# ac2 protocol

Normative description of the wire protocol between `ac2d` and its clients. The Rust types
in `crates/ac2-proto` are the implementation; `crates/ac2-proto/tests/it/doc_parity.rs` fails
when a command, reply body, event kind, error code, frame header field, frame kind, array
name or unit exists in the code but not here. Background: PLAN.md §6,
`docs/design/q2-q5-q6-protocol.md`, `docs/design/spike-zmq-curve.md`.

The protocol is transport-agnostic bytes. Under ZeroMQ, ctrl is DEALER ↔ ROUTER (one
message frame per request or reply) and data is XPUB/SUB (multipart).

## 1. Encoding rules

- Ctrl messages and events are **msgpack maps with named fields**; field order carries no
  meaning. Frame headers are the exception: **msgpack arrays**, positional (§5.3), since a
  frame goes out tens of times a second with a payload often smaller than its field names.
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

`PROTO_VERSION = 33`. Every ctrl message of every version is a map containing `v` (u16) and
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
| `session.detect_loopback` | `lease_token`, `backend`, `input_device`, `output_device`, `output: u16`, `level: Dbfs \| nil` | `loopback_detection` | L |
| `session.open` | `config: SessionConfig` | `session` | |
| `session.close` | — | `ack` | |
| `session.status` | — | `session` | |
| `session.inputs` | `inputs: [InputSetup]` (upserted by channel) | `inputs` (the whole setup) | |
| `session.outputs` | `outputs: [OutputSetup]` (upserted by channel; `label: nil` clears) | `outputs` (every label) | |
| `gen.acquire` | `force: bool` | `lease` (`lease_token`, `expires_in_ms`) | |
| `gen.set` | `lease_token`, `desired: {settings, armed, firing}` | `generator` | L |
| `gen.refresh` | `lease_token` | `lease` | L |
| `gen.release` | `lease_token` | `ack` | L |
| `gen.stop` | — | `ack` | universal |
| `gen.ceiling` | `ceiling: Dbfs`, `confirm_raise: bool` | `generator` | any client |
| `meas.create` | `config: MeasConfig` | `measurement` | |
| `meas.update` | `meas`, `config` | `measurement` | |
| `meas.delete` | `meas`, `traces: keep \| delete` (what becomes of the traces and math channels it owns) | `ack` | |
| `meas.start` | `meas` | `measurement` | |
| `meas.stop` | `meas` | `measurement` | |
| `meas.reset` | `meas` | `ack` | |
| `delay.find` | `meas`, `band: FinderBand`, `observation: Seconds \| nil` | `delay_finding` | |
| `delay.insert` | `meas`, `pick: first_arrival \| strongest \| ranked{index}` | `measurement` | |
| `delay.set` | `meas`, `delay: Seconds` | `measurement` | |
| `delay.nudge` | `meas`, `by: Seconds` | `measurement` | |
| `delay.track` | `meas`, `enabled` | `measurement` | |
| `trace.capture` | `meas`, `name`, `slot` (1…9 \| nil) | `trace` | |
| `trace.list` | — | `traces` | |
| `trace.get` | `trace` | `trace_data` | |
| `trace.update` | `trace`, `edit: TraceEdit` (full replacement) | `trace` | |
| `trace.delete` | `trace` | `ack` | |
| `trace.average` | `traces`, `method`, `reference`, `name` | `trace` | |
| `trace.import` | `file_name`, `format: ac2_csv \| analyzer_text \| auto`, `role: trace \| target`, `content: bin` | `trace` | |
| `trace.export` | `trace`, `format: ac2_csv` | `export` (`file_name`, `content: bin`) | |
| `trace.mic_curve` | `trace`, `curve: MicCurveId \| nil` (nil removes) | `trace` | |
| `cal.spl` | `input`, `mic`, `calibrator_level: DbSpl`, `calibrator_freq: Hz` | `calibration` | |
| `cal.spl_electrical` | `input`, `mic`, `connection: in_line \| injected`, `volts: Volts`, `freq: Hz`, `mic_sensitivity: MvPerPa \| nil`, `uncertainty: Db \| nil`, `replace_acoustic: bool` | `calibration` | |
| `cal.curve_import` | `mic`, `label: string \| nil`, `file_name`, `content: bin`, `input: u16 \| nil` | `mic` | |
| `cal.curve_rename` | `curve: MicCurveId`, `label` | `mic` | |
| `cal.curve_delete` | `curve: MicCurveId` | `ack` | |
| `cal.list` | — | `calibrations` (`calibrations`, `mics`) | |
| `cal.delete` | `key: CalKey` | `ack` | |
| `spl.log_get` | `meas`, `log: SplLogWhich`, `from: u64`, `max: u32` | `spl_log_page` | |
| `spl.log_new` | `meas` | `ack` | |
| `spl.history_get` | `meas`, `seconds: u32` | `spl_history` | |
| `spl.band_transfer` | `meas`, `foh`, `at_place: BandLevelSource`, `background: BandLevelSource \| nil`, `place: str` | `measurement` (as `meas.update`) | |
| `spl.band_log_get` | `meas`, `from`, `until: WallNs`, `step: u32 \| nil` | `spl_band_log` | non-mutating |
| `sweep.run` | `lease_token`, `meas` (a sweep measurement), `name: string \| nil` | `sweep` (the run as started) | L (held for the run), armed |
| `state.snapshot` | — | `snapshot` | |
| `state.since` | `rev` | `events` or `resync_required` | |
| `grid.get` | `grid_id` | `grid` | |
| `file.save` | `session: SessionRef` | `session_file` | |
| `file.load` | `session: SessionRef` | `session_file`; loads disarmed, no owner, new epoch | |
| `file.list` | — | `sessions` | |
| `rec.start` | `request: RecordRequest` | `recording` (the run as started) | |
| `rec.stop` | — | `recording` (the run, finished) | |
| `rec.list` | — | `recordings` | |
| `session.replay` | `recording: RecordingRef`, `pace: realtime \| fast` | `session` | |
| `server.info` | — | `server` | |
| `server.authorize` | `name`, `key` (Z85) | `server` | |
| `server.revoke` | `name` | `server` | |

Rules (Q6): `firing` requires `armed`; arming does not emit. `gen.set` carries the full
desired state and refreshes the lease. Refresh at least every 0.5 s; expiry 1.5 s after
the last refresh fades out (20 ms), disarms and clears the owner. `gen.acquire{force}`
stops and disarms before handing over. Every acquire, force, arm, fire, set, stop, release
and expiry is a `generator` event naming the client (`last_action.client`; for `expiry` the
owner whose lease expired). The output path enforces the deadline itself; if it mutes on an
expired deadline before the control side noticed, the daemon disarms with the same `expiry`
event, so the state never says firing while the output is silent.

**System max level (`gen.ceiling`).** `generator.ceiling` (dBFS RMS) is the system
maximum: `gen.set`, `sweep.run` and `session.detect_loopback` above it are `refused`, and
the output path limits every sample to the matching peak (6 × the RMS, at most full scale)
on the running stream at once. `generator.ceiling_bound` is the hard upper bound fixed at
daemon start (`ac2d --max-level`); `gen.ceiling` above it, above 0 dBFS or not finite is
`invalid`. Any client may change it (no lease):

- **Lowering** applies at once. A stimulus armed or playing at a level above the new
  maximum, and a sweep running at one, is stopped and disarmed (faded out, 20 ms): stopping
  is unambiguous where a quietly lowered level would leave the owner's level and the
  measurement's level disagreeing. One at or below the new maximum carries on untouched.
- **Raising** needs `confirm_raise: true` (`refused` without), and is `refused` while the
  generator is armed or firing, a sweep runs or a loopback detection plays.
- Each change is a `generator` event whose `last_action` is `ceiling_lowered` or
  `ceiling_raised` naming the client, and a line in the daemon's audit log. The daemon keeps
  the value in its rig settings file (atomic write) and starts with it; a value above a
  later, lower `--max-level` comes up at that bound. A change that cannot be written is
  `internal` and leaves the maximum as it was.

**Output labels (`session.outputs`).** `OutputSetup` = {`channel`: u16, `label`: string |
nil}. Labels name the rig's outputs (`Main L`, `Sub`) for every client; they are kept with
the rig settings (not in sessions) by channel number, whatever the device. A label is 1–32
characters, no control characters, no surrounding space (`invalid` otherwise); `nil` clears
it. `state.outputs` lists the labelled outputs, sorted by channel.

**Server (`server.*`).** `server.info` → `ServerInfo`: `mode` (tagged by `type`):
`embedded` (in an app's process) \| `local` {`ctrl`} (this machine only; the OS user is the
trust boundary, there are no client keys) \| `network` {`ctrl`, `data`, `server_key` (Z85),
`fingerprint`, `advertised_as`: string | nil (the mDNS name), `authorized`:
[`AuthorizedClient` {`name`, `key`, `fingerprint`}], `refused`: [`RefusedKey` {`key`:
string | nil (nil: not CURVE), `fingerprint`: string | nil, `address`, `count`, `last_at`:
WallNs}], newest first, at most 32}; `recording_dir`: string | nil (where `rec.start`
records on the daemon host). `server.authorize` adds `key` under `name` to the
authorized-clients file (atomic write) and to the running handshake check; `server.revoke`
removes `name`: its requests are `refused` at once and it cannot connect again (a
connection already up keeps receiving data frames until it drops). Both answer the new
`server` info; outside network mode they are `unsupported`. A name is 1–64 characters
without whitespace; `invalid` for a duplicate name or key, a malformed key, revoking an
unknown name (`not_found`) or the requesting client's own name (`refused`: that would lock
the operator out). Any authorized client may use them: every authorized client is equally
trusted.

The stream carries every output of the session: arming routes the generator to
`settings.outputs` without reopening the stream (same session epoch; running measurements,
and on JACK every port connection, stay). Re-routing while firing fades out, switches and
fades back in. On JACK the daemon connects the outputs the operator chose for the stimulus
(the generator's outputs and the session's loopback output), and only those, to the
physical playback ports of the same number as the device listing names them (output 1 →
the first playback port), removes the connections it made for outputs no longer chosen,
never touches connections made by anyone else, and connects again after any reopen.

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
  frequency (`1/f`); DC and bins narrower than the kernel pass through, and bins without
  power (no finite level) are gaps the kernel never crosses. Smoothing runs on every bin,
  before the bins are gathered into the frame's display columns (§5.4). A smoothed bin is
  no longer the tone level of that bin: frames say so (`SpecMeta.smoothing`) and clients
  label it.
- **SPL** (`SplConfig`): any change on the same `input` — `weighting`, `time_weighting`,
  `peak_weighting`, the Leq windows and peak limits (`leq: LeqConfig`), the measuring-position
  correction (`position`), the name — applies in place; the
  meter, its log and its windows go on (§3.2, SPL log). The meter runs every frequency
  weighting with every time weighting (and the peak with C and Z) all the time and reports
  the configured ones, so the first `spl` frame with the new `config_rev` reads the new
  weighting settled, and its `lmax`, `lmin`, `leq`, `lpeak` and `duration` cover the same
  interval as before the change (since the meter started or `meas.reset`). A new `input`
  restarts the meter.
- **Math channel** (`math`, `MathConfig`: `owner`, `domain`, `expr`, `reference`,
  `smoothing`). Design: `docs/design/math-channels.md`. `owner` (`TraceOwner`, below): the
  measurement it is listed under — front ends give it the one selected when it was made —
  or `imported`; its captures are filed under the same owner. A math channel or a
  measurement that does not exist as owner is `invalid` / `not_found`. A result the daemon computes from operands named
  by id — live measurements and stored traces — and publishes like a measurement of its
  `domain`'s kind: `transfer` (`tf`, combined as complex values), `spectrum` (`spec`, tone
  levels on one bin grid) or `rta` (`rta`, band powers on one band layout). Its `grid_id`
  is the published grid (a transfer result's is known before it runs; a spectrum's is the
  display grid of its operands' FFT, its capture every bin).
  - `expr` (`MathExpr`, tagged by `type`): `binary` {`a`, `op`, `b`} or `average` {`of`:
    2 … 16 distinct operands, `method` `power` \| `complex` \| `coherence_weighted`}. An
    operand (`Operand`, tagged by `type`) is `meas` {`meas`} or `trace` {`trace`}.
  - `op` (`MathOp`): `divide` (A ÷ B: A relative to B), `multiply` (A × B: the cascade),
    `add` (transfer: the complex sum, what A and B sum to acoustically; levels: the power
    sum `10·lg(10^{a/10} + 10^{b/10})`), `subtract` (transfer: the complex difference;
    levels: the level difference in dB). Levels take `add` and `subtract` only, and average
    on `power` only.
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
    kind than the domain, a math channel as an operand, live transfer operands on
    different grids, `a` = `b`, a duplicate in an average (`invalid`). While a math channel
    names a measurement, `meas.delete` of it (unless the channel goes with it: owned by it,
    `traces: delete`) and a `meas.update` that changes its kind,
    grid, FFT length or band layout are `refused`; `trace.delete` of a named trace is
    `refused`; `trace.mic_curve` on it restarts the channel with the corrected columns.
    `meas.reset` of a math channel is `invalid` (reset its operands).

- **Sweep measurement** (`sweep`, `SweepConfig`: `reference_input`, `measurement_input`,
  `outputs` ([u16], the speaker's and the loopback's), `level: Dbfs` (typed, no default),
  `sweep: EssSpec`, `repeats` (1…8), `gate: Seconds | nil`, `tail: Seconds | nil`,
  `lf_harmonics: LfHarmonics`). Settings
  only: it publishes no stream, `running` stays false, and `meas.start` / `meas.stop` /
  `meas.reset` of it are `invalid`. `sweep.run` plays it (below); each run is
  a stored `sweep` trace it owns. `meas.update` changes the settings for the next run.
  `invalid` at create / update: equal inputs, outputs empty or repeated, repeats outside
  1…8, a non-finite or positive level, a gate ≤ 0, a tail above 20 s, sweep parameters the
  generator refuses at the session's rate (checked again at every run, as are the
  session's inputs and outputs and the ceiling).

#### Ownership (`TraceOwner`) and `meas.delete`

Design: `docs/design/measurement-tree.md`. Every stored trace has an owner,
`TraceEdit.owner` (`TraceOwner`, tagged by `type`: `meas` {`meas`} or `imported`), and so
does every math channel (`MathConfig.owner`). A capture is owned by the measurement it came
from, a math capture by the math channel's owner, a sweep run by its sweep measurement, an
average by the owner its inputs share (else `imported`), an import by `imported`. Moving a
trace is a `trace.update` with another `owner` (allowed on a locked trace); moving a math
channel a `meas.update` with another `MathConfig.owner`. An owner must be an existing
measurement other than a math channel (`not_found` / `invalid`).

`meas.delete {meas, traces}` deletes the measurement and, by `traces` (`OwnedTraces`):
`keep` — its traces and math channels stay, their owner becomes `imported` (one `trace` /
`measurement` event each); `delete` — they are deleted with it (locked traces included:
the operator chose to). Refused (`refused`) while a math channel that stays names the
measurement or a trace deleted with it as an operand, and while a run of the measurement
plays or analyses.

#### SPL log and Leq windows (`spl.log_get`, `spl.log_new`, `leq` frames)

Design: `docs/design/leq.md`. `LeqConfig` = {`windows`: [`LeqWindow`] (at most 8, in
display order), `horizon`: Seconds (1 s … 1 h, whole seconds; default 60), `peaks`:
`PeakLimits` {`lcpeak`, `lafmax`: `PeakLimit` \| nil}}. `LeqWindow` =
{`duration`: Seconds (1 s … 24 h, whole seconds), `weighting`: `a` \| `c` \| `z`, `limit`:
DbSpl \| nil, `warn_margin`: Db ≥ 0 (default 3)}. `PeakLimit` = {`limit`: DbSpl, `warn_margin`:
Db ≥ 0}. `SplConfig.position`: `PositionCorrection` \| nil = {`level`: Db, `peak`: Db, each
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
configuration order), `scale`: `LevelScale`, `at`: [WallNs], `leq`: [[f32]], `over`:
[[bool]]}: each window second by second over the newest `seconds` (at most 14400, 4 h) of
the meter's current log, as its `leq` frames carried them — `at` the end of each second
(oldest first; a second without a row has no entry, as no frame was sent for it), `leq[w][k]`
window `w`'s Leq at `at[k]` in `scale` (NaN when nothing was measured in it), `over[w][k]`
whether it was over its limit then. The daemon replays the log as the meter's job computed
it (`docs/design/leq.md`, *The history strip*): a second lost while running is a gap in the
windows, a stretch without rows a restart whose windows were refilled from the rows in their
span; each second judged with the window's limit, the row's sensitivity and its position
correction, a filling window on its budget, with the hysteresis above (begun part way
through a log, a state held from before the first row read can be released up to 10 s
early). `leq` includes the row's position correction. Only seconds after the last change of
unit (a calibration, or a change of the position correction) are
returned; `scale` is theirs. The windows are the meter's current ones, also for seconds
logged before they were set. Empty `at` for an empty log. `invalid` for a measurement that
is not an SPL meter. A client that was not connected (an app restarted) draws the history
from it and continues with the frames.

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
22:00–07:00 local time, and 07:00–22:00 it is `limit` plus this), `warn_margin`: Db ≥ 0}; the
limit is at the transfer's place with a transfer, else at the mic. A range of bands is a
front-end convenience that adds one window per band; nothing range-shaped is on the wire. `PredictedWindow` = {`duration`:
Seconds, `day`, `night`: DbSpl \| nil, `warn_margin`: Db} (the A-weighted level predicted at
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
`night_day` window is judged by the night limits while it holds a night second, else by the
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
started on first: `kind: BackendKind` (`jack` \| `cpal` \| `fake`; `replay` is the kind of a
replay session and is never listed), `description` (for the
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
nothing), `system_default` (bool: the host's default device for this direction). A host
whose devices are single-direction endpoints (WASAPI) lists an interface as an input-only
and an output-only device, each with `duplex_clock: unknown`.

`SessionConfig`: `backend: BackendKind | nil` (nil = the default backend), `input_device`,
`output_device` (`DeviceSelector`: `default` \| `id` {`id`}), `input_channels` ([u16],
zero-based), `output_channels` (u16, a count), `sample_rate_hz`, `buffer_frames`,
`loopback` ({`output`, `input`} \| nil). `OpenSession` adds `backend` (the one used),
`input_device`, `output_device`, `sample_rate_hz`, `buffer_frames`, `clock`, `opened_at`,
`replay` (`ReplayInfo` \| nil: the recording a replay session plays, §3.2 raw capture
files). `session.open` with `backend: replay` is `invalid`: recordings open with
`session.replay`. Input and output may be two devices of one backend; an output device
without outputs (or an `output_device: default` the host has none for) is `not_found`
when `output_channels > 0`. Two devices open with `clock: unknown`: they may drift, which
the loopback monitor measures while a stimulus plays (`timing.drift`, §4.1).

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
The daemon closes the preview, opens `input_device` with every input and `output_device`
with `output + 1` outputs (the same id for one device),
plays a 0.5 s pink-noise burst band-limited to 100 Hz – 10 kHz at `level` (RMS) on `output`
only — faded in and out (20 ms), under the global ceiling and the output path's peak limit —
then closes the stream. Each input's capture is cross-correlated with the burst as emitted
(over delays 0 … 0.5 s). Reply `LoopbackDetection`: `backend`, `input_device`, `output_device`, `output`, `level`,
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
value) clears `last_finding`; a delay tracking moves keeps it. `delay.nudge` moves the
applied delay by `by` (either sign, fractions of a sample allowed) and keeps
`last_finding` (it refines that delay); like `delay.insert` and `delay.set` it resolves
`awaiting_pick`. A measurement has one delay, `applied`; the daemon keeps its offset from
the arrival apart as `nudged`: the applied delay is the *arrival* plus `nudged`. `delay.insert` sets a new arrival (`nudged` 0); `delay.set`
keeps the arrival and sets `nudged` to the value's distance from it, and `delay.nudge` adds
its step to `nudged` (both refused beyond ±10 s from the arrival); tracking compares the
finder's estimates with the arrival and moves it, keeping `nudged`. A view's shared time
base refers the live curve to the arrival, so a step moves that curve alone, like a
trace's `delay_nudge` (`docs/design/delay-no-resettle.md`, "What the keys mean"); the app's
plain `,` / `.` (0.1 ms) and Ctrl / Alt (a sample, a tenth) all send `delay.nudge`, and its
front ends say the delay with its offset: `delay 12.60 ms (+0.10 ms from arrival)`. A
capture records the applied delay as `TraceMeta.delay` and `nudged` as its
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
`coherence` (nil = unknown), `sweep` (`SweepData` of a sweep trace, else nil), column order =
the trace's grid, NaN = no value), `trace.export` and `file.save`. Columns are stored as
measured: offset, polarity, nudge, smoothing and a mic curve applied after capture are
display edits and are never applied to the stored data.

- **Smoothing.** `TraceEdit.smoothing` (`Smoothing` \| nil; transfer and spectrum traces
  only — any other kind is `invalid`) is applied by the daemon when it serves `trace.get`,
  with the live job's kernel (a spectrum has no phase: its power is smoothed in either
  `mode`; a spectrum capture starts with `magnitude`); coherence is never smoothed.
  `trace.export` and `file.save` write the unsmoothed columns (the setting is listed in the
  CSV header), and `trace.average` and math channels combine unsmoothed columns. A capture
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
  names the curve in its `# mic:` line; `trace.average` and math channels combine corrected
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
  It needs a result in the current session epoch (`invalid` otherwise: not running, no
  frame yet, SPL measurement). Metadata: `kind` (`TraceKind`, tagged by `type`: `transfer`,
  `target`, `spectrum` {`scale`}, `rta` {`scale`}), `source.captured` {`meas`, `meas_name`,
  `epoch`, `at_sample`}, `edit.owner` the measurement, `delay` (the delay the DSP used),
  `depth` (transfer),
  `cal` (spectrum / RTA: the calibration the measurement used, picked by the calibration
  matching rules below — its `key` names another mic or input when it was not this mic's;
  transfer functions are ratios and always `uncalibrated`), `mic` (the input setup's mic
  name and the mic curve applied to the captured columns as its full `MicCurveRef` — label,
  file, content hash —, nil without a mic name; a sweep
  names no curve, its analysis works on the raw recordings), `mic_curve` (nil: see
  *Mic curve after capture*), `created_at`. A math channel captures as its domain's kind
  (`transfer`, `spectrum` {`scale`}, `rta` {`scale`}) with `source.math` {`meas`,
  `meas_name`, `epoch`, `at_sample`, `expr`, `operands`: [`NamedOperand` {`operand`,
  `name`}] — the operands that went into the captured result, named as at capture —,
  `phase`: `PhaseBasis`}, `delay` its stated delay, `depth` and `mic` nil,
  `uncalibrated`; `refused` when the expression lacks its operands at that moment.
- **Slots.** `TraceEdit.slot` (1…9 or nil). A slot holds at most one trace: capturing or
  updating into a slot clears it on the trace that held it (a `trace` event for that one
  too).
- **Lock.** `trace.update` on a locked trace may change only `visible`, `order`, `slot` and
  `locked` (not `smoothing`); `trace.delete` and `trace.mic_curve` of a locked trace are
  `refused`.
- **Time base (decisions 8a / 8b).** Captured traces (`captured`, `sweep`) share the
  time base of their session epoch, and so does a `math` capture whose `phase` is
  `shared_time_base` and whose expression is a sum, difference or average (a ratio or a
  cascade is relative, in no time base); every other source (`imported`, `average`, other
  `math`) is independent.
- `trace.average`: ≥ 2 distinct traces of one kind (no targets). Transfer: `power` (RMS
  magnitude, phase of the complex mean), `complex`, `coherence_weighted` (weight
  γ²/(1 − γ²), γ² capped at 0.999); every input's phase is re-referred to `reference`
  (`DelayReference`: `trace` {`trace`} = that input's measured delay, or `fixed` {`delay`})
  before combining, and the result's `delay` is that reference. Phase methods need every
  input captured in the same epoch (`invalid` otherwise); a power average without a shared
  time base, or with an input without phase, keeps the magnitude only. Spectrum / RTA:
  `power` only, all on one grid. A column is valid only where every input is. The result is
  on the first trace's grid (others resampled).
- `trace.import`: the file text is parsed by `format` (§7.1) and, unless it is an ac2 CSV
  on a known grid, resampled onto a log grid (48 points per octave, 96 when the file is
  denser) — magnitude and coherence linear over log frequency, phase unwrapped first.
  `role: target` keeps the magnitude only and makes `kind: target`. The name is the ac2
  header's `name`, else the file name without extension; `delay` is the ac2 header's
  `delay_ms` (0 for other files). An ac2 sweep export with its analysis facts and impulse
  response imports as a `sweep` trace. `source.imported` {`file_name`, `format`, `notes`}:
  `notes` lists what the file held that the trace does not keep (`ImportNote`:
  `sweep_without_analysis` — a v1 sweep export, imported as its transfer function with the
  distortion dropped; `sweep_off_grid` — distortion is never resampled;
  `mic_curve_not_applied` — the export named a mic curve applied after capture, the
  columns are without it). A refused file is `invalid` with
  `detail: {type: import, line (1-based) | nil, problem: ImportProblem}`.
  `ImportProblem`: `not_text`, `no_data`, `bad_number`, `column_count`, `too_few_columns`,
  `not_ascending`, `out_of_range`, `too_many_rows` (> 65536), `bad_header`,
  `bad_coherence`.

#### Sweep runs (`sweep.run`)

Design: `docs/design/sweep-distortion.md`. `sweep.run {lease_token, meas, name}` runs the
sweep measurement `meas` (`MeasKind::Sweep`, `SweepConfig` above) with its settings:
`sweep: EssSpec` {`start: Hz`, `end: Hz`, `duration`, `fade_in`, `fade_out`}, `tail`
(silence recorded after each sweep: the room's decay and its noise, at most 20 s; nil or
shorter = the analysis minimum, ≥ 1 s), `lf_harmonics` (`LfHarmonics`, the harmonic windows
at the lowest columns, harmonics below about 1 kHz: `standard` puts every order in one shared
window, short enough for H5 — the lowest floor; `fine` puts each order in the longest window
between its neighbours' impulses, with the fundamental and the floor in that window — finer
low-frequency resolution and lower columns (a 5.5 s sweep from 10 Hz: H2 from 10 Hz where
the shared window stops near 20 Hz), a floor
higher by the window's length over the shared one, and a `post_roll` of at least four of the
longest window; it changes the recording, so it is a setting of the measurement). The emitted sweep keeps the rate but starts up to two
octaves below `start` (a whole number of cycles per rate constant, at least 1 Hz) and fades
in up to `start` in place of `fade_in`, so a path's switch-on transient lies below the
analysed band; responses are reported from `start`. `name` nil names the trace `Run <number>`.

- Refused (`refused`) above the ceiling, or while the generator is not armed by the caller
  (arm with `gen.set` first: like firing, the run needs it), while it fires, while a
  loopback detection or another sweep runs, or without an open session. `invalid`: not a
  sweep measurement, inputs not captured, outputs not in the session, sweep parameters the
  generator refuses at the session's rate.
- The daemon routes the generator to `outputs`, plays `repeats` synchronised sweeps each
  followed by its silence (`post_roll`, ≥ 1 s and ≥ `tail`), records both inputs, analyses on a job thread
  and stores a trace of `kind: sweep` owned by the measurement (source `sweep` {`meas`,
  `meas_name`, `run`, `number` — one more than the highest run number of the measurement's
  stored runs —, `epoch`, `sweep`, `level`, `repeats`, `lf_harmonics`, `reference_input`,
  `measurement_input`}, `delay` = the arrival). While it plays
  the generator is `firing` with the sweep as its settings; once the recording is in it is
  disarmed (`last_action` `stop` by the daemon; the lease stays with its holder), so the next
  sweep or stimulus needs an explicit arm.
- Progress and outcome are the `sweep` entity (§4.1), `SweepRun`: `id`, `meas`, `owner`,
  `name`,
  `reference_input`, `measurement_input`, `outputs`, `level`, `sweep`, `sweep_duration`
  (actual, of each emitted sweep), `post_roll`, `repeats`, `gate`, `lf_harmonics`, `started_at`, `status` (`SweepStatus`, tagged by
  `type`): `playing` {`repeat`, 1-based} → `analysing` → `done` {`trace`} or `failed`
  {`reason`, `msg`}. `SweepFailure`: `stopped` (`gen.stop`, `gen.release`, forced takeover),
  `lease_expired`, `session_closed`, `dropout` (audio lost while recording), `no_reference`
  (the reference carries no sweep), `analysis`. A failed run's audio is discarded.
- `trace.get` of a sweep trace: `mag_db` / `phase_deg` are the fundamental's response (mic re
  reference, phase referred to the arrival; NaN outside the sweep), `coherence` nil, and
  `sweep` (`SweepData`): `harmonics` ([`HarmonicCurve` {`order`, `curve`}], H2…H5), `thd`,
  each `DistortionCurve` {`level_db`, `floor_db`} in dB re the fundamental per grid column
  (harmonic k at k·f is reported at the fundamental f; NaN where that order is not measured);
  `ir` (`SweepIr` {`t0`, `dt` re the arrival, `linear`, `etc_db`}, from H5's window to the
  end of the linear window, at most 16384 points, peak-preserving); `info` (`SweepInfo`:
  `sample_rate`, `rate` (L: harmonic k's impulse at −L·ln k), `duration`, `repeats`,
  `arrival`, `reference_level` (loopback gain), `window_pre`, `window_post`, `gate_pre`,
  `gate`, `floor_margin`, `clipped`); `room` (`RoomAcoustics | nil`, below; nil only for a
  sweep imported from an export written without it). A distortion point is valid when
  `level_db ≥ floor_db + floor_margin`; otherwise it reads "< floor".
- `RoomAcoustics` (ISO 3382-1 room parameters of the full-rate impulse response, design
  `docs/design/room-metrics.md`): `broadband` (`RoomBand` of the IR as captured), `octave`
  ([`RoomBand`], 63 Hz…8 kHz) and `third` ([`RoomBand`], 50 Hz…10 kHz), each only bands
  whose edges lie inside the sweep's range; `span_end` (`Seconds`, end of the IR analysed re
  the arrival: the end of the silence after the sweep). `RoomBand`: `centre: Hz | nil` (nil =
  broadband), `onset` and `truncation` (`Seconds` re the arrival: the band's trigger and where
  its decay meets the noise), `decay_range` (`Db`: depth of the decay curve there), `edt`,
  `t20`, `t30` (seconds), `c50`, `c80` (dB), `d50` (ratio 0…1), each a `RoomValue` (tagged by
  `type`: `value` {`value`} or `refused` {`reason`}), and `curvature` (`f64 | nil`, percent
  100·(T30/T20 − 1) when both are given). `RoomRefusal` (tagged by `type`): `no_decay`,
  `insufficient_range` {`range`, `needed`} (`Db`; EDT, C50, C80, D50 need 20 dB, T20 35 dB,
  T30 45 dB), `filter_limited` {`bandwidth_decay`} (bandwidth × decay time below 8). A refused
  value is never sent as a number.
- Smoothing, averages and math channels treat a sweep trace as a transfer function (magnitude and
  phase; the distortion stays with the sweep trace).

#### Sessions (`file.*`)

`SessionRef` (tagged by `type`): `name` {`name`} — a directory in the daemon's session
directory (letters, digits, space, `-`, `_`, `.`; not starting with `.`) — or `path`
{`path`} — an absolute directory on the daemon host, accepted from local transports only
(`refused` in network mode). `SessionFile`: `name`, `path`, `saved_at`, `measurements`,
`traces`.

- `file.save` writes measurement configurations (applied delay, tracking,
  running) and every trace with metadata, edits, slots and columns (format §7.2). Never
  generator state; calibrations belong to the calibration store.
- `file.load` checks the whole session first (a refusal changes nothing), then stops and
  disarms the generator and drops its owner (the old lease is gone), deletes every
  measurement and trace, starts a new session epoch newer than every epoch recorded in the
  loaded traces (an open stream reopens without generator routes), and recreates the
  measurements (same ids, restarted when they were running) and traces (same ids). Errors:
  `not_found`; `invalid` (not a session, bad name, damaged files); `unsupported` with
  `detail: {type: session_version, found, supported}` for another format version.
- `file.list`: the sessions in the session directory, by name.
- The daemon also keeps an autosave of the same content (§7.3); its status is the
  `autosave` entity.

#### Raw capture files (`rec.*`, `session.replay`)

Design: `docs/design/raw-capture.md`; file format §7.5. A daemon has a recording directory
(`ac2d`: `recordings` in the data directory, `--recordings <dir>`); one without (an
embedded or test daemon not given one) answers `rec.start` `unsupported`, `rec.list` with
nothing and `session.replay` by name `not_found`.

`RecordRequest`: `inputs` ([u16] device inputs, each captured by the open session, in file
channel order), `name` (string \| nil: a file stem like a session name; nil =
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
  `frames`, `bytes` (file size), `discontinuities`, `max_duration`, `max_bytes`, `status`
  (tagged by `type`: `recording` \| `ended` {`reason`: `RecordingEnd`}). `frames`, `bytes`
  and `discontinuities` are updated about once a second while recording. `RecordingEnd`
  (tagged by `type`): `stopped`, `duration_limit`, `size_limit`, `write_failed` {`msg`},
  `session_closed`, `session_reopened`, `audio_stopped` (the session's audio stopped, §4.1.1;
  the file ends at the last audio received), `daemon_shutdown`, `interrupted`.
- `rec.list` → [`RecordingFile`]: `name`, `path`, `sample_rate_hz`, `inputs`, `frames`,
  `started_at`, `discontinuities`, `end` (`RecordingEnd` \| nil while being written),
  oldest first.
- Every committed change of a measurement, the generator, the input setup or a calibration
  while recording goes into the sidecar's timeline at the newest captured sample. Every
  block that does not follow the previous one (xrun, device gap, capture overflow,
  configuration change, or the recorder falling behind the fan-out by more than 10 s of
  audio) goes into its discontinuity list with the samples lost; the file holds only
  captured frames, never filler.

`RecordingRef` (tagged by `type`): `name` {`name`} in the recording directory, or `path`
{`path`}: the absolute path of the `.wav` or `.ac2rec.json` file, local transports only
(`refused` in network mode). `session.replay` closes any session and opens a new epoch on a
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
  state none or disagree). `method` is `electrical` {`connection`: `in_line` (pins 2–3 with
  the mic connected and powered) | `injected` (a generator in place of the mic), `volts`,
  `full_scale`: Volts (V_FS), `mic_sensitivity`: MvPerPa, `mic_sensitivity_from:
  SensitivitySource` (tagged by `type`: `typed` | `data_sheet` {`label`, `file_name`}),
  `uncertainty`: Db (± dB; 1 when nil; 0.05–6 accepted)}. Refused like `cal.spl`, and
  also above −3 dBFS or below −70 dBFS (a poor reading), for `volts` outside 0.1 mV–100 V,
  a mic sensitivity outside 0.1–1000 mV/Pa, or a `freq` outside 20 Hz–20 kHz (any `freq`
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
- `cal.delete` removes the sensitivity calibration `key` (any device; no open session
  needed); `not_found` when there is none. The input setup is not changed.
- `InputSetup` = {`channel`, `mic`: string | nil, `curve: CurveChoice`}; `CurveChoice` is
  tagged by `type`: `not_chosen` | `off` | `curve` {`label`}. `session.inputs` refuses
  (`invalid`) a row whose chosen label is not stored for its mic, unless the row is
  unchanged. The daemon chooses a mic's only curve on a row where none is chosen (on every
  change of the setup or the library); with several, the row stays `not_chosen` and no
  curve applies until the operator chooses. A new mic name on a row starts `not_chosen`.
  Mic names are 1–64 characters.
- When the daemon's calibration store file cannot be read it is never written: `cal.spl`,
  `cal.spl_electrical`,
  `cal.curve_*`, `cal.delete`, `cal.list` and `session.inputs` are `refused` with `detail:
  {type: cal_store, path, reason}`.

Which calibration a measurement uses (shown as `CalStatus` in `spl`, `rta` and `spec`
frames): the entry of device + input + the input's mic → `verified`; else the newest one
on the same device + input (another mic, or no mic name set), else the newest one for the
same mic elsewhere → `other_mic_or_input`; else `uncalibrated` (dBFS). `CalStatus` is
tagged by `type`: `uncalibrated` | `verified` {`calibrated_at`, `basis`} |
`other_mic_or_input` {`calibrated_at`, `basis`}; the age is `capture_wall_ns −
calibrated_at`. `basis: CalBasis` (tagged by `type`) says what the calibration rests on:
`acoustic` {`calibrator_level`} | `electrical` {`connection`, `mic_sensitivity`,
`data_sheet`: bool, `uncertainty`}. The mic curve is the input's chosen curve of its mic —
only that one; none when not chosen, off, or not stored — normalised to 0 dB at the
calibration's frequency in use (the calibrator's; 1 kHz for an electrical calibration and
uncalibrated). The rules are
the pure functions of `ac2_proto::cal`, which clients use to word what is in use.

#### Averaging depth

`TransferConfig.depth: DepthPolicy` (tagged by `type`): `equal_confidence` (every MTW stage
reaches the same effective-average count) or `fast_lf` {`max_settle_s`: Seconds > 0} (no
decimated stage averages over a longer span; those stages show a higher coherence floor).

### 3.3 Reply bodies

`{type, value}` with `type` one of: `ack` (`{rev}`), `welcome`, `backends`, `preview`,
`loopback_detection`, `session`,
`lease`, `generator`, `measurement`, `delay_finding`, `trace`, `traces`, `trace_data`,
`export`, `calibration`, `calibrations`, `mic`, `inputs`, `outputs`, `server`,
`spl_log_page`, `spl_history`, `spl_band_log`,
`snapshot`, `events`,
`grid`, `session_file`, `sessions`, `sweep`, `recording`, `recordings`.

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

The mirrored `State` holds: `session` (`epoch`, `open: OpenSession | nil`, `stopped:
AudioStopped | nil`; §4.1.1),
`measurements` (`id`, `config`, `config_rev`, `running`, `delay`, `grid_id`),
`traces` (`TraceMeta`: `id`, `edit` {name, color, visible, locked, order, offset,
polarity, delay_nudge, slot, smoothing, owner}, `kind`, `source` {captured | imported |
math | average | sweep}, `grid_id`, `delay`, `depth`, `cal`, `mic`, `mic_curve`, `created_at`;
`kind` one of
`transfer`, `target`, `spectrum`, `rta`, `sweep`), `generator` (`owner`,
`armed`, `firing`, `settings`, `ceiling`, `ceiling_bound`, `last_action` {`action`: acquire
\| force \| arm \| fire \| set \| stop \| release \| expiry \| ceiling_lowered \|
ceiling_raised, `client`, `at`}), `calibrations` (`CalEntry`:
`key` {device, channel, mic}, `spl`: SplCal), `mics` (`Mic`: `name`, `curves`
[MicCurveRef]), `inputs` ([InputSetup], sorted by channel), `outputs` ([OutputSetup], the
labelled outputs, sorted by channel), `spl_logs` (`SplLog` per SPL
meter: `meas`, `started_at`, `windows`, `peaks`, `alarms`; §3.2), `timing` (`TimingStatus`: `epoch`,
`state` {no_stimulus | acquiring | locked{offset} | jumped{from, to} | lost}, `last_lock`,
`drift` (`Drift` | nil: `ppm` output-vs-input clock drift from the loopback offset's slope,
`span` s regressed, `warning` true when output and input are on different clocks, `at`
WallNs of the newest window; kept after the stimulus stops and for the rest of the session;
committed when the warning flips, the span first reaches the judged length or the shown value changes: 1 ppm, 0.1 ppm below 10 ppm while warning), `internal_reference`), `sweep` (`SweepRun` | nil: the latest `sweep.run` run),
`autosave` (`Autosave`: `state` {off | saved | pending | failed{reason}}, `saved_at: WallNs |
nil`; see §7.3), `recording` (`RecordingRun` | nil: the latest recording, §3.2).


#### 4.1.1 Audio stopped and recovery

While a session is open the daemon watches its audio (`docs/design/audio-recovery.md`).
`session.stopped` is set when the stream stops delivering and stays set until the same
configuration is open again or a client closes the session; `session.open` keeps the
session as last opened throughout. `AudioStopped`: `since` (WallNs, the last audio
received, or the open when none came), `cause` (tagged by `type`): `not_delivering`
{`after_ms`: u32, the silence that counts as stopped: max(1 s, 20 periods)} \| `host_ended`
(the audio host ended the stream: server shut down, device removed) \| `device_changed`
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

### 4.2 Snapshot and events

`state.snapshot` → `{state, rev, daemon_incarnation, session_epoch}`.

An event is `{rev, kind, payload}`. `kind` is one of `session`, `measurement`, `trace`,
`generator`, `calibration`, `mic`, `inputs`, `outputs`, `spl_log`, `timing`, `sweep`, `autosave`, `recording`. `payload` is the entity's
full new value (`inputs`, `outputs`: the whole list); for keyed entities (`measurement`, `trace`,
`calibration`, `mic` (keyed by name), `spl_log`) it is `{type: "set", value: <entity>}` or `{type: "deleted", value: <key>}`.
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
| `d/<meas>/leq` | rolling Leq windows of an SPL meter, once a second |
| `d/<meas>/band_leq` | an SPL meter's band meter (1/3-octave band Leq per band window), once a second |
| `d/<meas>/levels` | input meters of the measurement's channels |
| `session/levels` | input meters of every input of the open session |
| `session/preview` | input meters of every input of the previewed device |
| `timing` | loopback timing monitor |
| `evt` | state events (§4.2) |
| `ka` | keepalive, every 250 ms |

A measurement topic is formed and sent only while someone subscribes to it. `tf`, `ir`,
`spec` and `rta` frames come when the result changes (at most at the daemon's publish rate);
`spl` at most 20 times a second (Lmax, Lmin, Lpeak and Leq cover the meter's interval, so
none is lost between frames). An unchanged result (settled, gated, a long hop) is re-sent with a
fresh header every 250 ms, so only a stream without audio goes STALE.

`<meas>` is the decimal measurement id without sign or leading zeros; a topic has exactly
one spelling. Prefixes: `d/` (all measurement streams), `d/<meas>/` (one measurement —
the trailing slash keeps `d/1/` from matching `d/12/…`), `session/` (both input-meter
topics). Longest topic: 32 bytes.

### 5.2 Layout

```
part 0   topic (UTF-8)
part 1   header (msgpack array, ≤ 8192 bytes)
part 2…  arrays: each exactly n × 4 bytes, little-endian; f32 or u32 per header
```

Columns are in grid order. Invalid values are NaN; where the reason matters a `validity`
bitmask array says why.

### 5.3 Header fields

The header is an array of these fields in this order; `v` comes first so a frame of another
version is recognised as such before the rest is read. Every struct inside it (array
descriptors, the per-kind metadata of §5.4 and the types they hold) is likewise an array of
its fields in the order listed; an enum with data is an array of its `type` name followed by
its fields (`["verified", calibrated_at, ["acoustic", calibrator_level]]`); the `meta` key
stays a one-entry map `{kind: [...]}`; enums without data stay strings. Nested orders:
`Smoothing` [fraction, mode]; `CalStatus` `uncalibrated` \| `verified` /
`other_mic_or_input` [calibrated_at, basis]; `CalBasis` `acoustic` [calibrator_level] \|
`electrical` [connection, mic_sensitivity, data_sheet, uncertainty]; `LeqRun` [started_at,
until, measured, gaps, trimmed, laeq, lceq, lzeq]; `LeqPeak` [level, judgement];
`PositionCorrection` [level, peak]; `TimingStatus` [epoch, state, last_lock,
drift, internal_reference]; `TimingState` `no_stimulus` \| `acquiring` \| `locked`
[offset] \| `jumped` [from, to] \| `lost`; `LastLock` [epoch, offset, at_sample, at];
`Drift` [ppm, span, warning, at]. `tools/protocol/ac2proto.py` (`HEADER`, `META`) is the same
layout as code.

| field | type | meaning |
|---|---|---|
| `v` | u16 | protocol version |
| `kind` | FrameKind | `tf`, `ir`, `rta`, `spec`, `spl`, `leq`, `band_leq`, `levels`, `session_levels`, `preview_levels`, `timing`, `ka`; equals the topic and the `meta` key |
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
| `arrays` | [[`name`, `unit`, `elem`]] | one descriptor per array part, in part order |
| `meta` | {kind: […]} | per-kind metadata (§5.4), fields in the order listed there |

`elem` is `f32` or `u32`. `unit` is one of `db`, `dbfs`, `db_spl`, `deg`, `coherence`
(γ², 0…1), `full_scale` (linear, full scale = 1), `seconds`, `bitmask` (always
`u32`).

### 5.4 Kinds

| kind | arrays (name: unit) | meta |
|---|---|---|
| `tf` | `mag`: db, `phase`: deg, `coh`: coherence, `validity`: bitmask | `delay`, `nudged` (Seconds: the part of `delay` `delay.nudge` steps added to the arrival; 0 for a math channel), `smoothing`, `mic_curve`, `math` (`MathState` \| nil: a math channel's operands, below) |
| `ir` | `ir_linear`: full_scale, `ir_etc`: db (optional) | `sample_rate`, `t0`, `dt`, `inserted_delay`; point i at `t0 + i·dt` |
| `rta` | `level`: dbfs or db_spl (band power), `validity`: bitmask | `fraction`, `weighting`, `scale`, `cal`, `mic_curve`, `math` (`MathState` \| nil) |
| `spec` | `level`: dbfs or db_spl (tone level; smoothed when `smoothing` is set; NaN for no power) on a `log_bins` grid: each column the highest level among its bins | `window`, `scale`, `cal`, `mic_curve`, `smoothing`, `math` (`MathState` \| nil) |
| `spl` | none (n = 0) | `scale`, `weighting`, `time_weighting`, `peak_weighting`, `level`, `lmax`, `lmin`, `leq`, `lpeak`, `duration`, `cal`, `mic_curve`, `position` (`PositionCorrection` \| nil: included in the levels) |
| `leq` | one column per window of the meter's configuration (`config_rev`), in its order: `leq`: dbfs or db_spl, `elapsed`: seconds, `measured`: seconds, `allowed`: dbfs or db_spl (headroom; NaN without a judged limit or when it cannot recover), `recover`: seconds (to recover at the limit; NaN unless it cannot within the horizon), `least`: dbfs or db_spl (the Leq the window ends at if the rest is silent; the Leq once full), `over_in`: seconds (until a window `ON_COURSE` spends its budget; else NaN), `leq_flags`: bitmask | `scale`, `cal`, `mic_curve`, `horizon`, `logged` (rows logged so far), `run` (`LeqRun` \| nil, §3.2 SPL log), `lcpeak`, `lafmax` (`LeqPeak` \| nil), `position` (`PositionCorrection` \| nil: included in every level) |
| `band_leq` | one column per band of each band window, window-major: `leq`: dbfs or db_spl, `limit`: dbfs or db_spl, `allowed`: dbfs or db_spl, `recover`: seconds, `leq_flags`: bitmask | `scale`, `cal`, `mic_curve`, `horizon`, `correction`, `limits_from`, `windows` ([`BandWindowState`]), `predicted` (`PredictedLeq` \| nil); §3.2 *Band meter* |
| `levels` | `peak`: dbfs, `rms`: dbfs, `clip`: bitmask | `channels` (device input per column; length n) |
| `session_levels` | `peak`: dbfs, `rms`: dbfs, `clip`: bitmask | `channels` (device input per column; length n) |
| `preview_levels` | `peak`: dbfs, `rms`: dbfs, `clip`: bitmask | `backend`, `device`, `channels` (device input per column; length n) |
| `timing` | none | `status` (TimingStatus), `window` {capture_start, offset, psr, loopback, stimulus} |
| `ka` | none | `rev`, `daemon_wall_ns`, `timing` (TimingState), `generator` {owner, armed, firing} |

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
`NO_SIGNAL`}, `mismatch` (does not combine with the others: another level scale, grid or
time base). A math channel's own `protection` is always 0: its operands' faults are in their
statuses. A spectrum math result's live frame gathers its bins into display columns like a
spectrum's, each the highest value among its bins (a level difference: the largest in the
column); its capture keeps every bin.

The unit of `level` must match `meta.scale`. A required array missing, an array listed
twice or one that does not belong to the kind refuses the frame.

### 5.5 Bitmasks

Undefined bits refuse the frame.

`validity` (0 = valid): `THINNED` 1, `OUT_OF_BAND` 2, `SETTLING` 4, `NO_REFERENCE` 8,
`NO_MEASUREMENT` 16, `PROTECTED` 32, `BELOW_FLOOR` 64, `INSUFFICIENT_RESOLUTION` 128,
`ABOVE_NYQUIST` 256, `FEW_OPERANDS` 512 (a math channel without the usable operands its
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

In order: 2 … 10 parts; total ≤ 2 MiB; topic valid; header ≤ 8192 bytes; then after the
bounded header parse: `v`, `n` ≤ 65536, array count = parts − 2, every array part exactly
n × 4 bytes, `kind` = topic = `meta` key, then the per-kind array schema and bitmask bits.
A malformed frame is dropped and counted by the client; decoders never panic.

### 5.7 Sizes

A 480-column `tf` frame: topic 6 B, header ≈ 150 B, four arrays (mag, phase, coh,
validity) of 1920 B — ≈ 7.8 KB. At 60 fps ≈ 0.47 MB/s per
measurement locally, half that remote at 30 fps. A default `spec` frame (65 536 points at
48 kHz: 897 `log_bins` columns of one f32) is ≈ 3.7 KB; it goes out with each new spectrum
(every hop: `n/8`, ≈ 6 per second at 65 536 points; ≈ 30 per second for short FFTs) and is
repeated every 0.25 s of audio while nothing changes, ≈ 22 KB/s.

## 6. Grids

`grid.get(grid_id)` returns a `GridDef`, valid for the incarnation:

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
u32, `ppo` u32. Example: log 48/−240/239 (480
columns) = `0x79ec3d16ae0e94d0`.

## 7. Files

### 7.1 Trace text (`trace.import` / `trace.export`)

**ac2 CSV** (`ac2_csv`, what `trace.export` writes): the first line is exactly
`# ac2 trace export v3` (`v2` and `v1` are read too; another version is `bad_header`); then
`# key: value` lines with every metadata field (`name`, `kind`, `source`, `time_base`,
`delay_ms`, `delay_nudge_ms`, `polarity`, `offset_db`, `smoothing` (display only, not
applied), `depth`, `cal`, `mic` (`name (curve: <label>, …; file …, hash …)`: in the
columns for a capture with a curve, or applied after capture as a display edit, not in the
columns), `mic_curve` (the
JSON `TraceMicCurve`, only when one is applied after capture), `created_ns`, `note`, `grid`
as the JSON `GridDef`); a sweep trace adds `sweep_info` (the JSON `SweepInfo`),
`sweep_ir` (JSON `{t0, dt, points}`) and `room_metrics` (the JSON `RoomAcoustics`). Then the
header
`freq_hz,mag_db[,phase_deg][,coherence]` and one row per grid column; a sweep trace
(`kind: sweep`) adds `h2_db,h2_floor_db,…,h5_db,h5_floor_db,thd_db,thd_floor_db` after
`phase_deg` (dB re the fundamental at the row's fundamental frequency), and after the
frequency rows its impulse response: the header `t_s,linear,etc_db` and `points` rows
(`t_s` = `t0 + i·dt`, for reading), then the room parameters as comment lines for reading
(`# band_hz,edt_s,t20_s,t30_s,c50_db,c80_db,d50,decay_range_db,curvature_pct,onset_s,
truncation_s`, one per band, broadband first; a refused value reads `refused:<why>`).
Values are written in their shortest exact form and gaps
as `nan`, so an export re-imports bit for bit onto the grid named in its header. Import
reads `name`, `kind`, `grid`, `delay_ms` and, for a sweep, `sweep_info`, `sweep_ir`,
`room_metrics`, the distortion columns and the impulse response; a v2 sweep export imports
as a sweep without room parameters; a v1 sweep export (no `sweep_info`) imports as
its transfer function (note `sweep_without_analysis`).

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

`session.json`: `{format: "ac2-session", version: 15, saved_at, measurements:
[{id, config: MeasConfig, running, delay: {applied, nudged, tracking} | null}], spl_logs:
[{meas, file, bands: file | null}], traces: [{meta: TraceMeta, grid: GridDef, file, mic_curve_points: [[Hz,
dB]] | null}]}` (JSON, field names as in this document; `mic_curve_points` are the points of
`meta.mic_curve`, a curve applied after capture). A trace
file is named by its trace id and a 64-bit FNV-1a hash of its content, so it never changes
once written. A save writes the trace files that are not there yet, then replaces
`session.json` atomically (temporary file + rename), then removes the files no manifest
names: a reader sees the old session or the new one, never a mix. An SPL log's
unterminated last line (an append cut short, §7.3) is not read and is not an error.
`format` and `version` are read first; any other version is
refused (no migration). Trace files hold the unsmoothed columns; each trace's display
smoothing is its `meta.edit.smoothing` (older versions — version 1 transfer captures could
hold smoothed columns, version 2 named smoothing modes `power` / `complex` and had no
spectrum smoothing, version 3 had no sweep traces, version 4 kept a sweep's impulse response
in a `*.sweep.json` sidecar and had no mic curves on traces, version 5 named a capture's
curve by name only, without its label, file and content hash, version 6 had no Leq windows
and no SPL logs, version 7 named files by save generation and its autosave kept the
previous one as a separate directory, version 8 had no spatial averages and no room
parameters on sweeps, version 9 held spatial averages where version 10 holds math channels, version 12 had no
band meters — are refused). A directory that holds other files is never written
into.

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
no owner, a new session epoch, no audio session opened — logs what it restored, and carries
on appending to the restored logs' files after their last whole line. An autosave of
another session format version is renamed to `<dir>.v<N>`, an unreadable one to
`<dir>.damaged` (a name taken gets the time appended), with a warning; neither is deleted.
`--no-restore` starts empty and moves the autosave to `<dir>.unrestored`.

The `autosave` entity: `off` (no autosave directory), `saved` (the disk holds the current
state; `saved_at` nil until the first write), `pending` (a change is waiting or being
written), `failed{reason}` (the last write failed; shown until a write succeeds, retried
10 s later). `saved_at` is when the autosave on disk was written (after a restore, when the
restored one was). Status changes are ordinary events, so they bump `rev`.

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
- `<name>.ac2rec.json`: the sidecar, JSON, `format: "ac2-raw-capture"`, `version: 1`
  (another version is refused), `software` {`ac2`, `build`, `protocol`}, `audio`
  {`file`, `sample_rate`, `channels`: [{`input`, `name`, `mic`, `roles`: [{type: loopback
  \| reference \| measured \| analysed, `measurement`}]}]}, `device` {`backend`,
  `input_device`, `output_device`, `buffer_frames`, `clock`, `session_epoch`, `loopback`},
  `start` and `end.at` (`Mark` {`session_sample`, `wall_ns`, `utc`}), `end` {`at`,
  `frames`, `reason`: RecordingEnd} \| nil while recording, `limits` {`max_duration`,
  `max_bytes`}, `started_by`, `initial` {`measurements`, `generator`, `inputs`,
  `calibrations` (of the recorded inputs on that device)}, `timeline` [{`at_sample`,
  `frame`, `wall_ns`, `change`: {type: measurement \| generator \| inputs \| calibration,
  value}}], `discontinuities` [{`frame` (first file frame after it), `session_sample`,
  `lost_frames`, `estimated`, `causes`: [xrun \| gap \| overflow \| config_change \|
  recorder_behind]}]. Entity values are the protocol types of `software.protocol` in JSON.

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
| `txtvers` | layout of this record, `2`. A reader that does not know the value ignores the advert. |
| `name` | the rig name, as above |
| `v` | daemon version (`ac2d --version`) |
| `proto` | `PROTO_VERSION` the daemon speaks (§2) |
| `fp` | fingerprint of the daemon's CURVE server key: the first 10 bytes of SHA-256 over the 32 raw key bytes as five dash-separated groups of four lowercase hex digits (`1a2b-3c4d-5e6f-7a8b-9c0d`) |

The `key` TXT value is the full 40-character Z85 public CURVE server key.
The advert carries no secret and grants nothing. A client connects only with a server key it
pinned after comparing the fingerprint with the one the daemon host shows
(`ac2 auth pair` or a discovered-key pairing dialog), and CURVE fails the handshake when the daemon does not hold that key. Clients
may use `fp` to pick which pinned key belongs to an advert and to warn when a known host
advertises a different fingerprint; they must not pin a key based on an advert alone.
