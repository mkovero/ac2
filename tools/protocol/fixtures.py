#!/usr/bin/env python3
"""Cross-language protocol fixtures (Rust ↔ Python).

    fixtures.py gen     write fixtures/protocol/py_*.bin and expected/*.json
    fixtures.py check   decode fixtures/protocol/rust_*.bin and compare with expected/*.json;
                        verify py_*.bin and expected/*.json are what `gen` would write

The values below mirror `crates/ac2-proto/src/samples.rs`. The Rust test
`crates/ac2-proto/tests/fixtures.rs` decodes py_*.bin and compares them with those samples;
this script decodes rust_*.bin and compares them with the dicts here. A field renamed,
re-typed or re-tagged on either side fails one of the two.
"""

from __future__ import annotations

import json
import os
import struct
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import ac2proto as p  # noqa: E402

ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))
FIX = os.path.join(ROOT, "fixtures", "protocol")

NAN = float("nan")
INF = float("inf")


def f32(x):
    return struct.unpack("<f", struct.pack("<f", x))[0]


# --------------------------------------------------------------------------------------
# Shared values (mirror samples.rs)

LOG_GRID = {"type": "log", "ppo": 48, "k_min": -240, "k_max": 239}
BAND_GRID = {
    "type": "iec_bands",
    "fraction": "third",
    "centres": [19.952623149688797, 1000.0, 20000.0],
}
LIN_GRID = {"type": "linear", "fs": 48000.0, "n": 65536}

WEAK_REFERENCE = 1 << 3
THINNED, OUT_OF_BAND, INSUFFICIENT_RESOLUTION = 1 << 0, 1 << 1, 1 << 7
CLIP, HELD = 1 << 0, 1 << 1


def stamp(grid, protection=WEAK_REFERENCE):
    return {
        "seq": 1234,
        "audio_sample": 48000 * 3600,
        "session_epoch": 2,
        "daemon_incarnation": 0x5EED5EED5EED5EED,
        "config_rev": 40,
        "config_applied_at": 48000 * 3500,
        "capture_wall_ns": 1790000000123456789,
        "grid_id": None if grid is None else p.grid_id(grid),
        "protection": protection,
    }


def arr(name, unit, elem="f32"):
    return {"name": name, "unit": unit, "elem": elem}


TIMING_STATUS = {
    "epoch": 3,
    "state": {"type": "locked", "offset": 312},
    "last_lock": {"epoch": 3, "offset": 312, "at_sample": 96000, "at": 1790000000000000000},
    "drift": {"ppm": 0.4, "span": 30.0, "warning": False},
    "internal_reference": True,
}


def frame(topic, kind, grid, meta, arrays, protection=WEAK_REFERENCE):
    """arrays: list of (desc, values). Values are rounded to what the wire carries."""
    n = len(arrays[0][1]) if arrays else 0
    header = {"v": 1, "kind": kind}
    header.update(stamp(grid, protection))
    header["n"] = n
    header["arrays"] = [d for d, _ in arrays]
    header["meta"] = {kind: meta}
    values = {
        d["name"]: [f32(v) if d["elem"] == "f32" else v for v in vals] for d, vals in arrays
    }
    return {"topic": topic, "header": header, "arrays": values}


def tf_frame():
    n = 480

    def val(i, v):
        return v if 4 <= i < 470 else NAN

    return frame(
        "d/1/tf",
        "tf",
        LOG_GRID,
        {
            "delay": 0.0125,
            "frozen": False,
            "smoothing": {"fraction": "sixth", "mode": "power"},
            "mic_curve": True,
        },
        [
            (arr("mag", "db"), [val(i, -6.0 + i * 0.03125) for i in range(n)]),
            (arr("phase", "deg"), [val(i, ((i * 15) % 720) * 0.5 - 180.0) for i in range(n)]),
            (arr("coh", "coherence"), [val(i, (i % 33) / 32.0) for i in range(n)]),
            (arr("eff_avg", "count"), [8.0 + (i % 5) for i in range(n)]),
            (
                arr("validity", "bitmask", "u32"),
                [THINNED if i < 4 else OUT_OF_BAND if i >= 470 else 0 for i in range(n)],
            ),
        ],
    )


CAL_AT = 1_789_000_000_000_000_000

INPUTS = [
    {"channel": 1, "mic": "M30 #1234", "mic_curve": True},
    {"channel": 2, "mic": None, "mic_curve": False},
]


def frames():
    return {
        "tf": tf_frame(),
        "ir": frame(
            "d/1/ir",
            "ir",
            None,
            {"sample_rate": 48000.0, "t0": -0.005, "dt": 1.0 / 12000.0, "inserted_delay": 0.0125},
            [
                (
                    arr("ir_linear", "full_scale"),
                    [0.0, 0.5, -1.0, 0.25, 1.1754943508222875e-38, -0.0],
                ),
                (arr("ir_etc", "db"), [-60.0, -6.0, 0.0, -12.0, -200.0, -200.0]),
            ],
        ),
        "rta": frame(
            "d/3/rta",
            "rta",
            BAND_GRID,
            {
                "fraction": "third",
                "weighting": "z",
                "scale": "db_spl",
                "cal": {"type": "verified", "calibrated_at": CAL_AT},
                "mic_curve": True,
            },
            [
                (arr("level", "db_spl"), [NAN, 74.5, 61.25]),
                (arr("validity", "bitmask", "u32"), [INSUFFICIENT_RESOLUTION, 0, 0]),
            ],
        ),
        "spec": frame(
            "d/5/spec",
            "spec",
            LIN_GRID,
            {"window": "hann", "scale": "dbfs", "cal": {"type": "uncalibrated"}, "mic_curve": False},
            [
                (arr("level", "dbfs"), [-120.0, -20.0, INF, -INF]),
                (arr("validity", "bitmask", "u32"), [0, 0, 0, 0]),
            ],
        ),
        "spl": frame(
            "d/4/spl",
            "spl",
            None,
            {
                "scale": "db_spl",
                "weighting": "a",
                "time_weighting": "fast",
                "peak_weighting": "c",
                "level": 94.1,
                "lmax": 101.3,
                "lmin": 40.2,
                "leq": 92.0,
                "lpeak": 112.7,
                "duration": 60.0,
                "cal": {"type": "other_mic_or_input", "calibrated_at": CAL_AT},
                "mic_curve": True,
            },
            [],
        ),
        "levels": frame(
            "d/1/levels",
            "levels",
            None,
            {"channels": [0, 1]},
            [
                (arr("peak", "dbfs"), [-0.1, -18.0]),
                (arr("rms", "dbfs"), [-12.0, -30.5]),
                (arr("clip", "bitmask", "u32"), [CLIP | HELD, 0]),
            ],
        ),
        "timing": frame(
            "timing",
            "timing",
            None,
            {
                "status": TIMING_STATUS,
                "window": {
                    "capture_start": 96000,
                    "offset": 312,
                    "psr": 24.0,
                    "loopback": -20.5,
                    "stimulus": -20.0,
                },
            },
            [],
        ),
        "ka": frame(
            "ka",
            "ka",
            None,
            {
                "rev": 40,
                "daemon_wall_ns": 1790000000123456789,
                "timing": {"type": "locked", "offset": 312},
                "generator": {"owner": "alice", "armed": True, "firing": True},
            },
            [],
            protection=0,
        ),
    }


TOKEN = bytes.fromhex("0123456789abcdeffedcba9876543210")


def req(idx, op, args=None, mutation=True):
    cmd = {"op": op}
    if args is not None:
        cmd["args"] = args
    return {"v": 1, "id": idx, "cmd": cmd, "expect_rev": 41 if mutation else None}


MEAS_CONFIG = {
    "name": "Main L",
    "kind": {
        "type": "transfer",
        "config": {
            "reference_input": 0,
            "measurement_input": 1,
            "averaging": {"type": "fifo", "blocks": 8},
            "grid": {"ppo": 48, "k_min": -240, "k_max": 239},
            "smoothing": {"fraction": "sixth", "mode": "power"},
            "depth": {"type": "fast_lf", "max_settle_s": 1.0},
        },
    },
}


def arrival(delay_samples, level):
    return {
        "delay": delay_samples / 48000.0,
        "delay_samples": delay_samples,
        "level": level,
        "phase": -12.5,
        "uncertainty_samples": 0.25,
        "misfit": 0.125,
        "refined": True,
    }


def finding():
    """samples::finding(): ambiguous, two near-equal arrivals."""
    return {
        "outcome": {
            "type": "ambiguous",
            "reasons": ["borderline_level", "merged_lobe"],
            "ranked": [arrival(600.0, -1.25), arrival(628.5, 0.0)],
            "strongest": arrival(628.5, 0.0),
        },
        "confidence": {
            "psr_db": 18.5,
            "psr_acq_db": 14.0,
            "band_snr_db": 21.25,
            "excited_fraction": 0.875,
            "uncertainty_samples": 0.25,
            "pulse_width_samples": 9.5,
            "period": None,
        },
        "band": {"type": "custom", "lo_hz": 80.0, "hi_hz": 800.0},
        "observation": 0.5,
        "candidates": [arrival(600.0, -1.25), arrival(628.5, 0.0)],
        "found_at": 1790000000000000000,
    }


def measurement():
    return {
        "id": 1,
        "config": MEAS_CONFIG,
        "config_rev": 40,
        "running": True,
        "frozen": False,
        "delay": {
            "applied": 0.0125,
            "applied_samples": 600,
            "tracking": True,
            "last_finding": finding(),
        },
        "grid_id": p.grid_id(LOG_GRID),
    }


def requests():
    """A subset of samples::commands(); `id` is the index in that list."""
    return [
        req(0, "hello", {"client": "ac2-cli 0.0.0"}, mutation=False),
        req(
            6,
            "gen.set",
            {
                "lease_token": TOKEN,
                "desired": {
                    "settings": {
                        "signal": {"type": "periodic_pink", "period": 131072},
                        "level": -20.0,
                        "band": {"highpass": 30.0, "lowpass": None, "order": "fourth"},
                        "outputs": [0, 1],
                    },
                    "armed": True,
                    "firing": False,
                },
            },
        ),
        req(9, "gen.stop"),
        req(10, "meas.create", {"config": MEAS_CONFIG}),
        req(
            17,
            "delay.find",
            {"meas": 1, "band": {"type": "sub"}, "observation": 8.0},
            mutation=False,
        ),
        req(18, "delay.insert", {"meas": 1, "pick": {"type": "ranked", "index": 1}}),
        req(
            28,
            "trace.import",
            {
                "file_name": "sub.txt",
                "format": "analyzer_text",
                "role": "target",
                "content": b"20 -3.0 10\n",
            },
        ),
        req(
            31,
            "cal.mic_curve",
            {
                "input": 1,
                "mic": "M30 #1234",
                "action": {
                    "type": "import",
                    "file_name": "M30-1234.frd",
                    "content": b"20 -0.5\n20000 1.5\n",
                },
            },
        ),
        req(42, "session.inputs", {"inputs": INPUTS}),
    ]


def events():
    """A subset of samples::events()."""
    return [
        {"kind": "measurement", "rev": 43, "payload": {"type": "set", "value": measurement()}},
        {"kind": "measurement", "rev": 44, "payload": {"type": "deleted", "value": 3}},
        {"kind": "trace", "rev": 46, "payload": {"type": "deleted", "value": 8}},
        {
            "kind": "calibration",
            "rev": 49,
            "payload": {
                "type": "deleted",
                "value": {"device": "hw:UMC1820", "channel": 1, "mic": "M30 #1234"},
            },
        },
        {"kind": "inputs", "rev": 50, "payload": INPUTS},
        {"kind": "timing", "rev": 54, "payload": TIMING_STATUS},
    ]


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
    grids = [LOG_GRID, BAND_GRID, LIN_GRID]
    _dump(
        os.path.join(exp, "grids.json"),
        [{"grid": g, "grid_id": p.grid_id(g)} for g in grids],
    )


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
    for bad in (parts[:1], parts[:-1], parts[:2] + [parts[2][:-1]] + parts[3:]):
        try:
            p.decode_frame(bad)
            ok(False, "malformed frame accepted")
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
