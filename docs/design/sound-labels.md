# Sound labels

Status: design, not implemented. Open items at the end.

## What the operator sees

A spectrum pane can show what the sound probably is: a small strip in a corner of the plot
reads `Music 82 % · Speech 9 % · Crowd 4 %`, updated about twice a second. It is a model's
guess, not a measurement, so it reads in the faint grey of the plot hints (gentle-hint style,
like the resolution marker), hides with the rest of the plot chrome under **T**, and shows `—`
when the best label is below a confidence threshold or the input is below the noise gate. A
key toggles it per pane (key to be decided; plain, Shift or Alt — no new Ctrl chord).

Why a guess earns a place: on a live rig the spectrum shows *how* the input looks, not *what*
it is. A label tells at a glance whether a hump is a voice on an open mic, crowd, wind on the
measurement mic, or programme — useful when the operator is not the one choosing the
programme (festival changeover, a band's line check, a remote rig seen from the Pi kiosk).

What it is not: the model ignores absolute level and calibration (its input is normalised
log-mel), was trained on AudioSet (YouTube audio, not PA rooms), and says nothing about
frequency response. The user guide says so where the strip is described; no other number in
ac2 depends on it.

## Model: YAMNet

| | YAMNet | PANNs CNN14 |
|---|---|---|
| licence | Apache-2.0 (code and weights) | code MIT, weights trained on AudioSet clips |
| size | 3.7 M params, ~15 MB | ~80 M params, ~300 MB |
| input | 16 kHz mono, 0.96 s window, 0.48 s hop | 32 kHz, 10 s clips (works shorter) |
| cost | ~70 M MACs per window | tens of GMACs per clip |
| classes | 521 AudioSet | 527 AudioSet |

YAMNet: a Pi 4 class daemon (PLAN §12 decision 4) runs it at a few percent of one core; the
labels are the same ontology; the licence is clean. CNN14's better mAP buys nothing for a
top-3 strip and costs the Pi.

## Runtime: `tract-onnx`

Pure Rust, builds on Linux, macOS, Windows and the Pi with no shared library to package —
the native-dependency cost already paid for JACK, ASIO and Core Audio is not paid again.
onnxruntime (`ort`) is faster but ships a C++ dylib per platform; candle has no YAMNet port.

The model is the waveform-input ONNX export (TF-Hub YAMNet through `tf2onnx`, frontend in the
graph), so ac2 does not reimplement YAMNet's exact log-mel: a mel frontend that differs in a
window or a log offset shifts every score, and the golden test would only check ac2 against
itself.

The model file is not in git. `ac2 model fetch yamnet` downloads it into the data dir
(`ac2-paths`) and checks a pinned SHA-256; release packaging may bundle it. Without it the
daemon answers the start command with a typed error, never a silent empty strip.

## Pipeline

- **Resample to 16 kHz** (`ac2-core`): 48 kHz is an integer 3:1 decimation, which the MTW
  ladder's `FirDesign::kaiser_lowpass` and `PairDecimator` (`mtw/fir.rs`) already cover;
  44.1 kHz needs a rational 160:441 polyphase stage. The anti-alias band edge sits below
  8 kHz; the model's mel bank tops out at 7.5 kHz, so nothing it reads is aliased.
- **Classifier crate `ac2-classify`**: loads the model and the class map CSV, one call
  `classify(window: &[f32; 15600]) -> [f32; 521]`. No threads, no I/O after load; keeps
  `tract` out of `ac2-core` (pure math, no heavy deps) and out of `ac2-ui` (no DSP).
- **Daemon job** (`ac2d`, feature `classify`): one per input channel, fed from the capture
  block stream on a worker thread, never the audio callback. A ring of 15 600 resampled
  samples; one inference per 7 680-sample hop. A gap in the block stream resets the ring
  (sync contract: nothing splices), so the first label after a gap comes 0.96 s later.
- **Smoothing and display rule** (`ac2-scene`): exponential average over about three hops,
  then top 3 with hysteresis so a near-tie does not flicker between two names; `—` below the
  threshold. The daemon sends raw per-hop scores; the averaging is display truth, tested
  headless like every other displayed string.

## Wire

- Commands `classify.start { channel }`, `classify.stop { channel }`.
- `ClassifierInfo { model, labels }` once per start: the class names are data from the
  model's map, not modes, so they travel as strings; the frames carry indices.
- `ClassifyFrame { channel, sample_index, top: [(ClassId(u16), f32); 5] }` per hop, the
  sample index of the window's last sample so the strip lines up with the spectrum frame.
- `ClassifierUnavailable { reason }` with `reason` an enum (`ModelMissing`,
  `ModelHashMismatch`, `FeatureNotBuilt`).

A `PROTO_VERSION` bump with `fixtures/protocol/WIRE_LOCK`, `docs/protocol.md` and the Python
fixtures in `tools/protocol`.

## Tests

- Resampler: golden against scipy `resample_poly` from `tools/refgen`.
- Classifier: `tools/refgen` runs TF-Hub YAMNet on fixed clips (speech, music, pink noise,
  silence) and stores the scores; `ac2-classify` must match within tolerance
  (`ac2-testkit`). Model absent locally → skipped with a note; CI fetches it in a cached step.
- Daemon: replay backend playing a clip — frames every 0.48 s of samples, gap resets the ring,
  missing model gives `ModelMissing`.
- Scene: strip text, threshold `—`, hysteresis, **T** hides it.
- Pi: CPU of one job on the pupu Pi, measured from a replay, no emission.

## Open

- Toggle key per pane.
- Model bundled in release packages, or fetched only.
- Feature `classify` on by default (build time of `tract` on CI and the Pi image).
- Which AudioSet classes to fold together for display (e.g. the 60 music genre classes under
  `Music`), or show the raw names.
