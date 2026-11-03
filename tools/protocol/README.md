# protocol — cross-language fixtures

Proves that the wire format in `docs/protocol.md` is what both sides speak: frames and
messages encoded by Rust (`crates/ac2-proto`) decode in Python, and frames and messages
encoded by Python decode in Rust. `ac2proto.py` is a small independent Python codec
written from the document (msgpack via the `msgpack` package).

## Setup and use

```
python3 -m venv tools/protocol/.venv                      # gitignored
tools/protocol/.venv/bin/pip install -r tools/protocol/requirements.txt
tools/protocol/.venv/bin/python tools/protocol/fixtures.py check
cargo test -p ac2-proto --test it fixtures::
```

Both must pass; CI runs both (the `protocol` job and the quick tier).

- `fixtures.py check` decodes `fixtures/protocol/rust_*.bin` and compares them with
  `fixtures/protocol/expected/*.json` (frames in full; a subset of requests and events in
  full, the rest structurally; grid ids recomputed in Python), checks that malformed
  frames are refused, and verifies the committed `py_*.bin` / `expected/*.json` are what
  `gen` would write.
- `cargo test --test it fixtures::` checks the committed `rust_*.bin` equal this build's
  encoding and decodes every `py_*.bin` to exactly the values in
  `crates/ac2-proto/src/samples/`.

## After a protocol change

Bump `PROTO_VERSION` first: the update refuses a changed wire under the committed version.

```
AC2_UPDATE_FIXTURES=1 cargo test -p ac2-proto --test it fixtures::   # rewrite rust_*.bin, WIRE_LOCK
tools/protocol/.venv/bin/python tools/protocol/fixtures.py gen   # rewrite py_*.bin, expected/
tools/protocol/.venv/bin/python tools/protocol/fixtures.py check
cargo test -p ac2-proto
```

Edit the Python values in `fixture_frames.py` / `fixture_messages.py` to mirror `samples/`
first; commit code, `docs/protocol.md` and fixtures together.

## Container format (`*.bin`)

u32 LE part count, then for each part a u32 LE length and the bytes. A frame file holds
one keyframe `[topic][header ++ payload]`; a chain file (`*_chain_<kind>.bin`) a keyframe
and then deltas, each message's two parts in turn, and `rust_chain_<kind>_keys.bin` the
same frames as keyframes (Python applies Rust's chain and builds its own from those
keyframes, which Rust applies); `*_requests.bin`, `*_replies.bin`,
`*_events.bin`, `*_data_requests.bin` (a client's data DEALER messages, §5.8) and
`rust_grids.bin` hold one msgpack message per part (`rust_grids.bin`: `{grid, grid_id}`);
`rust_notices.bin` holds the flow notices, `[flow][body]` in turn.
