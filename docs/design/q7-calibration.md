# Q7 — Calibration store, mic curves and SPL correction

Status: accepted for phase 5. Answers Q7 in `open-questions.md` within decisions 7a/7b
(calibration tied to device + input channel + mic name; mismatch → "cal from other mic /
input", otherwise cal age; no gain/phantom fields, no prompts), 7c (mic curve: TF /
spectrum / RTA subtract the file's dB from the displayed magnitude; SPL applies it as a
filter before weighting; phase never touched; on/off per input), K8 (a mic name per input
in the session's input setup) and PLAN.md §3.6, §5.3, §5.7.

## 1. What is stored

One **calibration entry** per key `(device uid, input channel, mic name)`:

| field | meaning |
|---|---|
| `key.device` | capture device id as the backend reports it (`DeviceId`) |
| `key.channel` | zero-based device input channel |
| `key.mic` | mic name as typed by the operator (exact, case-sensitive, non-empty, ≤ 64 chars) |
| `spl` | sensitivity calibration or nil: `sensitivity` (dB SPL of 0 dBFS), `calibrator_level` (dB SPL), `calibrator_freq` (Hz), `measured` (dBFS read from the calibrator), `calibrated_at` (daemon wall ns) |
| `mic_curve` | mic-curve reference or nil: `name` (file stem), `file_name`, `content_hash` (FNV-1a 64 of the file bytes, hex), `points`, `f_lo`, `f_hi` (Hz), `imported_at` |

The curve's points are kept in the store file and in the daemon; the mirrored state carries
only the reference (provenance), so a 2000-point curve never travels in every snapshot.

The **input setup** (K8) is daemon state too, one row per input channel:
`{channel, mic: string | nil, mic_curve: bool}`. `mic_curve` is the on/off switch of decision
7c and defaults to on (a curve only applies when one resolves, §3). The input setup is
keyed by channel, not by device: it describes what is plugged into the session's inputs,
and a device change shows up as a key mismatch, not as lost mic names.

Neither entries nor input setup hold preamp gain or phantom state (7b): ac2 cannot know
them, and recalibrating after a gain change is the operator's job.

## 2. Commands

- `cal.spl {input, mic, calibrator_level, calibrator_freq}` reads the input's broadband,
  uncorrected RMS (exponential mean square, τ = 1 s), stores `sensitivity = calibrator_level − measured` on the
  entry `(session input device, input, mic)` (keeping its mic curve), and sets the input
  setup's mic name to `mic`. The mic name is typed once, at calibration time (§5.7); setting
  it on the input in the same step means the readout is *verified* immediately rather than
  flagging the calibration that was just taken. Refused without an open session, without
  signal (below −80 dBFS), with a non-finite / non-positive level or frequency, and while the
  level is not steady: a companion τ = 0.2 s mean square must agree with the 1 s one within
  0.05 dB, which bounds the reading's settling error to 0.05 dB (about 5 s after the
  calibrator goes on; the operator is told to retry).
- `cal.mic_curve {input, mic, action}`: `import {file_name, content}` parses the file
  (§4) and stores it on the entry `(session input device, input, mic)`, setting the input's
  mic name as `cal.spl` does; `clear` removes the curve from that entry (the entry goes
  away when it holds neither a sensitivity nor a curve). Reply: the entry (`calibration`),
  or nil after a clear that removed it.
- `cal.list` → every entry.
- `session.inputs {inputs: [InputSetup]}` upserts the listed rows (others unchanged); reply
  `inputs` (the full list). Duplicate channels or empty / over-long mic names are invalid.

The mirrored state holds `calibrations` (keyed by `CalKey`, events `calibration`) and
`inputs` (one value, event `inputs`).

## 3. Matching (decisions 7a/7b, K8)

For a job on input `c` of the open session's capture device `D`, with the input setup's
mic `m` (possibly unset):

**Sensitivity** — the first match wins:

1. entry `(D, c, m)` with a sensitivity → **verified**;
2. otherwise the newest entry with a sensitivity on `(D, c)` (another mic, or no mic name
   set) → **other mic / input**;
3. otherwise the newest entry with a sensitivity for mic `m` on another device or channel →
   **other mic / input**;
4. otherwise **uncalibrated** (dBFS).

A mismatched calibration is still applied — the readouts stay in dB SPL — and every
readout that depends on it says so. ac2 does not prompt (7b).

**Mic curve** — it is a property of the capsule, so it follows the mic name: the curve of
entry `(D, c, m)`, else the newest curve imported for mic `m` on any device or channel.
With no mic name set, no curve applies. It applies only while the input's `mic_curve`
switch is on.

**Normalisation frequency** — the calibrator frequency of the sensitivity calibration in
use; 1 kHz when uncalibrated. The curve is shifted so that its value at that frequency is
0 dB: the calibrator tone was read uncorrected, so a correction of 0 dB there means
nothing double-counts (§5.7).

### Which readouts carry it

| readout | cal state | curve flag |
|---|---|---|
| SPL meter (`spl` frame) | `cal: uncalibrated \| verified{calibrated_at} \| other_mic_or_input{calibrated_at}` | `mic_curve` |
| RTA (`rta`), spectrum (`spec`) | same `cal` (they are in dB SPL when calibrated) | `mic_curve` |
| TF (`tf`) | — (a ratio; sensitivity cancels) | `mic_curve` (measurement input only) |

The age is `capture_wall_ns − calibrated_at`, both on the daemon clock, so no client clock
offset enters it. Wording (`ac2-scene`): `cal 3 h ago`, `cal from other mic / input`,
`uncalibrated`; when a curve is applied the line adds `· mic curve`.

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

- Location: `<config dir>/ac2/calibrations.json` (`--cal-store PATH` overrides; tests and
  in-process daemons may run without a file — memory only).
- Format: JSON `{"format": "ac2-calibrations", "version": 1, "entries": [...], "inputs":
  [...]}`, entries with their curve points. Human-readable, diffable, hand-repairable.
- Writes are atomic: a temporary file in the same directory, flushed and synced, renamed
  over the old one, directory synced. A crash leaves the old file or the new one, never a
  mix.
- Read once at daemon start. Missing → empty store (created on the first write).
  **Unparseable** (bad JSON, unknown `format`/`version`, an entry that fails §4
  validation) → the daemon starts with an empty, **read-only** store: the file is never
  overwritten; `cal.spl`, `cal.mic_curve`, `session.inputs` and `cal.list` are refused
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
