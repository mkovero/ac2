# Raw capture files

PLAN §3.5 "Raw capture files (lossless samples + config timeline) re-analysable within
tolerance" (P1, phase 7); §12 decision 5: f32 WAV/W64 + JSON sidecar, exact sample
preservation, DSP replay judged within tolerance. Wire: `docs/protocol.md` §3.2 (*Raw
capture files*) and §7.5.

## What it is for

A recording keeps exactly what the converter delivered on the chosen inputs, together with
what the operator was doing, so the measurements can be run again later — on another
machine, with another build, with other settings — and so rig problems (a dropout, an odd
transfer function) can be examined after the show. It is not a programme recorder: no
compression, no metering of its own, no output channels.

## Recording

`rec.start {inputs, name, max_duration, max_bytes}` on an open session; `ac2 rec start --in
1,2 --max 10min [--max-size 2GB] [--name show]`; in the app the palette's *Record* toggle
(every input of the session, one hour at most). One recording at a time.

```
audio callback ──► capture ring ──► fan-out ──► jobs (TF, RTA, SPL, …)
                                         └────► recorder job ──► WavWriter (own thread)
```

- **The audio callback is untouched.** The recorder is one more consumer of the capture
  fan-out, on its own thread, like a measurement job. The callback only fills the capture
  ring, as always; the fan-out hands the recorder the same blocks the analyses get.
- **A bounded queue, never a silent splice.** The fan-out queues at most 10 s of audio for
  the recorder (2 s for an analysis). A disk that stalls longer makes the fan-out drop
  batches for the recorder alone; the next block arrives with a jump in its sample index,
  and the recorder writes a `recorder_behind` discontinuity with the samples lost. Device
  gaps (xrun, counter jump, capture overflow, configuration change) are recorded the same
  way from the block flags. The file holds only captured frames — never filler — and the
  sidecar says where they do not follow each other.
- **Ends always finalise.** `rec.stop` (everything captured before the request is kept), the
  duration and size bounds (exact to the frame), a write error (disk full: `write_failed`
  with the OS message, everything before kept), the session closing or reopening (device
  change, `session.open`, `file.load`, `session.replay`) and daemon shutdown all finish the
  WAV header, sync the file, and rewrite the sidecar with the end and its reason. A daemon
  that is killed leaves a sidecar without an end and a header that states no audio; the next
  daemon on that directory repairs both (`interrupted`: the frames that reached the disk,
  a trailing partial frame cut) once the audio file has been idle for 30 s, and mirrors it
  as the `recording` entity so the operator sees it.
- **Status.** The `recording` entity (`RecordingRun`) carries frames, file size,
  dropouts and the status, updated about once a second; the app's top bar shows `REC 1:23 ·
  23.0 MB` (elapsed is audio in the file, not wall time), then `recorded show · 1:23 ·
  23.0 MB (time limit)` or the failure in warning colour; `ac2 rec status` and `ac2 session
  status` print the same text (`ac2_scene::recording`).

## Files

`<recording dir>/<name>.wav` and `<name>.ac2rec.json`; the directory is `recordings` in the
data directory (`ac2d --recordings <dir>`). A name is never reused.

**Audio.** 32-bit IEEE float, interleaved, the requested inputs in order, values bit for bit
as captured: float carries every converter word exactly, and what a replay feeds the
analyses is what they saw live. The header has a fixed 116-byte layout (`RIFF`/`WAVE`, a
28-byte `JUNK` chunk, `fmt ` WAVE_FORMAT_EXTENSIBLE with the float subformat and channel mask
0 — measurement inputs have no loudspeaker positions —, `fact`, `data`), so finishing a file
only patches sizes in place. Past 4 GiB the same bytes become RF64 (EBU Tech 3306): `RIFF` →
`RF64`, `JUNK` → `ds64` with 64-bit sizes; the audio never moves. RF64 rather than W64: it
keeps the WAV header shape that most tools already read, and the reserved `JUNK` chunk makes
the switch free. Written by a small writer in `ac2-traces::raw::wav` (no codec dependency;
the reader accepts plain float WAV too and refuses PCM).

**Sidecar** (`format: ac2-raw-capture`, `version: 2`; another version is refused):

| field | content |
|---|---|
| `software` | `ac2d <version>`, build id, protocol version of the quoted types |
| `audio` | file, rate, per channel: input, device's name, mic, roles (loopback / reference / measured / analysed by which measurement) |
| `device` | backend, devices, period, clock relation, session epoch, loopback route |
| `start`, `end.at` | session sample, daemon wall ns, UTC |
| `end` | frames, `RecordingEnd` reason; absent while recording |
| `limits` | duration and size bounds |
| `initial` | measurements, generator, input setup, the recorded inputs' calibrations |
| `timeline` | every committed change of a measurement (config, delay, running, deleted), the generator (arm, fire, level, signal), the input setup or a calibration: `at_sample` (newest captured sample at commit), `frame`, wall time, the new value |
| `discontinuities` | `frame` (first file frame after it), `session_sample`, `lost_frames`, `estimated`, `causes` |

Session sample of file frame *f* = `start.session_sample + f + Σ lost_frames` of the
discontinuities at or before *f*. A timeline entry's sample is when the daemon committed
the change; the analyses apply it with the next block they take, within one fan-out hand-off
(≤ 1/60 s), which the sidecar does not resolve further.

## Replay

`session.replay {recording, pace}`; `ac2 session replay <name|path> [--fast]`; the app's
palette *Replay a recording…*. It opens a session on a `ReplayBackend` (in `ac2-audio`): a
capture-only device whose "callback" is a thread reading the file. It reports the recorded
device's id (so the input's calibration and mic curve apply as they did live), the recorded
inputs under their device numbers, the recorded rate and period, no outputs. The running
measurements restart on it as on any reopen.

- **Same samples, same blocks, same resets.** Blocks have the recorded device period and
  start at sample 0 = the file's first frame; at each recorded discontinuity the index jumps
  by the samples lost and the block carries the recorded flags (a configuration change
  replays as a plain discontinuity, since a replay has no device to reopen). Blocks never
  straddle a discontinuity.
- **Lossless pace.** Unlike a device, the replay can wait: it holds a block back while the
  capture ring is full, and the fan-out of a replay session pops only as much as every
  consumer has room for. `fast` therefore never drops audio, however slow the analyses;
  `realtime` plays one second per second. The replay is held until the measurements are
  attached, so the first frame is seen by all of them.
- Replay of a replay records the same samples again (tested bit for bit).

## Tolerance

What `crates/ac2d/tests/recording.rs` asserts after recording the simulated rig (pink
noise, loopback + acoustic path, an xrun that loses 1000 frames mid-way) with a transfer
function, a spectrum, an RTA and an SPL meter running, and replaying it `fast`:

| quantity | live vs replay |
|---|---|
| samples (recording the replay) | bit-exact, sample index for sample index, xrun at the same index |
| frames, file size, discontinuity sample and loss | exact |
| TF magnitude / phase / coherence (480 columns) | ≤ 0.01 dB / ≤ 0.1° / ≤ 1e-4 |
| spectrum (every bin) | ≤ 0.01 dB |
| SPL meter level | ≤ 0.01 dB |
| third-octave RTA of a steady 1 kHz tone, bands within 30 dB of it | ≤ 0.05 dB |

The TF, spectrum and SPL meter average over blocks, so identical blocks give results equal
to rounding (in practice identical). The RTA averages one value per *publish interval*, and
publish intervals follow wall time: a fast replay packs more audio into an interval than a
live run did, so its average is over other spans of the same audio. With noise that is a
statistical difference that depends on scheduling (on a loaded CI runner more than 1.5 dB
in a third-octave band), so the RTA is compared on a steady tone, whose band levels do not
depend on the span (`replayed_rta_reads_a_steady_tone_alike`). A `realtime` replay is the
one to use when RTA readings of noise must match closely. The same holds for anything
paced by wall time (frame rates, the SPL meter's per-second log, which follows the replay's
own clock).

## Limits and open questions

- Only inputs are recorded. The generator output itself is not a channel of the file: the
  loopback return (an input) is the measured reference, which is what the analyses use; the
  generator's settings are in the timeline. Recording the rendered output (the history ring
  the timing monitor reads) would add a channel without a converter behind it — left out
  until someone needs it.
- One recording at a time, of one session; a session reopen ends it (a new epoch is a new
  time base). Recording across a reopen as one file with a marked discontinuity would be
  possible but the sidecar's single `device` would then lie.
- No transfer of recordings over the protocol: files stay on the daemon host (remote clients
  name them). PLAN §6.3's chunked binary transfer is the way when needed.
- The timeline applies to nothing on replay: the replayed session runs the measurements as
  they are now. Re-applying the recorded configuration changes at their samples would make
  a replay reproduce a whole live run; the sidecar holds what is needed for that.
- The fake rig's `Thread` pace is not deterministic against wall-clock-paced analyses; the
  test therefore drives the device by hand and compares results after the same audio.
