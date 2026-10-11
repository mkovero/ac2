# Wire slim: packed live frames

Status: implemented (protocol §5.2, §5.7). Supersedes the evaluation in
[`wire-size.md`](wire-size.md). Goal: every client, the Pi kiosk on Wi-Fi and a laptop on a VPN
included, takes every topic at 60 frames per second without a network concern, so the remote
default goes from 30 to 60 (decision 2b, [`open-questions.md`](open-questions.md)).

## Measured

`crates/ac2d/tests/wire_size.rs` (`tuning_session_bytes`, `--ignored --nocapture`) runs a
tuning session on the fake rig and counts what one subscriber receives, subscribed the way the
app is with the transfer and spectrum panes in view: three transfer functions (1/48 octave,
480 columns) and their average, three 65 536-point spectra and a 4 096-point one, a
1/3-octave RTA, two SPL meters with band meters, the session input meters. The fake inputs
are delay-compensated combs plus noise, so the high bands are noisy: dearer than most real
curves. `AC2_WIRE_DUMP=dir` writes every frame for offline trials.

| | f32 arrays (v35) | packed |
|---|---|---|
| `tf` frame | 7 863 B | ≈ 2 400 B |
| `spec` 65 536 (897 columns) | 3 696 B | ≈ 1 580 B |
| `spec` 4 096 (513 columns) | 2 161 B | ≈ 1 010 B |
| `rta` 1/3 octave | 368 B | ≈ 210 B |
| `band_leq`, `leq` | 988 B, 467 B | ≈ 870 B, 370 B (still f32) |
| `ir` (4 096 points, linear + ETC) | 32 896 B | ≈ 13 000 B (22 800 B with `ir_linear` f32) |
| session at 30 fps | 7.6 Mbit/s | 2.7 Mbit/s |
| session at 60 fps | 9.3 Mbit/s | 3.3 Mbit/s |
| session at 60 fps with an IR view | | 6.2 Mbit/s (8.8 with `ir_linear` f32) |

Rates: a `tf` stream comes at its analysis hop, ≈ 26 per second whatever the publish rate;
only the math average (up to every operand change: 29 → 45/s), the short spectrum (30 → 45/s)
and the RTA (30 → 58/s) go faster at 60 fps. So 60 fps costs ≈ 0.5 Mbit/s more than 30, and
60 fps packed is less than half of 30 fps before.

Headers are positional msgpack already (≈ 100–180 B; 70 B of a `tf` header are its array
descriptors). Transport: ZMTP adds 2–9 B per part, CURVE ≈ 33 B more per part; one payload
part instead of one part per array saves 3 × that on a `tf` frame.

What the app receives was already lean: it subscribes per topic, following the visible panes
(`crates/ac2-ui/src/link_wants.rs`): transfer frames only while a transfer pane draws them,
IR only for a pane in its IR view, spectra and RTA only while the spectrum pane, peak hold or
the spectrograph needs them, never per-measurement input meters. A stopped measurement sends
nothing (no audio, no frames); an unchanged result is re-sent every 250 ms, which on a running
stream never happens in practice (the hops are shorter).

## Encoding

Each array has a fixed element encoding by name; the arrays are concatenated in the header's
order and deflated (raw RFC 1951, level 1) into one payload part.

| arrays | encoding | worst error | display needs |
|---|---|---|---|
| `mag`, `level` (spec, rta), `ir_etc` | `i16` hundredths of a dB | 0.005 dB | readout 0.1 dB |
| `phase` | `i16` hundredths of a degree, wrapped to [−180°, 180°) | 0.005° | readout 1°; group delay below |
| `coh` | `u16` ten-thousandths | 5·10⁻⁵ | readout 0.01; crosscheck ≤ 10⁻⁴ |
| bitmasks | `u32` | exact | |
| `ir_linear` | `bfp16`: an exponent byte per frame, `i16` mantissas | 1/32 765 of the peak (−90.3 dB) | linear view: sub-pixel; log view 0.3 dB at −60 dB |
| meters, `leq`, `band_leq` | `f32` | exact | |

NaN and ±∞ have codes of their own; levels beyond ±327.66 dB clamp. A 16-bit array goes as
neighbour differences in a low-byte and a high-byte plane, so deflate finds the high plane's
runs; on the measured frames that is 15–25 % smaller than plain codes.

### The linear IR

`ir_linear` is block floating point: value = mantissa · 2^e, one exponent per frame, the
smallest that puts the frame's largest finite |x| at or below 32 765 (odd, so a decoded
frame picks the same exponent again and re-encodes to the same bytes). Every sample is within
2^(e−1), under 1/32 765 of the peak: −90.3 dB re peak at worst, −96 dB when the peak fills
the mantissa. What the IR pane does with it:

- **Linear view** (amplitude on a linear axis): at the default zoom (±1.1 × peak over a few
  hundred pixels) one pixel is ≈ −47 dB re peak, so the error is a thousandth of a pixel; it
  reaches a pixel only zoomed in a thousandfold. The cursor reads three decimals of FS.
- **Log view** (20·log10 |h| / peak): 0.03 dB off at −40 dB, 0.3 dB at −60 dB (the default
  floor), ±3 dB on single samples at −80 dB. A live acoustic IR's own noise floor (mic, room
  noise, 4 096-point transfer stage) is far above −90 dB re peak; on the fake rig the median
  sample is −18 dB. Only an electrical loopback goes deeper, and there the log view's floor
  shows the quantisation (≈ −101 dB RMS) instead of the true one; the ETC, in `i16` dB, keeps
  any floor, and a captured trace keeps f32.
- **Crosscheck**: reads IRs from stored sweep and transfer traces, never from frames.

`f16` scaled by a per-frame power of two was the other candidate: relative error 2⁻¹¹ over
≈ 170 dB, so the log view would be exact to 0.004 dB down to any floor. Measured on the fake
rig's frames both are ≈ 8 kB of linear IR per frame (its IR is noise from −18 dB on, which no
16-bit code compresses). On synthetic IRs with a cleaner floor block floating point is
smaller and gains as the floor drops, because a tail sample is a small integer whose
differences deflate, while a float carries its full 11-bit mantissa at any level:

| IR floor re peak | f32 | f16 | `bfp16` |
|---|---|---|---|
| −40 dB | 15.3 kB | 7.7 kB | 6.7 kB |
| −60 dB | 15.5 kB | 7.7 kB | 5.8 kB |
| −80 dB | 15.6 kB | 7.7 kB | 5.2 kB |
| −100 dB | 15.6 kB | 7.7 kB | 4.0 kB |

Integers also decode the same way in every client language, where binary16 needs a
conversion routine in some. The −90 dB bound is below anything the pane shows of a measured
IR, so the smaller encoding wins.

Group delay is the slope of the phase over 1/12 octave (4 columns at 1/48 octave). A uniform
phase error of ±0.005° moves it by σ ≈ 12 µs at 20 Hz and 2.5 µs at 100 Hz, against a 0.01 ms
readout and a measurement whose own phase noise at 20 Hz is degrees. 0.1° would be ten times
that, so phase stays at hundredths.

Verified by `ac2-proto` tests: `fuzz::quantisation_stays_in_budget` (random values, the bounds
above), round trips that re-encode to the same bytes, malformed payloads (truncated, not
deflate, inflating past its size, undefined coherence codes, unknown bits) refused without a
panic; `tools/protocol` decodes the Rust fixtures and the reverse.

### What stays full precision

Captures, stored traces, exports (`trace export --csv`), math, sweeps and session files are
formed in the daemon from the analysis in f32, never from a frame
(`traces::spectrum::spectrum_live_columns_are_the_captured_bins_maxima` checks a capture keeps values
off the wire grid). A transfer trace's IR comes from the job's own inverse FFT at capture
(`capture_ir`), not from the last `ir` frame, so it keeps every f32 sample. For crosscheck
(peer session constraints): `ac2 spl watch` reads `spl`
frames, whose readings are header f64 and unchanged; `leq`, `band_leq` and the meters stay f32;
coherence resolution is 10⁻⁴ or finer. A client that needs f32 curves captures a trace.

## Cost

Release build, 480-column `tf` frame: encode ≈ 37 µs, decode ≈ 17 µs on a Threadripper 3945WX
core (`roundtrip::codec_cost`). A 4 096-point IR frame: encode ≈ 165 µs, decode ≈ 61 µs,
the same and less than with `ir_linear` in f32 (165 µs, 74 µs): the quantising pass costs what
deflate saves on the smaller input. It runs at ≈ 29 frames/s only while an IR view is open. The session's ≈ 320 frames/s cost the daemon ≈ 1 % of a core;
pupu's i5-2415M is perhaps twice as slow, a Pi 4 three to four times on decode: ≈ 2 % of a
core there. Encoding runs on the measurement job threads, decoding on the client's I/O
thread; neither is on the audio callback.

## Considered, not done

- **zstd / LZ4.** zstd links C and needs a Python package; LZ4 has no entropy coder and gains
  nothing on 16-bit noise. Deflate is in every runtime a viewer may be written in (Python
  `zlib`, `java.util.zip.Inflater(nowrap)`), and level 1 is within 1 % of level 6 here.
- **A Rice coder** of the differences (block-adaptive, as FLAC): ≈ 19 % smaller payloads than
  deflate on the measured frames, but custom bit-level code in every client.
- **Deltas against the previous frame** of a topic: frames are dropped and coalesced by
  design (PUB/SUB high-water marks, latest-only slots), so a delta stream needs keyframes and
  per-client state; and frame-to-frame changes of a noisy measurement are as large as
  neighbour differences.
- **Header-only repeats** of an unchanged result: they only replace the 250 ms refresh of a
  gated or settled stream, a few frames a second, and a late joiner whose full frame was
  dropped would hold nothing.
- **Dropping array descriptors** from the header (implied by the kind): ≈ 3 % of the session.
- **Coarser steps** (0.05 dB, 0.1°, 10⁻³ γ²) would bring the session under 2 Mbit/s but break
  the group-delay and crosscheck budgets above.
- **Per-client rate negotiation**: the operator prefers a smaller wire to a negotiated one.

## Left

- The session is ≈ 3.3 Mbit/s at 60 fps on noisy fake curves, not under 2: the floor is the
  measurement noise at the precision above (≈ 11–13 bits per column per array after deflate).
  Real curves with high coherence are cheaper.
- The IR view's stream (`ir`: ≈ 13 kB at ≈ 29/s, ≈ 3 Mbit/s) is still the dearest single
  topic. About half is now the ETC (`i16` dB of a noise envelope: ≈ 6.7 kB on the fake
  rig); the IR is circular over 4 096 points, so the pane could take a decimated or
  time-windowed span when zoomed, at the cost of a per-client request.
