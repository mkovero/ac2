"""The graph comparison: ac2's drawing of a frequency response against REW's, both made from
the same numbers.

The numeric checks say ac2's analysis agrees with REW's. They say nothing about the picture:
whether the curve the app draws sits where its axes say, and whether it is the curve REW
draws of the same data. This stage puts the same export in front of both renderers and
reads the curve back out of each picture.

Rig half (`save_graphs`, run inside the REW import): REW's main graph is pointed at the
measurement just imported, smoothing off, fixed axes computed from the export, and saved
as a PNG through REW's own image capture (`Save main graph image`): `allspl` (magnitude
alone) and `splphase` (magnitude and phase). The axes used go to `<prefix>_graph.json`.

Host half (`python -m crosscheck visual <run>`): the export REW drew is written as a text
table and drawn by `plotcheck` (tools/plotcheck: `ac2_scene::tf::transfer_scene` rendered by
`ac2_plot`, the app's own two calls) on the same axes. Each picture's curve is read back by
colour, column by column, and mapped to values: REW's through the plot frame found from
its grid, ac2's through the geometry plotcheck wrote beside the picture (and the grid found
in the picture, which must agree with it). Three comparisons per quantity:

- `ac2_vs_data_px`: ac2's drawn curve against the numbers it was given, in pixels. The
  question the stage exists for.
- `rew_vs_data_px`: REW's drawn curve against the same numbers. Validates the reader, and
  REW's own mapping.
- `ac2_vs_rew_px`: the two drawn curves against each other, in ac2's pixels.

A drawn line is wider than a pixel and a column of the picture spans a band of frequencies,
so a curve is held against the span (min…max) of the data inside its column, not a point:
the residual is the distance to that span, 0 when the line passes through it. A tolerance
in pixels is the right unit: a residual under a pixel cannot be seen.

REW's phase axis can be neither set nor read through the API, and it does not follow the
magnitude axis. The reader takes it from the picture: the right-axis labels are 60° apart,
and the one-glyph label among them is `0`. Where a label's text sits against its value is
calibrated on the left axis, whose labels stand beside grid rows of known value.

Pixels are read at their centres (row index + ½); plotcheck's geometry is in those
coordinates. REW's own curve reads half a pixel below its grid, consistently; the signed
mean offset is reported beside each residual so that such a constant stands apart from
noise."""
from __future__ import annotations

import json
import math
import os
import shutil
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from . import formats
from dataclasses import asdict

from .checks_common import Check, judge

GRAPHS = ("allspl", "splphase")
SPAN_DB = 60.0
PHASE_LABEL_STEP_DEG = 60.0  # REW labels its phase axis every 60°
IMAGE_WIDTH = 1200
# REW's capture settings for the comparison: no legend/title/cursor (text inside the plot
# would read as curve), thin lines (a thick line is a wider band around the same centre).
IMAGE_SETTINGS = {"imageWidth": IMAGE_WIDTH, "graphTextSize": 12, "traceThickness": 1, "useDarkScheme": False,
                  "includeLegend": False, "includeTitle": False, "includeCursor": False, "includeTimestamp": False,
                  "includeBorder": False, "monochrome": False, "scaleOverlays": False, "asPNG": True, "comment": ""}
GRAPH_SETTINGS = ("/application/graph-main", "/application/y-axis-main", "/application/graph-limits-main",
                  "/application/image-capture-settings-main")


# ---------------------------------------------------------------- rig half


def graph_limits(curve: formats.RewCurve, lo_hz: float = 20.0, hi_hz: float = 20_000.0) -> dict:
    """Axes for the pictures: 20 Hz – 20 kHz, and `SPAN_DB` of level with the curve's maximum
    just under the top, on 5 dB marks. The phase (REW's right axis, not settable) then sits
    in the lower part of the frame for a flat path."""
    m = (curve.f >= lo_hz) & (curve.f <= hi_hz) & np.isfinite(curve.mag)
    top = 5.0 * math.ceil(float(np.max(curve.mag[m])) / 5.0) + 5.0
    return {"left": lo_hz, "right": hi_hz, "top": top, "bottom": top - SPAN_DB}


def save_graphs(rew, mid: str, d: Path, prefix: str, unit: str) -> dict:
    """Saves REW's graphs of measurement `mid` as `<prefix>_graph_<kind>.png` in `d`, the axes
    and settings as `<prefix>_graph.json`. Reads the export the import already fetched
    (`<prefix>_fr.json`, or `_fr_spl.json` for SPL) for the axes. REW's graph settings are
    put back afterwards. Returns the sidecar's content."""
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
            rew.post("/application/graph-main", g)
            rew.post("/application/y-axis-main", unit)
            rew.post("/application/graph-limits-main", limits)
            time.sleep(0.3)  # the graph redraws on its own thread; the capture takes what is drawn
            got = rew.get("/application/graph-limits-main")
            name = f"{prefix}_graph_{g}.png"
            rew.command("/application/command", "Save main graph image", [str((d / name).resolve())])
            side["graphs"][g] = {"file": name, "limits_set": got, "y_axis": rew.get("/application/y-axis-main")}
    finally:
        for p in GRAPH_SETTINGS:
            if before.get(p) is not None:
                try:
                    (rew.put if p.endswith("settings-main") else rew.post)(p, before[p])
                except Exception as e:  # noqa: BLE001 - restoring is best effort, the stage's result stands
                    side.setdefault("restore_errors", []).append(f"{p}: {e}")
    (d / f"{prefix}_graph.json").write_text(json.dumps(side, indent=1))
    return side


# ---------------------------------------------------------------- picture reading


@dataclass
class Mapping:
    """Pixel ↔ value of one picture: x log in frequency between `x0` (at `f0`) and `x1` (at
    `f1`); y linear between `y_top` (value `v_top`) and `y_bot` (value `v_bot`)."""
    x0: float
    x1: float
    f0: float
    f1: float
    y_top: float
    y_bot: float
    v_top: float
    v_bot: float

    def f_of_x(self, x):
        t = (np.asarray(x, float) - self.x0) / (self.x1 - self.x0)
        return self.f0 * (self.f1 / self.f0) ** t

    def x_of_f(self, f):
        return self.x0 + (self.x1 - self.x0) * np.log(np.asarray(f, float) / self.f0) / np.log(self.f1 / self.f0)

    def v_of_y(self, y):
        return self.v_top + (np.asarray(y, float) - self.y_top) / (self.y_bot - self.y_top) * (self.v_bot - self.v_top)

    @property
    def px_per_unit(self) -> float:
        return abs(self.y_bot - self.y_top) / abs(self.v_bot - self.v_top)


def load_rgb(path: Path) -> np.ndarray:
    from PIL import Image
    return np.asarray(Image.open(path).convert("RGB")).astype(np.int32)


def find_frame_light(im: np.ndarray) -> dict:
    """REW's picture: white page, grey grid. The plot frame is the extent of the grid: the
    rows and columns most of whose pixels are grid grey. Also the darker decade lines
    (100, 1k, 10k) for an independent check of the frequency mapping."""
    lum = im.mean(axis=2)
    sat = im.max(axis=2) - im.min(axis=2)
    h, w = lum.shape
    grid = (lum < 240) & (lum > 150) & (sat < 20)
    gx = np.where(grid.sum(axis=0) > 0.5 * h)[0]
    gy = np.where(grid.sum(axis=1) > 0.5 * w)[0]
    if len(gx) < 4 or len(gy) < 4:
        raise ValueError("no grid found in the REW picture")
    dark = (lum < 140) & (sat < 30)
    dx = np.where(dark.sum(axis=0) > 0.5 * h)[0]
    return {"x0": gx[0] + 0.5, "x1": gx[-1] + 0.5, "y_top": gy[0] + 0.5, "y_bot": gy[-1] + 0.5,
            "decade_lines": [c + 0.5 for c in _group(dx)], "major_rows": [r + 0.5 for r in _group(gy)]}


def label_bands(im: np.ndarray, x_from: int, x_to: int, y_from: int, y_to: int) -> list[dict]:
    """Rows of text in a strip of the picture: each band's centre (pixel-centre coordinates)
    and width in pixels. Text is the dark ink; the strip must hold nothing else."""
    lum = im[y_from:y_to, x_from:x_to].mean(axis=2)
    dark = lum < 120
    rows = np.where(dark.any(axis=1))[0]
    out = []
    for y in _runs(rows):
        band = dark[y[0]:y[-1] + 1]
        cols = np.where(band.any(axis=0))[0]
        out.append({"y": (y[0] + y[-1]) / 2 + 0.5 + y_from, "width": int(cols.max() - cols.min() + 1),
                    "height": int(y[-1] - y[0] + 1)})
    return out


def _runs(xs: np.ndarray) -> list[list[int]]:
    out: list[list[int]] = []
    run: list[int] = []
    for x in xs:
        if run and x != run[-1] + 1:
            out.append(run)
            run = []
        run.append(int(x))
    if run:
        out.append(run)
    return out


def rew_phase_axis(im: np.ndarray, frame: dict, limits: dict) -> dict:
    """REW's right (phase) axis from its labels: 60° per label, the narrowest label is `0`.
    The text's vertical centre against its value is calibrated on the left axis, whose
    labels stand beside grid rows: there a 5 dB label's band centre minus its row gives the
    offset, applied to the right axis. Returns the row of 0° and the pixels per degree,
    with what it was read from."""
    h, w = im.shape[:2]
    x1 = int(round(frame["x1"]))
    right = [b for b in label_bands(im, x1 + 3, w, 0, h) if frame["y_top"] - 10 < b["y"] < frame["y_bot"] + 2]
    if len(right) < 3:
        raise ValueError("no phase labels right of the frame")
    steps = np.diff([b["y"] for b in right])
    step = float(np.median(steps))
    keep = [right[0]] + [b for b, d in zip(right[1:], steps) if abs(d - step) < 0.25 * step]
    if len(keep) < 3:
        raise ValueError("phase labels are not evenly spaced")
    zero = min(keep, key=lambda b: b["width"])
    # left axis: label bands against the major grid rows (5 dB apart on a 60 dB span)
    x0 = int(round(frame["x0"]))
    left = [b for b in label_bands(im, 0, x0 - 2, 0, h) if frame["y_top"] < b["y"] < frame["y_bot"]]
    rows = np.array(frame["major_rows"])
    offs = []
    for b in left:
        r = rows[np.argmin(np.abs(rows - b["y"]))]
        if abs(r - b["y"]) < 4:
            offs.append(b["y"] - r)
    text_offset = float(np.median(offs)) if offs else 0.0
    return {"y_zero": zero["y"] - text_offset, "px_per_deg": step / PHASE_LABEL_STEP_DEG, "labels": len(keep),
            "zero_label_width": zero["width"], "text_offset_px": text_offset, "left_labels_used": len(offs),
            "v_top": (zero["y"] - text_offset - frame["y_top"]) / (step / PHASE_LABEL_STEP_DEG),
            "v_bot": (zero["y"] - text_offset - frame["y_bot"]) / (step / PHASE_LABEL_STEP_DEG)}


def _group(xs: np.ndarray) -> list[float]:
    """Centres of runs of adjacent pixel columns."""
    out: list[float] = []
    run: list[int] = []
    for x in xs:
        if run and x != run[-1] + 1:
            out.append(float(np.mean(run)))
            run = []
        run.append(int(x))
    if run:
        out.append(float(np.mean(run)))
    return out


def find_grid_lines_dark(im: np.ndarray, rect: dict) -> dict:
    """ac2's picture: the lines brighter than the plot background, with no colour, that run
    the height (or width) of the plot. Returns their centres in pixels."""
    x0, y0, x1, y1 = (int(round(rect["x"])), int(round(rect["y"])), int(round(rect["x"] + rect["w"])),
                      int(round(rect["y"] + rect["h"])))
    sub = im[y0:y1, x0:x1]
    lum = sub.mean(axis=2)
    sat = sub.max(axis=2) - sub.min(axis=2)
    base = float(np.median(lum))
    line = (lum > base + 6) & (sat < 20)
    h, w = lum.shape
    cols = np.where(line.sum(axis=0) > 0.7 * h)[0] + x0
    rows = np.where(line.sum(axis=1) > 0.7 * w)[0] + y0
    # a line's centre, and its brightness: the major lines are the brighter ones
    col_runs = [(np.mean(r) + 0.5, float(lum[:, [x - x0 for x in r]].mean())) for r in _runs(cols)]
    row_runs = [(np.mean(r) + 0.5, float(lum[[y - y0 for y in r], :].mean())) for r in _runs(rows)]
    return {"cols": [c for c, _ in col_runs], "rows": [r for r, _ in row_runs],
            "col_strength": [s for _, s in col_runs], "row_strength": [s for _, s in row_runs]}


def read_curve(im: np.ndarray, x0: int, x1: int, y0: int, y1: int, hue: str = "any", min_sat: int = 25) -> np.ndarray:
    """The drawn curve: for each pixel column in `x0..x1`, the saturation-weighted mean row of
    the coloured pixels in `y0..y1` (NaN where none). Grid, text and background are grey on
    both renderers; the trace is the only colour. `hue` picks one of two traces drawn in
    different colours (`blue`: b > r, `red`: r > b)."""
    sub = im[y0:y1 + 1, x0:x1 + 1]
    sat = sub.max(axis=2) - sub.min(axis=2)
    mask = sat >= min_sat
    if hue == "blue":
        mask &= sub[..., 2] > sub[..., 0] + 20
    elif hue == "red":
        mask &= sub[..., 0] > sub[..., 2] + 20
    w = np.where(mask, sat, 0).astype(float)
    rows = np.arange(y0, y1 + 1, dtype=float)[:, None] + 0.5  # pixel centres
    tot = w.sum(axis=0)
    with np.errstate(invalid="ignore", divide="ignore"):
        y = (w * rows).sum(axis=0) / tot
    y[tot == 0] = np.nan
    return y


# ---------------------------------------------------------------- comparison


@dataclass
class CurveRead:
    """One drawn curve read back: pixel column, frequency, drawn value, the data's span in
    that column, and the residual (distance to the span, 0 inside) in value units."""
    x: np.ndarray
    f: np.ndarray
    drawn: np.ndarray
    lo: np.ndarray
    hi: np.ndarray
    px_per_unit: float

    @property
    def residual(self) -> np.ndarray:
        r = np.where(self.drawn < self.lo, self.lo - self.drawn, np.where(self.drawn > self.hi, self.drawn - self.hi, 0.0))
        return np.where(np.isfinite(self.drawn), r, np.nan)

    def stats(self) -> dict:
        r = self.residual
        ok = np.isfinite(r)
        px = r * self.px_per_unit
        signed = (self.drawn - (self.lo + self.hi) / 2) * self.px_per_unit
        return {"columns": int(ok.sum()), "empty_columns": int((~ok).sum()),
                "mean_signed_px": float(np.nanmean(signed)) if ok.any() else None,
                "max_units": float(np.nanmax(r)) if ok.any() else None,
                "p95_units": float(np.nanpercentile(r, 95)) if ok.any() else None,
                "max_px": float(np.nanmax(px)) if ok.any() else None,
                "p95_px": float(np.nanpercentile(px, 95)) if ok.any() else None,
                "worst_hz": float(self.f[int(np.nanargmax(r))]) if ok.any() else None}


def data_span(f: np.ndarray, v: np.ndarray, mapping: Mapping, xs: np.ndarray, wrap: bool = False):
    """For each pixel column (centre `x`), the min and max of the data whose frequency falls
    in [f(x − ½), f(x + ½)], the data interpolated at the edges included (a column with no
    sample inside still has the line crossing it). Wrapped phase: a span across the ±180
    seam is split, and the drawn value is held against the nearer branch later."""
    fe_lo = mapping.f_of_x(xs - 0.5)
    fe_hi = mapping.f_of_x(xs + 0.5)
    logf = np.log(f)
    ok = np.isfinite(v)
    f_ok, v_ok, lf_ok = f[ok], v[ok], logf[ok]
    lo = np.full(len(xs), np.nan)
    hi = np.full(len(xs), np.nan)
    i0 = np.searchsorted(f_ok, fe_lo, side="left")
    i1 = np.searchsorted(f_ok, fe_hi, side="right")
    for k, (a, b) in enumerate(zip(i0, i1)):
        if fe_lo[k] < f_ok[0] or fe_hi[k] > f_ok[-1]:
            continue
        vals = [np.interp(np.log(fe_lo[k]), lf_ok, v_ok), np.interp(np.log(fe_hi[k]), lf_ok, v_ok)]
        if b > a:
            seg = v_ok[a:b]
            vals.extend([float(seg.min()), float(seg.max())])
        if wrap:
            # a step of more than 180° between neighbours in the column is the seam, not the data
            allv = np.array(vals + ([float(x) for x in v_ok[a:b]] if b > a else []))
            if allv.max() - allv.min() > 180.0:
                lo[k], hi[k] = -180.0, 180.0
                continue
        lo[k], hi[k] = min(vals), max(vals)
    return lo, hi


def compare_curve(drawn_y: np.ndarray, xs: np.ndarray, mapping: Mapping, f: np.ndarray, v: np.ndarray,
                  wrap: bool = False) -> CurveRead:
    drawn = mapping.v_of_y(drawn_y)
    lo, hi = data_span(f, v, mapping, xs, wrap=wrap)
    return CurveRead(x=xs, f=mapping.f_of_x(xs), drawn=drawn, lo=lo, hi=hi, px_per_unit=mapping.px_per_unit)


def rew_mapping(frame: dict, limits: dict, v_top: float, v_bot: float) -> Mapping:
    return Mapping(x0=frame["x0"], x1=frame["x1"], f0=limits["left"], f1=limits["right"],
                   y_top=frame["y_top"], y_bot=frame["y_bot"], v_top=v_top, v_bot=v_bot)


def ac2_mapping(meta: dict, pane: dict) -> Mapping:
    s = float(meta["scale"])
    xa, ya = meta["x_axis"], pane["y_axis"]
    return Mapping(x0=xa["px_lo"] * s, x1=xa["px_hi"] * s, f0=xa["range"]["lo"], f1=xa["range"]["hi"],
                   y_top=ya["px_hi"] * s, y_bot=ya["px_lo"] * s, v_top=ya["range"]["hi"], v_bot=ya["range"]["lo"])


def decade_check(mapping: Mapping, found: list[float]) -> dict:
    """The decade lines a picture shows against where the mapping puts 100 Hz, 1 kHz, 10 kHz."""
    want = {hz: float(mapping.x_of_f(hz)) for hz in (100.0, 1000.0, 10_000.0) if mapping.f0 < hz < mapping.f1}
    out = {}
    for hz, x in want.items():
        near = min(found, key=lambda c: abs(c - x)) if found else None
        out[str(int(hz))] = {"expected_px": x, "found_px": near, "error_px": None if near is None else near - x}
    errs = [abs(v["error_px"]) for v in out.values() if v["error_px"] is not None]
    return {"lines": out, "max_error_px": max(errs) if errs else None}


# ---------------------------------------------------------------- the host half


def find_plotcheck(explicit: str | None = None) -> str | None:
    if explicit:
        return explicit
    env = os.environ.get("PLOTCHECK")
    if env:
        return env
    return shutil.which("plotcheck")


def render_ac2(plotcheck: str, csv: Path, out_png: Path, width: int, height: int, limits: dict, phase: bool) -> dict:
    cmd = [plotcheck, "render", "--input", str(csv), "--out", str(out_png), "--width", str(width), "--height", str(height),
           f"--freq-lo={limits['left']}", f"--freq-hi={limits['right']}", f"--db-lo={limits['bottom']}",
           f"--db-hi={limits['top']}"]
    if phase:
        cmd.append("--phase")
    subprocess.run(cmd, check=True, capture_output=True, text=True)
    return json.loads(out_png.with_suffix(".json").read_text())


def write_csv(curve: formats.RewCurve, path: Path) -> None:
    with path.open("w") as fh:
        fh.write("freq_hz,mag_db,phase_deg\n" if curve.phase is not None else "freq_hz,mag_db\n")
        for i in range(len(curve.f)):
            if curve.phase is not None:
                fh.write(f"{curve.f[i]:.6f},{curve.mag[i]:.5f},{curve.phase[i]:.4f}\n")
            else:
                fh.write(f"{curve.f[i]:.6f},{curve.mag[i]:.5f}\n")


@dataclass
class PathResult:
    name: str
    prefix: str
    checks: list[Check] = field(default_factory=list)
    detail: dict = field(default_factory=dict)
    images: dict = field(default_factory=dict)


def _tol(tol: dict | None, key: str):
    t = (tol or {}).get("visual", {}).get(key)
    return (float(t[0]), float(t[1])) if t else None


def analyse_path(run: Path, pname: str, side_path: Path, out: Path, plotcheck: str, tol: dict | None) -> PathResult:
    d = side_path.parent
    prefix = side_path.name[: -len("_graph.json")]
    side = json.loads(side_path.read_text())
    limits = side["limits"]
    curve = formats.read_rew_curve(d / side["export"])
    res = PathResult(name=pname, prefix=prefix, detail={"limits": limits, "unit": side["unit"],
                                                        "other_measurements": side.get("other_measurements")})
    out.mkdir(parents=True, exist_ok=True)
    csv = out / f"{prefix}.csv"
    write_csv(curve, csv)
    t_curve = _tol(tol, "curve_px")
    t_axis = _tol(tol, "axis_px")
    cross = _tol(tol, "cross_px")
    for g in GRAPHS:
        gfile = d / side["graphs"][g]["file"]
        if not gfile.exists():
            continue
        rew_im = load_rgb(gfile)
        h_px, w_px = rew_im.shape[:2]
        frame = find_frame_light(rew_im)
        phase = g == "splphase"
        ac2_png = out / f"ac2_{prefix}_{g}.png"
        meta = render_ac2(plotcheck, csv, ac2_png, w_px, h_px, limits, phase)
        res.images[g] = {"rew": os.path.relpath(gfile, out), "ac2": ac2_png.name}
        ac2_im = load_rgb(ac2_png)
        quantities = [("mag", "magnitude", curve.mag, limits["top"], limits["bottom"], False, "blue" if phase else "any")]
        if phase and curve.phase is not None:
            pa = rew_phase_axis(rew_im, frame, limits)
            res.detail[f"{g}.rew_phase_axis"] = pa
            quantities.append(("phase", "phase", curve.phase, pa["v_top"], pa["v_bot"], True, "red"))
        for q, pane_kind, values, v_top, v_bot, wrap, hue in quantities:
            key = f"visual.{pname}.{g}.{q}"
            # REW
            rm = rew_mapping(frame, limits, v_top, v_bot)
            fx0, fx1 = int(frame["x0"]), int(frame["x1"])  # the frame's own pixel columns
            xs = np.arange(fx0 + 1, fx1, dtype=float) + 0.5
            ry = read_curve(rew_im, fx0 + 1, fx1 - 1, int(frame["y_top"]), int(frame["y_bot"]), hue=hue)
            rr = compare_curve(ry, xs, rm, curve.f, values, wrap=wrap)
            rs = rr.stats()
            # ac2
            pane = next(p for p in meta["panes"] if p["kind"] == pane_kind)
            am = ac2_mapping(meta, pane)
            s = float(meta["scale"])
            px0, py0 = int(round(pane["plot"]["x"] * s)), int(round(pane["plot"]["y"] * s))
            px1, py1 = int(round((pane["plot"]["x"] + pane["plot"]["w"]) * s)) - 1, int(round((pane["plot"]["y"] + pane["plot"]["h"]) * s)) - 1
            axs = np.arange(px0 + 1, px1, dtype=float) + 0.5
            ay = read_curve(ac2_im, px0 + 1, px1 - 1, py0, py1, hue="any")
            ar = compare_curve(ay, axs, am, curve.f, values, wrap=wrap)
            as_ = ar.stats()
            # the two drawings against each other, on a common frequency axis (ac2's columns)
            fin = np.isfinite(rr.drawn)  # a dotted line leaves columns empty
            if fin.sum() < 2:
                diff = np.full(len(ar.f), np.nan)
            elif wrap:
                rd = np.interp(np.log(ar.f), np.log(rr.f[fin]), np.unwrap(np.deg2rad(rr.drawn[fin])))
                diff = np.rad2deg(np.angle(np.exp(1j * (np.deg2rad(ar.drawn) - rd))))
            else:
                diff = ar.drawn - np.interp(np.log(ar.f), np.log(rr.f[fin]), rr.drawn[fin])
            inside = (ar.f > limits["left"] * 1.02) & (ar.f < limits["right"] / 1.02)
            dd = np.where(inside, diff, np.nan)
            cross_px = float(np.nanmax(np.abs(dd))) * am.px_per_unit if np.isfinite(dd).any() else None
            res.detail[f"{g}.{q}"] = {"rew": rs, "ac2": as_, "ac2_vs_rew": {
                "max_units": float(np.nanmax(np.abs(dd))) if cross_px is not None else None, "max_px": cross_px,
                "worst_hz": float(ar.f[int(np.nanargmax(np.abs(dd)))]) if cross_px is not None else None},
                "px_per_unit": {"rew": rm.px_per_unit, "ac2": am.px_per_unit}}
            unit = "dB" if q == "mag" else "°"
            res.checks.append(Check(id=f"{key}.ac2_vs_data_px", group="visual", path=pname, title=f"{g} {q}: ac2 drawn vs data", value=as_["max_px"], unit="px", status=judge(as_["max_px"], t_curve),
                                    tol=t_curve, detail={"p95_px": as_["p95_px"], "mean_signed_px": as_["mean_signed_px"], "max_units": as_["max_units"], "worst_hz": as_["worst_hz"],
                                                               "columns": as_["columns"], "empty": as_["empty_columns"], "unit": unit},
                                    meaning=f"ac2's drawn {q} against the numbers it drew, worst column, pixels"))
            res.checks.append(Check(id=f"{key}.rew_vs_data_px", group="visual", path=pname, title=f"{g} {q}: REW drawn vs data", value=rs["max_px"], unit="px", status=judge(rs["max_px"], t_curve),
                                    tol=t_curve, detail={"p95_px": rs["p95_px"], "mean_signed_px": rs["mean_signed_px"], "max_units": rs["max_units"], "worst_hz": rs["worst_hz"],
                                                               "columns": rs["columns"], "empty": rs["empty_columns"], "unit": unit},
                                    meaning=f"REW's drawn {q} against the same numbers, worst column, pixels"))
            res.checks.append(Check(id=f"{key}.ac2_vs_rew_px", group="visual", path=pname, title=f"{g} {q}: ac2 drawn vs REW drawn", value=cross_px, unit="px", status=judge(cross_px, cross),
                                    tol=cross, detail=res.detail[f"{g}.{q}"]["ac2_vs_rew"],
                                    meaning=f"the two drawn {q} curves against each other, in ac2's pixels"))
        # axis placement: the decade lines each picture shows against its mapping
        rdc = decade_check(rew_mapping(frame, limits, limits["top"], limits["bottom"]), frame["decade_lines"])
        pane = next(p for p in meta["panes"] if p["kind"] == "magnitude")
        am = ac2_mapping(meta, pane)
        lines = find_grid_lines_dark(ac2_im, {k: v * float(meta["scale"]) for k, v in pane["plot"].items()})
        cut = np.percentile(lines["col_strength"], 75) if lines["col_strength"] else 0
        major = [c for c, st in zip(lines["cols"], lines["col_strength"]) if st >= cut]
        adc = decade_check(am, major or lines["cols"])
        res.detail[f"{g}.axes"] = {"rew_frame": frame, "rew_decades": rdc, "ac2_decades": adc, "ac2_meta_x": meta["x_axis"]["px_lo"],
                                   "adapter": meta.get("adapter")}
        res.checks.append(Check(id=f"visual.{pname}.{g}.axis.rew_decade_px", group="visual", path=pname, title=f"{g}: REW decade lines", value=rdc["max_error_px"], unit="px",
                                status=judge(rdc["max_error_px"], t_axis), tol=t_axis, detail=rdc["lines"],
                                meaning="REW's 100 Hz / 1 kHz / 10 kHz lines against the frame's log mapping"))
        res.checks.append(Check(id=f"visual.{pname}.{g}.axis.ac2_decade_px", group="visual", path=pname, title=f"{g}: ac2 decade lines", value=adc["max_error_px"], unit="px",
                                status=judge(adc["max_error_px"], t_axis), tol=t_axis, detail=adc["lines"],
                                meaning="ac2's 100 Hz / 1 kHz / 10 kHz grid lines against the axis plotcheck reported"))
        _composite(gfile, ac2_png, out / f"{prefix}_{g}_side_by_side.png")
        res.images[g]["side_by_side"] = f"{prefix}_{g}_side_by_side.png"
    return res


def _composite(a: Path, b: Path, out: Path) -> None:
    from PIL import Image
    ia, ib = Image.open(a).convert("RGB"), Image.open(b).convert("RGB")
    h = max(ia.height, ib.height)
    im = Image.new("RGB", (ia.width + ib.width + 8, h), (128, 128, 128))
    im.paste(ia, (0, 0))
    im.paste(ib, (ia.width + 8, 0))
    im.save(out, optimize=True)


def analyse(run: Path, plotcheck: str | None = None, tol: dict | None = None, out: Path | None = None) -> dict:
    run = Path(run).resolve()
    pc = find_plotcheck(plotcheck)
    if not pc:
        raise FileNotFoundError("plotcheck not found: build it (cargo build -p ac2-plotcheck) and pass --plotcheck or set PLOTCHECK")
    if tol is None:
        import tomllib
        from .checks_common import TOLERANCES
        tol = tomllib.loads(TOLERANCES.read_text())
    out = out or (run / "visual")
    out.mkdir(parents=True, exist_ok=True)
    results: list[PathResult] = []
    for side in sorted(run.glob("*/rew/*_graph.json")):
        pname = side.parent.parent.name
        results.append(analyse_path(run, pname, side, out / pname, pc, tol))
    doc = {"run": str(run), "plotcheck": pc, "when": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
           "paths": {r.name: {"prefix": r.prefix, "checks": [asdict(c) for c in r.checks], "detail": r.detail,
                              "images": r.images} for r in results}}
    (out / "results.json").write_text(json.dumps(doc, indent=1, default=_json_default))
    (out / "visual.md").write_text(report(doc))
    return doc


def _json_default(o):
    if isinstance(o, (np.floating, np.integer)):
        return o.item()
    if isinstance(o, np.ndarray):
        return o.tolist()
    raise TypeError(type(o))


def report(doc: dict) -> str:
    lines = ["# Graphs: ac2 against REW on the same numbers", "",
             "Each picture's curve read back by colour and held against the export both drew. Residuals in "
             "pixels of the picture they were read from (a column spans a band of frequencies; the residual is "
             "the distance to the data's span in that band, 0 when the line passes through it).", ""]
    counts: dict[str, int] = {}
    for p in doc["paths"].values():
        for c in p["checks"]:
            counts[c["status"]] = counts.get(c["status"], 0) + 1
    lines.append("Verdicts: " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())) + ".")
    lines.append("")
    for name, p in doc["paths"].items():
        lines += [f"## {name} ({p['prefix']}, {p['detail'].get('unit')})", ""]
        if p["detail"].get("other_measurements"):
            lines += [f"REW held {p['detail']['other_measurements']} other measurement(s) when the pictures were taken: "
                      "`allspl` may show more than one curve.", ""]
        lines += ["| check | value | verdict | worst at | notes |", "|---|---|---|---|---|"]
        for c in p["checks"]:
            d = c.get("detail") or {}
            worst = d.get("worst_hz")
            notes = []
            if "max_units" in d and d["max_units"] is not None:
                notes.append(f"{d['max_units']:.3g} {d.get('unit', '')}".strip())
            if "p95_px" in d and d["p95_px"] is not None:
                notes.append(f"p95 {d['p95_px']:.2f} px")
            if d.get("mean_signed_px") is not None:
                notes.append(f"mean {d['mean_signed_px']:+.2f} px")
            if d.get("empty"):
                notes.append(f"{d['empty']} empty columns")
            v = "—" if c["value"] is None else f"{c['value']:.2f} {c.get('unit', '')}"
            lines.append(f"| `{c['id']}` | {v} | {c['status']} | {'' if worst is None else f'{worst:.0f} Hz'} | {'; '.join(notes)} |")
        lines.append("")
        for g, im in p["images"].items():
            if "side_by_side" in im:
                lines += [f"### {g}: REW (left) and ac2 (right)", "", f"![{name} {g}]({name}/{im['side_by_side']})", ""]
    lines += ["REW's phase axis is read from the picture's right-axis labels (60° apart, the one-glyph label is 0°); "
              "the API neither sets nor reports it. A signed mean is the drawn line's constant offset from the value, "
              "in pixels, positive downwards; a residual is what remains outside the data's span in the column.", ""]
    return "\n".join(lines)


def main(a) -> int:
    doc = analyse(a.run, plotcheck=a.plotcheck, out=a.out)
    n = {"PASS": 0, "WARN": 0, "FAIL": 0, "INCONCLUSIVE": 0, "INFO": 0}
    for p in doc["paths"].values():
        for c in p["checks"]:
            n[c["status"]] = n.get(c["status"], 0) + 1
    out = a.out or (Path(a.run) / "visual")
    print(f"visual: {len(doc['paths'])} path(s); FAIL {n['FAIL']}, WARN {n['WARN']}, PASS {n['PASS']}; report {out / 'visual.md'}")
    return 1 if n["FAIL"] else 0
