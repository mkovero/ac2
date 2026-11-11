# Wire delta: send only what changed

Status: built, stages A–C (PROTO 83; where the build differs is said under each stage); design of 2026-11-03, review round 2. Operator: lean network protocols come
first, "we are after all sending processed audio information over network with reasonably
small sample and bitrates, shouldn't be rocket science"; send only what changed, as video
codes inter frames. Operator decisions: (a) build both, deltas first: stage A deltas over
today's XPUB/SUB, then flow control ([`flow-control.md`](flow-control.md)) in the same round,
before one deploy; (b) no per-kind rate caps: decision 2b stands; (c) PROTO bumps are free,
no compatibility. Builds on [`wire-slim.md`](wire-slim.md) and revisits its "Deltas against
the previous frame" (`wire-slim.md:139`). Decisions W1–W12 are listed at the end.

## Measured

`tuning_session_bytes` (`crates/ac2d/tests/wire_size.rs`, fake rig, 60 fps, the app's
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
| delta against the previous frame | 1 201 | 2 808 | 10 202 | 1 354 | 879 | 62 |
| chain + keyframe every 16 | 1 278 | 2 880 | | | | |
| chain + keyframe every 32 | 1 249 | 2 842 | | | | |
| delta against the last keyframe, K = 16 | 1 920 | 3 659 | | | | |
| delta over two frames | 1 641 | 3 371 | 12 485 | | 906 | |
| dead band ±0.02 dB, ±0.0005 γ², phase 0 | 1 162 | 2 636 | | 1 354 | 880 | 62 |
| one deflate stream per topic | 1 389 | 3 308 | 10 527 | 1 440 | 893 | 61 |
| delta + stream | 1 224 | 2 854 | | | | |

What these say:

- **Transfer functions are where deltas pay (−45 %), and the reason is structural.** The
  columns a deeper MTW stage serves repeat bit for bit until that stage hops: on `d/1/tf`
  ≈ 80 % of frames leave a column of the lowest stage unchanged, ≈ 60 % of the middle stage,
  ≈ 17 % of the full-rate stage. A real rig repeats them just as often.
- **Only a chain works.** A delta grows fast with distance (`tf`: 1 201 B one frame back,
  1 923 B four back, 2 032 B sixteen back), so deltas against the last keyframe keep only
  −12…−21 %. A chain against the previous frame with a keyframe every 16 keeps 92 % of the
  chain's gain (972 → 895 B saved per `tf` frame), every 32 keeps 95 %.
- **Spectra and the IR barely delta** (−3…−21 %): every frame is a fresh FFT or inverse FFT.
- **A dead band buys 3–6 % on fake-rig noise**; a settled real curve is unmeasured.
- **A persistent stream and a delta find the same redundancy**; together they gain nothing.
- **Headers are 115–297 B, ≈ 85 % of it unchanged** within a topic: `tf` 185 B = array
  descriptors 69, meta 69 (only `filled` moves), incarnation 9, grid 9, …; the per-frame part
  (seq, audio sample, wall clock, protection, `filled`) is ≈ 25 B.

## Today's wire, as it bears on deltas

- **Encoding** runs on the job thread (`ac2d/src/jobs/mod.rs:128-139`, `Emitter::send` →
  `encode_frame`), stateless per frame (`ac2-proto/src/frame/encode.rs:52-246`; header
  `:209-224`; one deflated payload part `:242`, level 1 `packing.rs:39`).
- **Conflation**: the I/O thread keeps the newest encoded frame per topic and replaces an
  unsent one (`ac2d/src/io.rs:274-292`). So `seq` (`jobs/mod.rs:131`) is never contiguous on
  the wire: a delta names its base.
- **XPUB, `SNDHWM` 48** (`io.rs:85-99`, `lib.rs:308-309`). A peer at its high-water mark
  silently loses the newest message (`io.rs:362-369`); the daemon cannot know who missed what.
- **Late joiner**: a subscription re-sends every matching slot (`io.rs:200-212`), on an XPUB
  to every peer of that topic.
- **Client**: decodes only the newest message per topic (`ac2-client/src/data.rs:240-275`);
  STALE 1 s after `Kept.received` (`data.rs:28-29`, `:58`), drawn dimmed
  (`ac2-scene/src/theme.rs:68-69`, decision 2a).
- **Captures are daemon-side**: Ctrl+1…9 sends `trace.capture`
  (`ac2-ui/src/conn/request.rs:450-468`), formed from the f32 analysis. Peak hold and the
  spectrograph fold received frames in the app.
- **Pacing**: `Pace` per topic (`jobs/mod.rs:160-235`): on a new result, a protection
  change, and every `REFRESH` 250 ms while unchanged.

## Stage A: chain deltas over the XPUB (W1–W8)

**W1. One chain per topic, shared by all peers, built in the I/O thread at send time.** The
base of a delta is the last frame of that topic actually handed to libzmq, so conflation
(`io.rs:274-292`) never breaks the chain: a frame replaced in its slot was never a base. The
base advances only on a successful hand-off: today `send_data` treats `Ok` and `WouldBlock`
alike (`io.rs:362-369`); on `WouldBlock` the base does not advance and the slot stays dirty,
so the next send round retries the frame, as a keyframe (W4). The job quantises to codes (half the encode cost, still on its
thread) and hands codes to the I/O thread, which forms keyframe or delta and deflates. The
job→I/O outbox therefore carries the codes plus a header struct, no longer encoded parts; the
msgpack and deflate encoding moves to the I/O thread. Cost at `roundtrip::codec_cost`
(37 µs per `tf` encode, 165 µs per IR) for this session at today's rates: ≈ 11 ms per second,
≈ 1 % of a Threadripper core, ≈ 2 % on pupu, 4–5 % on a CM4, once for all peers. If the
I/O thread runs late, an encode worker beside it is the escape hatch, not the jobs.

**W2. Two forms; header and payload in one part, the topic part stays.** A *keyframe* is
today's full header and intra payload. A *delta* has a short header (positional msgpack,
≈ 30 B): `v`, `seq`, `base` (absolute u64 `seq`), `audio_sample`, `capture_wall_ns`,
`protection`, `crc`, and the kind's per-frame meta (`filled` for `tf`, the readings for
`spl`; fixed per kind in `ac2-proto`, `META` in `tools/protocol` the same). Everything else
(`kind`, incarnation, epoch, `config_rev`, `config_applied_at`, `grid_id`, `n`, `arrays`,
static meta) is taken from the keyframe the chain started from. The topic stays the first
part for SUB filtering; header and payload become one part, which saves one part's framing
(CURVE ≈ 33 B, ZMTP 2–9 B, `wire-slim.md`): ≈ 0.09 Mbit/s for this session.

**W3. Delta payload.** Per array: `i16`/`u16` codes minus the base's codes modulo 2¹⁶ (phase
wrapped to ±18 000), 0 unchanged, the same low/high byte planes; `u32` bitmasks XOR the
base; `bfp16` as a mantissa delta only when the base has the same exponent, otherwise that
array intra inside the delta; `f32` (meters, Leq) intra. When the delta is not smaller than
intra the frame goes as a keyframe, so spectra and most IR frames stay keyframes on their own.

**W4. Keyframes:** every ≈ 0.5 s of wall time per topic; on a subscription, instead of today's
slot re-send (`io.rs:200-212`); on a change of any static field or of the epoch; on
`TAG_CLEAR`; when the base does not fit (another `n`, grid or array set); when no base
exists (a new slot's first frame); on the retry after a `WouldBlock` (W1); and on
`Command::DataKeyframe { topic }` (`data.keyframe`, a serde variant on ctrl like
`trace.capture`) from any client, at most one per topic per 250 ms whoever asks. Over the
XPUB a keyframe goes to every peer of the topic: a join costs each of them one keyframe per
matching topic (≈ 29 kB for this session, once). The interval is set by recovery: a client
that lost a delta and could not ask still draws again within 0.5 s, half the STALE threshold,
so a single loss never dims a pane. At ≈ 28 frames/s that is a keyframe every ≈ 14 frames:
7 % of `tf` frames, 92–94 % of the chain's gain kept.

**W5. CRC-32 over the decoded codes, keyframes included.** The arrays' codes in the order of
the keyframe's `arrays`, which the chain inherits (each as little-endian code bytes, `bfp16`
with its exponent byte), not over the deflated bytes: it checks what the client will draw.
The array set is conditional (`ac2-proto` `encode.rs:58-77`: `align` and `range` arrays
only when present), so a change to the set forces a keyframe (W4, base does not fit). The Python codec computes the same. It catches what no sequence check can (an encoder
or decoder bug, a chain applied on a frame the client patched).

**W6. The client applies frames in order, only on an exact base with a matching CRC.** It
decodes every message (no more newest-only, `data.rs:240-275`; a delta decodes in about the
17 µs a frame costs today). A delta whose `base` is not the `seq` it holds, or whose result
fails `crc`, is refused: the client keeps the last applied frame and asks `data.keyframe`
once per `REFRESH`. A refused frame never touches `Kept.received`, so STALE (1 s) dims the
held curve if no keyframe comes: never a wrong curve, and an old one reads as old (decision
2a, no-misreadable-verdicts). Blanking at once would throw away a true reading for a 50 ms
hiccup and flicker the pane on every Wi-Fi drop; drawing a delta on the wrong base is the
only way to show a plausible wrong curve, and the base and CRC checks rule it out.
Recovery: the next keyframe, ≤ 0.5 s, or one RTT plus one frame after a request.
libzmq applies a SUB's filter before its verbose subscribe reaches the daemon, so a new peer
can see deltas before its subscribe-keyframe. The client therefore asks `data.keyframe` only
when a chain it *held* breaks; with no base yet it waits for the keyframe the subscribe
brings. A paused topic answers nothing: no keyframe comes, and STALE dims the held curve.

**W7. Exact deltas; the dead band is deferred** until a replayed pupu capture shows ≥ 15 %
(open question 1). When it comes, its rules: against the reconstructed reference only (a
column moved by no more than the band goes as 0 and the reference keeps the old code, so a
slow drift reaches the band and is sent); `mag`/`level` ±0.02 dB, `coh` ±0.0005; **phase 0**
(±0.02° per column moves the group delay over 1/12 octave by ≈ 100 µs at 20 Hz against a
0.01 ms readout); never across a coherence-mask threshold; NaN/∞ transitions and bitmask bits
exact. Captures are daemon-side and unaffected.

**W8. Per-frame deflate, no persistent stream.** A stream finds the same redundancy as the
chain (−40 % on `tf` against −45 %, nothing more together), and its small-topic gain is the
header W2 shortens. It would add a 32 kB window and compressor state per topic (per peer in
stage C) and the same gap problem, with no way to share encodes between peers.

## Stage B: the range band (W9)

**W9. `range_lo`/`range_hi` as `i16` offsets from the same column's `mag`, `range_coh` as
`u8` hundredths** (`floor(γ² × 100)`, 255 = NaN). `range_coh` is only ever compared with the
coherence mask (`ac2-scene/src/range_band.rs:44-57`); floor keeps that comparison exact for
any threshold in hundredths (0.2996 reads 0.29 and stays masked at 0.3), and the mask steps
stay app policy, not protocol. On the dump the average's intra payload falls 4 155 → ≈ 3 740 B
and its delta 2 808 → 2 605 B (as mask steps it would be 3 330 / 2 488 B; not worth fixing the
thresholds in the protocol).

Built (PROTO 82): `range_lo`/`range_hi` are `i16_mag_offset` (the edge's `i16` code minus the
`mag` code, modulo 2¹⁶, so every code pair is exact), `range_coh` is `u8`, the largest `k`
with `k ÷ 100` as f32 ≤ γ² (a plain `floor(γ² × 100)` reads 0.29 as f32 back as 28). Its
delta is the code minus the base's modulo 2⁸: a held column's γ² repeats, so the byte is 0.
Measured on `wire_size` at 60 fps (whole frames, header included): the average's keyframe
4 470 → ≈ 3 966 B, its delta ≈ 2 900 → ≈ 2 694–2 739 B; the trial's payload predictions
(3 740 / 2 605 B) plus the ≈ 225 B keyframe header and ≈ 90 B delta header. Session 4.62 /
2.18 Mbit/s with / without the IR view (stage A 4.7 / 2.2). Budget for the average:
keyframe < 4 400 B, delta < 3 050 B.

## Stage C: flow control (W10)

**W10. ROUTER (daemon) / DEALER (client) data socket with credit, as flow-control.md, with
these settled before code:**

1. **Credit in bytes.** The client states a window ≥ the largest frame (`IR_FRAME_BUDGET`
   17 kB, `wire_size.rs:224`); default 64 kB; credits batched (flow-control.md, *Cost*).
   Message credit caps throughput at W/RTT messages whatever their size: W = 4 at 50 ms is
   80 messages/s for a session that sends 338, while one IR frame and one `spl` frame would
   cost the same. Bytes cap it at window/RTT (64 kB / 50 ms ≈ 10 Mbit/s) and bound the delay
   to window / link rate.
2. **Credit returned on receipt, not on application.** A refused delta or an old-epoch frame
   still consumed link; withholding its credit would starve the window.
3. **Per (peer, topic): an `Arc` to the codes last sent to that peer**; its delta is taken
   against exactly that, so a slow peer gets deltas over a longer distance, not intra. Encodes
   are cached by (topic, base `seq`) within one send round, so peers in step share one. No
   global ring: a slow peer's base would leave a ring of W and it would get only intra.
   Memory: at most ≈ 0.6 MB per peer (40 topics of an IR's size), mostly `Arc`s shared with
   peers in step. Stage A's chain becomes this per-peer base; keyframe triggers of W4 stay
   (the 0.5 s interval per peer, join per peer only, `data.keyframe` per peer).
4. **Peer lifecycle.** The client never sets a routing id (`socket.rs:388` exists, but a
   fixed id lets a reconnect inherit stale bases): libzmq generates one per connection, so a
   reconnect is a new peer whose first join gets keyframes. A gone peer is dropped by
   `ROUTER_MANDATORY`: its first send fails `EHOSTUNREACH` (item 5). The connection itself
   is closed by `PEER_LIVENESS` (`ac2d/src/lib.rs:102`; ≤ 8 s idle, 15 s unacknowledged).
   The socket monitor (`ac2-zmq/src/monitor.rs:47`) reports a disconnect by endpoint, not
   routing id, so it cannot name the peer; `ZMQ_ROUTER_NOTIFY` would, but it is a draft
   option in libzmq 4.3 and not bound in `ac2-zmq/src/raw.rs`: not relied on. CURVE and ZAP
   on the ROUTER as on ctrl (`ac2-zmq/src/zap.rs:290`, `curve_server_socket`).
5. **`EHOSTUNREACH` means the peer is gone**: drop its state and subscriptions, log once.
6. **Events and keepalives outside credit, in their own bound** (≤ 256 queued per peer;
   past it the peer gets `resync_required`, as flow-control.md). Credit buys frames that a
   newer one can replace; an event cannot be replaced or dropped, and a keepalive must reach
   a starved peer, so neither may wait for a window that frames fill. They are small (4 `ka`
   a second, events on change), so the bound costs no budget. The event bound is also what
   the delay bound tolerates: a peer's standing delay is window / rate plus at most its
   queued events, never more.
7. **The ROUTER's `SNDHWM` holds the whole window by construction**: window ÷ smallest frame
   plus the event bound, ≈ 1 024 for the default window (64 kB ÷ ≈ 85 B, the smallest delta
   frame, ≈ 770, plus 256). Credit then caps what is queued long before libzmq would, so
   `EAGAIN` on a data send is impossible; seen, it is a bug and is logged as one, never a
   silent drop. (flow-control.md's "`SNDHWM` as safety net above W" was written for message
   credit; with byte credit the HWM is sized from the window, not a margin above it.)

A kiosk on a starved link (2 Mbit/s, IR + `tf` + levels): the daemon rotates over the
peer's dirty topics, one message each, the newest slot. A round is ≈ 11.6–12.9 kB (IR delta
10.2 kB one frame back, 12.5 kB two back), so each topic runs at ≈ 19–22 frames/s, and the
standing delay is at most window / rate = 64 kB / 250 kB/s ≈ 0.26 s.

Built (PROTO 83, `ac2d/src/peers.rs`, `chain.rs`, `io.rs`; protocol §5.8). Where it
differs from the items above:

- `data.keyframe` left ctrl: with `subscribe`, `unsubscribe`, `window` and `credit` it is a
  DEALER message (`ac2_proto::flow::DataUp`), since per-peer state is keyed by the data
  connection, which ctrl cannot name, and on the DEALER it stays ordered with the credit.
  The daemon answers with notices on `flow`: `resync_required` (item 6) and `rejoin` (a
  peer dropped as stalled, 60 s silent with messages unacknowledged, that reads again).
- Item 7: libzmq tells the writer what the connection took from its queue only every half
  queue (`lwm = (hwm + 1) / 2`), so the writer may count half a queue too many. `SNDHWM` is
  twice the outstanding bound, with the window clamped to 20 KiB … 256 KiB and every frame
  charged at least 64 B: `2 × (256 KiB ÷ 64 + 1 + 256) = 8 706` messages, sized for the
  largest window rather than the default.
- Item 4: a peer whose topics are quiet would never be sent anything, so `EHOSTUNREACH`
  would never come; such a peer is sent a keepalive each second whatever it subscribes to.
- The delay bound is window ÷ rate plus one rotation round: a frame may wait in its slot
  for its turn before it is sent. Measured on `wire_size` (starved peer, 64 KiB at
  250 kB/s, tuning session with the IR view): median 0.27 s, p95 0.33 s, max ≈ 0.37 s; the
  counted peer's 4.66 Mbit/s as before.
- Credit batching: one `credit` per read burst, and mid-burst whenever half the window is
  owed; the sync worker credits once per drain.

## Budget and tests (W11, W12)

Payload Mbit/s for the `wire_size` tuning session (with / without the IR view), keyframes
every 0.5 s (K ≈ 13–30 by topic rate) amortised, short header 40 B with the topic:

| after stage | with IR | without | what moved |
|---|---|---|---|
| today | 6.39 | 3.39 | |
| A chain deltas over XPUB | 4.60 | 2.20 | `tf` 2 360 → 1 320 B, average 4 450 → 2 970 B, IR 13.0 → 10.4 kB |
| B range band | 4.55 | 2.15 | average 2 970 → 2 770 B |
| C flow control | 4.55 | 2.15 | the same, per peer; a slow peer takes what it consumes |

One part instead of two saves ≈ 0.09 Mbit/s of framing on top. The intra share of `tf` frames
after the first second is 7 % (14 frames per 0.5 s at 28.8/s).

**W11. `wire_size.rs` measures the scheme it budgets.** It counts keyframes and deltas apart.
`slow_tuning_session_fits_a_remote_link` (`:368-401`): < 5.0 Mbit/s with the IR view and
< 2.4 without (from 7.2 / 4.0); per frame keyframe `tf` < 3 000 B, delta `tf` < 1 500 B, IR
< `IR_FRAME_BUDGET`; keyframe share of `tf` after the first second < 10 %.
`tuning_session_bytes` prints the split. Stage C adds a credit-starved peer (64 kB window
drained at 250 kB/s) beside a fast one: the fast peer's totals unchanged, the slow one's
delay ≤ 0.3 s, every topic still served (built: median ≤ window ÷ rate, p95 ≤ (window +
one rotation round) ÷ rate, see *Built* under W10).

**W12. Tests.**

- *Round trips* (`ac2-proto`): keyframe and delta of every kind re-encode to the same bytes;
  delta(base → x) applied to base equals x's codes; `bfp16` exponent change; NaN/∞
  transitions; CRC on keyframes and deltas; a delta on the wrong base or with a bad CRC
  refused without a panic; fuzzed short headers within §5.6's bounds.
- *Lossy link* (`ac2d` it, fake rig): a test subscriber drops N = 1, 3, 30 frames of a topic,
  or every k-th, at the application layer before apply (a loss the daemon cannot see). The
  slow-peer case, where libzmq drops at the high-water mark, uses a tiny `RCVHWM`. Every frame it *holds* is bit-identical to the frame of the same `seq` on a
  lossless peer; it draws again within one keyframe interval, or within one RTT plus one
  frame after `data.keyframe`; STALE only past 1 s; a second request inside 250 ms gets no
  second keyframe. Stage C adds a reconnect (DEALER closed and reopened) and a dead peer.
- *Late joiner*: a second client joins mid-stream and draws its first curve from one
  keyframe; in stage A each existing peer receives one keyframe per matching topic for it
  (one or two: the scheduled 0.5 s keyframe can coincide), in stage C none.
- *Python parity* (`tools/protocol`): keyframe and delta fixtures both ways, CRC included; the
  Python decoder applies a Rust-made chain and matches the codes; `WIRE_LOCK` per stage.
- *App*: `ac2-scene` text of a pane while its topic waits for a keyframe (last curve, its age;
  dimmed past 1 s), from an empty daemon (e2e).

## Stages

Each lands green on its own with its own `PROTO_VERSION` bump, `WIRE_LOCK`, Python codec,
fixtures and `docs/protocol.md` §5. All three land before one deploy (decision a).

1. **A, chain deltas over the XPUB** (W1–W8, W11, W12): codes from the jobs, keyframe/delta
   in the I/O thread, `data.keyframe`, client applying in order, lossy-link tests. Budget
   4.60 / 2.20 Mbit/s.
2. **B, range band** (W9): offsets and `u8` hundredths in proto, codec and `range_band.rs`.
   Budget 4.55 / 2.15.
3. **C, flow control** (W10): ROUTER/DEALER, byte credit, per-peer bases, lifecycle, events
   bound, starved-peer test; all-OS CI (the transport on every OS). Budget the same, per peer.

## Open questions

1. **Dead band on real curves.** Measure stage A's deltas and W7's bands on a replayed pupu
   capture (`session.replay`; private recordings stay local); land it only past ≈ 15 %. On
   the same replay measure the delta-by-distance curve d1…d16 (the fake rig's 1 201 → 2 032 B
   for `tf`), which sets the keyframe interval's cost and a slow peer's in stage C.
2. **IR span.** At ≈ 2.4 Mbit/s the IR is still the dearest topic; a decimated or windowed
   span for a zoomed pane (`wire-slim.md`, *Left*) is a per-client request stage C makes
   possible. Worth it only if a kiosk shows the IR.
3. **ac2-remote** (the Android viewer) decodes frames too. In stage A, updated to the new
   version, it can apply keyframes and ignore deltas: two frames a second per topic, never a
   wrong one. From stage C it goes dark until it implements the DEALER, join and credit.
4. **Rice coding** of the deltas (`wire-slim.md`, *Considered*): deltas are mostly zero and
   small; re-measure once stage A lands.

## Row for open-questions.md

| W | Data frames: one delta chain per topic against the last frame sent (per peer once ROUTER/credit lands), keyframe every 0.5 s, on join and on request (`data.keyframe`); CRC-32 over codes; a client applies only on the exact base, else keeps the last applied frame (STALE rules); byte credit, events outside it. [wire-delta.md](wire-delta.md) |

## Decisions

Operator: (a) deltas first over the XPUB, flow control in the same round before one deploy;
(b) no per-kind rate caps, 2b stands; (c) PROTO bumps free. Taken in review round 1: credit
in bytes; no per-peer topic context, handle or `ctx_rev` (the keyframe is the context);
`range_coh` as `u8` hundredths, no mask thresholds in `ac2-proto`; dead band deferred until a
replayed pupu capture shows ≥ 15 %; no `intra_only` join flag. Taken in review round 2: the
base advances only on a successful hand-off; the ROUTER's `SNDHWM` is sized from the window
(window ÷ smallest frame + event bound), so `EAGAIN` is a bug.

W1 one chain per topic in the I/O thread, base = last frame handed to libzmq · W2 keyframe
and short-header delta, header and payload in one part · W3 delta payload, keyframe when not
smaller · W4 keyframes every 0.5 s, on join, change, clear, misfit, rate-limited
`data.keyframe` · W5 CRC-32 over codes · W6 apply only on the exact base, hold the last
applied frame · W7 exact deltas, dead band deferred · W8 per-frame deflate · W9 range band
offsets and `u8` hundredths · W10 ROUTER/DEALER, byte credit on receipt, per-peer bases,
lifecycle, events bound · W11 `wire_size` budgets · W12 tests.
