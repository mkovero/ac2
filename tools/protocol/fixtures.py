#!/usr/bin/env python3
"""Cross-language protocol fixtures (Rust ↔ Python).

    fixtures.py gen     write fixtures/protocol/py_*.bin and expected/*.json
    fixtures.py check   decode fixtures/protocol/rust_*.bin and compare with expected/*.json;
                        verify py_*.bin and expected/*.json are what `gen` would write

The values in `fixture_frames.py` and `fixture_messages.py` mirror
`crates/ac2-proto/src/samples/`. The Rust test
`crates/ac2-proto/tests/it/fixtures.rs` decodes py_*.bin and compares them with those samples;
this script decodes rust_*.bin and compares them with those dicts. A field renamed,
re-typed or re-tagged on either side fails one of the two.
"""

from __future__ import annotations

import json
import os
import struct
import sys
import tempfile
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import ac2proto as p  # noqa: E402

ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))
FIX = os.path.join(ROOT, "fixtures", "protocol")

from fixture_frames import (  # noqa: E402
    arr,
    BAND_GRID,
    CAL_AT,
    CANNOT_RECOVER,
    CLIP,
    frame,
    frames,
    HELD,
    INCOMPLETE,
    INF,
    INPUTS,
    INSUFFICIENT_RESOLUTION,
    JUDGED,
    LIMIT,
    LIN_GRID,
    LOG_BINS_GRID,
    LOG_GRID,
    NAN,
    NEAR,
    ON_COURSE,
    OUT_OF_BAND,
    OVER,
    stamp,
    tf_frame,
    THINNED,
    TIMING_STATUS,
    WEAK_REFERENCE,
)
from fixture_messages import (  # noqa: E402
    arrival,
    BAND_LEQ_CONFIG,
    data_requests,
    events,
    finding,
    FINLAND_545_BANDS,
    FINLAND_545_NIGHT,
    _layout_pane,
    MATH_MEASUREMENT,
    MEAS_CONFIG,
    measurement,
    notices,
    PANE_LAYOUT,
    req,
    requests,
    SPL_LOG,
    SPL_MEASUREMENT,
    SWEEP,
    SWEEP_MEASUREMENT,
    TOKEN,
)


# --------------------------------------------------------------------------------------
# gen / check


def _pack(x):
    return p.msgpack.packb(x, use_bin_type=True)


def generate(out):
    exp = os.path.join(out, "expected")
    os.makedirs(exp, exist_ok=True)
    for name, f in frames().items():
        h = f["header"]
        parts = p.encode_frame(f["topic"], h, [f["arrays"][d["name"]] for d in h["arrays"]])
        p.write_container(os.path.join(out, f"py_frame_{name}.bin"), parts)
        _dump(os.path.join(exp, f"frame_{name}.json"), f)
    p.write_container(os.path.join(out, "py_requests.bin"), [_pack(r) for r in requests()])
    _dump(os.path.join(exp, "requests.json"), requests())
    p.write_container(os.path.join(out, "py_events.bin"), [_pack(e) for e in events()])
    _dump(os.path.join(exp, "events.json"), events())
    p.write_container(
        os.path.join(out, "py_data_requests.bin"), [p.encode_data_request(m) for m in data_requests()]
    )
    # Delta chains: the frames of Rust's chain, as Python's keyframe and deltas.
    for name in _chains():
        keys = _chain_keys(name)
        parts = p.encode_key(keys[0])
        for base, c in zip(keys, keys[1:]):
            d = p.encode_delta(c, base)
            if d is None:
                raise SystemExit(f"chain {name}: a frame that does not fit its base")
            parts += d
        p.write_container(os.path.join(out, f"py_chain_{name}.bin"), parts)
    grids = [LOG_GRID, BAND_GRID, LIN_GRID, LOG_BINS_GRID]
    _dump(
        os.path.join(exp, "grids.json"),
        [{"grid": g, "grid_id": p.grid_id(g)} for g in grids],
    )


def _chains():
    pre, suf = "rust_chain_", "_keys.bin"
    return sorted(f[len(pre) : -len(suf)] for f in os.listdir(FIX) if f.startswith(pre) and f.endswith(suf))


def _messages(path):
    parts = p.read_container(path)
    return [parts[i : i + 2] for i in range(0, len(parts), 2)]


def _chain_keys(name):
    out = []
    for m in _messages(os.path.join(FIX, f"rust_chain_{name}_keys.bin")):
        form, c = p.decode_wire(m)
        if form != "key":
            raise p.DecodeError(f"chain {name}: a delta among the keyframes")
        out.append(c)
    return out


def _dump(path, obj):
    with open(path, "w") as f:
        json.dump(p.to_json(obj), f, indent=1, sort_keys=False)
        f.write("\n")


def _load(path):
    with open(path) as f:
        return p.from_json(json.load(f))


def check():
    failures = 0

    def ok(cond, what):
        nonlocal failures
        if not cond:
            failures += 1
            print("FAIL", what)

    # 1. Committed Python outputs are current.
    with tempfile.TemporaryDirectory() as tmp:
        generate(tmp)
        for root, _, files in os.walk(tmp):
            for fn in files:
                rel = os.path.relpath(os.path.join(root, fn), tmp)
                committed = os.path.join(FIX, rel)
                fresh = open(os.path.join(root, fn), "rb").read()
                ok(
                    os.path.exists(committed) and open(committed, "rb").read() == fresh,
                    f"{rel} is stale; run fixtures.py gen",
                )

    # 2. Rust-encoded frames decode to the expected values.
    for name in frames():
        path = os.path.join(FIX, f"rust_frame_{name}.bin")
        try:
            got = p.decode_frame(p.read_container(path))
            p.same(got, _load(os.path.join(FIX, "expected", f"frame_{name}.json")))
        except Exception as e:  # noqa: BLE001
            ok(False, f"rust_frame_{name}: {e}")

    # 2b. Rust's delta chains apply here to the frames of its keyframes; a delta on another
    # frame than its base, or whose result misses its CRC, is refused.
    for name in _chains():
        try:
            keys = _chain_keys(name)
            msgs = _messages(os.path.join(FIX, f"rust_chain_{name}.bin"))
            ok(len(msgs) == len(keys), f"chain {name}: {len(msgs)} messages")
            held = None
            for i, m in enumerate(msgs):
                form, x = p.decode_wire(m)
                ok((form == "key") == (i == 0), f"chain {name} {i}: {form}")
                held = x if form == "key" else p.apply(x, held)
                ok(held["codes"] == keys[i]["codes"], f"chain {name} {i}: codes")
                p.same(held["header"], keys[i]["header"])
                p.same(p.values(held), p.values(keys[i]))
            # The first delta: on the frame after its base, or on its base with one code
            # changed (the last: never a `bfp16` exponent, which a delta may not use).
            _, d = p.decode_wire(msgs[1])
            bad = [(keys[1], "wrong base")]
            if keys[0]["codes"]:
                c = keys[0]["codes"]
                bad.append(({**keys[0], "codes": c[:-1] + bytes([c[-1] ^ 1])}, "patched base"))
            for base, what in bad:
                try:
                    p.apply(d, base)
                    ok(False, f"chain {name}: delta on a {what} accepted")
                except p.DecodeError:
                    pass
        except Exception as e:  # noqa: BLE001
            ok(False, f"rust_chain_{name}: {e}")

    # 3. Rust-encoded ctrl and event messages.
    rust_reqs = [p.msgpack.unpackb(b, raw=False) for b in p.read_container(os.path.join(FIX, "rust_requests.bin"))]
    by_id = {r["id"]: r for r in rust_reqs}
    for r in rust_reqs:
        ok(r["v"] == p.PROTO_VERSION and isinstance(r["cmd"]["op"], str), f"request {r['id']} envelope")
    for want in _load(os.path.join(FIX, "expected", "requests.json")):
        try:
            p.same(by_id[want["id"]], want)
        except Exception as e:  # noqa: BLE001
            ok(False, f"request {want['id']} ({want['cmd']['op']}): {e}")

    for b in p.read_container(os.path.join(FIX, "rust_replies.bin")):
        r = p.msgpack.unpackb(b, raw=False)
        ok(r["v"] == p.PROTO_VERSION and list(r["result"]) in (["Ok"], ["Err"]), f"reply {r.get('id')}")
        ok(isinstance(r["warnings"], list) and all(isinstance(w["type"], str) for w in r["warnings"]),
           f"reply {r.get('id')} warnings")

    rust_evts = {}
    for b in p.read_container(os.path.join(FIX, "rust_events.bin")):
        e = p.msgpack.unpackb(b, raw=False)
        ok(set(e) == {"rev", "kind", "payload"}, f"event {e.get('rev')} shape")
        rust_evts[e["rev"]] = e
    for want in _load(os.path.join(FIX, "expected", "events.json")):
        try:
            p.same(rust_evts[want["rev"]], want)
        except Exception as e:  # noqa: BLE001
            ok(False, f"event {want['rev']}: {e}")

    # Flow control: Rust's data requests and notices.
    try:
        got = [p.decode_data_request(b) for b in p.read_container(os.path.join(FIX, "rust_data_requests.bin"))]
        p.same(got, data_requests())
        parts = p.read_container(os.path.join(FIX, "rust_notices.bin"))
        ok(all(t == p.FLOW_TOPIC for t in parts[0::2]), "notice topics")
        ok([p.decode_notice(b) for b in parts[1::2]] == notices(), "notices")
    except Exception as e:  # noqa: BLE001
        ok(False, f"flow: {e}")
    for bad in ({"type": "window"}, {"type": "credit", "bytes": 1, "events": 0, "x": 1}, {"type": "open"}):
        try:
            p.decode_data_request(p.encode_data_request(bad))
            ok(False, f"bad data request {bad} accepted")
        except p.DecodeError:
            pass

    # 4. Grid ids: Rust-encoded definitions hash to the same ids here.
    rust_grids = [p.msgpack.unpackb(b, raw=False) for b in p.read_container(os.path.join(FIX, "rust_grids.bin"))]
    for g, want in zip(rust_grids, _load(os.path.join(FIX, "expected", "grids.json")), strict=True):
        try:
            p.same(g["grid"], want["grid"])
            ok(g["grid_id"] == want["grid_id"] == p.grid_id(g["grid"]), f"grid id {g['grid']['type']}")
        except Exception as e:  # noqa: BLE001
            ok(False, f"grid {want['grid']['type']}: {e}")

    # 5. The Python decoder refuses malformed frames without crashing.
    parts = p.read_container(os.path.join(FIX, "rust_frame_tf.bin"))
    body = parts[1]
    for bad in (parts[:1], [parts[0], body[:-1]], [parts[0], body[:20]], [parts[0], body + b"\0"],
                parts + [b""], [parts[0], body[:3] + b"\x07" + body[4:]]):
        try:
            p.decode_frame(bad)
            ok(False, "malformed frame accepted")
        except p.DecodeError:
            pass
    # A `bfp16` exponent above 113 would decode to infinity: refused.
    _, ir = p.decode_wire(p.read_container(os.path.join(FIX, "rust_frame_ir.bin")))
    ir["codes"] = bytes([114]) + ir["codes"][1:]
    try:
        p.decode_frame(p.encode_key(ir))
        ok(False, "bfp16 exponent 114 accepted")
    except p.DecodeError:
        pass

    print("ok" if failures == 0 else f"{failures} failure(s)")
    return 1 if failures else 0


def main(argv):
    if argv[1:] == ["gen"]:
        generate(FIX)
        return 0
    if argv[1:] == ["check"]:
        return check()
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
