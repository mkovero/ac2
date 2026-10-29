"""Fixture ctrl requests and events (mirror `crates/ac2-proto/src/samples`)."""

from __future__ import annotations

import ac2proto as p
from fixture_frames import (
    INPUTS,
    LOG_GRID,
    PREVIEW_INPUTS,
    PREVIEW_PAIRS,
    TIMING_STATUS,
)


TOKEN = bytes.fromhex("0123456789abcdeffedcba9876543210")


def req(idx, op, args=None, mutation=True):
    cmd = {"op": op}
    if args is not None:
        cmd["args"] = args
    return {"v": p.PROTO_VERSION, "id": idx, "cmd": cmd, "expect_rev": 41 if mutation else None}


def osc_message(path, args, append_level):
    return {"path": path, "args": args, "append_level": append_level}


def dmx(pairs):
    return [{"channel": c, "value": v} for c, v in pairs]


LIGHT_CUE = {
    "ok": {
        "osc": osc_message("/light/red", [{"type": "int", "value": 0}], False),
        "dmx": dmx([(12, 0), (13, 255)]),
    },
    "near": {
        "osc": osc_message(
            "/light/red",
            [{"type": "float", "value": 0.5}, {"type": "str", "value": "near"}],
            False,
        ),
        "dmx": [],
    },
    "over": {
        "osc": osc_message("/light/red", [{"type": "int", "value": 1}], True),
        "dmx": dmx([(12, 255), (14, 40)]),
    },
    "fault": {"osc": None, "dmx": dmx([(512, 255)])},
}

RIG = {
    "rev": 38,
    "osc_target": "192.168.9.20:9000",
    "artnet_target": {"addr": "192.168.9.255:6454", "universe": 1},
    "artnet_fade": 1.5,
    "monitor_at_start": True,
    "levels": {
        "ballistic": "type1",
        "correlation_window": "short",
        "over_threshold": -1.0,
        "correlation_gate": -50.0,
        "loudness_target": -16.0,
    },
    "osc": {"sent": 12, "last_error": "Connection refused (os error 111)"},
    "artnet": {"sent": 340, "last_error": None},
    "dmx": dmx([(12, 255), (14, 40)]),
    "cues": [
        {"meas": 4, "subject": {"type": "meter"}, "state": "over"},
        {
            "meas": 4,
            "subject": {"type": "window", "duration": 1800.0, "weighting": "a"},
            "state": "near",
        },
    ],
    "damaged_files": [
        {
            "kind": "rig_settings",
            "path": "/home/op/.config/ac2/rig.json",
            "set_aside_as": "/home/op/.config/ac2/rig.json.damaged-1790000000",
            "reason": "EOF while parsing an object at line 3 column 0; restored the previous copy",
            "at": 1790000000000000000,
        }
    ],
}

MEAS_CONFIG = {
    "name": "Main L",
    "scope": "session",
    "kind": {
        "type": "transfer",
        "config": {
            "reference_input": 0,
            "measurement_input": 1,
            "averaging": {"type": "fifo", "blocks": 8},
            "resolution": "ninety_sixth",
            "smoothing": {"fraction": "sixth", "mode": "magnitude"},
            "depth": {"type": "fast_lf", "max_settle_s": 1.0},
        },
    },
}


FINLAND_545_BANDS = [20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0]
FINLAND_545_NIGHT = [74.0, 64.0, 56.0, 49.0, 44.0, 42.0, 40.0, 38.0, 36.0, 34.0, 32.0]

BAND_LEQ_CONFIG = {
    "windows": [
        {
            "band": hz,
            "duration": 3600.0,
            "weighting": "z",
            "limit": night,
            "day_offset": 5.0,
            "warn_margin": 3.0,
            "cue": None,
        }
        for hz, night in zip(FINLAND_545_BANDS, FINLAND_545_NIGHT)
    ]
    + [
        {
            "band": 1000.0,
            "duration": 900.0,
            "weighting": "a",
            "limit": 30.5,
            "day_offset": None,
            "warn_margin": 2.0,
            "cue": LIGHT_CUE,
        }
    ],
    "predicted": {
        "duration": 3600.0,
        "day": None,
        "night": 25.0,
        "warn_margin": 3.0,
        "cue": None,
    },
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
        "scope": "session",
        "kind": {
            "type": "spl",
            "config": {
                "input": 1,
                "weighting": "a",
                "time_weighting": "fast",
                "peak_weighting": "c",
                "leq": {
                    "windows": [
                        {
                            "duration": 60.0,
                            "weighting": "a",
                            "limit": None,
                            "warn_margin": 3.0,
                            "cue": None,
                        },
                        {
                            "duration": 1800.0,
                            "weighting": "a",
                            "limit": 99.0,
                            "warn_margin": 3.0,
                            "cue": None,
                        },
                    ],
                    "horizon": 60.0,
                    "peaks": {
                        "lcpeak": {"limit": 135.0, "warn_margin": 3.0, "cue": None},
                        "lafmax": None,
                    },
                },
                "position": {"level": 2.5, "peak": 1.5},
                "bands": BAND_LEQ_CONFIG,
                "cue": LIGHT_CUE,
            },
        },
    },
    "config_rev": 40,
    "running": True,
    "delay": None,
    "grid_id": None,
    "colour": None,
    "layout": None,
    "compared": [],
    "offset": 0.0,
    "eq": [],
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


def _plate(at=None, scale=1.0):
    return {"at": at, "scale": scale}


def _layout_pane(
    kind,
    transfer_view,
    chrome="full",
    content=None,
    transfer_levels=None,
    readout=None,
    programme_view="meters",
    rig=None,
):
    return {
        "kind": kind,
        "transfer_view": transfer_view,
        "spectrum_view": "spectrograph",
        "spl_view": "meter_leq",
        "sweep_view": "response",
        "programme_view": programme_view,
        "chrome": chrome,
        "content": content,
        "levels": {
            "transfer": transfer_levels,
            "spectrum_dbfs": None,
            "spectrum_spl": None,
            "distortion": None,
        },
        "plates": {
            "legend": _plate(),
            "readout": readout or _plate(),
            "mark": _plate(),
            "peaks": _plate(),
            "loudness": _plate(),
        },
        "rig": rig or {"type": "this_rig"},
    }


# samples::pane_layout(): a transfer pane (level axis zoomed, cursor values dragged to the
# bottom-left at twice the text size) beside an SPL meter picked for measurement 2 over a
# Programme pane metering the pair of inputs 3 and 4 in its goniometer view. The meter is
# pinned to another rig; the other two show this session's.
PANE_LAYOUT = {
    "type": "split",
    "axis": "row",
    "ratio": 0.625,
    "a": {
        "type": "pane",
        "pane": _layout_pane(
            "transfer",
            "phase",
            transfer_levels={"lo": -24.0, "hi": 12.0},
            readout=_plate([0.0, 1.0], 2.0),
        ),
    },
    "b": {
        "type": "split",
        "axis": "column",
        "ratio": 0.5,
        "a": {
            "type": "pane",
            "pane": _layout_pane(
                "spl",
                "magnitude",
                "no_grid",
                content={"type": "meas", "meas": 2},
                rig={"type": "rig", "id": 0x0FEDCBA987654321, "name": "ketunkolo"},
            ),
        },
        "b": {
            "type": "pane",
            "pane": _layout_pane(
                "programme",
                "magnitude",
                content={"type": "pair", "left": 2, "right": 3},
                programme_view="goniometer",
            ),
        },
    },
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
        "colour": {"type": "hue", "index": 5},
        "layout": PANE_LAYOUT,
        "compared": [],
        "offset": 0.0,
        "eq": [],
    }


MATH_MEASUREMENT = {
    "id": 6,
    "config": {
        "name": "FOH average",
        "scope": "session",
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
    "colour": None,
    "layout": None,
    "compared": [],
    "offset": 0.0,
    "eq": [],
}


ALIGN_MEASUREMENT = {
    **MATH_MEASUREMENT,
    "id": 9,
    "config": {
        "name": "Sub ↔ Main",
        "scope": "session",
        "kind": {
            "type": "math",
            "config": {
                "owner": {"type": "meas", "meas": 1},
                "domain": "transfer",
                "expr": {
                    "type": "align",
                    "a": {"type": "trace", "trace": 7},
                    "b": {"type": "meas", "meas": 1},
                    "band": {"lo": 60.0, "hi": 120.0},
                    "max_delay": 0.05,
                },
                "reference": {"type": "operand", "operand": {"type": "trace", "trace": 7}},
                "smoothing": None,
            },
        },
    },
}


SWEEP = {"start": 20.0, "end": 20000.0, "duration": 5.0, "fade_in": 0.01, "fade_out": 0.01}

SWEEP_MEASUREMENT = {
    "id": 7,
    "config": {
        "name": "Genelec 1 m",
        "scope": "session",
        "kind": {
            "type": "sweep",
            "config": {
                "reference_input": 1,
                "measurement_inputs": [0, 2],
                "outputs": [0, 1],
                "level": -50.0,
                "sweep": SWEEP,
                "repeats": 2,
                "gate": 0.005,
                "tail": 3.0,
                "lf_harmonics": "fine",
                "resolution": "twenty_fourth",
            },
        },
    },
    "config_rev": 66,
    "running": False,
    "delay": None,
    "grid_id": None,
    "colour": {"type": "neutral"},
    "layout": None,
    "compared": [],
    "offset": 0.0,
    "eq": [],
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
                    "backend": "coreaudio",
                    "input_device": {"type": "id", "id": "hw:UMC1820"},
                    "output_device": {"type": "default"},
                    "input_channels": [0, 1, 2],
                    "spare_inputs": 0,
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
                "file_name": "genelec.wav",
                "format": "ir_wav",
                "role": "trace",
                "ir": {
                    "channel": 1,
                    "window": {"type": "gate", "pre": 0.001, "post": 0.01},
                    "align": "peak",
                    "resolution": "forty_eighth",
                },
                "content": b"RIFF",
                "owner": {"type": "imported"},
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
        req(39, "session.inputs", INPUTS),
        req(
            40,
            "cal.delete",
            {"key": {"type": "line", "device": "hw:UMC1820", "channel": 4}},
        ),
        req(
            41,
            "session.preview",
            {
                "backend": "jack",
                "device": "jack",
                "inputs": PREVIEW_INPUTS,
                "pairs": PREVIEW_PAIRS,
            },
            mutation=False,
        ),
        req(42, "session.preview_stop", mutation=False),
        req(
            43,
            "session.detect_loopback",
            {
                "lease_token": TOKEN,
                "backend": "jack",
                "input_device": "jack",
                "output_device": "jack",
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
                "loading": {"mic_impedance": 330.0, "input_impedance": 2000.0},
                "uncertainty": None,
                "replace_acoustic": False,
            },
        ),
        req(
            49,
            "spl.history_get",
            {
                "meas": 4,
                "span": {
                    "type": "range",
                    "from": 1789500000000000000,
                    "to": 1789510800000000000,
                    "step_s": 30,
                },
            },
            mutation=False,
        ),
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
        req(62, "meas.colour", {"meas": 3, "colour": {"type": "hue", "index": 2}}),
        req(63, "file.new", {"discard": False}),
        req(
            67,
            "file.upload_chunk",
            {"transfer": 3, "offset": 1048576, "data": bytes([0x1F, 0x8B, 0x08, 0x00])},
            mutation=False,
        ),
        req(
            71,
            "file.download_end",
            {"transfer": 4, "saved_as": "/run/media/foh/STICK/friday show.ac2session"},
        ),
        req(
            73,
            "session.control_panel",
            {"backend": "asio", "device": "Fireface USB"},
            mutation=False,
        ),
        req(74, "osc.target", {"base": 38, "addr": "[fe80::1]:9000"}),
        req(
            75,
            "osc.test",
            {"msg": osc_message("/light/red", [{"type": "int", "value": 1}], True)},
        ),
        req(76, "rig.monitor_start", {"base": 38, "on": True}),
        req(
            77,
            "artnet.target",
            {"base": 38, "target": {"addr": "2.255.255.255:6454", "universe": 18}},
        ),
        req(78, "artnet.test", {"channels": dmx([(12, 255), (13, 40)])}),
        req(79, "artnet.fade", {"base": 38, "seconds": 2.5}),
        req(82, "cal.line", {"channel": 4, "volts": 1.2277}),
        req(
            85,
            "levels.set",
            {
                "base": 38,
                "ballistic": "short_term",
                "correlation_window": None,
                "over_threshold": -1.0,
                "correlation_gate": None,
                "loudness_target": -16.0,
            },
        ),
        req(86, "levels.reset", {"channels": [0, 1]}),
        req(88, "plan.list", mutation=False),
        req(89, "plan.delete", {"name": "Club"}),
        req(
            90,
            "walk.new",
            {
                "name": "Mains",
                "plan": "Hall",
                "kind": "live_transfer",
                "meas": [1, 2],
                "points": [
                    {"label": "P1", "at": [5.0, 12.0, 1.2]},
                    {"label": "P2", "at": [12.5, 15.0, 1.2]},
                ],
            },
        ),
        req(92, "walk.goto", {"walk": 1, "step": 1}),
        req(93, "walk.capture", {"walk": 1}),
        req(94, "walk.skip", {"walk": 1, "point": 1}),
        req(95, "walk.swap", {"walk": 1, "points": [2, 3]}),
        req(96, "walk.redo", {"walk": 1, "point": 0}),
    ]


def events():
    """A subset of samples::events()."""
    return [
        {"kind": "measurement", "rev": 43, "payload": {"type": "set", "value": measurement()}},
        {"kind": "measurement", "rev": 44, "payload": {"type": "deleted", "value": 3}},
        {"kind": "plan", "rev": 81, "payload": {"type": "deleted", "value": "Club"}},
        {"kind": "walk", "rev": 83, "payload": {"type": "deleted", "value": 2}},
        {"kind": "walk_plan", "rev": 85, "payload": {"type": "deleted", "value": "Club"}},
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
            "kind": "line_calibration",
            "rev": 74,
            "payload": {"type": "deleted", "value": {"device": "hw:UMC1820", "channel": 4}},
        },
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
                "measurement_inputs": [0, 2],
                "outputs": [0, 1],
                "level": -50.0,
                "sweep": SWEEP,
                "sweep_duration": 4.75,
                "post_roll": 1.0,
                "repeats": 2,
                "gate": None,
                "lf_harmonics": "standard",
                "resolution": "forty_eighth",
                "status": {"type": "done", "traces": [9, 10]},
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
            "kind": "working",
            "rev": 67,
            "payload": {"origin": None, "dirty": False, "saved_at": None, "recover": None},
        },
        {"kind": "rig", "rev": 68, "payload": RIG},
        {
            "kind": "measurement",
            "rev": 69,
            "payload": {"type": "set", "value": ALIGN_MEASUREMENT},
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
                "log_failures": [
                    {
                        "meas": 4,
                        "log": "bands",
                        "reason": "disk full",
                        "since": 1790000030000000000,
                    }
                ],
            },
        },
    ]
