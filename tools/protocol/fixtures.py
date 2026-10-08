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


def arr(name, unit, elem="f32"):
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
        },
        [
            (arr("mag", "db"), [val(i, -6.0 + i * 0.03125) for i in range(n)]),
            (arr("phase", "deg"), [val(i, ((i * 15) % 720) * 0.5 - 180.0) for i in range(n)]),
            (arr("coh", "coherence"), [val(i, (i % 33) / 32.0) for i in range(n)]),
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
            },
            [
                (arr("level", "dbfs"), [-120.0, -20.0, INF, NAN]),
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
                        "uncertainty": 1.0,
                    },
                },
                "mic_curve": True,
                "horizon": 60.0,
                "correction": 5.0,
                "limits_from": "transferred",
                "bands": [4, 5, 17],
                "windows": [
                    {
                        "duration": 3600.0,
                        "weighting": "z",
                        "elapsed": 1800.0,
                        "measured": 1790.0,
                        "period": "night",
                        "period_after_horizon": "night",
                        "worst": 1,
                    },
                    {
                        "duration": 900.0,
                        "weighting": "a",
                        "elapsed": 900.0,
                        "measured": 900.0,
                        "period": "night",
                        "period_after_horizon": "night",
                        "worst": 1,
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
                (arr("leq", "db_spl"), [58.0, 62.5, 41.0, 31.5, 36.25, 40.0]),
                (arr("limit", "db_spl"), [60.0, 58.0, NAN, NAN, 74.0, 46.5]),
                (arr("allowed", "db_spl"), [59.5, NAN, NAN, NAN, 80.0, 49.0]),
                (arr("recover", "seconds"), [NAN, 412.0, NAN, NAN, NAN, NAN]),
                (
                    arr("leq_flags", "bitmask", "u32"),
                    [
                        LIMIT | JUDGED | NEAR | ON_COURSE,
                        LIMIT | JUDGED | OVER | CANNOT_RECOVER,
                        0,
                        0,
                        LIMIT | JUDGED,
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


TOKEN = bytes.fromhex("0123456789abcdeffedcba9876543210")


def req(idx, op, args=None, mutation=True):
    cmd = {"op": op}
    if args is not None:
        cmd["args"] = args
    return {"v": p.PROTO_VERSION, "id": idx, "cmd": cmd, "expect_rev": 41 if mutation else None}


MEAS_CONFIG = {
    "name": "Main L",
    "kind": {
        "type": "transfer",
        "config": {
            "reference_input": 0,
            "measurement_input": 1,
            "averaging": {"type": "fifo", "blocks": 8},
            "grid": {"ppo": 48, "k_min": -240, "k_max": 239},
            "smoothing": {"fraction": "sixth", "mode": "magnitude"},
            "depth": {"type": "fast_lf", "max_settle_s": 1.0},
        },
    },
}


BAND_NOMINAL_HZ = [
    20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0,
    500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0,
    6300.0, 8000.0, 10000.0,
]
FINLAND_545_NIGHT = [74.0, 64.0, 56.0, 49.0, 44.0, 42.0, 40.0, 38.0, 36.0, 34.0, 32.0]

BAND_LEQ_CONFIG = {
    "windows": [
        {
            "duration": 3600.0,
            "weighting": "z",
            "limits": {
                "type": "night_day",
                "night": FINLAND_545_NIGHT + [None] * 17,
                "day_offset": 5.0,
            },
            "warn_margin": 3.0,
        },
        {
            "duration": 900.0,
            "weighting": "a",
            "limits": {
                "type": "always",
                "limits": [48.0 if i == 5 else 30.5 if i == 17 else None for i in range(28)],
            },
            "warn_margin": 2.0,
        },
    ],
    "bands": BAND_NOMINAL_HZ[:11] + [1000.0],
    "predicted": {"duration": 3600.0, "day": None, "night": 25.0, "warn_margin": 3.0},
    "correction": {"impulse": "plus5", "tonal": "none"},
    "transfer": {
        "place": "flat 4 bedroom",
        "measured_at": 1789500000000000000,
        "origin": "measured",
        "bands": [
            {"status": "unchecked", "attenuation": 20.0},
            {"status": "clean", "attenuation": 25.5},
            {"status": "corrected", "attenuation": 30.0, "margin": 5.5},
            {"status": "unusable", "at_least": 35.0},
        ]
        + [{"status": "missing"}] * 24,
    },
}

SPL_MEASUREMENT = {
    "id": 4,
    "config": {
        "name": "FOH SPL",
        "kind": {
            "type": "spl",
            "config": {
                "input": 1,
                "weighting": "a",
                "time_weighting": "fast",
                "peak_weighting": "c",
                "leq": {
                    "windows": [
                        {"duration": 60.0, "weighting": "a", "limit": None, "warn_margin": 3.0},
                        {"duration": 1800.0, "weighting": "a", "limit": 99.0, "warn_margin": 3.0},
                    ],
                    "horizon": 60.0,
                    "peaks": {
                        "lcpeak": {"limit": 135.0, "warn_margin": 3.0},
                        "lafmax": None,
                    },
                },
                "position": {"level": 2.5, "peak": 1.5},
                "bands": BAND_LEQ_CONFIG,
            },
        },
    },
    "config_rev": 40,
    "running": True,
    "delay": None,
    "grid_id": None,
}

SPL_LOG = {
    "meas": 4,
    "started_at": 1790000000000000000,
    "windows": [
        {
            "duration": 60.0,
            "weighting": "a",
            "judgement": "no_limit",
            "since": 1790000000000000000,
        },
        {
            "duration": 1800.0,
            "weighting": "a",
            "judgement": "over",
            "since": 1790000600000000000,
        },
    ],
    "peaks": {
        "lcpeak": {"judgement": "near", "since": 1790000500000000000},
        "lafmax": {"judgement": "no_limit", "since": 1790000000000000000},
    },
    "alarms": [
        {
            "at": 1790000600000000000,
            "subject": {"type": "window", "duration": 1800.0, "weighting": "a"},
            "kind": "over",
            "level": 99.25,
            "limit": 99.0,
            "position": 2.5,
        },
        {
            "at": 1790000610000000000,
            "subject": {"type": "peak", "quantity": "lcpeak"},
            "kind": "recovered",
            "level": 133.5,
            "limit": 135.0,
            "position": None,
        },
        {
            "at": 1790000620000000000,
            "subject": {"type": "band", "duration": 3600.0, "weighting": "z", "nominal": 63.0},
            "kind": "over",
            "level": 82.5,
            "limit": 80.0,
            "position": None,
        },
        {
            "at": 1790000630000000000,
            "subject": {"type": "predicted"},
            "kind": "over",
            "level": 26.5,
            "limit": 25.0,
            "position": None,
        },
    ],
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
        "delay": {
            "applied": 0.0125,
            "applied_samples": 600.25,
            "nudged": 0.25 / 48000.0,
            "nudged_samples": 0.25,
            "tracking": True,
            "awaiting_pick": False,
            "last_finding": finding(),
        },
        "grid_id": p.grid_id(LOG_GRID),
    }


MATH_MEASUREMENT = {
    "id": 6,
    "config": {
        "name": "FOH average",
        "kind": {
            "type": "math",
            "config": {
                "owner": {"type": "meas", "meas": 1},
                "domain": "transfer",
                "expr": {
                    "type": "average",
                    "of": [
                        {"type": "meas", "meas": 1},
                        {"type": "meas", "meas": 2},
                        {"type": "trace", "trace": 7},
                    ],
                    "method": "coherence_weighted",
                },
                "reference": {"type": "operand", "operand": {"type": "meas", "meas": 2}},
                "smoothing": {"fraction": "third", "mode": "magnitude_phase"},
            },
        },
    },
    "config_rev": 62,
    "running": True,
    "delay": None,
    "grid_id": p.grid_id(LOG_GRID),
}


SWEEP = {"start": 20.0, "end": 20000.0, "duration": 5.0, "fade_in": 0.01, "fade_out": 0.01}

SWEEP_MEASUREMENT = {
    "id": 7,
    "config": {
        "name": "Genelec 1 m",
        "kind": {
            "type": "sweep",
            "config": {
                "reference_input": 1,
                "measurement_input": 0,
                "outputs": [0, 1],
                "level": -50.0,
                "sweep": SWEEP,
                "repeats": 2,
                "gate": 0.005,
                "tail": 3.0,
            },
        },
    },
    "config_rev": 66,
    "running": False,
    "delay": None,
    "grid_id": None,
}


def requests():
    """A subset of samples::commands(); `id` is the index in that list."""
    return [
        req(0, "hello", {"client": "ac2-cli 0.0.0"}, mutation=False),
        req(
            2,
            "session.open",
            {
                "config": {
                    "backend": "cpal",
                    "input_device": {"type": "id", "id": "hw:UMC1820"},
                    "output_device": {"type": "default"},
                    "input_channels": [0, 1, 2],
                    "output_channels": 2,
                    "sample_rate_hz": 48000,
                    "buffer_frames": None,
                    "loopback": {"output": 1, "input": 0},
                }
            },
        ),
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
            16,
            "delay.find",
            {"meas": 1, "band": {"type": "sub"}, "observation": 8.0},
            mutation=False,
        ),
        req(17, "delay.insert", {"meas": 1, "pick": {"type": "ranked", "index": 1}}),
        req(
            26,
            "trace.import",
            {
                "file_name": "sub.txt",
                "format": "analyzer_text",
                "role": "target",
                "content": b"20 -3.0 10\n",
            },
        ),
        req(
            29,
            "cal.curve_import",
            {
                "mic": "M30 #1234",
                "label": "0°",
                "file_name": "M30-1234.frd",
                "content": b"20 -0.5\n20000 1.5\n",
                "input": 1,
            },
        ),
        req(
            31,
            "spl.log_get",
            {"meas": 4, "log": "previous", "from": 120, "max": 3600},
            mutation=False,
        ),
        req(
            32,
            "sweep.run",
            {"lease_token": TOKEN, "meas": 7, "name": "1083 sweep"},
        ),
        req(39, "session.inputs", {"inputs": INPUTS}),
        req(
            40,
            "cal.delete",
            {"key": {"device": "hw:UMC1820", "channel": 1, "mic": "M30 #1234"}},
        ),
        req(41, "session.preview", {"backend": "jack", "device": "jack"}, mutation=False),
        req(42, "session.preview_stop", mutation=False),
        req(
            43,
            "session.detect_loopback",
            {
                "lease_token": TOKEN,
                "backend": "jack",
                "device": "jack",
                "output": 0,
                "level": -30.0,
            },
            mutation=False,
        ),
        req(44, "trace.mic_curve", {"trace": 8, "curve": {"mic": "M30 #1234", "label": "0°"}}),
        req(46, "cal.curve_delete", {"curve": {"mic": "M30 #1234", "label": "0°"}}),
        req(47, "spl.log_new", {"meas": 4}),
        req(
            48,
            "cal.spl_electrical",
            {
                "input": 1,
                "mic": "M30 #1234",
                "connection": "in_line",
                "volts": 0.015,
                "freq": 1000.0,
                "mic_sensitivity": 15.0,
                "uncertainty": None,
                "replace_acoustic": False,
            },
        ),
        req(49, "spl.history_get", {"meas": 4, "seconds": 14400}, mutation=False),
        req(50, "delay.nudge", {"meas": 1, "by": -0.25 / 48000.0}),
        req(
            51,
            "rec.start",
            {
                "request": {
                    "inputs": [0, 1],
                    "name": "soundcheck",
                    "max_duration": 600.0,
                    "max_bytes": None,
                }
            },
        ),
        req(53, "rec.list", mutation=False),
        req(
            54,
            "session.replay",
            {"recording": {"type": "name", "name": "soundcheck"}, "pace": "fast"},
        ),
        req(55, "gen.ceiling", {"ceiling": -40.0, "confirm_raise": True}),
        req(
            56,
            "session.outputs",
            {"outputs": [{"channel": 0, "label": "Main L"}, {"channel": 1, "label": None}]},
        ),
        req(57, "server.info", mutation=False),
        req(59, "server.revoke", {"name": "laptop"}),
        req(
            60,
            "spl.band_transfer",
            {
                "meas": 4,
                "foh": {
                    "type": "log",
                    "meas": 4,
                    "from": 1789500000000000000,
                    "until": 1789500030000000000,
                },
                "at_place": {
                    "type": "levels",
                    "levels": [50.0, 49.0, 48.0] + [None] * 25,
                },
                "background": None,
                "place": "flat 4 bedroom",
            },
        ),
        req(
            61,
            "spl.band_log_get",
            {
                "meas": 4,
                "from": 1789500000000000000,
                "until": 1789500030000000000,
                "step": 10,
            },
            mutation=False,
        ),
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
        {
            "kind": "outputs",
            "rev": 64,
            "payload": [{"channel": 0, "label": "Main L"}, {"channel": 3, "label": "Sub"}],
        },
        {"kind": "mic", "rev": 58, "payload": {"type": "deleted", "value": "ECM"}},
        {
            "kind": "measurement",
            "rev": 59,
            "payload": {"type": "set", "value": SPL_MEASUREMENT},
        },
        {
            "kind": "measurement",
            "rev": 62,
            "payload": {"type": "set", "value": MATH_MEASUREMENT},
        },
        {"kind": "spl_log", "rev": 52, "payload": {"type": "set", "value": SPL_LOG}},
        {"kind": "timing", "rev": 54, "payload": TIMING_STATUS},
        {
            "kind": "sweep",
            "rev": 55,
            "payload": {
                "id": 3,
                "meas": 7,
                "owner": "alice",
                "name": "1083 sweep",
                "reference_input": 1,
                "measurement_input": 0,
                "outputs": [0, 1],
                "level": -50.0,
                "sweep": SWEEP,
                "sweep_duration": 4.75,
                "post_roll": 1.0,
                "repeats": 2,
                "gate": None,
                "status": {"type": "done", "trace": 9},
                "started_at": 1790000000000000000,
            },
        },
        {
            "kind": "measurement",
            "rev": 66,
            "payload": {"type": "set", "value": SWEEP_MEASUREMENT},
        },
        {
            "kind": "recording",
            "rev": 60,
            "payload": {
                "name": "soundcheck",
                "path": "/home/op/.local/share/ac2/recordings/soundcheck.wav",
                "inputs": [0, 1],
                "sample_rate_hz": 48000,
                "session_epoch": 2,
                "start_sample": 96000,
                "started_at": 1789999100000000000,
                "started_by": "alice",
                "frames": 480000,
                "bytes": 3840116,
                "discontinuities": 1,
                "max_duration": 10.0,
                "max_bytes": 1073741824,
                "status": {"type": "ended", "reason": {"type": "duration_limit"}},
            },
        },
        {
            "kind": "autosave",
            "rev": 57,
            "payload": {
                "state": {
                    "type": "failed",
                    "reason": "No space left on device (os error 28)",
                },
                "saved_at": 1790000000000000000,
            },
        },
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
    grids = [LOG_GRID, BAND_GRID, LIN_GRID, LOG_BINS_GRID]
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
