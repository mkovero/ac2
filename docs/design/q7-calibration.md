# Q7 — Calibration store, mic curves and SPL correction

Status: implemented (phase 5; several curves per mic and the active curve per input, §10). Answers Q7 in `open-questions.md` within decisions 7a/7b
(calibration tied to device + input channel + mic name; mismatch → "cal from other mic /
input", otherwise cal age; no gain/phantom fields, no prompts), 7c (mic curve: TF /
spectrum / RTA subtract the file's dB from the displayed magnitude; SPL applies it as a
filter before weighting; phase never touched; the active curve chosen per input, §10), K8 (a mic name per input
in the session's input setup) and PLAN.md §3.6, §5.3, §5.7.

## 1. What is stored

Two stores and the input setup (§10 says why the curves are a library of their own).

One **sensitivity calibration** per key `(device uid, input channel, mic name)`:

| field | meaning |
|---|---|
| `key.device` | capture device id as the backend reports it (`DeviceId`) |
| `key.channel` | zero-based device input channel |
| `key.mic` | mic name as typed by the operator (exact, case-sensitive, non-empty, ≤ 64 chars) |
| `spl` | `sensitivity` (dB SPL of 0 dBFS), `method` (acoustic {calibrator level} or electrical {§11}), `freq` (Hz, the tone read), `measured` (dBFS read), `calibrated_at` (daemon wall ns) |

The **mic library**: per mic name, any number of **curves**, each a `MicCurveRef` —
`label` (short, unique per mic: `0°`, `90°`, or the operator's), `file_name`,
`content_hash` (FNV-1a 64 of the file bytes, hex), `points`, `f_lo`, `f_hi` (Hz),
`imported_at`, `stated_sensitivity` (mV/Pa as the file's header states it, or nil;
the default sensitivity of an electrical calibration, §11, and nothing else) — with its points. The points are kept in the store file and in the
daemon; the mirrored state carries only the references, so a 2000-point curve never travels
in every snapshot.

The **input setup** (K8) is daemon state too, one row per input channel:
`{channel, mic: string | nil, curve: not_chosen | off | curve{label}}`. The input setup is
keyed by channel, not by device: it describes what is plugged into the session's inputs,
and a device change shows up as a key mismatch, not as lost mic names.

Neither store nor the input setup holds preamp gain or phantom state (7b): ac2 cannot know
them, and recalibrating after a gain change is the operator's job.

## 2. Commands

- `cal.spl {input, mic, calibrator_level, calibrator_freq}` reads the input's broadband,
  uncorrected RMS (exponential mean square, τ = 1 s), stores `sensitivity = calibrator_level − measured` on the
  key `(session input device, input, mic)`, and sets the input setup's mic name to `mic`.
  The mic name is typed once, at calibration time (§5.7); setting
  it on the input in the same step means the readout is *verified* immediately rather than
  flagging the calibration that was just taken. Refused without an open session, without
  signal (below −80 dBFS), with a non-finite / non-positive level or frequency, and while the
  level is not steady: a companion τ = 0.2 s mean square must agree with the 1 s one within
  0.05 dB, which bounds the reading's settling error to 0.05 dB (about 5 s after the
  calibrator goes on; the operator is told to retry).
- `cal.curve_import {mic, label?, file_name, content, input?}` parses the file (§4) and
  stores it as `mic`'s curve `label` (default from the file, §10; a curve of that label is
  replaced). With `input`, the input's mic name becomes `mic` as `cal.spl` does, and the
  curve becomes the input's active one when it is the mic's only curve. Reply: the mic.
  No open session needed (the library is not tied to a device).
- `cal.curve_rename {curve: {mic, label}, label}`: inputs that chose the curve follow.
  `cal.curve_delete {curve}`: a mic left without curves is deleted; inputs that chose the
  curve keep the label and show it as not stored (§10).
- `cal.list` → the sensitivity calibrations and the mic library.
- `cal.delete {key}` removes the sensitivity calibration `key`, on any device: a mic sold or
  an interface retired leaves entries that no session can reach through `cal.spl`, so
  deletion is by the full key and needs no open session. The input setup is untouched.
  `not_found` when there is none. CLI `ac2 cal rm --input N [--mic NAME] [--device ID]`
  (device: the open session's, else the one device holding that input + mic); UI palette
  `Calibration: delete a sensitivity calibration…` and the calibrations view.
- `session.inputs {inputs: [InputSetup]}` upserts the listed rows (others unchanged); reply
  `inputs` (the full list). Duplicate channels or empty / over-long mic names are invalid,
  as is a chosen curve not stored for the row's mic (unless the row is unchanged: a row
  whose curve was deleted can be sent back as it is).

The mirrored state holds `calibrations` (keyed by `CalKey`, events `calibration`), `mics`
(keyed by name, events `mic`) and `inputs` (one value, event `inputs`).

## 3. Matching (decisions 7a/7b, K8)

For a job on input `c` of the open session's capture device `D`, with the input setup's
mic `m` (possibly unset):

**Sensitivity** — the first match wins:

1. entry `(D, c, m)` → **verified**;
2. otherwise the newest entry on `(D, c)` (another mic, or no mic name set) → **other mic /
   input**;
3. otherwise the newest entry for mic `m` on another device or channel → **other mic /
   input**;
4. otherwise **uncalibrated** (dBFS).

A mismatched calibration is still applied — the readouts stay in dB SPL — and every
readout that depends on it says so. ac2 does not prompt (7b).

**Mic curve** — the input's chosen curve of mic `m`, and only that one: none without a mic
name, none when the choice is `off` or `not_chosen`, none when the chosen label is not
stored for `m` (§10). A curve is a property of the capsule, so the library follows the mic
name to any device and channel; which curve applies is the input's explicit choice.

These rules are the pure functions of `ac2_proto::cal` (`input_use`, `settle`, `step`):
the daemon applies what they say and every client words its readouts from them, so what is
shown as in use is what is in use.

**Normalisation frequency** — the calibrator frequency of the sensitivity calibration in
use; 1 kHz for an electrical calibration (§11) and when uncalibrated. The curve is shifted so that its value at that frequency is
0 dB: the calibrator tone was read uncorrected, so a correction of 0 dB there means
nothing double-counts (§5.7).

### Which readouts carry it

| readout | cal state | curve flag |
|---|---|---|
| SPL meter (`spl` frame) | `cal: uncalibrated \| verified{calibrated_at} \| other_mic_or_input{calibrated_at}` | `mic_curve` |
| RTA (`rta`), spectrum (`spec`) | same `cal` (they are in dB SPL when calibrated); the caption shows it too (round 5, C3) | `mic_curve` |
| TF (`tf`) | — (a ratio; sensitivity cancels) | `mic_curve` (measurement input only) |

The age is `capture_wall_ns − calibrated_at`, both on the daemon clock, so no client clock
offset enters it. Wording (`ac2-scene`): `cal 94 dB · 3 h ago` (electrical, §11: `electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB · 3 h ago`), `cal from other mic / input`,
`uncalibrated`; the frame's `mic_curve` flag with the input's setup gives the curve note
(§10): `mic curve: MM1 34804 90°`, or why none applied. Spectrum and RTA captions append the
same (`1/3 oct · A-weighted · cal 94 dB · 3 h ago · mic curve: MM1 34804 90°`), saying nothing about
the calibration when uncalibrated (the axis unit says dBFS).

## 4. Mic-curve files

Liberal parse, strict validation. Typed errors (`MicCurveFileError` in `ac2-core`, mirrored
as `ErrorDetail::mic_curve_file {line, reason}`), never a partial import.

Parse:
- Bytes are decoded as UTF-8; invalid sequences are replaced (comments in Latin-1 must not
  refuse a file). A BOM is ignored. Lines end in `\n`, `\r\n` or `\r`.
- A line whose first field is not a number is text (headers such as `Freq(Hz) SPL(dB)
  Phase`, `"Sens Factor =-1.37dB"`, `*` / `#` / `;` / `//` comments) and is skipped,
  anywhere in the file.
- Fields are separated by whitespace and commas, or by semicolons — in which case a comma
  inside a field is a decimal comma (`20,5;-1,2`).
- A data line is `frequency gain [phase …]`; further columns are ignored (phase is never
  used, 7c).

Validate (the line number is reported where there is one):
- the second field is a number (`missing_gain`, `bad_number`);
- frequency > 0 and finite, gain finite (`non_positive_frequency`, `non_finite`);
- |gain| ≤ 40 dB (`gain_out_of_range`): measurement-mic deviations are a few dB; tens of
  dB means a wrong column or an absolute-SPL file;
- frequencies strictly ascending (`not_ascending`, duplicates included);
- 2 … 10 000 points (`too_few_points`, `too_many_points`).

Sign: the file holds the mic's response (deviation from flat); the correction is its
negative.

## 5. Display correction (TF, spectrum, RTA)

`c(f)`: the file's gain interpolated linearly in log-frequency between points, held flat
outside the file's range (also at 0 Hz). `cₙ(f) = c(f) − c(f_norm)`. Displayed value =
value − cₙ(f):

- **TF**: per log-grid column, after smoothing; magnitude only, phase and coherence
  untouched; only for the measurement input (the reference is an electrical loopback).
- **Spectrum**: per bin centre `k·fs/N`.
- **RTA**: per band, the power average over the ideal band edges in log-frequency,
  `−10·lg(mean 10^(−cₙ/10))` — exact for pink noise filling the band; within 0.05 dB of the
  centre value for 1/3-octave bands on smooth curves, and the right thing for 1/1-octave
  bands where a curve's slope makes centre and average differ.

## 6. SPL correction filter

**Design.** A minimum-phase FIR whose magnitude is `10^(−cₙ(f)/20)`, by the homomorphic
(real-cepstrum) method: sample `ln|H|` on an `N_design = 8·L` uniform grid 0 … fs/2, take
the real cepstrum, fold it onto causal quefrencies (`c[0]`, `2c[n]` for 0 < n < N/2,
`c[N/2]`), exponentiate the spectrum and inverse transform; keep `L` taps with a half-Hann
taper over the last `L/8`. Finally the taps are scaled so that the DTFT at `f_norm` is
exactly 1 (0 dB): truncation never shifts the calibration point.

**Length.** `L` = the power of two ≥ 340 ms × fs (16384 at 44.1/48 kHz, 32768 at
88.2/96 kHz, 65536 at 192 kHz): about 3 Hz design resolution. Measured: a 2nd-order mic
roll-off at 30 Hz is held within 0.01 dB from 31.5 Hz up (85 ms left 0.2 dB and 170 ms
0.04 dB at 31.5 Hz); the refgen mic model within 0.006 dB from 31.5 Hz to 16 kHz.
Accuracy is a test (§8), not a hope.

**Run time.** Uniformly partitioned overlap-save convolution, partition = the power of two
≥ 5.3 ms (256 samples at 44.1/48 kHz, 512 at 96 kHz, 1024 at 192 kHz; FFT twice that):
latency one partition, always 64 partitions, i.e. about 64 complex multiply-adds per input
sample plus two FFTs per partition. No allocation after construction. This runs on the SPL
job thread, never in the audio callback.

**Placement** (§5.3 / §5.7): raw → correction → A/C/Z → time weighting / Leq. Minimum
phase puts the filter's energy at its start, so there is no pre-ringing: the 35 ms Impulse
rise and toneburst responses keep their behaviour (tested). **LCpeak stays on the
uncorrected path** as PLAN.md fixes it: the peak path is a sample peak of the C-weighted
raw signal; the corrected path is one partition later and has the curve's HF boost, so
applying it there would change a crest-factor reading without a standard tolerance to judge
it by. The peak readout therefore never carries the curve; this is stated in the protocol.

## 7. Store file

- Location: `calibrations.json` in the platform config directory (`ac2_paths::cal_store`):
  `~/.config/ac2` on Linux (`$XDG_CONFIG_HOME/ac2`), `~/Library/Application Support/ac2` on
  macOS, `%APPDATA%\ac2\config` on Windows; `$AC2_CONFIG_DIR` or `--cal-store PATH`
  override (tests and in-process daemons may run without a file — memory only). The store
  is machine configuration (it describes the hardware), so it lives with the config, not with
  the sessions in the data directory (round 5, C2).
- Format: JSON `{"format": "ac2-calibrations", "version": 3, "sensitivities": [CalEntry],
  "mics": [{"name", "curves": [{"reference": MicCurveRef, "points": [[Hz, dB]]}]}],
  "inputs": [InputSetup]}`. Human-readable, diffable, hand-repairable.
- Writes are atomic: a temporary file in the same directory, flushed and synced, renamed
  over the old one, directory synced. A crash leaves the old file or the new one, never a
  mix.
- Read once at daemon start. Missing → empty store (created on the first write).
  **Another version** of this format (version 1 held one curve per device + input + mic
  entry and an on/off switch per input; version 2 had no calibration method) → the file is set aside as `<file>.v<N>` (the time
  appended when that exists; never deleted) and the daemon starts with an empty, writable
  store, logging what happened and how to recover: calibrate again (`ac2 cal spl`) and
  import the curves again (`ac2 cal curve import FILE --mic NAME`); the old file shows the
  mic and file names. There is no migration: a version-1 entry's curve says nothing about
  which incidence angle it was, which is exactly what the new format makes explicit.
  **Unparseable** (bad JSON, unknown `format`/`version`, an entry that fails §4
  validation) → the daemon starts with an empty, **read-only** store: the file is never
  overwritten; `cal.spl`, `cal.curve_*`, `cal.delete`, `session.inputs` and `cal.list` are refused
  (`refused`, detail `cal_store {path, reason}`) with "calibration store … is unreadable
  …; fix or move it away and restart ac2d". Measurements keep running uncalibrated.
- One daemon per file; the daemon is the only writer.

## 8. Tests

- Core: parse (UMIK-style header, REW `.frd`, semicolon/decimal-comma, comments, phase
  column, every error kind with its line); interpolation is exact on log-linear segments
  and flat outside; normalisation is exactly 0 at `f_norm`; band average of a constant is
  the constant; a flat curve designs a unit impulse; the FIR of a curve that is the
  magnitude of a minimum-phase digital biquad reproduces that biquad's inverse impulse
  response; FIR magnitude within ±0.1 dB of the target over 31.5 Hz … min(16 kHz,
  0.4·fs) for the refgen analog mic model at 44.1/48/96 kHz; partitioned convolution
  equals direct convolution; SPL meter: 0 dB at the calibrator frequency, a tone elsewhere
  reads its level − cₙ(f), LCpeak unchanged, toneburst Table 4 limits still met with a
  curve.
- refgen `calibration_mic_curve`: analog mic model points, exact cₙ at test frequencies,
  log-f band averages for 1/3-octave bands.
- Daemon: store round trip, atomic write leaves no temp files, unparseable file refused
  and untouched, matching rules (verified / other mic / other input / uncalibrated),
  frames carry the state, TF/RTA/spectrum corrected and flagged, on/off.
- CLI, client fake, scene wording, UI reducer (mic-name prompt, curve toggle).
- `cal.delete`: daemon (persisted across a restart, refused on an unreadable store,
  `not_found`), client fake, CLI `cal rm`, UI palette prompt; spectrum / RTA caption age
  (`ac2-scene`).
- §10: see there.

## 9. Mic curve on a stored trace

A trace captured before the mic had a curve (or with no mic name, or the input's curve
off) can be corrected afterwards: `trace.mic_curve {trace, curve: {mic, label}}` (CLI
`ac2 trace mic <trace> <mic|none> [--label L]`, the label optional for a mic with one curve;
UI palette **Mic curve on the selected trace…**, typed as `MM1 34804 90°`).

**Choice: a display edit, like smoothing.** The stored columns stay as measured; the
correction is applied when the trace is served (`trace.get`), after the display smoothing —
the order the live transfer job applies them in. Reasons: the raw stays recoverable (`none`
removes the curve exactly, with no rounding left behind), exports state what was done
without changing the numbers (`# mic: … (curve: …, applied after capture as a display edit,
not in the columns; 0 dB at … Hz; file …, hash …)` plus a machine-readable `# mic_curve:`
JSON line), and the same rules hold for every display edit (offset, polarity, nudge,
smoothing, mic curve: never in the columns). Baking the curve into the columns would have
needed the curve's points kept anyway to undo it.

- **Which curve.** The one named: a mic with several curves has no "the" curve (§10).
- **Normalisation** (§3, §5.7): the calibrator frequency of the trace's sensitivity
  calibration, else of the mic's newest sensitivity calibration, else 1 kHz; recorded as
  `f_norm`.
- **Recorded** in `TraceMeta.mic_curve` (`TraceMicCurve`: mic name, the `MicCurveRef` —
  label, file, content hash, points, range, import time — and `f_norm`). The daemon keeps the
  curve's **points** with the trace (and a session keeps them in its manifest,
  `mic_curve_points`, session format 6), so deleting or replacing the curve in the store
  later never changes a stored trace.
- **Where it applies** (§5): transfer and sweep traces per log-grid column, spectra per
  bin, RTA bands as the band power average; phase and coherence never. A sweep's
  distortion is corrected too: order n at fundamental f is the ratio of what the mic picked
  up at n·f to what it picked up at f, so level and floor move by c(f) − c(n·f); THD is
  re-summed from the corrected orders (the analysis's power sum). The impulse response is
  not corrected (a time-domain view; the curve is a magnitude-only display correction).
- **No double correction.** `TraceMeta.mic.curve` (the full `MicCurveRef`: label, file,
  hash) set means the capture's columns carry that curve already (the live job corrected
  them): `trace.mic_curve` is refused (`refused`,
  "captured with mic curve … applied: it is in the columns already, a second curve would
  correct twice"). The two fields are never set together. A sweep's analysis works on the
  raw recordings, so a sweep trace never names a curve in `mic.curve` (the earlier capture
  wrongly named the input's curve there) and can always take one afterwards. Targets are
  refused (`invalid`), locked traces `refused`.
- **Derived traces.** `trace.average` / `trace.math` combine the corrected columns; the
  result names the curve in `mic.curve` (its columns carry it now).
- **Import.** An export's columns are uncorrected, so an import of a trace that had a curve
  applied comes back without it, with `ImportNote::MicCurveNotApplied` on its source (apply
  it again with `trace.mic_curve`).

Tests: `ac2-traces` (display correction against the curve at every column, export header,
session round trip with points, averages of corrected columns, refusals, sweep distortion
and THD), daemon (`mic_curve_on_a_stored_trace`: apply from the store, served magnitude,
export, a capture with the curve refused, session reload, removal), CLI (`trace mic`,
`trace show` wording), UI reducer (palette prompt prefilled with the trace's mic).

## 10. Several curves per mic, the active curve per input

A measurement mic usually comes with more than one calibration file: one per incidence
angle (beyerdynamic MM1: `…_0Grad.txt` for a mic pointed at the source, `…_90Grad.txt` with
the header `rel. Level [dB], 90-degree-curve` for grazing incidence). They differ by up to a
few dB above 5 kHz, so the curve in use is part of the measurement: the wrong one is a
measurement error, and one that changes silently (a newer import replacing the older one,
as "the newest curve for the mic" did) cannot be told from a change in the system.

**Model.** Curves live in a mic library keyed by mic name, each with a short `label`; the
sensitivity calibration stays per (device, input, mic), because it calibrates the chain
including the preamp gain. Each input chooses explicitly which curve of its mic applies:
`not_chosen`, `off`, or `curve{label}`.

**Labels.** The default label is the incidence angle the file states — in a header line
(`90-degree-curve`, `90°`, `90 deg`) or else in the file name (`_90Grad`, `0deg`) — as `N°`;
else the file stem (cut to 32 characters). A number of at most three digits directly before
`°`, `deg`, `degree(s)` or `Grad` counts; longer digit runs are serial numbers, `Phase
(degrees)` has no number, `gradient` is not `Grad`. `--label` overrides; `cal.curve_rename`
renames later. Labels are unique per mic and never `off` / `none` (the CLI's words for no
curve).

**Stated sensitivity.** A header's `Sensitivity: 15.0mV/Pa` is kept as
`stated_sensitivity` and shown (`15.0 mV/Pa (−36.5 dBV/Pa)`, "data sheet"). An acoustic
calibration measures the whole chain (preamp gain, converter), which a capsule's data sheet
value cannot know; the value is used only as the default mic sensitivity of an electrical
calibration (§11), which measures the rest of the chain with a voltmeter.

**Choosing, never guessing.** A mic with exactly one curve has nothing to confuse it with:
the daemon chooses it on any row of that mic where none is chosen (`ac2_proto::cal::settle`,
run on every change of the setup or the library), and importing a mic's first curve on an
input chooses it. With several curves and none chosen, no curve applies and the input says
`choose: 0°, 90°`. A new mic name on a row starts `not_chosen`: the previous choice named
another capsule's curve. Deleting the chosen curve leaves the label on the row, shown as
`90° — not stored for MM1 34804`, and no curve applies; re-importing that label restores it.

**Switching** is one action: ←/→ on the input's row (session dialog, input setup view) step
off → 0° → 90° … (`ac2_proto::cal::step`: off, then the mic's curves in import order); the
palette's **Mic curve on input N…** (`2=90°`) and **Mic curve: next curve on the selected
measurement's input**; `ac2 cal use <in> <label|off>`, `ac2 session inputs --curve 2=90°`.
The daemon hands the running jobs the new correction at once. The correction is a display
correction of the magnitude (TF, spectrum, RTA) applied after averaging, so averages need
no reset; the SPL path's correction filter changes with it (its Leq / Lmax then mix the two
for the interval, as after any calibration change).

**Always visible.** Every place a corrected readout is shown says which curve is in it, or
why none is (`ac2-scene::cal`):

| where | applied | not applied |
|---|---|---|
| sidebar input label | `MM1 34804 · 90° · mic (in 1)` | `· curve off`, `· curve not chosen`, `· no curve stored`, `· 90° not stored` |
| pane captions (TF title, spectrum / RTA legend, SPL readout, stored traces, sweep) | `mic curve: MM1 34804 90°` | `mic curve off`, `mic curve not chosen`, `no mic curve stored for MM1 34804`, `mic curve 90° not stored for MM1 34804` |
| session dialog / input setup rows, `ac2 cal list`, `ac2 status` | `90°` | `off`, `choose: 0°, 90°`, `no curve stored for MM1 34804`, `90° — not stored for MM1 34804` |

with the sensitivity state beside it: `verified · 94.0 dB SPL at 1.00 kHz · 3 h ago`,
`from ECM on in 2 · …`, `uncalibrated`. The pane caption takes "applied" from the frame (what
the daemon did) and the label from the input setup; a frame from before a switch says just
`mic curve` until the next one arrives.

**Traces** record exactly which curve their columns carry: `MicState.curve` is the full
`MicCurveRef` (label, file, content hash), and the export header names it (`# mic: MM1 34804
(curve: 90°, in the columns; file "449350_34804_90Grad.txt", hash …)`).

**The calibrations view** (palette **Calibrations…**; **Input setup…** opens it on the
selected measurement's input) lists what each input uses (mic, curve state, sensitivity
state), every mic with its curves (file, points, range, data-sheet sensitivity, which inputs
use it) and every sensitivity calibration (device, input, mic, calibrator level and
frequency, reading, age). Keys: ↑/↓, ←/→ (an input's curve), N (name the mic), I (import a
curve file by path), R (rename a curve), Delete twice (delete a curve or a sensitivity
calibration), Enter, Esc.

Tests: `ac2-core` (angle and stated sensitivity from the real MM1 headers and from file
names; what is not an angle), `ac2-proto::cal` (states, `settle`, `step`, matching),
calstore (several curves per mic round trip, points per label, labels and stated
sensitivity from the real files, a version-1 store set aside and the store writable),
daemon `two_curves_of_one_mic_switched_on_an_input` (both MM1 files imported, the first
chosen, 0° → 90° → off moves the served TF magnitude by exactly the curves' difference,
captures record label + hash, export header, a deleted chosen curve shows as not stored),
`ac2-scene::cal` wording, CLI (`cal curve import/rename/rm`, `cal use`, `cal list` with
curves per mic and the active curve per input, `status`), UI reducer (stepping, the
palette prompts, the session dialog rows), and the end-to-end UI test
`mic_curves_imported_and_switched_in_the_input_setup` from an empty daemon.

## 11. Electrical calibration (no acoustic calibrator)

An acoustic calibrator measures the whole chain at once: capsule, preamp gain, converter.
Without one, the chain splits into the capsule — whose sensitivity `S` (V/Pa at 1 kHz) the
data sheet states — and the electrical rest, which a voltmeter measures: a steady tone of
RMS voltage `V` at the input reads `L` dBFS, so

    V_FS = V / 10^(L/20)                      (volts at 0 dBFS)
    sensitivity = 20·lg(V_FS / (S · p₀))      (dB SPL of 0 dBFS; p₀ = 20 µPa)

since 0 dBFS is `V_FS / S` pascal. Check: 15 mV read at −40.0 dBFS → `V_FS` = 1.5 V; with
15 mV/Pa, 0 dBFS = 100 Pa = 133.98 dB SPL; the mic at 1 Pa (93.98 dB SPL) gives 15 mV, which
reads −40 dBFS → 93.98 dB SPL, shown 94.0 (`ac2_proto::cal` tests).

**Command.** `cal.spl_electrical {input, mic, connection: in_line | injected, volts, freq,
mic_sensitivity?, uncertainty?, replace_acoustic}`. The level is read exactly as `cal.spl`
reads it (broadband, uncorrected, τ = 1 s; refused without signal, while the 0.2 s and 1 s
readings differ by more than 0.05 dB, and when the input reached full scale in the last 2 s —
a clipped tone reads low; the last applies to `cal.spl` too). Further refusals, because the
reading would be poor: above −3 dBFS (compression and the next gain change are near) and
below −70 dBFS (noise). Invalid: `volts` outside 0.1 mV … 100 V, `S` outside 0.1 … 1000 mV/Pa,
`freq` outside 20 Hz … 20 kHz, uncertainty outside ±0.05 … ±6 dB — ranges that catch a unit
slip (mV typed as V), not limits of the method. Without `mic_sensitivity` the daemon takes the
one value the mic's curve files state (`ac2_proto::cal::data_sheet`); none, or files that
disagree, is `invalid` with the reason.

**Stored** as the key's sensitivity calibration with `method: electrical {connection, volts,
full_scale (V_FS), mic_sensitivity, mic_sensitivity_from: typed | data_sheet {label,
file_name}, uncertainty}`; `freq` is the tone's. Frames carry `CalStatus … {basis}` with the
connection, sensitivity, whether it is the data sheet's and the uncertainty, so every readout
words it from the frame: `electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB · 2 h ago`
(an acoustic one: `cal 94 dB · 2 h ago`; a mismatch: `electrical cal from other mic / input
±1 dB`). The input setup rows and `ac2 cal list` give the full record (`electrical, in-line ·
15.03 mV at 1.00 kHz read −40.0 dBFS · 0 dBFS = 1.500 V · 15.0 mV/Pa from the data sheet (0°,
449350_34804_0Grad.txt) · ±1 dB`). It is a calibration for every purpose: values in dB SPL,
Leq limits judged (`leq.md`).

**Precedence.** One entry per key. A calibrator reading replaces an electrical one without
asking (it measured the capsule too). An electrical reading is refused over an acoustic one
unless `replace_acoustic` (CLI `--replace-acoustic`, app: Enter twice after the question).

**Normalisation.** The data-sheet sensitivity is the capsule's at 1 kHz, so the mic curve is
normalised to 0 dB at 1 kHz whatever tone was read (`ac2_proto::cal::f_norm`); a tone at
another frequency only probes the electrical chain, assumed flat between it and 1 kHz — the
clients note it (`the tone was 400 Hz: …`).

**Uncertainty budget** (stated default ±1 dB):

| term | typical | note |
|---|---|---|
| mic sensitivity, data sheet | ±0.5 … 1 dB | dominates; a capsule's individual calibration sheet: ±0.2 dB, state `--uncertainty` |
| voltmeter | ±0.05 … 0.2 dB | handheld true-RMS DMM ±(0.5–2 %) at 1 kHz on its lowest fitting range; AD2 scope ±0.5 % of range; check the meter's AC bandwidth |
| level reading | ≤ 0.05 dB | the settling rule; the tone steady and unclipped |
| loading (injected only) | 0 … 0.3 dB | the mic's source impedance (~50–200 Ω) differs from the generator's (AD2: 50 Ω); in-line measures the real loading and removes the term |
| preamp flatness (tone ≠ 1 kHz) | small | usually < 0.1 dB between 400 Hz and 1 kHz |
| gain changed afterwards | unbounded | a gain change invalidates any calibration (7b) |

Root-sum-square of the typical terms is 0.6 … 1.1 dB, hence ±1 dB by default.

**Procedures.** *In-line* (the default, `connection: in_line`): the mic stays connected
with phantom power on, an XLR breakout in the cable; a steady 1 kHz tone at the mic (the
operator's own source, or ac2's generator under the usual arm/fire and ceiling — ac2 never
emits by itself); AC volts between pins 2 and 3 on the meter's lowest fitting range while
ac2 reads. *Injected*: a generator (AD2, any sine source) in place of the mic, phantom power
off.

**Safety** (shown in the dialog, the CLI help and the user guide, by method): in-line —
phantom power is common-mode +48 V on pins 2 and 3 against pin 1: measure pins 2–3 only, in
ACV, never to pin 1, never short pins (use a breakout); injected — switch phantom power OFF on
the input first (48 V on the XLR can damage the generator; ac2 cannot switch it) and back on
for the mic afterwards; either way keep the gain you will measure with.

**App.** The Calibrations view: **E** on an input of the open session with a mic name opens
the dialog — where measured (←/→: in-line / injected, its safety note prominent), the input's
level meter live, the voltage typed with its unit (`15.03 mV`), the tone (`1 kHz`), the
sensitivity prefilled from the data sheet with its source (`data sheet (MM1 34804 0°)`; typing
makes it `typed`); Enter reads and stores, a refusal stays in the dialog to retry, success
closes it with what to do next. **CLI.** `ac2 cal electrical --input N --volts 15.03mv
[--freq 1khz] [--sensitivity 15.0mv/pa | -36.5dbv/pa] [--method inline|injected]
[--uncertainty 1db] [--mic NAME] [--replace-acoustic]` prints the record, `0 dBFS = … dB SPL`,
the notes and the phantom reminder.

Tests: `ac2-proto::cal` (the formulas by hand, `f_norm`, `data_sheet`), calstore (round
trip with a method, version 2 set aside, electrical normalised at 1 kHz), daemon
`electrical_calibration_end_to_end` (invalid inputs, no tone, too low, a tone at −26.02 dBFS
with 15 mV and the data sheet's 15 mV/Pa → 120.0 dB and the SPL meter at 93.98 dB SPL with an
electrical basis, acoustic precedence, persistence), `ac2-scene::cal` wording, CLI (`cal
electrical` parse and run, `cal list`), UI reducer (the dialog: prefill, refusal, retry,
success, injected and typed) and the end-to-end UI test `electrical_calibration_from_the_app`
from an empty daemon.
