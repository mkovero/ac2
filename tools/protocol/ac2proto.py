"""Minimal Python codec for the ac2 protocol (docs/protocol.md).

Independent of the Rust implementation: it is written from the normative document so the
cross-language fixtures prove the document, not just one codebase.
"""

from __future__ import annotations

import math
import re
import struct

import msgpack

PROTO_VERSION = 6
MAX_HEADER_BYTES = 1024
MAX_N = 1 << 16
MAX_ARRAYS = 8
MAX_FRAME_BYTES = 2 << 20
MAX_TOPIC_BYTES = 32

STREAMS = ("tf", "ir", "rta", "spec", "spl", "levels")
_DATA_TOPIC = re.compile(r"^d/(0|[1-9][0-9]*)/(" + "|".join(STREAMS) + r")$")


# Input-meter topics and the frame kind each carries.
_METER_TOPICS = {"session/levels": "session_levels", "session/preview": "preview_levels"}


class DecodeError(Exception):
    pass


# --------------------------------------------------------------------------------------
# Fixture container: u32 LE part count, then per part u32 LE length + bytes.


def write_container(path, parts):
    with open(path, "wb") as f:
        f.write(struct.pack("<I", len(parts)))
        for p in parts:
            f.write(struct.pack("<I", len(p)))
            f.write(p)


def read_container(path):
    with open(path, "rb") as f:
        data = f.read()
    (count,) = struct.unpack_from("<I", data, 0)
    off = 4
    parts = []
    for _ in range(count):
        (n,) = struct.unpack_from("<I", data, off)
        off += 4
        parts.append(data[off : off + n])
        off += n
    if off != len(data):
        raise DecodeError("trailing bytes in container")
    return parts


# --------------------------------------------------------------------------------------
# Topics


def parse_topic(b: bytes):
    if len(b) > MAX_TOPIC_BYTES:
        raise DecodeError("topic too long")
    s = b.decode("utf-8")
    if s in ("timing", "evt", "ka"):
        return {"topic": s}
    if s in _METER_TOPICS:
        return {"topic": s, "kind": _METER_TOPICS[s]}
    m = _DATA_TOPIC.match(s)
    if not m:
        raise DecodeError(f"bad topic {s!r}")
    meas = int(m.group(1))
    if meas >= 1 << 32:
        raise DecodeError("meas id out of range")
    return {"topic": "data", "meas": meas, "stream": m.group(2)}


# --------------------------------------------------------------------------------------
# Grids


def grid_id(g: dict) -> int:
    t = g["type"]
    if t == "log":
        b = b"\x01" + struct.pack("<Iii", g["ppo"], g["k_min"], g["k_max"])
    elif t == "iec_bands":
        frac = {"octave": 1, "third": 3, "sixth": 6, "twelfth": 12, "twenty_fourth": 24}
        b = b"\x02" + struct.pack("<II", frac[g["fraction"]], len(g["centres"]))
        b += b"".join(struct.pack("<d", float(c)) for c in g["centres"])
    elif t == "linear":
        b = b"\x03" + struct.pack("<dI", float(g["fs"]), g["n"])
    else:
        raise ValueError(t)
    h = 0xCBF29CE484222325
    for x in b:
        h = ((h ^ x) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


# --------------------------------------------------------------------------------------
# Frames

_ELEM = {"f32": "f", "u32": "I"}


def encode_frame(topic: str, header: dict, arrays: list) -> list:
    """`arrays` is a list of value lists in the order of header["arrays"]."""
    parts = [topic.encode(), msgpack.packb(header, use_bin_type=True)]
    for desc, values in zip(header["arrays"], arrays, strict=True):
        parts.append(struct.pack("<%d%s" % (len(values), _ELEM[desc["elem"]]), *values))
    return parts


def decode_frame(parts: list) -> dict:
    """Validates sizes before parsing, like the Rust decoder."""
    if len(parts) < 2 or len(parts) > 2 + MAX_ARRAYS:
        raise DecodeError(f"{len(parts)} parts")
    if sum(len(p) for p in parts) > MAX_FRAME_BYTES:
        raise DecodeError("frame too large")
    topic = parse_topic(parts[0])
    if len(parts[1]) > MAX_HEADER_BYTES:
        raise DecodeError("header too large")
    h = msgpack.unpackb(parts[1], raw=False, strict_map_key=True)
    if h.get("v") != PROTO_VERSION:
        raise DecodeError(f"version {h.get('v')!r}")
    n = h["n"]
    if not isinstance(n, int) or n < 0 or n > MAX_N:
        raise DecodeError("n out of range")
    arrays = parts[2:]
    if len(h["arrays"]) != len(arrays):
        raise DecodeError("array count")
    out = {}
    for desc, p in zip(h["arrays"], arrays):
        if len(p) != 4 * n:
            raise DecodeError(f"array {desc['name']}: {len(p)} bytes, expected {4 * n}")
        out[desc["name"]] = list(struct.unpack("<%d%s" % (n, _ELEM[desc["elem"]]), p))
    expected_kind = topic.get("stream", topic.get("kind", topic["topic"]))
    if h["kind"] != expected_kind or list(h["meta"].keys()) != [h["kind"]]:
        raise DecodeError("kind mismatch")
    return {"topic": parts[0].decode(), "header": h, "arrays": out}


# --------------------------------------------------------------------------------------
# JSON-friendly comparison


def to_json(x):
    """msgpack values → JSON-able values (bytes as {"$bin": hex}, floats kept)."""
    if isinstance(x, bytes):
        return {"$bin": x.hex()}
    if isinstance(x, dict):
        return {k: to_json(v) for k, v in x.items()}
    if isinstance(x, (list, tuple)):
        return [to_json(v) for v in x]
    return x


def from_json(x):
    if isinstance(x, dict):
        if list(x.keys()) == ["$bin"]:
            return bytes.fromhex(x["$bin"])
        return {k: from_json(v) for k, v in x.items()}
    if isinstance(x, list):
        return [from_json(v) for v in x]
    return x


def same(a, b, path="$"):
    """Deep equality; NaN equals NaN; ints and floats must match in type family."""
    if isinstance(a, float) and isinstance(b, float):
        if math.isnan(a) and math.isnan(b):
            return
        if a == b and math.copysign(1, a) == math.copysign(1, b):
            return
        raise AssertionError(f"{path}: {a!r} != {b!r}")
    if isinstance(a, dict) and isinstance(b, dict):
        if set(a) != set(b):
            raise AssertionError(f"{path}: keys {sorted(a)} != {sorted(b)}")
        for k in a:
            same(a[k], b[k], f"{path}.{k}")
        return
    if isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            raise AssertionError(f"{path}: len {len(a)} != {len(b)}")
        for i, (x, y) in enumerate(zip(a, b)):
            same(x, y, f"{path}[{i}]")
        return
    if type(a) is not type(b) or a != b:
        raise AssertionError(f"{path}: {a!r} != {b!r}")
