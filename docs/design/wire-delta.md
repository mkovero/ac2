# Wire delta: send only what changed

Status: design (2026-11-03), not built. Operator: lean network protocols come first, "we are
after all sending processed audio information over network with reasonably small sample and
bitrates, shouldn't be rocket science"; send only what changed, as video codes inter frames.
PROTO bumps are free pre-1.0; no interim fixes, no compatibility. Builds on
[`wire-slim.md`](wire-slim.md) (16-bit codes, deflate) and implements
[`flow-control.md`](flow-control.md) (credit pacing), which W1 needs. Revisits wire-slim's
"Deltas against the previous frame" (`wire-slim.md:139`): it was right that deltas need
keyframes and per-client state; this note supplies both. Decisions W1–W12 are listed at the end.

## Measured

`tuning_session_bytes` (`crates/ac2d/tests/wire_size.rs:407`, fake rig, 60 fps, the app's
topics with an IR view, `AC2_WIRE_DUMP`), payload bytes without ZMTP/CURVE framing:

| topic | B/frame | frames/s | Mbit/s |
|---|---|---|---|
| `d/1/ir` (4 096 points, linear + ETC) | 13 042 | 28.8 | 3.00 |
| `d/1..3/tf` (480 columns) | ≈ 2 390 | 28.5–28.8 | 3 × 0.55 |
| `d/4/tf` (average of three, with range band) | 4 453 | 25.5 | 0.91 |
| `d/8/spec` (4 096-point FFT) | 1 018 | 46.8 | 0.38 |
| `d/5..7/spec` (65 536-point) | ≈ 1 590 | 5.8 | 3 × 0.07 |
| `d/9/rta`, `session/levels`, 2 × `spl` | 211, 301, 137 | 60, 30, 20 | 0.10, 0.07, 0.04 |
| total | | 338 | **6.39** (3.39 without the IR view) |

Trials on the dump (exact deltas unless said; level-1 raw deflate per frame):

| per frame, B | `tf` | `d/4/tf` | `ir` | `spec` 65 536 | `spec` 4 096 | `rta` |
|---|---|---|---|---|---|---|
| today's payload | 2 173 | 4 155 | 12 913 | 1 467 | 903 | 83 |
| delta against the last frame | 1 201 | 2 808 | 10 202 | 1 354 | 879 | 62 |
| delta, dead band ±0.02 dB, ±0.0005 γ², phase 0 | 1 162 | 2 636 | — | 1 354 | 880 | 62 |
| delta, dead band ±0.05 dB, ±0.001 γ², phase 0 | 1 127 | 2 506 | — | 1 347 | 878 | 62 |
| delta over two frames (half rate) | 1 641 | 3 371 | 12 485 | | 906 | |
| one deflate stream per topic | 1 389 | 3 308 | 10 527 | 1 440 | 893 | 61 |
| delta + stream | 1 224 | 2 854 | | | | |

What these say:

- **Transfer functions are where deltas pay (−45 %), and the reason is structural.** The
  columns a deeper MTW stage serves repeat bit for bit until that stage hops: on `d/1/tf`
  ≈ 80 % of frames leave a column of the lowest stage unchanged, ≈ 60 % of the middle stage,
  ≈ 17 % of the full-rate stage. A real rig repeats them just as often.
- **Spectra and the IR barely delta** (−3…−21 %): every frame is a fresh FFT or a fresh
  inverse FFT, the noise floor new in every sample. Over two frames the IR's delta is as big
  as the frame itself.
- **A dead band buys 3–6 % on fake-rig noise**, whose frame-to-frame steps are well above
  any honest band. A settled real curve steps by less; unmeasured (open question 1).
- **A persistent stream and a delta find the same redundancy**; together they gain nothing.
  On small topics the stream's gain is the header repeating, which W2 removes outright.
- **Headers are 115–297 B, of which ≈ 80 % never changes** within a topic: `tf` 185 B =
  array descriptors 69, meta 69 (only `filled` moves), incarnation 9, grid 9, …; the
  per-frame part (seq, audio sample, wall clock, protection, `filled`) is ≈ 25 B.

## Today's wire, as it bears on deltas

- **Encoding** runs on the job thread (`ac2d/src/jobs/mod.rs:128-139`, `Emitter::send` →
  `encode_frame`), stateless per frame (`ac2-proto/src/frame/encode.rs:52-246`; header
  `:209-224`; one deflated payload part `:242`, level 1 `packing.rs:39`).
- **Conflation**: the I/O thread keeps the newest encoded frame per topic and replaces an
  unsent one (`ac2d/src/io.rs:274-292`), at most one per publish period. So `seq` (per topic,
  `jobs/mod.rs:131`) is never contiguous on the wire: a delta must name its base.
- **XPUB, `SNDHWM` 48** (`io.rs:85-99`, `lib.rs:308-309`, verbose subscriptions). A peer at
  its high-water mark silently loses the newest message (`io.rs:362-369`); the daemon cannot
  know which peer missed what. No back channel except subscription messages.
- **Late joiner**: a subscription sends every matching slot at once (`io.rs:200-212`), on an
  XPUB to *every* peer of that topic, not only the newcomer.
- **Client**: drains the socket and decodes only the newest message per topic
  (`ac2-client/src/data.rs:240-275`); STALE after 1 s without a new frame
  (`data.rs:28-29`), drawn dimmed (`ac2-scene/src/theme.rs:68-69`, decision 2a).
- **Captures are daemon-side**: Ctrl+1…9 sends `trace.capture` (`ac2-ui/src/conn/request.rs:450-468`)
  and the daemon forms the trace from its f32 analysis (`wire-slim.md`, *What stays full
  precision*). Peak hold and the spectrograph fold received frames in the app.
- **Pacing**: `Pace` per topic (`jobs/mod.rs:160-235`): on a new result, on a protection
  change, and every `REFRESH` 250 ms while unchanged. Only `spl` has a rate cap
  (`jobs/spl.rs:278`); `tf`, `ir`, `spec`, `rta` go at the I/O thread's publish rate
  (`transfer.rs:266-267`, `spectrum.rs:263`, `rta.rs:110`); the average at its operands'
  pace (`jobs/math/pace.rs:1-19`).

## 1. Transport (W1)

**W1. The data socket becomes ROUTER (daemon) / DEALER (client) with credit pacing,
exactly as [`flow-control.md`](flow-control.md).** A delta is only as good as the daemon's
knowledge of what each peer holds. Over a ROUTER the daemon itself decides, per peer, which
frame goes; on a live TCP connection nothing queued is lost; credit keeps the queue at W ≈ 4,
so `ROUTER_MANDATORY` never meets the high-water mark. The DEALER is the back channel the
XPUB lacks: credit, join (the topic prefixes, sent again after every reconnect), resync.
Keeping the XPUB would mean a slow peer's silent drops become constant gaps, each answered
by a keyframe sent to all peers: the Wi-Fi kiosk would degrade everyone's wire. The ctrl
ROUTER could carry a resync command, but it is another peer identity (mapping it to a data
peer needs a binding) and queues behind replies; the data DEALER needs neither.

## 2. Frames: context, intra, delta (W2–W4)

**W2. A topic context per peer, then one-part frames.** On join, on resync and whenever a
static field changes, the daemon sends the peer a `ctx` message for the topic: today's full
header minus the per-frame fields (`v`, `kind`, incarnation, epoch, `config_rev`,
`config_applied_at`, `grid_id`, `n`, `arrays`, the static meta) plus a `u16` handle and a
`ctx_rev`. Every frame of the topic is then a single part: handle, short header, payload.
Short header (positional msgpack, ≈ 25–35 B): `seq`, `base` (nil = intra, else the base
`seq`'s distance back), `audio_sample`, `capture_wall_ns`, `protection`, `crc` (CRC-32 of
the frame's codes after decoding), and the kind's *frame meta*: `filled` for `tf`, the
readings for `spl`, nothing for most. Which meta fields are static is fixed per kind in
`ac2-proto` (`HEADER`/`META` in `tools/protocol` the same); a change of a static one is a new
context. The topic string stops travelling: the daemon filters per peer, so it is no longer
a ZMQ prefix part. Under CURVE each part costs ≈ 33 B more (`wire-slim.md`), so one part
instead of three also saves ≈ 65 B per frame on the wire, ≈ 0.18 Mbit/s for this session.

**W3. Payload intra or delta, chosen per frame by size.** Intra is today's payload. Delta is
per array: `i16`/`u16` codes minus the base's codes modulo 2¹⁶ (phase wrapped to
±18 000), 0 for unchanged, in the same low/high byte planes; `u32` bitmasks XOR the base;
`bfp16` as a delta of mantissas only when the base has the same exponent, otherwise that
array goes intra inside the delta; `f32` (meters, Leq) always intra: they are header-sized
noise. The encoder sends the smaller of the two (as a video encoder picks an intra block), so
spectra and the IR stay intra by themselves and no kind is special-cased.

**W4. Bases are frames the peer was sent; one encode per distinct base.** The I/O thread
keeps per topic a ring of the last W (= the credit window) sent frames' codes, shared
(`Arc`), and per peer the last `seq` it sent on the topic. A peer whose last `seq` is in the
ring gets the delta from it, cached by (topic, base) so peers in step share one encode; any
other peer gets intra. Encoding therefore moves out of `Emitter::send`: the job quantises to
codes (half the cost, still on its thread) and the I/O thread deltas and deflates at send,
the only place that knows the bases. Cost at today's `roundtrip::codec_cost` (≈ 37 µs per
`tf` encode, 165 µs per IR): ≈ 1 % of a core per distinct base for this session, the IR at
15/s (W9) included; pupu ≈ 2 %, a CM4 3–4 %. If profiling shows the I/O thread late, the
encode goes to a worker beside it, not back to the jobs.

## 3. Gaps and resync (W5, W6)

**W5. A frame applies only onto its exact base; otherwise the client keeps what it has.**
The client keeps, per topic, the last frame it applied (its codes and `seq`). A frame whose
base is not that `seq`, whose `ctx_rev` is unknown, or whose result fails `crc` is not
applied: the client sends `resync(topic)` on the DEALER and keeps showing the last applied
frame. That frame is a true value with its true capture time, so the existing STALE rule
(1 s after the last *applied* frame, `data.rs:28-29`) dims it if resync takes longer: never a
wrong curve, and the operator sees an old one as old (decision 2a, no-misreadable-verdicts).
Blanking at once would throw away a correct reading for a 50 ms hiccup and flicker the pane
on every Wi-Fi retry; drawing a delta on the wrong base is the one thing that could show a
plausible wrong curve, and the base check and CRC make that impossible. The client applies
every frame in order (no more newest-only decoding, `data.rs:240-275`); decoding a delta costs
what a decode costs today (≈ 17 µs for `tf`).

**W6. Keyframes: on join, context change, resync, or a base that left the ring; no
interval.** The resync request is answered with the next frame of the topic as intra
(ctx first if the client lacks it). Recovery = detection (the next frame, ≤ one period or
the 250 ms refresh) + one RTT + one frame period: 50–100 ms on Wi-Fi. An interval could only
beat that by being shorter than three frames, which spends nearly all of W3's gain; any
longer interval recovers *slower* than the request. On TCP nothing is lost silently except on
reconnect, which the client sees (unknown base or `ctx_rev`) and answers with a join; the CRC
catches what no sequence check can (an encoder or decoder bug). The client re-sends a
resync not answered within `REFRESH`. So the interval is "never", by recovery latency, and
the lossy-link test (§8) holds the latency to it.

## 4. Dead band (W7)

**W7. Deltas are exact in the stages that land first; a dead band comes only if replayed
real sessions show it pays (open question 1), and then on these rules:**

- *Physically honest only against the reconstructed reference.* A column whose code moved
  by no more than the band is sent as 0 and the reference keeps the old code; a slow drift
  therefore reaches the band, is sent, and is never lost. The ring (W4) then holds the chain
  of reconstructed references, one chain per topic, so every peer still holds the same codes.
- *Per quantity, below what any reading resolves:* `mag`/`level` ±2 codes (0.02 dB: a fifth
  of the 0.1 dB readout, under a pixel at 0.03 dB/px on a 4K 60 dB plot); `coh` ±5 codes
  (5·10⁻⁴, a twentieth of the 0.01 readout); **phase 0**: the group delay is the phase slope
  over 1/12 octave, and ±0.02° per column moves it by ≈ 100 µs at 20 Hz against a 0.01 ms
  readout (`wire-slim.md`, group-delay budget); ETC and IR 0 (their log view resolves less
  than a code at the floor).
- *Never across a threshold:* a `coh` change that crosses a `COHERENCE_MASKS` step
  (`ac2-ui/src/state/msg.rs:297`: 0.3, 0.5, 0.7, 0.9) is always sent, so a masked column is
  masked as the daemon's value would be; NaN and ±∞ transitions and every bitmask bit are
  exact.
- *Captures* are daemon-side already (f32 analysis), so they equal the daemon's value
  exactly; the screen at the moment of capture is within the band of it. Peak hold and the
  spectrograph fold received frames and so carry at most the band.

## 5. Compression context (W8)

**W8. Per-frame deflate, no persistent stream.** A stream finds the same inter-frame
redundancy as W3 (−40 % on `tf` against −45 %, nothing more together), and its small-topic
gain is the repeated header W2 removes. It would also make every frame depend on all earlier
ones: the same gap problem as deltas, but with a 32 kB window and compressor state per peer
per topic instead of a ring of codes per topic, and no sharing of encodes between peers
(each peer's stream differs). Deflate stays level 1, raw, as today.

## 6. Rate matched to what is drawn (W9)

**W9. A fixed cap per kind, not per view; the link's own limit is credit (W1).**

- `tf` and the average: at their analysis hop / operands' pace, as now. They are the
  measurement; deltas (W3) already make the unchanged stages cost nothing.
- `ir`: **15/s** (`ir_pace`, `transfer.rs:267`). The IR is the inverse FFT of the full-rate
  stage's *averaged* response, which moves by about one block's share per hop; a picture at
  15/s lags the stream by ≤ 67 ms, about one and a half of that stage's hops, and the
  arrival readout depends on the average, not on frame rate. Its delta over two hops is as big as intra, so
  this halves the IR: 3.0 → 1.5 Mbit/s.
- `spec`: **30/s**, the spectrograph's slot rate (`ac2-scene/src/spectrograph.rs:44`): two
  frames in one slot keep only their maximum, and a 4 096-point window (85 ms at 48 kHz)
  overlaps the next frame by more than half anyway. Long FFTs stay below it on their own.
- `rta`, meters, `spl`: unchanged (meter-like, 60/s and 20/s; 0.2 Mbit/s together).

Per view (a small kiosk pane taking fewer frames) would need per-client rate negotiation,
which the operator turned down (`wire-slim.md`, *Considered*); credit already gives a slow
Wi-Fi peer exactly what it consumes, newest first, and with deltas a skipped frame costs it
an intra only when its base fell out of the ring. This amends decision 2b ("up to 60 fps per
topic") for `ir` and `spec`: needs the operator's yes (row below).

## 7. The range band (W10)

**W10. `range_lo`/`range_hi` as offsets from `mag`, `range_coh` as a mask step.**
`range_coh` is only ever compared with the coherence mask (`ac2-scene/src/range_band.rs:44-57`),
so it goes as a `u8` step: the number of `COHERENCE_MASKS` thresholds it meets, 255 for NaN;
the thresholds move from `ac2-ui` into `ac2-proto` as protocol constants. `range_lo` and
`range_hi` go as `i16` code differences from the same column's `mag` (NaN where either is).
On the dump the band falls from 1.93 kB to 1.28 kB per intra frame (`d/4/tf` 4 048 → 3 395 B
of payload) and the average's delta from 2.81 to 2.49 kB.

## 8. Budget and tests (W11, W12)

Expected for the `wire_size` tuning session, payload Mbit/s (with / without the IR view;
headers of the short form estimated at 40 B with the handle):

| after stage | with IR | without | what moved |
|---|---|---|---|
| today | 6.39 | 3.39 | |
| 1 ROUTER + credit | 6.39 | 3.39 | transport only |
| 2 context + exact deltas | 4.46 | 2.10 | `tf` 2 360 → 1 240 B, headers 115–297 → ≈ 40 B, IR 13.0 → 10.2 kB |
| 3 rates per kind | 3.47 | 1.98 | IR 15/s intra, `spec` 4 096 at 30/s |
| 4 range band | 3.41 | 1.91 | `d/4/tf` 2 850 → 2 530 B |
| 5 dead band (if it lands) | 3.34 | 1.85 | fake rig; real settled curves: to be measured |

Keyframes amortised: a join sends one intra per topic, ≈ 29 kB for the whole session, once;
the test's 4 s run carries it as 0.06 Mbit/s. CURVE framing saved by W2 comes on top
(≈ 0.18 Mbit/s).

**W11. `wire_size.rs` measures the scheme it budgets.** The test subscribes through a
DEALER that grants credit as fast as it reads, and counts intra, delta and `ctx` messages
apart. `slow_tuning_session_fits_a_remote_link` (`:368-401`): totals to < 2.4 Mbit/s without
the IR view and < 4.0 with (from 4.0 / 7.2); per frame, intra `tf` < 3 000 B and IR
< `IR_FRAME_BUDGET` (`:224`) stay, delta `tf` < 1 500 B, the average < 3 000 B; the share
of intra frames after the first second < 5 % on `tf` (an intra storm is a bug, not a cost).
`tuning_session_bytes` prints the split and the keyframe cost. A second case runs a
credit-starved peer (one credit per 100 ms) beside a fast one: the fast peer's totals
unchanged, the slow one never above its consumption.

**W12. Tests.**

- *Round trips* (`ac2-proto`): intra and delta of every kind re-encode to the same bytes;
  delta(base → x) applied to base equals x's codes; `bfp16` exponent change; NaN/∞
  transitions; a delta on the wrong base or a bad CRC refused without a panic; fuzzed short
  headers within §5.6's bounds.
- *Lossy link* (`ac2d` it, fake rig): a test DEALER drops N = 1, 3, 30 consecutive frames
  of a topic, or every k-th, and reorders none (TCP cannot). Checks: every frame the lossy
  client *holds* is bit-identical to the frame of the same `seq` on a lossless peer; a resync
  is answered by intra within one RTT + one period (< 100 ms locally); STALE only past 1 s.
  The same with a reconnect (DEALER dropped and reopened).
- *Late joiner*: a second client joins mid-stream and draws its first curve from one intra,
  while the first client receives no extra frame (today's XPUB re-sends to both).
- *Python parity* (`tools/protocol`): ctx, intra and delta fixtures both ways; the Python
  decoder applies a Rust-made delta chain and matches the codes; `WIRE_LOCK` per stage.
- *App*: `ac2-scene` text of a pane while its topic resyncs (last curve, age; dimmed past
  1 s), from an empty daemon (e2e).

## Stages

Each lands green on its own and bumps `PROTO_VERSION` with fixtures, `WIRE_LOCK`, the Python
codec and `docs/protocol.md` §5 in the same change.

1. **ROUTER data socket + credit** (W1, flow-control.md): events and keepalives on the
   DEALER too, join after reconnect, per-peer dirty set, `wire_size` through the DEALER.
   Frames unchanged. The transport on every OS: dispatch the all-OS CI.
2. **Context + intra/delta + CRC + resync** (W2–W6, W8, W11, W12), exact deltas.
   Encoding to the I/O thread, the client applying in order, Python codec, lossy-link tests.
3. **Rates per kind** (W9): two `Pace::new` arguments and their tests; needs 2b amended.
4. **Range band** (W10): mask steps into `ac2-proto`, `range_band.rs` reads steps.
5. **Dead band** (W7), only if open question 1 shows ≥ 15 % on real sessions.

## Open questions

1. **Dead band on real curves.** Measure stage 2's deltas and W7's bands on a replayed pupu
   capture (`session.replay`, private recordings stay local) with a settled Genelec transfer;
   land stage 5 only past ≈ 15 %.
2. **IR span.** The IR is 1.5 Mbit/s after stage 3, still the dearest topic; a decimated or
   windowed span for a zoomed pane (`wire-slim.md`, *Left*) is a per-client request W1 makes
   possible. Worth it only if a kiosk shows the IR.
3. **ac2-remote** (the Android viewer) decodes frames too: it needs ctx/delta/credit, or a
   join flag `intra_only` that makes the daemon send it intra frames (its bandwidth only).
4. **Rice coding** of the deltas (`wire-slim.md`, *Considered*): deltas are mostly zero and
   small; re-measure once stage 2 lands before reopening.

## Row for open-questions.md

| W | Data frames: per-peer ROUTER/credit; context once, then intra or delta per frame by size, on an exact base with CRC; gap → keep the last applied frame (STALE rules) and resync, no keyframe interval; `ir` 15/s, `spec` 30/s (amends 2b). [wire-delta.md](wire-delta.md) |

## Decisions (proposed)

W1 ROUTER/DEALER data socket with credit · W2 topic context, one-part frames, short header ·
W3 intra or delta per frame by size · W4 bases are sent frames, ring of W, shared encodes,
encoding in the I/O thread · W5 apply only on the exact base, keep the last applied frame,
STALE unchanged · W6 keyframes on join/ctx/resync/ring miss, no interval · W7 exact deltas;
dead band later, honest rules · W8 per-frame deflate · W9 `ir` 15/s, `spec` 30/s, credit
for slow links · W10 range band as offsets and mask steps · W11 `wire_size` budgets ·
W12 tests.
