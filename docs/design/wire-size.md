# Wire size: encoding and compression of data frames

Status: evaluation, not implemented. Wire format today: `docs/protocol.md` §1, §5. Related:
[`flow-control.md`](flow-control.md) (pacing a slow peer, which bounds latency whatever the
frame size).

## Question

Would a different encoding (CBOR or another binary JSON) or compression (zlib, zstd, LZ4)
cut network traffic meaningfully?

## Where the bytes are

- Ctrl replies and events are msgpack maps with named fields. They are small and infrequent;
  only the snapshot on sync is sizeable, and it is sent once per (re)connect.
- A data frame is a topic, a positional msgpack header (≈ 150 B) and raw little-endian
  f32/u32 array parts (`docs/protocol.md` §5.2–5.3).
- A 480-column `tf` frame is ≈ 7.8 KB: four arrays (`mag`, `phase`, `coh`, `validity`) of
  1920 B. At 30 fps remote that is ≈ 0.23 MB/s ≈ 1.9 Mbit/s per measurement; four
  measurements ≈ 8 Mbit/s. A default `spec` frame is ≈ 3.7 KB.

Almost all traffic is the array parts of `tf`, `spec` and `rta` frames.

## CBOR or similar

msgpack and CBOR are the same family: self-describing binary with compact integers and raw
floats. Sizes differ by a few percent either way. The arrays are already raw binary, the
headers already positional. Switching costs a protocol bump, a new serde crate and a rewrite
of the Python fixtures (`tools/protocol`) for no measurable gain. **Not worth doing.**

## Compression

Measured on a synthetic 480-column `tf` frame: smooth magnitude with 0.3 dB noise, a phase
random walk, coherence near 1, `validity` zero except a few out-of-band columns at each end.
Byte-shuffle = transpose the n × 4 bytes so all first bytes come first, then all second
bytes, and so on (as in Blosc). CLI sizes (`lz4 -1`, `zstd -1`) include ≈ 15–20 B of
container overhead that an in-process block API would not have. Real captures will differ;
the ratios are indicative.

| array | raw | zlib 6 | shuffle + zlib 6 | lz4 | shuffle + lz4 | zstd 1 | shuffle + zstd 1 |
|---|---|---|---|---|---|---|---|
| `mag` | 1920 | 1820 | 1609 | 1939 | 1635 | 1818 | 1608 |
| `phase` | 1920 | 1733 | 1538 | 1939 | 1523 | 1723 | 1501 |
| `coh` | 1920 | 1627 | 1430 | 1939 | 1483 | 1610 | 1441 |
| `validity` | 1920 | 30 | 32 | 49 | 53 | 31 | 31 |
| **frame arrays** | 7680 | 5210 (−32 %) | 4609 (−40 %) | ≈ 5870 | ≈ 4690 (−39 %) | 5182 | 4581 (−40 %) |

Findings:

- **Measured floats are close to incompressible.** The low mantissa bits of a noisy
  measurement are random; generic compressors gain 5–15 % on them. LZ4, which has no entropy
  coding, finds no repeated runs at all and slightly expands raw floats.
- **Byte-shuffling is what makes any compressor work.** It groups the sign/exponent bytes,
  which repeat; with it LZ4, zlib and zstd all land at ≈ 20 % on the float arrays.
- **`validity` is most of the lossless gain.** A u32 bitmask that is almost always 0 is a
  quarter of a `tf` frame.
- **CPU and latency do not matter at these sizes.** ≈ 8 KB per frame: zstd ≈ 50 µs, LZ4 a
  few µs on a Pi 4. The decompressed size is already bounded (`n × 4` per part, §5.6), so
  decompression bombs are not a new risk.

## Options, in order of payoff for effort

1. **Compact `validity` (lossless, no dependency).** Omit the part when every column is valid
   (the header says so), or send it as u8. Saves ≈ 25 % of a `tf` frame and ≈ half of an
   `rta` frame without a compressor.
2. **Quantise for display (lossy).** `mag` as i16 at 0.01 dB halves it; delta-coded and
   compressed it went 1920 → 700 B (−64 %). `phase` as i16 at 0.01°, `coh` as u16. Frames
   shrink to ≈ 35–40 % of today's. The cost: the wire is no longer bit-exact with what the
   DSP computed, and captures, A−B and goldens rely on that. Only acceptable as an opt-in
   for remote display-only subscriptions; a capture still needs the f32 values (fetched
   from the daemon, not taken from frames).
3. **Shuffle + LZ4 on the float parts (lossless).** A further ≈ 20 % on `mag`/`phase`/`coh`
   after option 1. Prefer `lz4_flex` (pure Rust, no C link, builds on all three OS) over
   zstd, which links C and buys nothing measurable here. Adds shuffle/compress code to the
   daemon, the client and `tools/protocol/ac2proto.py`.
4. **Fewer frames or columns.** Already the main knob (30 fps remote) and it scales
   linearly; credit pacing (`flow-control.md`) adapts it per peer.

## Recommendation

Not needed until a field report shows remote clients falling behind on bandwidth (as opposed
to latency, which `flow-control.md` addresses). When it is: option 1 first, then option 2 as
an explicit remote mode. Option 3 only if 1 and 2 are not enough. Every option changes what
goes on the wire: `PROTO_VERSION` bump, `fixtures/protocol/WIRE_LOCK`, the Python fixtures
and `docs/protocol.md` §5.7 sizes.
