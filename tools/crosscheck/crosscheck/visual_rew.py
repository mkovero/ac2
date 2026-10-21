"""The graph comparison's rig half: REW's own pictures, saved through REW's image capture
with axes set from the data REW exported, so the host half (`visual.py`) can draw the same
numbers with plotcheck and read both pictures back.

Of an imported measurement (`save_graphs`, run inside the REW import): `allspl` and
`splphase` (magnitude, magnitude and phase), `groupdelay` (REW's group delay, ms), `impulse`
in percent of full scale (the linear IR) and in dBFS (`impulse_db`, the log IR). Of REW's
RTA on the rig's input (`save_rta`, capture only, nothing played): the RTA window's picture
and the captured spectrum it shows. Every graph's axes and REW's settings go to a sidecar
`<prefix>_graph.json`; REW's settings are put back afterwards.

REW takes impulse graph limits in seconds and percent / dBFS, group delay limits in ms
(the axis is labelled in s with SI prefixes), frequency graph limits in Hz and dB. The
phase axis is neither settable nor readable; the host half reads it from the picture.
"""
from __future__ import annotations

import json
import math
import time
from pathlib import Path

import numpy as np

from . import formats

GRAPHS = ("allspl", "splphase")
SPAN_DB = 60.0
IMAGE_WIDTH = 1200
# REW's capture settings for the comparison: no legend/title/cursor (text inside the plot
# would read as curve), thin lines (a thick line is a wider band around the same centre).
IMAGE_SETTINGS = {"imageWidth": IMAGE_WIDTH, "graphTextSize": 12, "traceThickness": 1, "useDarkScheme": False,
                  "includeLegend": False, "includeTitle": False, "includeCursor": False, "includeTimestamp": False,
                  "includeBorder": False, "monochrome": False, "scaleOverlays": False, "asPNG": True, "comment": ""}
GRAPH_SETTINGS = ("/application/graph-main", "/application/y-axis-main", "/application/graph-limits-main",
                  "/application/image-capture-settings-main")
RTA_SETTINGS = ("/rta/configuration", "/rta/y-axis", "/rta/graph-limits", "/rta/image-capture-settings",
                "/rta/appearance-configuration")
# The RTA captures, name → (REW mode, bars on or off (None: leave)). Spectrum mode draws
# every FFT bin, far more points than pixels: how each renderer thins a dense line. A
# fractional-octave mode is not taken: REW's captured data in those modes matches neither
# its picture nor the same frame's bins summed into bands (several dB, shape included), so
# the picture has no numbers to be held against.
RTA_MODES = {"rta": ("Spectrum", None)}
# ac2's log IR view is dB re the peak on this range; REW's dBFS axis is set to the same span
# around the peak so the two pictures map a dB to the same pixel.
IR_LOG_RANGE_DB = (-60.0, 3.0)
# The RTA frame REW's picture and data are taken from: one capture of this length, stopped,
# then read and saved (the data and the picture are the same frozen frame).
RTA_CAPTURE_S = 3.0


# ---------------------------------------------------------------- axes from the data


def graph_limits(curve: formats.RewCurve, lo_hz: float = 20.0, hi_hz: float = 20_000.0) -> dict:
    """Axes for the frequency pictures: 20 Hz – 20 kHz, and `SPAN_DB` of level with the
    curve's maximum just under the top, on 5 dB marks. The phase (REW's right axis, not
    settable) then sits in the lower part of the frame for a flat path."""
    m = (curve.f >= lo_hz) & (curve.f <= hi_hz) & np.isfinite(curve.mag)
    top = 5.0 * math.ceil(float(np.max(curve.mag[m])) / 5.0) + 5.0
    return {"left": lo_hz, "right": hi_hz, "top": top, "bottom": top - SPAN_DB}


def gd_limits(gd: formats.RewCurve, lo_hz: float = 20.0, hi_hz: float = 20_000.0) -> dict:
    """Axes for the group-delay picture, ms: the band 20 Hz – 20 kHz, and a level range that
    holds the curve resampled at 1/48 octave between its 1st and 99th percentiles (so the
    low-frequency rise of a high-pass counts as much as the flat band, and the noise spikes
    of a few bins do not set the scale), a millisecond of room on each side, 2 ms at least."""
    m = (gd.f >= lo_hz) & (gd.f <= hi_hz) & np.isfinite(gd.mag)
    f, v = gd.f[m], gd.mag[m] * 1e3
    fl = lo_hz * 2 ** (np.arange(0, 48 * np.log2(hi_hz / lo_hz) + 1) / 48)
    vl = np.interp(np.log(fl), np.log(f), v)
    lo, hi = np.percentile(vl, [1, 99])
    lo, hi = math.floor(min(lo, 0.0)) - 1.0, math.ceil(max(hi, 0.0)) + 1.0
    if hi - lo < 2.0:
        hi = lo + 2.0
    return {"left": lo_hz, "right": hi_hz, "top": hi, "bottom": lo}


def ir_limits(ir: formats.RewIR) -> dict:
    """Axes for the impulse pictures. Time (seconds, REW's unit for them): from 5 ms before
    REW's zero to 30 ms after it, or to 1.5 × the last time the IR is above 0.1 % of its peak
    if that is later (a room's tail). The span keeps at least two samples a pixel across the
    picture: both renderers then thin each column to its extremes and draw the samples'
    envelope, where at a finer zoom each draws its own curve between the samples (REW a
    band-limited interpolation, ac2 the polyline through them) and the two part at the peak.
    Linear: REW draws the impulse normalised to a peak of 100 % whatever the export says
    (its percent export is of full scale unless asked for `normalised=true`), so ±110 %;
    the host sets ac2's full-scale axis to the same span of the same peak. Log:
    `IR_LOG_RANGE_DB` around the peak in dBFS."""
    k = int(np.argmax(np.abs(ir.h)))
    peak = float(abs(ir.h[k]))
    above = np.where(np.abs(ir.h) > 1e-3 * peak)[0]
    t_last = float(ir.t[above[-1]]) if len(above) else float(ir.t[k])
    t_hi = max(30e-3, 1.5 * t_last)
    top = 110.0
    peak_db = 20.0 * math.log10(peak / 100.0)
    return {"peak_pct": peak, "peak_dbfs": peak_db, "peak_t_s": float(ir.t[k]), "t_last_0p1pct_s": t_last,
            "linear": {"left": -5e-3, "right": t_hi, "top": top, "bottom": -top},
            "log": {"left": -5e-3, "right": t_hi, "top": peak_db + IR_LOG_RANGE_DB[1], "bottom": peak_db + IR_LOG_RANGE_DB[0]}}


def rta_limits(curve: formats.RewCurve, lo_hz: float = 20.0, hi_hz: float = 20_000.0) -> dict:
    """Axes for the RTA picture: the band, the top on a 10 dB mark above the maximum, the
    bottom 10 dB under the 5th percentile on a 10 dB mark (the deep nulls between bins fall
    out of the frame; the host half skips columns whose data leaves it)."""
    m = (curve.f >= lo_hz) & (curve.f <= hi_hz) & np.isfinite(curve.mag)
    top = 10.0 * math.ceil(float(np.max(curve.mag[m])) / 10.0) + 10.0
    bottom = 10.0 * math.floor(float(np.percentile(curve.mag[m], 5)) / 10.0) - 10.0
    return {"left": lo_hz, "right": hi_hz, "top": top, "bottom": bottom}


# ---------------------------------------------------------------- REW's pictures


def _restore(rew, before: dict, side: dict, put_suffixes=("settings-main", "configuration", "settings")) -> None:
    for p, v in before.items():
        if v is None:
            continue
        try:
            (rew.put if p.endswith(put_suffixes) else rew.post)(p, v)
        except Exception as e:  # noqa: BLE001 - restoring is best effort, the stage's result stands
            side.setdefault("restore_errors", []).append(f"{p}: {e}")


def _shot(rew, d: Path, name: str, graph: str, y_axis: str | None, limits: dict) -> dict:
    """Points REW's main graph at `graph` with `y_axis` and `limits`, saves it as `name`."""
    rew.post("/application/graph-main", graph)
    if y_axis:
        rew.post("/application/y-axis-main", y_axis)  # per graph: set after the graph
    rew.post("/application/graph-limits-main", limits)
    time.sleep(0.3)  # the graph redraws on its own thread; the capture takes what is drawn
    got = rew.get("/application/graph-limits-main")
    rew.command("/application/command", "Save main graph image", [str((d / name).resolve())])
    return {"file": name, "graph": graph, "limits": limits, "limits_set": got, "y_axis": rew.get("/application/y-axis-main")}


def save_graphs(rew, mid: str, d: Path, prefix: str, unit: str) -> dict:
    """Saves REW's graphs of measurement `mid` as `<prefix>_graph_<kind>.png` in `d`, the axes
    and settings as `<prefix>_graph.json`. Reads the exports the import already fetched
    (`<prefix>_fr.json` or `_fr_spl.json` for SPL, `_gd.json`, `_ir.json`) for the axes.
    REW's graph settings are put back afterwards. Returns the sidecar's content."""
    export = f"{prefix}_fr_spl.json" if unit == "SPL" else f"{prefix}_fr.json"
    curve = formats.read_rew_curve(d / export)
    limits = graph_limits(curve)
    before = {p: rew.get(p) for p in GRAPH_SETTINGS}
    n_meas = len(rew.get("/measurements") or {})
    side = {"export": export, "unit": unit, "smoothing": "None", "limits": limits, "span_db": SPAN_DB,
            "phase_axis": "not settable over the API; read from the picture's right-axis labels",
            "image_settings": dict(IMAGE_SETTINGS), "graphs": {}, "rew_version": rew.version(),
            "other_measurements": n_meas - 1}
    try:
        rew.command(f"/measurements/{mid}/command", "Smooth", [["smoothing", "None"]])
        rew.post("/measurements/selected-uuid", mid)
        rew.put("/application/image-capture-settings-main", IMAGE_SETTINGS)
        for g in GRAPHS:
            side["graphs"][g] = {**_shot(rew, d, f"{prefix}_graph_{g}.png", g, unit, limits), "kind": "tf", "export": export}
        gd_file = d / f"{prefix}_gd.json"
        if gd_file.exists():
            gl = gd_limits(formats.read_rew_curve(gd_file))
            side["graphs"]["groupdelay"] = {**_shot(rew, d, f"{prefix}_graph_groupdelay.png", "groupdelay", "ms", gl),
                                            "kind": "gd", "export": gd_file.name, "unit": "ms"}
        ir_file = d / f"{prefix}_ir.json"
        if ir_file.exists():
            il = ir_limits(formats.read_rew_ir(ir_file))
            side["graphs"]["impulse"] = {**_shot(rew, d, f"{prefix}_graph_impulse.png", "impulse", "percent", il["linear"]),
                                         "kind": "ir", "mode": "linear", "export": ir_file.name, "unit": "percent", "ir": il}
            side["graphs"]["impulse_db"] = {**_shot(rew, d, f"{prefix}_graph_impulse_db.png", "impulse", "dBFS", il["log"]),
                                            "kind": "ir", "mode": "log", "export": ir_file.name, "unit": "dBFS", "ir": il}
    finally:
        _restore(rew, before, side)
    (d / f"{prefix}_graph.json").write_text(json.dumps(side, indent=1))
    return side


def save_rta(rew, d: Path, prefix: str = "rta", seconds: float = RTA_CAPTURE_S) -> dict:
    """RTA captures on REW's input (nothing played), one per [`RTA_MODES`] entry: 64k FFT, no
    averaging, no smoothing, stopped after `seconds`; then the frozen frame's data
    (`<prefix>_<g>_data.json`, dBFS; the peak trace beside it as `<prefix>_<g>_peak.json`) and
    REW's picture of it (`<prefix>_graph_<g>.png`) with the axes from the data. Writes
    `<prefix>_graph.json`."""
    d.mkdir(parents=True, exist_ok=True)
    before = {p: rew.get(p) for p in RTA_SETTINGS}
    side = {"kind": "rta", "capture_s": seconds, "image_settings": dict(IMAGE_SETTINGS), "graphs": {}, "configuration": {},
            "rew_version": rew.version()}
    try:
        for g, (mode, bars) in RTA_MODES.items():
            cfg = dict(before["/rta/configuration"] or {})
            cfg.update({"mode": mode, "fftLength": "64k", "averaging": "None", "smoothing": "None",
                        "calcDistortionEnabled": False})
            rew.put("/rta/configuration", cfg)
            if bars is not None:
                look = dict(before["/rta/appearance-configuration"] or {})
                look["useBarsOnRTA"] = bars
                rew.put("/rta/appearance-configuration", look)
            side["configuration"][g] = rew.get("/rta/configuration")
            rew.command("/rta/command", "Start")
            time.sleep(seconds)
            rew.command("/rta/command", "Stop")
            time.sleep(0.3)
            data = rew.get("/rta/captured-data?unit=dBFS")
            export = f"{prefix}_{g}_data.json"
            (d / export).write_text(json.dumps(data))
            peak = f"{prefix}_{g}_peak.json"
            try:
                (d / peak).write_text(json.dumps(rew.get("/rta/captured-peak-data?unit=dBFS")))
            except Exception as e:  # noqa: BLE001 - the peak trace is only for telling the picture's traces apart
                side[f"{g}_peak_error"] = str(e)
                peak = None
            limits = rta_limits(formats.read_rew_curve(data))
            rew.post("/rta/y-axis", "dBFS")
            rew.put("/rta/image-capture-settings", IMAGE_SETTINGS)
            rew.post("/rta/graph-limits", limits)
            time.sleep(0.3)
            name = f"{prefix}_graph_{g}.png"
            rew.command("/rta/command", "Save graph image", [str((d / name).resolve())])
            after = rew.get("/rta/captured-data?unit=dBFS")
            side["graphs"][g] = {"file": name, "kind": "spectrum", "mode": mode, "export": export, "peak": peak, "unit": "dBFS",
                                 "limits": limits, "limits_set": rew.get("/rta/graph-limits"), "y_axis": rew.get("/rta/y-axis"),
                                 "appearance": rew.get("/rta/appearance-configuration"),
                                 "data_unchanged_after_image": after.get("magnitude") == data.get("magnitude")}
        side["export"] = side["graphs"]["rta"]["export"]
        side["unit"] = "dBFS"
        side["limits"] = side["graphs"]["rta"]["limits"]
    finally:
        _restore(rew, before, side)
    (d / f"{prefix}_graph.json").write_text(json.dumps(side, indent=1))
    return side
