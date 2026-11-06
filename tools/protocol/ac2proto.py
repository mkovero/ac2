"""Minimal Python codec for the ac2 protocol (docs/protocol.md).

Independent of the Rust implementation: it is written from the normative document so the
cross-language fixtures prove the document, not just one codebase.
"""

from __future__ import annotations

import math
import re
import struct
import zlib

import msgpack

PROTO_VERSION = 84
MAX_HEADER_BYTES = 8192
MAX_N = 1 << 16
MAX_ARRAYS = 12
MAX_PARTS = 2
MAX_FRAME_BYTES = 2 << 20
MAX_TOPIC_BYTES = 32

STREAMS = ("tf", "ir", "rta", "spec", "spl", "leq", "band_leq", "levels")
_DATA_TOPIC = re.compile(r"^d/(0|[1-9][0-9]*)/(" + "|".join(STREAMS) + r")$")


# Input-meter topics and the frame kind each carries.
_METER_TOPICS = {
    "session/levels": "session_levels",
    "session/preview": "preview_levels",
    "session/gonio": "session_gonio",
    "preview/gonio": "preview_gonio",
}


class DecodeError(Exception):
    pass


# --------------------------------------------------------------------------------------
# Flow control on the data socket (§5.8): the client's DEALER messages, the daemon's
# notices, what a frame costs of the window.

FLOW_TOPIC = b"flow"
DATA_WINDOW_DEFAULT = 64 * 1024
DATA_WINDOW_MIN = 20 * 1024
DATA_WINDOW_MAX = 256 * 1024
FRAME_CHARGE_MIN = 64
EVENT_BOUND = 256
NOTICES = ("resync_required", "rejoin")
_DATA_UP = {
    "window": {"bytes"},
    "subscribe": {"sub"},
    "unsubscribe": {"sub"},
    "keyframe": {"topic"},
    "credit": {"bytes", "events"},
}


def frame_charge(nbytes: int) -> int:
    """What a frame message of `nbytes` (every part) costs of the window."""
    return max(nbytes, FRAME_CHARGE_MIN)


def outside_window(topic: bytes) -> bool:
    """An event, keepalive or notice: credited as a message, not by its bytes."""
    return topic in (b"evt", b"ka", FLOW_TOPIC)


def encode_data_request(msg: dict) -> bytes:
    """One DEALER message, `{v, msg: {type, ...}}`."""
    return msgpack.packb({"v": PROTO_VERSION, "msg": msg}, use_bin_type=True)


def decode_data_request(b: bytes) -> dict:
    r = msgpack.unpackb(b, raw=False)
    if not isinstance(r, dict) or r.get("v") != PROTO_VERSION:
        raise DecodeError("data request: version")
    m = r.get("msg")
    if set(r) != {"v", "msg"} or not isinstance(m, dict):
        raise DecodeError("data request: shape")
    t = m.get("type")
    if t not in _DATA_UP or set(m) != _DATA_UP[t] | {"type"}:
        raise DecodeError(f"data request: {t!r}")
    return m


def decode_notice(body: bytes) -> str:
    """A notice's body (the part after `flow`): its type."""
    r = msgpack.unpackb(body, raw=False)
    if not isinstance(r, dict) or r.get("v") != PROTO_VERSION or set(r) != {"v", "msg"}:
        raise DecodeError("notice: version or shape")
    m = r["msg"]
    if not isinstance(m, dict) or set(m) != {"type"} or m["type"] not in NOTICES:
        raise DecodeError("notice: type")
    return m["type"]


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
    elif t == "log_bins":
        b = b"\x04" + struct.pack("<dII", float(g["fs"]), g["n"], g["ppo"])
    else:
        raise ValueError(t)
    h = 0xCBF29CE484222325
    for x in b:
        h = ((h ^ x) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


# --------------------------------------------------------------------------------------
# Frame header layout (docs/protocol.md §5.3): msgpack arrays, fields in this order. A type
# is None (a plain value), ("opt", t), ("list", t), a list of (field, type) for a struct,
# ("tagged", {variant: fields}) for a `type`-tagged enum (array: variant name, then its
# fields), or ("meta", {kind: t}) for the one-key metadata map.

_SMOOTHING = [("fraction", None), ("mode", None)]
_MIC_LOADING = [("mic_impedance", None), ("input_impedance", None)]
_CAL_BASIS = ("tagged", {
    "acoustic": [("calibrator_level", None)],
    "electrical": [("connection", None), ("mic_sensitivity", None), ("data_sheet", None),
                   ("loading", ("opt", _MIC_LOADING)), ("uncertainty", None)],
})
_CAL = ("tagged", {
    "uncalibrated": [],
    "verified": [("calibrated_at", None), ("basis", _CAL_BASIS)],
    "other_mic_or_input": [("calibrated_at", None), ("basis", _CAL_BASIS)],
})
_TIMING_STATE = ("tagged", {
    "no_stimulus": [],
    "acquiring": [],
    "locked": [("offset", None)],
    "jumped": [("from", None), ("to", None)],
    "lost": [],
})
_TIMING_STATUS = [
    ("epoch", None),
    ("state", _TIMING_STATE),
    ("last_lock", ("opt", [("epoch", None), ("offset", None), ("at_sample", None), ("at", None)])),
    ("drift", ("opt", [("ppm", None), ("span", None), ("warning", None), ("at", None)])),
    ("internal_reference", None),
]
_LEVELS = [("channels", None)]
# `InputKind` is tagged by `kind`, not `type`: its variants name what is on the input.
_INPUT_KIND = ("kinded", {
    "mic": [("mic", None), ("curve", ("tagged", {"not_chosen": [], "off": [], "curve": [("label", None)]}))],
    "line": [],
    "digital": [],
})
_PREVIEW_INPUT = [("channel", None), ("kind", _INPUT_KIND)]
_PAIR = [("left", None), ("right", None), ("name", None)]
_OPERAND = ("tagged", {"meas": [("meas", None)], "trace": [("trace", None)]})
_OPERAND_STATUS = ("tagged", {
    "included": [],
    "stopped": [],
    "settling": [],
    "refused": [("protection", None)],
    "mismatch": [],
})
_ALIGN_CANDIDATE = [("delay", None), ("delayed", None), ("inverted", None), ("residual_deg", None),
                    ("score", None)]
_SUMMATION = [("mean_db", None), ("worst_db", None)]
_ALIGN = ("opt", [("outcome", ("tagged", {
    "fit": [("fit", [("best", _ALIGN_CANDIDATE), ("other_polarity", ("opt", _ALIGN_CANDIDATE)),
                     ("runner_up", ("opt", _ALIGN_CANDIDATE)), ("before", _SUMMATION),
                     ("after", _SUMMATION), ("columns", None)])],
    "no_fit": [("why", ("tagged", {"few_operands": [], "few_columns": [("found", None)],
                                   "no_weight": []}))],
}))])
_MATH = ("opt", [("operands", ("list", [("operand", _OPERAND), ("status", _OPERAND_STATUS)])),
                 ("phase", None), ("align", _ALIGN),
                 ("range", ("opt", ("tagged", {"too_few": [], "envelope": [("positions", None)],
                                               "percentiles": [("positions", None)]})))])
_POSITION = [("level", None), ("peak", None)]
_PEAK = [("level", None), ("judgement", None)]
_PAIR_LEVELS = [("left", None), ("right", None), ("correlation", None), ("quiet", None)]
_LOUDNESS = [("source", ("tagged", {"pair": [("left", None), ("right", None)],
                                     "input": [("channel", None)]})),
             ("momentary", None), ("short_term", None), ("integrated", None), ("range", None),
             ("max_momentary", None), ("max_short_term", None), ("max_true_peak", None),
             ("duration", None), ("since", None)]
_GONIO_PAIR = [("left", None), ("right", None), ("gain_dbfs", None)]
META = {
    "tf": [("delay", None), ("nudged", None), ("smoothing", ("opt", _SMOOTHING)), ("mic_curve", None),
           ("math", _MATH),
           ("unresolved", ("opt", [("resolution", None), ("ranges", ("list", [("lo", None), ("hi", None)]))])),
           ("filled", None)],
    "ir": [("sample_rate", None), ("t0", None), ("dt", None), ("inserted_delay", None)],
    "rta": [("fraction", None), ("weighting", None), ("scale", None), ("cal", _CAL), ("mic_curve", None),
            ("math", _MATH)],
    "spec": [("window", None), ("scale", None), ("level", None), ("cal", _CAL), ("mic_curve", None), ("smoothing", None),
             ("math", _MATH),
             ("bands", ("opt", [("bands", ("list", [("window", None), ("hop", None), ("sole", None),
                                                     ("age", None)])),
                                ("unresolved", [("resolution", None),
                                                ("ranges", ("list", [("lo", None), ("hi", None)]))])]))],
    "spl": [("scale", None), ("weighting", None), ("time_weighting", None), ("peak_weighting", None),
            ("level", None), ("lmax", None), ("lmin", None), ("leq", None), ("lpeak", None),
            ("duration", None), ("cal", _CAL), ("mic_curve", None), ("position", ("opt", _POSITION))],
    "leq": [("scale", None), ("cal", _CAL), ("mic_curve", None), ("horizon", None), ("logged", None),
            ("run", ("opt", [("started_at", None), ("until", None), ("measured", None), ("gaps", None),
                             ("trimmed", None), ("laeq", None), ("lceq", None), ("lzeq", None)])),
            ("lcpeak", ("opt", _PEAK)), ("lafmax", ("opt", _PEAK)), ("position", ("opt", _POSITION))],
    "band_leq": [("scale", None), ("cal", _CAL), ("mic_curve", None), ("horizon", None),
                 ("correction", None), ("limits_from", None),
                 ("windows", ("list", [("band", None), ("duration", None), ("weighting", None), ("elapsed", None),
                                       ("measured", None), ("period", None), ("period_after_horizon", None)])),
                 ("predicted", ("opt", [("duration", None), ("estimate", None), ("at_most", None),
                                        ("limit", None), ("judgement", None)]))],
    "levels": _LEVELS,
    "session_levels": [("channels", None), ("ballistic", None), ("pair_levels", ("list", _PAIR_LEVELS)),
                       ("loudness", ("list", _LOUDNESS))],
    "preview_levels": [("backend", None), ("device", None), ("channels", None),
                       ("inputs", ("list", _PREVIEW_INPUT)), ("pairs", ("list", _PAIR)),
                       ("ballistic", None), ("pair_levels", ("list", _PAIR_LEVELS)),
                       ("loudness", ("list", _LOUDNESS))],
    "session_gonio": [("pairs", ("list", _GONIO_PAIR))],
    "preview_gonio": [("pairs", ("list", _GONIO_PAIR))],
    "timing": [("status", _TIMING_STATUS),
               ("window", ("opt", [("capture_start", None), ("offset", None), ("psr", None),
                                   ("loopback", None), ("stimulus", None)]))],
    "ka": [("rev", None), ("daemon_wall_ns", None), ("timing", _TIMING_STATE),
           ("generator", [("owner", None), ("armed", None), ("firing", None)]),
           ("health", [("host", None), ("cpu", None),
                       ("audio", ("opt", [("load", None), ("peak", None), ("session_load", None),
                                          ("session_peak", None), ("xruns", None)]))])],
}
HEADER = [
    ("v", None), ("form", None), ("kind", None), ("seq", None), ("audio_sample", None), ("session_epoch", None),
    ("daemon_incarnation", None), ("config_rev", None), ("config_applied_at", None),
    ("capture_wall_ns", None), ("grid_id", None), ("protection", None), ("n", None),
    ("arrays", ("list", [("name", None), ("unit", None), ("elem", None)])),
    ("meta", ("meta", META)),
    ("crc", None),
]
FORM_KEY, FORM_DELTA = 0, 1
# What a delta carries of the metadata, per kind; every other kind nothing (nil).
DELTA_META = {
    "tf": [("filled", None)],
    "spl": [("level", None), ("lmax", None), ("lmin", None), ("leq", None), ("lpeak", None),
            ("duration", None)],
}
DELTA_HEADER = [
    ("v", None), ("form", None), ("seq", None), ("base", None), ("audio_sample", None),
    ("capture_wall_ns", None), ("protection", None), ("crc", None), ("meta", None),
]
# Header fields a delta inherits from its base.
_STATIC = ("kind", "session_epoch", "daemon_incarnation", "config_rev", "config_applied_at",
           "grid_id", "n", "arrays")


def positional(t, x):
    """Named (dict) value → its wire layout."""
    if t is None or x is None:
        return x
    if isinstance(t, list):
        if set(x) != {f for f, _ in t}:
            raise ValueError(f"fields {sorted(x)}")
        return [positional(ft, x[f]) for f, ft in t]
    tag, sub = t
    if tag == "opt":
        return positional(sub, x)
    if tag == "list":
        return [positional(sub, v) for v in x]
    if tag in ("tagged", "kinded"):
        key = "type" if tag == "tagged" else "kind"
        fields = sub[x[key]]
        return [x[key]] + positional(fields, {k: v for k, v in x.items() if k != key})
    if tag == "meta":
        ((k, v),) = x.items()
        return {k: positional(sub[k], v)}
    raise ValueError(tag)


def named(t, x):
    """Wire layout → named (dict) value; DecodeError when it does not fit."""
    if t is None or x is None:
        return x
    if isinstance(t, list):
        if not isinstance(x, list) or len(x) != len(t):
            raise DecodeError(f"expected {len(t)} fields, got {x!r}")
        return {f: named(ft, v) for (f, ft), v in zip(t, x)}
    tag, sub = t
    if tag == "opt":
        return named(sub, x)
    if tag == "list":
        if not isinstance(x, list):
            raise DecodeError(f"expected a list, got {x!r}")
        return [named(sub, v) for v in x]
    if tag in ("tagged", "kinded"):
        if not isinstance(x, list) or not x or not isinstance(x[0], str) or x[0] not in sub:
            raise DecodeError(f"bad tagged value {x!r}")
        key = "type" if tag == "tagged" else "kind"
        return {key: x[0], **named(sub[x[0]], x[1:])}
    if tag == "meta":
        if not isinstance(x, dict) or len(x) != 1 or not isinstance(next(iter(x)), str) or next(iter(x)) not in sub:
            raise DecodeError(f"bad meta {x!r}")
        ((k, v),) = x.items()
        return {k: named(sub[k], v)}
    raise ValueError(tag)


# --------------------------------------------------------------------------------------
# Frames

_ELEM_SIZE = {"f32": 4, "u32": 4, "i16": 2, "i16_mag_offset": 2, "u16": 2, "u8": 1, "bfp16": 2, "i8": 1}
BFP_EXP_MIN, BFP_EXP_MAX, BFP_PEAK = -128, 113, 32765.0


def array_size(elem: str, n: int) -> int:
    """A `bfp16` array leads with its exponent byte."""
    return n * _ELEM_SIZE[elem] + (1 if elem == "bfp16" else 0)
I16_NAN, I16_NEG_INF, I16_POS_INF = -32768, -32767, 32767
U16_NAN, U16_ONE = 65535, 10000


def _round_half_up(x: float) -> int:
    return math.floor(x + 0.5)


def to_i16(x: float, deg: bool) -> int:
    """Hundredths of the unit; degrees wrapped to [-180, 180) on the hundredths."""
    if math.isnan(x) or (deg and math.isinf(x)):
        return I16_NAN
    if deg:
        return (_round_half_up(x * 100.0) + 18000) % 36000 - 18000
    if x == math.inf:
        return I16_POS_INF
    if x == -math.inf:
        return I16_NEG_INF
    return max(-32766, min(32766, _round_half_up(x * 100.0)))


def from_i16(q: int) -> float:
    return {I16_NAN: math.nan, I16_NEG_INF: -math.inf, I16_POS_INF: math.inf}.get(q, q / 100.0)


def _code_i16(x: float) -> int:
    if math.isnan(x):
        return I16_NAN
    if x == math.inf:
        return I16_POS_INF
    if x == -math.inf:
        return I16_NEG_INF
    return max(-32766, min(32766, _round_half_up(x)))


def bfp_exponent(values: list) -> int:
    """Smallest e in [-128, 113] with every finite |x| * 2^-e <= 32765; 0 without a peak."""
    peak = max((abs(v) for v in values if math.isfinite(v)), default=0.0)
    if peak == 0.0:
        return 0
    _, k = math.frexp(peak)
    e = k - 14 if math.ldexp(peak, 15 - k) > BFP_PEAK else k - 15
    return max(BFP_EXP_MIN, min(BFP_EXP_MAX, e))


def _f32(v: float) -> float:
    return struct.unpack("<f", struct.pack("<f", v))[0]


def from_bfp(q: int, e: int) -> float:
    if q in (I16_NAN, I16_NEG_INF, I16_POS_INF):
        return from_i16(q)
    # Rounded to f32 like the Rust decoder (exact unless among the f32 subnormals).
    return _f32(math.ldexp(q, e))


def to_u16(x: float) -> int:
    if math.isnan(x):
        return U16_NAN
    return max(0, min(U16_ONE, _round_half_up(x * U16_ONE)))


def from_u16(q: int) -> float:
    if q == U16_NAN:
        return math.nan
    if q > U16_ONE:
        raise DecodeError(f"undefined coherence code {q}")
    return q / U16_ONE


U8_NAN, U8_ONE = 255, 100


def _u8_value(q: int) -> float:
    """q / 100 as the nearest f32 (the Rust decoder divides in f32)."""
    return _f32(q / U8_ONE)


def to_u8(x: float) -> int:
    """The largest q in 0..100 whose value is at most x (as f32): hundredths rounded down,
    so a mask in hundredths compares the same on the code as on x."""
    if math.isnan(x):
        return U8_NAN
    x = _f32(x)
    q = max(0, min(U8_ONE, math.floor(x * U8_ONE))) if math.isfinite(x) else (U8_ONE if x > 0 else 0)
    if q < U8_ONE and _u8_value(q + 1) <= x:
        q += 1
    elif q > 0 and _u8_value(q) > x:
        q -= 1
    return q


def from_u8(q: int) -> float:
    if q == U8_NAN:
        return math.nan
    if q > U8_ONE:
        raise DecodeError(f"undefined coherence code {q}")
    return _u8_value(q)


def wire_value(desc: dict, v, array=None):
    """What `v` decodes to after a trip over the wire in `desc`'s element encoding; a
    `bfp16` value needs its whole `array` for the exponent."""
    e = desc["elem"]
    if e == "bfp16":
        x = bfp_exponent([_f32(a) for a in array])
        return from_bfp(_code_i16(math.ldexp(_f32(v), -x)), x)
    if e in ("i16", "i16_mag_offset"):
        return from_i16(to_i16(v, desc["unit"] == "deg"))
    if e == "u16":
        return from_u16(to_u16(v))
    if e == "u8":
        return from_u8(to_u8(v))
    if e == "f32":
        return struct.unpack("<f", struct.pack("<f", v))[0]
    return v


def _le16(codes: list) -> bytes:
    return struct.pack("<%dH" % len(codes), *codes)


def _unle16(b: bytes) -> list:
    return list(struct.unpack("<%dH" % (len(b) // 2), b))


def _planes(codes: list) -> bytes:
    """Neighbour differences (mod 2^16), low-byte plane then high-byte plane."""
    prev, d = 0, []
    for q in codes:
        d.append((q - prev) & 0xFFFF)
        prev = q
    return _raw_planes(d)


def _unplanes(b: bytes) -> list:
    prev, out = 0, []
    for d in _unraw_planes(b):
        prev = (prev + d) & 0xFFFF
        out.append(prev)
    return out


def _raw_planes(v: list) -> bytes:
    return bytes(x & 0xFF for x in v) + bytes(x >> 8 for x in v)


def _unraw_planes(b: bytes) -> list:
    n = len(b) // 2
    return [b[i] | b[n + i] << 8 for i in range(n)]


def _codes(desc: dict, values: list) -> bytes:
    """One array's codes: little-endian, a `bfp16` array's exponent byte first; an
    `i16_mag_offset` array as its `i16` codes (`coded` takes them relative to `mag`)."""
    e = desc["elem"]
    if e in ("i16", "i16_mag_offset"):
        return _le16([to_i16(v, desc["unit"] == "deg") & 0xFFFF for v in values])
    if e == "u16":
        return _le16([to_u16(v) for v in values])
    if e == "u8":
        return bytes(to_u8(v) for v in values)
    if e == "bfp16":
        values = [_f32(v) for v in values]
        x = bfp_exponent(values)
        return struct.pack("<b", x) + _le16([_code_i16(math.ldexp(v, -x)) & 0xFFFF for v in values])
    return struct.pack("<%d%s" % (len(values), {"f32": "f", "u32": "I", "i8": "b"}[e]), *values)


def _values(desc: dict, b: bytes) -> list:
    e = desc["elem"]
    s16 = lambda q: q - 0x10000 if q >= 0x8000 else q  # noqa: E731
    if e in ("i16", "i16_mag_offset"):
        return [from_i16(s16(q)) for q in _unle16(b)]
    if e == "u16":
        return [from_u16(q) for q in _unle16(b)]
    if e == "u8":
        return [from_u8(q) for q in b]
    if e == "bfp16":
        if not b:
            raise DecodeError("bfp16 array without its exponent")
        x = struct.unpack_from("<b", b)[0]
        if x > BFP_EXP_MAX:
            raise DecodeError(f"bfp16 exponent {x}")
        return [from_bfp(s16(q), x) for q in _unle16(b[1:])]
    size, code = {"f32": (4, "f"), "u32": (4, "I"), "i8": (1, "b")}[e]
    return list(struct.unpack("<%d%s" % (len(b) // size, code), b))


def _intra(desc: dict, c: bytes) -> bytes:
    """Keyframe form of one array's codes."""
    e = desc["elem"]
    if e in ("i16", "i16_mag_offset", "u16"):
        return _planes(_unle16(c))
    if e == "u8":
        return bytes((q - p) & 0xFF for p, q in zip(b"\0" + c, c))
    if e == "bfp16":
        return c[:1] + _planes(_unle16(c[1:]))
    return c


def _unintra(desc: dict, w: bytes) -> bytes:
    e = desc["elem"]
    if e in ("i16", "i16_mag_offset", "u16"):
        return _le16(_unplanes(w))
    if e == "u8":
        prev, out = 0, bytearray()
        for d in w:
            prev = (prev + d) & 0xFF
            out.append(prev)
        return bytes(out)
    if e == "bfp16":
        return w[:1] + _le16(_unplanes(w[1:]))
    return w


DEG_HALF, DEG_TURN = 18000, 36000


def _signed(q: int) -> int:
    return q - 0x10000 if q >= 0x8000 else q


def _is_angle(q: int) -> bool:
    return -DEG_HALF <= q < DEG_HALF


def _angle_delta(x: int, b: int):
    """Signed codes; the short way round; None: `x` is no code an angle array holds."""
    if not _is_angle(b):
        return (x - b) & 0xFFFF
    if x == I16_NAN:
        return I16_NAN & 0xFFFF
    if not _is_angle(x):
        return None
    return ((x - b + DEG_HALF) % DEG_TURN - DEG_HALF) & 0xFFFF


def _angle_apply(d: int, b: int) -> int:
    if not _is_angle(b):
        return (b + d) & 0xFFFF
    d = _signed(d)
    if d == I16_NAN:
        return I16_NAN & 0xFFFF
    return ((b + d + DEG_HALF) % DEG_TURN - DEG_HALF) & 0xFFFF


def _delta(desc: dict, c: bytes, base: bytes):
    """Delta form of one array's codes against the base's; None: it must go in a keyframe."""
    e = desc["elem"]
    if e == "i16" and desc["unit"] == "deg":
        d = [_angle_delta(_signed(x), _signed(b)) for x, b in zip(_unle16(c), _unle16(base))]
        return None if None in d else _raw_planes(d)
    if e in ("i16", "i16_mag_offset", "u16"):
        return _raw_planes([(x - b) & 0xFFFF for x, b in zip(_unle16(c), _unle16(base))])
    if e == "u8":
        return bytes((x - b) & 0xFF for x, b in zip(c, base))
    if e == "bfp16":
        if c[:1] == base[:1]:
            return c[:1] + _raw_planes([(x - b) & 0xFFFF
                                        for x, b in zip(_unle16(c[1:]), _unle16(base[1:]))])
        return _intra(desc, c)
    if e == "u32":
        return bytes(x ^ b for x, b in zip(c, base))
    return c


def _undelta(desc: dict, w: bytes, base: bytes) -> bytes:
    e = desc["elem"]
    if e == "i16" and desc["unit"] == "deg":
        return _le16([_angle_apply(d, _signed(b)) for d, b in zip(_unraw_planes(w), _unle16(base))])
    if e in ("i16", "i16_mag_offset", "u16"):
        return _le16([(b + d) & 0xFFFF for d, b in zip(_unraw_planes(w), _unle16(base))])
    if e == "u8":
        return bytes((b + d) & 0xFF for d, b in zip(w, base))
    if e == "bfp16":
        if w[:1] == base[:1]:
            return w[:1] + _le16([(b + d) & 0xFFFF
                                  for d, b in zip(_unraw_planes(w[1:]), _unle16(base[1:]))])
        return _unintra(desc, w)
    if e == "u32":
        return bytes(x ^ b for x, b in zip(w, base))
    return w


def _deflate(raw: bytes) -> bytes:
    c = zlib.compressobj(1, zlib.DEFLATED, -15)
    return c.compress(raw) + c.flush()


def _inflate(payload: bytes, expected: int) -> bytes:
    z = zlib.decompressobj(-15)
    try:
        raw = z.decompress(payload, expected + 1)
    except zlib.error as e:
        raise DecodeError(f"payload: {e}") from e
    if len(raw) != expected or not z.eof or z.unused_data:
        raise DecodeError(f"payload does not inflate to {expected} bytes")
    return raw


def _split(descs: list, n: int, b: bytes) -> list:
    out, off = [], 0
    for d in descs:
        k = array_size(d["elem"], n)
        out.append(b[off : off + k])
        off += k
    return out


def coded(topic: str, header: dict, arrays: list) -> dict:
    """A frame as codes: `header` the named keyframe header without `form` and `crc`,
    `arrays` value lists in the order of header["arrays"]."""
    parts = [_codes(d, v) for d, v in zip(header["arrays"], arrays, strict=True)]
    _on_mag(header["arrays"], parts, -1)
    return {"topic": topic, "header": header, "codes": b"".join(parts)}


def _on_mag(descs: list, parts: list, sign: int):
    """`i16_mag_offset` arrays relative to `mag`'s codes (sign -1) or back (+1), in place."""
    mag = next((p for d, p in zip(descs, parts) if d["name"] == "mag"), None)
    for i, d in enumerate(descs):
        if d["elem"] == "i16_mag_offset":
            if mag is None:
                raise DecodeError(f"{d['name']} needs a mag array")
            parts[i] = _le16([(x + sign * m) & 0xFFFF for x, m in zip(_unle16(parts[i]), _unle16(mag))])


def values(c: dict) -> dict:
    """The arrays of coded frame `c`, by name."""
    h = c["header"]
    parts = _split(h["arrays"], h["n"], c["codes"])
    _on_mag(h["arrays"], parts, 1)
    return {d["name"]: _values(d, b) for d, b in zip(h["arrays"], parts)}


def _body(header: list, payload: bytes) -> bytes:
    return msgpack.packb(header, use_bin_type=True) + payload


def encode_key(c: dict) -> list:
    """A keyframe: `[topic, header ++ deflated intra payload]`."""
    h = c["header"]
    wire = {"v": PROTO_VERSION, "form": FORM_KEY, **{k: v for k, v in h.items() if k != "v"},
            "crc": zlib.crc32(c["codes"])}
    payload = b""
    if h["arrays"]:
        payload = _deflate(b"".join(_intra(d, b) for d, b in
                                    zip(h["arrays"], _split(h["arrays"], h["n"], c["codes"]))))
    return [c["topic"].encode(), _body(positional(HEADER, wire), payload)]


def _per_frame(h: dict):
    (kind, m), = h["meta"].items()
    fields = DELTA_META.get(kind)
    return None if fields is None else [m[f] for f, _ in fields]


def _static_meta(h: dict) -> bytes:
    """The metadata without the per-frame fields, as bytes (NaN-safe)."""
    (kind, m), = h["meta"].items()
    drop = {f for f, _ in DELTA_META.get(kind, [])}
    return msgpack.packb(positional(META[kind], {k: (None if k in drop else v) for k, v in m.items()}))


def encode_delta(c: dict, base: dict):
    """A delta of `c` on `base`; None if `base` does not fit, so `c` goes as a keyframe."""
    h, b = c["header"], base["header"]
    if (c["topic"] != base["topic"] or any(h[k] != b[k] for k in _STATIC)
            or len(c["codes"]) != len(base["codes"]) or _static_meta(h) != _static_meta(b)):
        return None
    raw = []
    for d, x, y in zip(h["arrays"], _split(h["arrays"], h["n"], c["codes"]),
                       _split(b["arrays"], b["n"], base["codes"])):
        w = _delta(d, x, y)
        if w is None:
            return None
        raw.append(w)
    head = [PROTO_VERSION, FORM_DELTA, h["seq"], b["seq"], h["audio_sample"], h["capture_wall_ns"],
            h["protection"], zlib.crc32(c["codes"]), _per_frame(h)]
    payload = _deflate(b"".join(raw)) if h["arrays"] else b""
    return [c["topic"].encode(), _body(head, payload)]


def _header(body: bytes):
    """The msgpack header at the front of `body` (at most MAX_HEADER_BYTES), and the rest."""
    u = msgpack.Unpacker(raw=False, strict_map_key=True, max_buffer_size=MAX_HEADER_BYTES)
    head = body[:MAX_HEADER_BYTES]
    u.feed(head)
    try:
        h = u.unpack()
    except msgpack.OutOfData:
        raise DecodeError("header too large" if len(head) < len(body) else "header truncated")
    except Exception as e:  # noqa: BLE001 - any malformed msgpack
        raise DecodeError(f"header: {e}") from e
    return h, body[u.tell():]


def decode_wire(parts: list):
    """("key", coded frame) or ("delta", delta); sizes are validated before parsing, a
    keyframe's CRC is checked here, a delta's when it is applied."""
    if len(parts) != MAX_PARTS:
        raise DecodeError(f"{len(parts)} parts")
    if sum(len(p) for p in parts) > MAX_FRAME_BYTES:
        raise DecodeError("frame too large")
    topic = parse_topic(parts[0])
    raw, payload = _header(parts[1])
    # `v` first, on its own: another version may lay out the rest differently.
    if not isinstance(raw, list) or not raw or raw[0] != PROTO_VERSION:
        raise DecodeError(f"version {raw[0] if isinstance(raw, list) and raw else raw!r}")
    form = raw[1] if len(raw) > 1 else None
    expected_kind = topic.get("stream", topic.get("kind", topic["topic"]))
    if form == FORM_DELTA:
        d = named(DELTA_HEADER, raw)
        fields = DELTA_META.get(expected_kind)
        d["meta"] = None if fields is None else named(fields, d["meta"])
        if fields is None and raw[8] is not None:
            raise DecodeError("per-frame metadata on a kind without any")
        return "delta", {"topic": parts[0].decode(), "header": d, "payload": payload}
    if form != FORM_KEY:
        raise DecodeError(f"form {form!r}")
    h = named(HEADER, raw)
    n = h["n"]
    if not isinstance(n, int) or n < 0 or n > MAX_N:
        raise DecodeError("n out of range")
    descs = h["arrays"]
    if len(descs) > MAX_ARRAYS or (len(descs) > 0) != (len(payload) > 0):
        raise DecodeError("array count")
    for d in descs:
        if d["elem"] not in _ELEM_SIZE:
            raise DecodeError(f"elem {d['elem']!r}")
    if h["kind"] != expected_kind or list(h["meta"].keys()) != [h["kind"]]:
        raise DecodeError("kind mismatch")
    codes = b""
    if descs:
        expected = sum(array_size(d["elem"], n) for d in descs)
        codes = b"".join(_unintra(d, w) for d, w in zip(descs, _split(descs, n, _inflate(payload, expected))))
    crc = h.pop("crc")
    h.pop("form")
    if zlib.crc32(codes) != crc:
        raise DecodeError("crc")
    return "key", {"topic": parts[0].decode(), "header": h, "codes": codes}


def apply(delta: dict, base: dict) -> dict:
    """The frame `delta` makes of `base`; refused unless `base` is its base and the result
    matches its CRC."""
    d, b = delta["header"], base["header"]
    if base["topic"] != delta["topic"] or b["seq"] != d["base"]:
        raise DecodeError(f"wrong base: held {b['seq']}, delta on {d['base']}")
    expected = len(base["codes"])
    if b["arrays"]:
        raw = _inflate(delta["payload"], expected)
    elif delta["payload"]:
        raise DecodeError("payload on a frame without arrays")
    else:
        raw = b""
    codes = b"".join(_undelta(x, w, y) for x, w, y in zip(
        b["arrays"], _split(b["arrays"], b["n"], raw), _split(b["arrays"], b["n"], base["codes"])))
    if zlib.crc32(codes) != d["crc"]:
        raise DecodeError("crc")
    h = dict(b)
    for k in ("seq", "audio_sample", "capture_wall_ns", "protection"):
        h[k] = d[k]
    if d["meta"] is not None:
        (kind, m), = b["meta"].items()
        h["meta"] = {kind: {**m, **d["meta"]}}
    return {"topic": base["topic"], "header": h, "codes": codes}


def encode_frame(topic: str, header: dict, arrays: list) -> list:
    """A keyframe of `header` (named, without `form` and `crc`) and its arrays' values."""
    return encode_key(coded(topic, header, arrays))


def decode_frame(parts: list) -> dict:
    """A keyframe's values; a delta is refused, as it needs the frame before it."""
    form, c = decode_wire(parts)
    if form != "key":
        raise DecodeError("a delta needs its base")
    return {"topic": c["topic"], "header": c["header"], "arrays": values(c)}


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
