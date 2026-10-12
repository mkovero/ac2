"""Fixture frames and the shared values they use (mirror `crates/ac2-proto/src/samples`)."""

from __future__ import annotations

import ac2proto as p


NAN = float("nan")
INF = float("inf")


# --------------------------------------------------------------------------------------
# Shared values (mirror samples.rs)

LOG_GRID = {"type": "log", "ppo": 48, "k_min": -240, "k_max": 239}
BAND_GRID = {
    "type": "iec_bands",
    "fraction": "third",
    "centres": [19.952623149688797, 1000.0, 20000.0],
}
LIN_GRID = {"type": "linear", "fs": 48000.0, "n": 65536}
LOG_BINS_GRID = {"type": "log_bins", "fs": 48000.0, "n": 65536, "ppo": 96}

WEAK_REFERENCE = 1 << 3
THINNED, OUT_OF_BAND, INSUFFICIENT_RESOLUTION = 1 << 0, 1 << 1, 1 << 7
CLIP, HELD = 1 << 0, 1 << 1
LIMIT, JUDGED, NEAR, OVER, CANNOT_RECOVER, INCOMPLETE, ON_COURSE = (1 << i for i in range(7))


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


def arr(name, unit, elem=None):
    """The element encoding is fixed per array (docs/protocol.md §5.3)."""
    if elem is None:
        elem = {"mag": "i16", "phase": "i16", "level": "i16", "ir_etc": "i16", "coh": "u16", "ir_linear": "bfp16"}.get(name, "f32")
    return {"name": name, "unit": unit, "elem": elem}


TIMING_STATUS = {
    "epoch": 3,
    "state": {"type": "locked", "offset": 312},
    "last_lock": {"epoch": 3, "offset": 312, "at_sample": 96000, "at": 1790000000000000000},
    "drift": {"ppm": 0.4, "span": 30.0, "warning": False, "at": 1790000000500000000},
    "internal_reference": True,
}


def frame(topic, kind, grid, meta, arrays, protection=WEAK_REFERENCE):
    """arrays: list of (desc, values). Values are rounded to what the wire carries."""
    n = len(arrays[0][1]) if arrays else 0
    header = {"v": p.PROTO_VERSION, "kind": kind}
    header.update(stamp(grid, protection))
    header["n"] = n
    header["arrays"] = [d for d, _ in arrays]
    header["meta"] = {kind: meta}
    values = {
        d["name"]: [p.wire_value(d, v, vals) for v in vals] for d, vals in arrays
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
            "nudged": 0.0,
            "smoothing": {"fraction": "sixth", "mode": "magnitude"},
            "mic_curve": True,
            "math": {
                "operands": [
                    {"operand": {"type": "meas", "meas": 2}, "status": {"type": "included"}},
                    {"operand": {"type": "trace", "trace": 7}, "status": {"type": "included"}},
                    {
                        "operand": {"type": "meas", "meas": 4},
                        "status": {"type": "refused", "protection": 2},
                    },
                    {"operand": {"type": "meas", "meas": 5}, "status": {"type": "stopped"}},
                ],
                "phase": "shared_time_base",
            },
            "unresolved": {
                "resolution": "ninety_sixth",
                "ranges": [{"lo": 31.25, "hi": 135.5}, {"lo": 1625.0, "hi": 3250.0}],
            },
        },
        [
            (arr("mag", "db"), [val(i, (3 * i - 600) / 100.0) for i in range(n)]),
            (arr("phase", "deg"), [val(i, ((i * 15) % 720) * 0.5 - 180.0) for i in range(n)]),
            (arr("coh", "coherence"), [val(i, (i % 41) * 250 / 10000.0) for i in range(n)]),
            (
                arr("validity", "bitmask", "u32"),
                [THINNED if i < 4 else OUT_OF_BAND if i >= 470 else 0 for i in range(n)],
            ),
        ],
    )


CAL_AT = 1_789_000_000_000_000_000

INPUTS = [
    {"channel": 1, "mic": "M30 #1234", "curve": {"type": "curve", "label": "0°"}},
    {"channel": 2, "mic": None, "curve": {"type": "not_chosen"}},
    {"channel": 3, "mic": "ECM", "curve": {"type": "off"}},
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
                    [0.0, 0.5, -1.0, 0.3, 1.0e-3, -0.0],
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
                "cal": {
                    "type": "verified",
                    "calibrated_at": CAL_AT,
                    "basis": {"type": "acoustic", "calibrator_level": 94.0},
                },
                "mic_curve": True,
                "math": None,
            },
            [
                (arr("level", "db_spl"), [NAN, 74.5, 61.25]),
                (arr("validity", "bitmask", "u32"), [INSUFFICIENT_RESOLUTION, 0, 0]),
            ],
        ),
        "spec": frame(
            "d/5/spec",
            "spec",
            LOG_BINS_GRID,
            {
                "window": "hann",
                "scale": "dbfs",
                "level": "density",
                "cal": {"type": "uncalibrated"},
                "mic_curve": False,
                "smoothing": "sixth",
                "math": {
                    "operands": [
                        {"operand": {"type": "meas", "meas": 3}, "status": {"type": "included"}},
                        {"operand": {"type": "trace", "trace": 8}, "status": {"type": "mismatch"}},
                    ],
                    "phase": "no_phase",
                },
                "bands": {
                    "bands": [
                        {
                            "window": 0.6826666666666666,
                            "hop": 0.08533333333333333,
                            "sole": 202.875,
                            "age": 0.0,
                        },
                        {
                            "window": 0.021333333333333333,
                            "hop": 0.010666666666666666,
                            "sole": 6492.0,
                            "age": None,
                        },
                    ],
                    "unresolved": {
                        "resolution": "forty_eighth",
                        "ranges": [{"lo": 7.8125, "hi": 101.5}],
                    },
                },
            },
            [
                (arr("level", "dbfs_per_hz"), [-120.0, -20.0, INF, NAN]),
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
                "cal": {
                    "type": "other_mic_or_input",
                    "calibrated_at": CAL_AT,
                    "basis": {"type": "acoustic", "calibrator_level": 114.0},
                },
                "mic_curve": True,
                "position": {"level": 2.5, "peak": 1.5},
            },
            [],
        ),
        "leq": frame(
            "d/4/leq",
            "leq",
            None,
            {
                "scale": "db_spl",
                "cal": {
                    "type": "verified",
                    "calibrated_at": CAL_AT,
                    "basis": {
                        "type": "electrical",
                        "connection": "injected",
                        "mic_sensitivity": 15.0,
                        "data_sheet": False,
                        "loading": {"mic_impedance": 330.0, "input_impedance": 2000.0},
                        "uncertainty": 1.0,
                    },
                },
                "mic_curve": False,
                "horizon": 60.0,
                "logged": 1800,
                "run": {
                    "started_at": 1_790_000_000_000_000_000,
                    "until": 1_790_001_810_000_000_000,
                    "measured": 1790.0,
                    "gaps": 20.0,
                    "trimmed": False,
                    "laeq": 97.8,
                    "lceq": 110.25,
                    "lzeq": 112.5,
                },
                "lcpeak": {"level": 133.5, "judgement": "near"},
                "lafmax": None,
                "position": {"level": 2.5, "peak": 1.5},
            },
            [
                (arr("leq", "db_spl"), [96.5, 99.25, 101.5]),
                (arr("elapsed", "seconds"), [60.0, 1800.0, 600.0]),
                (arr("measured", "seconds"), [60.0, 1790.0, 600.0]),
                (arr("allowed", "db_spl"), [NAN, NAN, 99.5]),
                (arr("recover", "seconds"), [NAN, 412.0, NAN]),
                (arr("least", "db_spl"), [96.5, 99.25, 93.75]),
                (arr("over_in", "seconds"), [NAN, NAN, 1948.5]),
                (
                    arr("leq_flags", "bitmask", "u32"),
                    [
                        0,
                        LIMIT | JUDGED | OVER | CANNOT_RECOVER | INCOMPLETE,
                        LIMIT | JUDGED | NEAR | ON_COURSE,
                    ],
                ),
            ],
        ),
        "band_leq": frame(
            "d/4/band_leq",
            "band_leq",
            None,
            {
                "scale": "db_spl",
                "cal": {
                    "type": "verified",
                    "calibrated_at": CAL_AT,
                    "basis": {
                        "type": "electrical",
                        "connection": "injected",
                        "mic_sensitivity": 15.0,
                        "data_sheet": False,
                        "loading": {"mic_impedance": 330.0, "input_impedance": 2000.0},
                        "uncertainty": 1.0,
                    },
                },
                "mic_curve": True,
                "horizon": 60.0,
                "correction": 5.0,
                "limits_from": "transferred",
                "windows": [
                    {
                        "band": 50.0,
                        "duration": 3600.0,
                        "weighting": "z",
                        "elapsed": 1800.0,
                        "measured": 1790.0,
                        "period": "night",
                        "period_after_horizon": "night",
                    },
                    {
                        "band": 63.0,
                        "duration": 3600.0,
                        "weighting": "z",
                        "elapsed": 1800.0,
                        "measured": 1790.0,
                        "period": "night",
                        "period_after_horizon": "night",
                    },
                    {
                        "band": 1000.0,
                        "duration": 900.0,
                        "weighting": "a",
                        "elapsed": 900.0,
                        "measured": 900.0,
                        "period": "night",
                        "period_after_horizon": "night",
                    },
                ],
                "predicted": {
                    "duration": 3600.0,
                    "estimate": 23.5,
                    "at_most": 27.25,
                    "limit": 25.0,
                    "judgement": "near",
                },
            },
            [
                (arr("leq", "db_spl"), [58.0, 62.5, 40.0]),
                (arr("limit", "db_spl"), [60.0, 58.0, 46.5]),
                (arr("allowed", "db_spl"), [59.5, NAN, 49.0]),
                (arr("recover", "seconds"), [NAN, 412.0, NAN]),
                (
                    arr("leq_flags", "bitmask", "u32"),
                    [
                        LIMIT | JUDGED | NEAR | ON_COURSE,
                        LIMIT | JUDGED | OVER | CANNOT_RECOVER,
                        LIMIT | JUDGED,
                    ],
                ),
            ],
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
        "session_levels": frame(
            "session/levels",
            "session_levels",
            None,
            {"channels": [0, 1, 2]},
            [
                (arr("peak", "dbfs"), [-6.0, -0.0625, -INF]),
                (arr("rms", "dbfs"), [-18.5, -3.25, -INF]),
                (arr("clip", "bitmask", "u32"), [0, HELD, 0]),
            ],
            protection=0,
        ),
        "preview_levels": frame(
            "session/preview",
            "preview_levels",
            None,
            {"backend": "jack", "device": "jack", "channels": [0, 1]},
            [
                (arr("peak", "dbfs"), [-12.0, -40.5]),
                (arr("rms", "dbfs"), [-20.0, -52.25]),
                (arr("clip", "bitmask", "u32"), [0, CLIP]),
            ],
            protection=0,
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
