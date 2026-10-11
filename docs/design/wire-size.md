# Wire size: encoding and compression of data frames

Status: evaluation, superseded by [`wire-slim.md`](wire-slim.md) (16-bit arrays and deflate,
implemented). This file keeps only the findings that decided against the alternatives.

- **CBOR or another binary JSON: not worth it.** msgpack and CBOR are the same family
  (compact integers, raw floats); sizes differ by a few percent either way. The arrays are
  already raw binary and the headers positional, so a switch buys nothing for a protocol bump,
  a new serde crate and new Python fixtures.
- **Measured f32 floats are close to incompressible.** On a synthetic 480-column `tf` frame
  (7 680 B of arrays) zlib, LZ4 and zstd gained 5–15 % on `mag`/`phase`/`coh`: the low
  mantissa bits of a noisy measurement are random, and LZ4 (no entropy coder) slightly
  expanded them. Byte-shuffling (as in Blosc) groups the repeating sign/exponent bytes and
  brought every compressor to ≈ −20 % on the floats, ≈ −40 % on the frame.
- **The near-always-zero `validity` bitmask was a quarter of a `tf` frame**; any compressor
  removes it.
- **CPU does not matter at these sizes** (≈ 8 KB per frame: zstd ≈ 50 µs, LZ4 a few µs on a
  Pi 4), and the decompressed size is bounded by the header, so decompression bombs are not a
  new risk.

Quantising to 16 bits for display was the large win (frames to ≈ 35–40 %); `wire-slim.md`
gives the encoding, its error budget and why stored traces keep f32. Pacing a slow peer, which
bounds latency whatever the frame size, is [`flow-control.md`](flow-control.md).
