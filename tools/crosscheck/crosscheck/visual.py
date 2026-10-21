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

from .visual_rew import graph_limits  # the axes the rig half gave REW; the host half gives ac2 the same

PHASE_LABEL_STEP_DEG = 60.0  # REW labels its phase axis every 60°


# ---------------------------------------------------------------- picture reading


@dataclass
class Mapping:
    """Pixel ↔ value of one picture: x between `x0` (at abscissa `f0`) and `x1` (at `f1`),
    logarithmic in the abscissa (frequency) unless `xlog` is false (time); y linear between
    `y_top` (value `v_top`) and `y_bot` (value `v_bot`). `u` is the axis coordinate the
    abscissa is linear in (log f, or t): both renderers interpolate linearly in it."""
    x0: float
    x1: float
    f0: float
    f1: float
    y_top: float
    y_bot: float
    v_top: float
    v_bot: float
    xlog: bool = True

    def u(self, f):
        return np.log(np.asarray(f, float)) if self.xlog else np.asarray(f, float)

    def f_of_x(self, x):
        t = (np.asarray(x, float) - self.x0) / (self.x1 - self.x0)
        return self.f0 * (self.f1 / self.f0) ** t if self.xlog else self.f0 + t * (self.f1 - self.f0)

    def x_of_f(self, f):
        if self.xlog:
            t = np.log(np.asarray(f, float) / self.f0) / np.log(self.f1 / self.f0)
        else:
            t = (np.asarray(f, float) - self.f0) / (self.f1 - self.f0)
        return self.x0 + (self.x1 - self.x0) * t

    def v_of_y(self, y):
        return self.v_top + (np.asarray(y, float) - self.y_top) / (self.y_bot - self.y_top) * (self.v_bot - self.v_top)

    def y_of_v(self, v):
        return self.y_top + (self.v_top - np.asarray(v, float)) / (self.v_top - self.v_bot) * (self.y_bot - self.y_top)

    @property
    def px_per_unit(self) -> float:
        return abs(self.y_bot - self.y_top) / abs(self.v_bot - self.v_top)

    @property
    def v_range(self) -> tuple[float, float]:
        return min(self.v_top, self.v_bot), max(self.v_top, self.v_bot)


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
    # The impulse graph hangs an overview strip under the plot with a frame of its own, so
    # the last grey row is not the plot's bottom. A horizontal line belongs to the plot when
    # the grid columns run through the rows just above it; above the strip's lines there is
    # label text or the strip's own interior, where they do not.
    def plot_row(r: int) -> bool:
        band = grid[max(gy[0], r - 5):max(gy[0], r - 1)][:, gx]
        return band.size == 0 or band.any(axis=0).mean() >= 0.5
    gy = gy[[plot_row(int(r)) for r in gy]]
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


def hue_deg(sub: np.ndarray) -> np.ndarray:
    """Hue angle (degrees) of each pixel of an RGB array; 0 where grey."""
    rgb = sub.astype(float)
    mx, mn = rgb.max(axis=2), rgb.min(axis=2)
    c = mx - mn
    r, g, b = rgb[..., 0], rgb[..., 1], rgb[..., 2]
    with np.errstate(invalid="ignore", divide="ignore"):
        h = np.where(mx == r, (g - b) / c, np.where(mx == g, 2 + (b - r) / c, 4 + (r - g) / c))
    h = np.where(c > 0, h, 0.0)
    return (h * 60.0) % 360.0


def trace_hues(sub: np.ndarray, min_sat: int = 60, bins: int = 36) -> list[dict]:
    """The coloured traces in a region, told apart by hue: clusters of the saturated pixels'
    hue angle (10° bins, a cluster the peak bin and its two neighbours each side), each with
    its pixel count and the pixel columns it covers, ordered solid first (most columns, then
    most pixels). REW colours a measurement by its place in the session, so no fixed colour
    names a trace; the picture does."""
    sat = sub.max(axis=2).astype(int) - sub.min(axis=2).astype(int)
    on = sat >= min_sat
    if not on.any():
        return []
    h = hue_deg(sub)
    b = (h[on] / (360.0 / bins)).astype(int) % bins
    cols = np.nonzero(on)[1]
    hist = np.bincount(b, minlength=bins).astype(float)
    total = hist.sum()
    out = []
    while len(out) < 4 and hist.max() >= 0.01 * total:
        k = int(hist.argmax())
        members = [(k + d) % bins for d in range(-2, 3)]
        sel = np.isin(b, members)
        out.append({"hue": float(k * 360.0 / bins), "pixels": int(sel.sum()), "columns": int(len(np.unique(cols[sel])))})
        for d in range(-4, 5):
            hist[(k + d) % bins] = 0.0
    out.sort(key=lambda t: (t["columns"], t["pixels"]), reverse=True)
    return out


def read_curve(im: np.ndarray, x0: int, x1: int, y0: int, y1: int, hue: str = "any", min_sat: int = 25,
               stat: str = "centroid") -> np.ndarray:
    """The drawn curve's row in each pixel column `x0..x1`: the saturation-weighted mean row of
    the coloured pixels in `y0..y1` (NaN where none), or with `stat="top"` the line's upper
    edge (the first row of its core, half a pixel down: the centre of a line that is all
    edge; a log IR hangs a hair down to the floor at every zero crossing, and its curve is
    the envelope above them). Grid, text and background are grey on both renderers; the
    trace is the only colour. `hue` picks one of two traces drawn in
    different colours: `dominant` the solid one (see [`trace_hues`]), `second` the other
    (REW's dotted phase); `blue` (b > r) and `red` (r > b, so orange and yellow count) name a
    colour outright; `dark` reads a black trace instead (REW's RTA draws the captured spectrum
    in black), weighted by its darkness."""
    sub = im[y0:y1 + 1, x0:x1 + 1]
    sat = sub.max(axis=2) - sub.min(axis=2)
    if hue == "dark":
        # REW's trace is a one-pixel black line, anti-aliased to a grey as light as luminance
        # 150 where it is steep or straddles two columns; its grid lines are greys of their
        # own (about 65, 130 or 190 by graph and rank). A grid line runs the whole width or
        # height, so a row or column whose median is not the background is one, and a trace
        # pixel on it is darker than the line by the trace's own coverage: 0.6 × the line.
        # Unweighted: a steep stretch is faint but is the trace all the same (RTA spikes).
        lum = sub.mean(axis=2)
        med_r, med_c = np.median(lum, axis=1), np.median(lum, axis=0)
        thr = np.full(lum.shape, 150.0)
        thr = np.minimum(thr, np.where(med_r < 220, 0.6 * med_r, 255.0)[:, None])
        thr = np.minimum(thr, np.where(med_c < 220, 0.6 * med_c, 255.0)[None, :])
        mask = (lum < thr) & (sat < 60)
        w = mask.astype(float)
    else:
        mask = sat >= min_sat
        if hue == "blue":
            mask &= sub[..., 2] > sub[..., 0] + 20
        elif hue == "red":
            mask &= sub[..., 0] > sub[..., 2] + 20
        elif hue in ("dominant", "second"):
            traces = trace_hues(sub)
            want = traces[0 if hue == "dominant" else 1]["hue"] if len(traces) > (hue == "second") else None
            if want is None:
                mask &= False
            else:
                dist = np.abs((hue_deg(sub) - want + 180.0) % 360.0 - 180.0)
                mask &= dist <= 25.0
        w = np.where(mask, sat, 0).astype(float)
    rows = np.arange(y0, y1 + 1, dtype=float)[:, None] + 0.5  # pixel centres
    tot = w.sum(axis=0)
    if stat == "top":
        # the stroke's core: pixels the line covers by at least 40 % (a solid pixel's
        # saturation is the trace's own; the dark mask is already that cut), so a steep
        # segment's faint fringe counts the same on both renderers
        full = float(np.percentile(w[w > 0], 95)) if (w > 0).any() else 1.0
        core = w >= 0.4 * full
        y = rows[np.argmax(core, axis=0), 0] + 0.5
    else:
        with np.errstate(invalid="ignore", divide="ignore"):
            y = (w * rows).sum(axis=0) / tot
    y[tot == 0] = np.nan
    return y


@dataclass
class CurveRead:
    """One drawn curve read back: pixel column, abscissa, drawn value, the data's span in
    that column, and the residual: the distance in pixels from the drawn point to the
    polyline the data makes in the picture (`dist_px`), given in value units too. A column
    whose data leaves the axis range has no span (NaN): the drawn line is clipped there.
    `band`, `seam` and `between` columns (see [`data_span`]; a seam with its neighbours, which
    the vertical stroke's antialiasing spills into) are counted and their worst distance
    kept, but stay out of the verdict: a band's centre is not a line's; at a wrap REW draws
    the jump as a vertical (dotted) line where ac2 leaves a gap, so the column holds the
    whole axis in one picture and nothing in the other; and
    between export samples REW draws what it has, not the export's segment (ac2 draws the
    export, so for it `between` is empty)."""
    x: np.ndarray
    f: np.ndarray
    drawn: np.ndarray
    lo: np.ndarray
    hi: np.ndarray
    px_per_unit: float
    dist_px: np.ndarray
    band: np.ndarray
    seam: np.ndarray
    between: np.ndarray

    @property
    def residual(self) -> np.ndarray:
        return np.where(np.isfinite(self.drawn) & np.isfinite(self.lo), self.dist_px / self.px_per_unit, np.nan)

    def stats(self) -> dict:
        r_all = self.residual
        aside = self.band | self.seam | self.between
        r = np.where(aside, np.nan, r_all)
        ok = np.isfinite(r)
        px = r * self.px_per_unit
        signed = np.where(ok, (self.drawn - (self.lo + self.hi) / 2) * self.px_per_unit, np.nan)
        band_px = np.where(self.band, r_all, np.nan) * self.px_per_unit
        seam_px = np.where(self.seam, r_all, np.nan) * self.px_per_unit
        between_px = np.where(self.between, r_all, np.nan) * self.px_per_unit
        return {"columns": int(ok.sum()), "empty_columns": int((~np.isfinite(self.drawn)).sum()),
                "skipped_columns": int((np.isfinite(self.drawn) & ~np.isfinite(self.lo)).sum()),
                "band_columns": int((self.band & np.isfinite(r_all)).sum()),
                "band_max_px": float(np.nanmax(band_px)) if np.isfinite(band_px).any() else None,
                "seam_columns": int((self.seam & np.isfinite(r_all)).sum()),
                "seam_max_px": float(np.nanmax(seam_px)) if np.isfinite(seam_px).any() else None,
                "between_columns": int((self.between & np.isfinite(r_all)).sum()),
                "between_max_px": float(np.nanmax(between_px)) if np.isfinite(between_px).any() else None,
                "mean_signed_px": float(np.nanmean(signed)) if ok.any() else None,
                "max_units": float(np.nanmax(r)) if ok.any() else None,
                "p95_units": float(np.nanpercentile(r, 95)) if ok.any() else None,
                "max_px": float(np.nanmax(px)) if ok.any() else None,
                "p95_px": float(np.nanpercentile(px, 95)) if ok.any() else None,
                "worst_x": float(self.f[int(np.nanargmax(r))]) if ok.any() else None}


def data_span(f: np.ndarray, v: np.ndarray, mapping: Mapping, xs: np.ndarray, wrap: bool = False):
    """For each pixel column (centre `x`), the min and max of the data whose abscissa falls
    in [f(x − ½), f(x + ½)], the data interpolated at the edges included (a column with no
    sample inside still has the line crossing it; both renderers draw straight segments
    between samples, linear in the axis coordinate). Tells whether the column's data is on
    the axis at all, and gives the signed offset its centre. Wrapped phase: a span across the
    ±180 seam is the whole range. Also each column's shape: `seam` (the data wraps in it),
    `zigzag` (more than four samples turning back and forth over more than two pixels: the
    column holds a band, whose centre is no line's) and `between` (crossed by a segment
    between export samples a pixel or more apart that drops more than four pixels: what a
    renderer with finer data than the export draws there is not that straight segment)."""
    fe_lo = mapping.f_of_x(xs - 0.5)
    fe_hi = mapping.f_of_x(xs + 0.5)
    ok = np.isfinite(v)
    f_ok, v_ok = f[ok], v[ok]
    u_ok = mapping.u(f_ok)
    lo = np.full(len(xs), np.nan)
    hi = np.full(len(xs), np.nan)
    seam = np.zeros(len(xs), bool)
    zigzag = np.zeros(len(xs), bool)
    between = np.zeros(len(xs), bool)
    i0 = np.searchsorted(f_ok, fe_lo, side="left")
    i1 = np.searchsorted(f_ok, fe_hi, side="right")
    u_lo, u_hi = mapping.u(fe_lo), mapping.u(fe_hi)
    for k, (a, b) in enumerate(zip(i0, i1)):
        if fe_lo[k] < f_ok[0] or fe_hi[k] > f_ok[-1]:
            continue
        vals = [np.interp(u_lo[k], u_ok, v_ok), np.interp(u_hi[k], u_ok, v_ok)]
        if b > a:
            seg = v_ok[a:b]
            vals.extend([float(seg.min()), float(seg.max())])
        if wrap:
            # a step of more than 180° between neighbouring samples is the seam, not the data:
            # every column between the two samples that straddle it (sparse data puts the
            # jump across several columns, with no sample inside them)
            ext = v_ok[max(a - 1, 0):min(b + 1, len(v_ok))]
            if len(ext) > 1 and np.abs(np.diff(ext)).max() > 180.0:
                lo[k], hi[k] = -180.0, 180.0
                seam[k] = True
                continue
        if b - a > 4:
            turns = np.diff(np.sign(np.diff(v_ok[a:b])))
            zigzag[k] = (turns != 0).sum() >= 2 and (max(vals) - min(vals)) * mapping.px_per_unit > 2.0
        lo[k], hi[k] = min(vals), max(vals)
    x_ok = np.asarray(mapping.x_of_f(f_ok), float)
    steep = np.where((np.diff(x_ok) >= 1.0) & (np.abs(np.diff(v_ok)) * mapping.px_per_unit > 4.0))[0]
    for i in steep:
        between |= (xs >= x_ok[i]) & (xs <= x_ok[i + 1])
    return lo, hi, {"seam": seam, "zigzag": zigzag, "between": between}


def polyline_distance(px: np.ndarray, py: np.ndarray, X: np.ndarray, Y: np.ndarray, window: float = 4.0) -> np.ndarray:
    """Distance in pixels from each point (`px`, `py`) to the polyline through (`X`, `Y`),
    all in pixel coordinates, `X` ascending, a NaN in `Y` breaking the line. Only the segments
    within `window` px of the point in x are tried; with none there the distance is at least
    `window` and that is returned. NaN for a NaN point."""
    out = np.full(len(px), np.nan)
    n = len(X)
    if n < 2:
        return out
    lo_i = np.searchsorted(X, px - window) - 1
    hi_i = np.searchsorted(X, px + window)
    for i in range(len(px)):
        if not np.isfinite(py[i]):
            continue
        a, b = max(int(lo_i[i]), 0), min(int(hi_i[i]), n - 1)
        if b <= a:
            out[i] = window
            continue
        x0, y0, x1, y1 = X[a:b], Y[a:b], X[a + 1:b + 1], Y[a + 1:b + 1]
        ok = np.isfinite(y0) & np.isfinite(y1)
        if not ok.any():
            out[i] = window
            continue
        x0, y0, x1, y1 = x0[ok], y0[ok], x1[ok], y1[ok]
        dx, dy = x1 - x0, y1 - y0
        l2 = dx * dx + dy * dy
        t = np.clip(np.where(l2 > 0, ((px[i] - x0) * dx + (py[i] - y0) * dy) / np.where(l2 > 0, l2, 1.0), 0.0), 0.0, 1.0)
        out[i] = float(np.min(np.hypot(px[i] - (x0 + t * dx), py[i] - (y0 + t * dy))))
    return out


def _break_seams(X: np.ndarray, Y: np.ndarray, v: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """The polyline with a break (a NaN point) after every step of more than 180° in `v`:
    the ±180° seam of a wrapped phase, not a segment either renderer draws."""
    j = np.where(np.abs(np.diff(v)) > 180.0)[0]
    if len(j) == 0:
        return X, Y
    return np.insert(X, j + 1, X[j + 1]), np.insert(Y, j + 1, np.nan)


def curve_px_distance(px: np.ndarray, py: np.ndarray, mapping: Mapping, f: np.ndarray, v: np.ndarray, wrap: bool = False) -> np.ndarray:
    """Pixel distance from drawn points to the curve (`f`, `v`) as `mapping` draws it; a
    wrapped phase is tried at 0, +360 and −360°, broken at its seams, the nearest counts."""
    ok = np.isfinite(v) & np.isfinite(f)
    fo, vo = f[ok], v[ok]
    X = np.asarray(mapping.x_of_f(fo), float)
    dist = np.full(len(px), np.nan)
    for off in ((0.0, 360.0, -360.0) if wrap else (0.0,)):
        Xo, Yo = X, np.asarray(mapping.y_of_v(vo + off), float)
        if wrap:
            Xo, Yo = _break_seams(Xo, Yo, vo)
        dist = np.fmin(dist, polyline_distance(px, py, Xo, Yo))
    return dist


def compare_curve(drawn_y: np.ndarray, xs: np.ndarray, mapping: Mapping, f: np.ndarray, v: np.ndarray,
                  wrap: bool = False, bands: bool = True, between: bool = False) -> CurveRead:
    """The drawn rows against the data: each column's drawn point and its distance, in
    pixels of the picture, to the polyline the data makes there (both renderers join samples
    with straight segments; the nearest segment, so a point may sit a pixel off along the
    line as well as across it, which a steep edge or a dense zigzag needs). Columns whose
    data span leaves the axis range are skipped (the renderer clips the line there, the
    picture holds no value). `bands=False` when the drawn point is a stroke's top edge, which
    a band has as well as a line; `between=True` for a renderer that draws finer data than
    the export (REW)."""
    drawn = mapping.v_of_y(drawn_y)
    lo, hi, shape = data_span(f, v, mapping, xs, wrap=wrap)
    seam = shape["seam"].copy()
    seam[1:] |= shape["seam"][:-1]
    seam[:-1] |= shape["seam"][1:]
    v_lo, v_hi = mapping.v_range
    out = (lo < v_lo) | (hi > v_hi)
    lo[out] = np.nan
    hi[out] = np.nan
    dist = curve_px_distance(xs, np.asarray(drawn_y, float), mapping, f, v, wrap=wrap)
    return CurveRead(x=xs, f=mapping.f_of_x(xs), drawn=drawn, lo=lo, hi=hi, px_per_unit=mapping.px_per_unit, dist_px=dist,
                     band=shape["zigzag"] if bands else np.zeros(len(xs), bool), seam=seam,
                     between=shape["between"] if between else np.zeros(len(xs), bool))


def rew_mapping(frame: dict, limits: dict, v_top: float, v_bot: float, xlog: bool = True) -> Mapping:
    return Mapping(x0=frame["x0"], x1=frame["x1"], f0=limits["left"], f1=limits["right"],
                   y_top=frame["y_top"], y_bot=frame["y_bot"], v_top=v_top, v_bot=v_bot, xlog=xlog)


def ac2_mapping(meta: dict, pane: dict, x_factor: float = 1.0) -> Mapping:
    """plotcheck's geometry as a mapping; `x_factor` converts the x axis's unit to the data's
    (ac2's IR axis is ms, REW's export seconds)."""
    s = float(meta["scale"])
    xa, ya = meta["x_axis"], pane["y_axis"]
    return Mapping(x0=xa["px_lo"] * s, x1=xa["px_hi"] * s, f0=xa["range"]["lo"] * x_factor, f1=xa["range"]["hi"] * x_factor,
                   y_top=ya["px_hi"] * s, y_bot=ya["px_lo"] * s, v_top=ya["range"]["hi"], v_bot=ya["range"]["lo"],
                   xlog=xa.get("scale", "log") == "log")


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


def _plotcheck(plotcheck: str, sub: str, csv: Path, out_png: Path, width: int, height: int, *flags: str) -> dict:
    cmd = [plotcheck, sub, "--input", str(csv), "--out", str(out_png), "--width", str(width), "--height", str(height), *flags]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"plotcheck {sub} failed ({r.returncode}): {r.stderr.strip()}\n  {' '.join(cmd)}")
    return json.loads(out_png.with_suffix(".json").read_text())


def write_csv(path: Path, header: str, *cols: np.ndarray) -> None:
    with path.open("w") as fh:
        fh.write(header + "\n")
        for row in zip(*cols):
            fh.write(",".join(f"{float(x):.15g}" for x in row) + "\n")


def _pane_px(meta: dict, pane: dict) -> tuple[int, int, int, int]:
    """The pane's plot rectangle in device pixels: first and last column and row."""
    s = float(meta["scale"])
    r = pane["plot"]
    return (int(round(r["x"] * s)), int(round(r["y"] * s)),
            int(round((r["x"] + r["w"]) * s)) - 1, int(round((r["y"] + r["h"]) * s)) - 1)


@dataclass
class Quantity:
    """One curve to compare in one graph: the data each renderer drew (the same numbers,
    unless the renderer transforms them: ac2's log IR is re its peak, its group delay is
    its own), the value range REW's axis spans, and how to read it."""
    name: str
    pane_kind: str
    x: np.ndarray
    v_rew: np.ndarray
    rew_axis: tuple[float, float]  # (v_top, v_bot)
    unit: str
    x_ac2: np.ndarray | None = None  # ac2's abscissa, when its data differs
    v_ac2: np.ndarray | None = None
    ac2_offset: float = 0.0  # v_rew = v_ac2 * ac2_scale + ac2_offset (ac2's axis unit differs: FS vs %)
    ac2_scale: float = 1.0
    wrap: bool = False
    hue_rew: str = "any"
    xlog: bool = True
    x_factor: float = 1.0  # ac2's x unit → the data's
    ac2_label: str = "data"  # what ac2's drawing is held against
    stat: str = "centroid"  # how a column is read: its centroid, or the line's top (a log IR)
    curve_key: str = "curve_px"  # the tolerance for a drawing against its numbers
    rew_verdict: bool = True  # False: REW's drawing is not of these numbers; its checks are notes
    rew_y: np.ndarray | None = None  # REW's curve already read (a picture with two traces to tell apart)


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


def _compare(q: Quantity, rew_im: np.ndarray, frame: dict, limits: dict, ac2_im: np.ndarray, meta: dict) -> dict:
    """Reads `q` out of both pictures; the three residual sets."""
    rm = rew_mapping(frame, limits, *q.rew_axis, xlog=q.xlog)
    fx0, fx1 = int(frame["x0"]), int(frame["x1"])  # the frame's own pixel columns
    xs = np.arange(fx0 + 1, fx1, dtype=float) + 0.5
    ry = q.rew_y if q.rew_y is not None else \
        read_curve(rew_im, fx0 + 1, fx1 - 1, int(frame["y_top"]), int(frame["y_bot"]), hue=q.hue_rew, stat=q.stat)
    bands = q.stat == "centroid"
    rr = compare_curve(ry, xs, rm, q.x, q.v_rew, wrap=q.wrap, bands=bands, between=True)
    pane = next(p for p in meta["panes"] if p["kind"] == q.pane_kind)
    am = ac2_mapping(meta, pane, q.x_factor)
    px0, py0, px1, py1 = _pane_px(meta, pane)
    axs = np.arange(px0 + 1, px1, dtype=float) + 0.5
    ay = read_curve(ac2_im, px0 + 1, px1 - 1, py0, py1, hue="any", stat=q.stat)
    ar = compare_curve(ay, axs, am, q.x if q.x_ac2 is None else q.x_ac2, q.v_rew if q.v_ac2 is None else q.v_ac2, wrap=q.wrap, bands=bands)
    # the two drawings against each other: ac2's drawn points to the polyline of REW's drawn
    # curve, both in ac2's pixels (REW's drawn values in ac2's units)
    fin = np.isfinite(rr.drawn)  # a dotted line leaves columns empty
    a_fin = np.isfinite(ar.drawn)
    if fin.sum() < 2:
        dist = np.full(len(ar.f), np.nan)
        diff = np.full(len(ar.f), np.nan)
    else:
        rew_ac2 = (rr.drawn[fin] - q.ac2_offset) / q.ac2_scale  # REW's drawn values in ac2's unit
        dist = curve_px_distance(axs, np.asarray(ay, float), am, rr.f[fin], rew_ac2, wrap=q.wrap)
        if q.wrap:
            rd = np.interp(am.u(ar.f), rm.u(rr.f[fin]), np.unwrap(np.deg2rad(rew_ac2)))
            diff = np.rad2deg(np.angle(np.exp(1j * (np.deg2rad(ar.drawn) - rd))))
        else:
            diff = ar.drawn - np.interp(am.u(ar.f), rm.u(rr.f[fin]), rew_ac2)
    # only where both pictures hold the curve: inside both axis ranges and the band's margin,
    # and where REW's picture has the curve within 1.5 px of the column (a gap, as under a
    # grid line drawn over the trace, is no disagreement)
    inside = a_fin & np.isfinite(ar.lo) & (ar.f > min(rm.f0, rm.f1) + 0.02 * abs(rm.f1 - rm.f0) * (not q.xlog)) \
        & (ar.f < max(rm.f0, rm.f1) - 0.02 * abs(rm.f1 - rm.f0) * (not q.xlog))
    if q.xlog:
        inside &= (ar.f > rm.f0 * 1.02) & (ar.f < rm.f1 / 1.02)
    if fin.sum() >= 2:
        rpx = np.asarray(am.x_of_f(rr.f[fin]), float)
        k = np.clip(np.searchsorted(rpx, axs), 1, len(rpx) - 1)
        inside &= np.minimum(np.abs(axs - rpx[k - 1]), np.abs(axs - rpx[k])) <= 1.5
    _, _, rshape = data_span(q.x, q.v_rew, am, axs, wrap=q.wrap)  # REW's export on ac2's columns
    aside = ar.band | ar.seam | rshape["between"]
    dd = np.where(inside & ~aside, dist, np.nan)
    sg = np.where(inside & ~aside, diff, np.nan)
    has = np.isfinite(dd).any()
    seam_px = np.where(inside & ar.seam, dist, np.nan)
    band_px = np.where(inside & ar.band, dist, np.nan)
    between_px = np.where(inside & rshape["between"], dist, np.nan)
    cross = {"band_columns": int(np.isfinite(band_px).sum()), "band_max_px": float(np.nanmax(band_px)) if np.isfinite(band_px).any() else None,
             "between_columns": int(np.isfinite(between_px).sum()),
             "between_max_px": float(np.nanmax(between_px)) if np.isfinite(between_px).any() else None,
             "seam_columns": int(np.isfinite(seam_px).sum()), "seam_max_px": float(np.nanmax(seam_px)) if np.isfinite(seam_px).any() else None,
             "max_units": float(np.nanmax(dd)) / am.px_per_unit * q.ac2_scale if has else None,
             "max_px": float(np.nanmax(dd)) if has else None,
             "p95_px": float(np.nanpercentile(dd, 95)) if has else None,
             "mean_signed_px": float(np.nanmean(sg)) * am.px_per_unit if has else None,
             "worst_x": float(ar.f[int(np.nanargmax(dd))]) if has else None, "columns": int(np.isfinite(dd).sum())}
    return {"rew": rr.stats(), "ac2": ar.stats(), "ac2_vs_rew": cross, "px_per_unit": {"rew": rm.px_per_unit, "ac2": am.px_per_unit},
            "_ac2_mapping": am, "_pane": pane}


def _check(key: str, pname: str, title: str, st: dict, tol, meaning: str, unit: str, x_unit: str) -> Check:
    d = {k: st.get(k) for k in ("p95_px", "mean_signed_px", "max_units", "worst_x", "columns", "empty_columns", "skipped_columns",
                                "band_columns", "band_max_px", "seam_columns", "seam_max_px", "between_columns", "between_max_px")}
    d.update({"unit": unit, "x_unit": x_unit})
    return Check(id=key, group="visual", path=pname, title=title, value=st.get("max_px"), unit="px",
                 status=judge(st.get("max_px"), tol), tol=tol, detail=d, meaning=meaning)


def _quantity_checks(res: PathResult, pname: str, g: str, q: Quantity, cmp: dict, tol: dict | None, cross_key: str = "cross_px",
                      rew_key: str | None = None) -> None:
    key = f"visual.{pname}.{g}.{q.name}"
    x_unit = "Hz" if q.xlog else "ms"
    t_curve, t_cross, t_rew = _tol(tol, q.curve_key), _tol(tol, cross_key), _tol(tol, rew_key or q.curve_key)
    res.detail[f"{g}.{q.name}"] = {k: v for k, v in cmp.items() if not k.startswith("_")}
    res.checks.append(_check(f"{key}.ac2_vs_{q.ac2_label}_px", pname, f"{g} {q.name}: ac2 drawn vs {q.ac2_label}", cmp["ac2"], t_curve,
                             f"ac2's drawn {q.name} against the numbers it drew ({q.ac2_label}), worst column, pixels", q.unit, x_unit))
    rew = _check(f"{key}.rew_vs_data_px", pname, f"{g} {q.name}: REW drawn vs data", cmp["rew"], t_rew,
                 f"REW's drawn {q.name} against REW's numbers, worst column, pixels", q.unit, x_unit)
    cross = _check(f"{key}.ac2_vs_rew_px", pname, f"{g} {q.name}: ac2 drawn vs REW drawn", cmp["ac2_vs_rew"], t_cross,
                   f"the two drawn {q.name} curves against each other, in ac2's pixels", q.unit, x_unit)
    if not q.rew_verdict:
        for c in (rew, cross):
            c.status = "INFO"
            c.meaning += " (a note: REW's picture is not of these numbers, see the report)"
    res.checks += [rew, cross]


def _axis_checks(res: PathResult, pname: str, g: str, frame: dict, limits: dict, ac2_im: np.ndarray, meta: dict, pane_kind: str,
                 tol: dict | None) -> None:
    """Axis placement on a log frequency axis: the decade lines each picture shows against its mapping."""
    t_axis = _tol(tol, "axis_px")
    rdc = decade_check(rew_mapping(frame, limits, limits["top"], limits["bottom"]), frame["decade_lines"])
    pane = next(p for p in meta["panes"] if p["kind"] == pane_kind)
    am = ac2_mapping(meta, pane)
    lines = find_grid_lines_dark(ac2_im, {k: v * float(meta["scale"]) for k, v in pane["plot"].items()})
    # ac2 draws the labelled 2 and 5 lines as dark as the decades: the strong set is all of
    # them, and the one nearest each decade is that decade's line
    cut = 0.8 * max(lines["col_strength"]) if lines["col_strength"] else 0
    major = [c for c, st in zip(lines["cols"], lines["col_strength"]) if st >= cut]
    adc = decade_check(am, major or lines["cols"])
    res.detail[f"{g}.axes"] = {"rew_frame": frame, "rew_decades": rdc, "ac2_decades": adc, "ac2_meta_x": meta["x_axis"]["px_lo"],
                               "adapter": meta.get("adapter")}
    res.checks.append(Check(id=f"visual.{pname}.{g}.axis.rew_decade_px", group="visual", path=pname, title=f"{g}: REW decade lines",
                            value=rdc["max_error_px"], unit="px", status=judge(rdc["max_error_px"], t_axis), tol=t_axis, detail=rdc["lines"],
                            meaning="REW's 100 Hz / 1 kHz / 10 kHz lines against the frame's log mapping"))
    res.checks.append(Check(id=f"visual.{pname}.{g}.axis.ac2_decade_px", group="visual", path=pname, title=f"{g}: ac2 decade lines",
                            value=adc["max_error_px"], unit="px", status=judge(adc["max_error_px"], t_axis), tol=t_axis, detail=adc["lines"],
                            meaning="ac2's 100 Hz / 1 kHz / 10 kHz grid lines against the axis plotcheck reported"))


def _graph_tf(res, pname, g, entry, d, out, plotcheck, tol, rew_im, frame, prefix):
    """`allspl` / `splphase`: magnitude (and phase) through ac2's transfer pane."""
    limits = entry["limits"]
    curve = formats.read_rew_curve(d / entry["export"])
    csv = out / f"{prefix}_fr.csv"
    if curve.phase is not None:
        write_csv(csv, "freq_hz,mag_db,phase_deg", curve.f, curve.mag, curve.phase)
    else:
        write_csv(csv, "freq_hz,mag_db", curve.f, curve.mag)
    phase = g == "splphase" and curve.phase is not None
    h_px, w_px = rew_im.shape[:2]
    ac2_png = out / f"ac2_{prefix}_{g}.png"
    meta = _plotcheck(plotcheck, "tf", csv, ac2_png, w_px, h_px, f"--freq-lo={limits['left']}", f"--freq-hi={limits['right']}",
                      f"--db-lo={limits['bottom']}", f"--db-hi={limits['top']}", *(["--phase"] if phase else []))
    ac2_im = load_rgb(ac2_png)
    qs = [Quantity("mag", "magnitude", curve.f, curve.mag, (limits["top"], limits["bottom"]), "dB", hue_rew="dominant" if phase else "any")]
    if phase:
        pa = rew_phase_axis(rew_im, frame, limits)
        res.detail[f"{g}.rew_phase_axis"] = pa
        qs.append(Quantity("phase", "phase", curve.f, curve.phase, (pa["v_top"], pa["v_bot"]), "°", wrap=True, hue_rew="second"))
    for q in qs:
        # REW's phase is dotted: its dots' centres read coarser than a line
        _quantity_checks(res, pname, g, q, _compare(q, rew_im, frame, limits, ac2_im, meta), tol,
                         rew_key="dotted_px" if q.name == "phase" else None)
    _axis_checks(res, pname, g, frame, limits, ac2_im, meta, "magnitude", tol)
    return ac2_png


def _graph_gd(res, pname, g, entry, d, out, plotcheck, tol, rew_im, frame, prefix):
    """`groupdelay`: REW's group delay (its export, ms) in REW's picture; ac2's own group
    delay, computed from the same export's phase, in ac2's phase pane in group-delay view.
    ac2's drawing is held against the numbers ac2 computed (plotcheck's `--dump`): the
    drawing check stays a drawing check; the two drawings against each other also carry the
    two estimators' difference, so they get their own tolerance. So does REW's drawing
    against REW's export: below 40 Hz the export has a point every other pixel while the
    group delay swings tens of ms between them, and REW draws from finer data than it
    exports; the steepest stretches are the `between` columns [`data_span`] sets aside, the
    rest still read a few pixels off the export's segments."""
    limits = entry["limits"]
    gd = formats.read_rew_curve(d / entry["export"])
    fr = formats.read_rew_curve(d / f"{prefix}_fr.json")
    if fr.phase is None:
        res.detail[f"{g}.skipped"] = "no phase in the export: ac2 cannot form a group delay"
        return None
    csv = out / f"{prefix}_fr.csv"
    write_csv(csv, "freq_hz,mag_db,phase_deg", fr.f, fr.mag, fr.phase)
    h_px, w_px = rew_im.shape[:2]
    ac2_png = out / f"ac2_{prefix}_{g}.png"
    dump = out / f"ac2_{prefix}_gd.csv"
    mag = graph_limits(fr)
    meta = _plotcheck(plotcheck, "tf", csv, ac2_png, w_px, h_px, f"--freq-lo={limits['left']}", f"--freq-hi={limits['right']}",
                      f"--db-lo={mag['bottom']}", f"--db-hi={mag['top']}", "--phase", "--phase-view", "group-delay",
                      f"--gd-lo={limits['bottom']}", f"--gd-hi={limits['top']}", "--dump", str(dump))
    ac2_im = load_rgb(ac2_png)
    own = np.loadtxt(dump, delimiter=",", comments="#", skiprows=sum(1 for ln in dump.read_text().splitlines() if ln.startswith("#")) + 1, ndmin=2)
    pane_kind = next((p["kind"] for p in meta["panes"] if p["kind"] in ("group_delay", "phase")), "phase")
    q = Quantity("gd", pane_kind, gd.f, gd.mag * 1e3, (limits["top"], limits["bottom"]), "ms", x_ac2=own[:, 0], v_ac2=own[:, 1],
                 hue_rew="any", ac2_label="own")
    cmp = _compare(q, rew_im, frame, limits, ac2_im, meta)
    # ac2's numbers against REW's numbers, the estimators alone (no drawing), for the record
    f_ok = (own[:, 0] >= limits["left"]) & (own[:, 0] <= limits["right"]) & np.isfinite(own[:, 1])
    rew_on_own = np.interp(np.log(own[f_ok, 0]), np.log(gd.f[np.isfinite(gd.mag)]), gd.mag[np.isfinite(gd.mag)] * 1e3)
    cmp["estimators"] = {"max_ms": float(np.max(np.abs(own[f_ok, 1] - rew_on_own))), "p95_ms": float(np.percentile(np.abs(own[f_ok, 1] - rew_on_own), 95))}
    _quantity_checks(res, pname, g, q, cmp, tol, cross_key="gd_cross_px", rew_key="gd_curve_px")
    _axis_checks(res, pname, g, frame, limits, ac2_im, meta, "magnitude", tol)
    return ac2_png


def _graph_ir(res, pname, g, entry, d, out, plotcheck, tol, rew_im, frame, prefix):
    """`impulse` (percent, linear) and `impulse_db` (dBFS, log): REW's impulse response in
    ac2's IR pane. REW's x axis is seconds, ac2's ms; REW's log IR is dBFS, ac2's dB re the
    peak (its axis set so the same dB lands on the same pixel), so the offset is the peak."""
    limits = entry["limits"]
    ir = formats.read_rew_ir(d / entry["export"])
    mode = entry["mode"]
    csv = out / f"{prefix}_ir.csv"
    write_csv(csv, "t_s,value_fs", ir.t, ir.h / 100.0)  # REW's percent of full scale → FS
    h_px, w_px = rew_im.shape[:2]
    ac2_png = out / f"ac2_{prefix}_{g}.png"
    flags = [f"--t-lo={limits['left'] * 1e3}", f"--t-hi={limits['right'] * 1e3}", "--mode", mode]
    peak = float(np.max(np.abs(ir.h)))
    peak_db = 20.0 * math.log10(peak / 100.0)
    if mode == "linear":
        # REW draws the impulse normalised to a peak of 100 % (its percent export is of full
        # scale; the picture's own peak is kept as a check on that); ac2 draws FS, so its axis
        # is the same span of its own peak. The stroke's top edge is read, not its centre: at
        # the peak the line is vertical and a centre lies on it at any height.
        pct = 100.0 / peak  # REW's percent per export percent
        flags += [f"--amp-lo={limits['bottom'] / 100.0 / pct}", f"--amp-hi={limits['top'] / 100.0 / pct}"]
        v_rew = ir.h * pct
        v_ac2 = ir.h / 100.0
        q = Quantity("ir", "ir", ir.t, v_rew, (limits["top"], limits["bottom"]), "%", v_ac2=v_ac2, ac2_offset=0.0,
                     ac2_scale=100.0 * pct, hue_rew="dominant", stat="top", xlog=False, x_factor=1e-3)
        m = rew_mapping(frame, limits, limits["top"], limits["bottom"], xlog=False)
        x0, x1, y0, y1 = int(frame["x0"]), int(frame["x1"]), int(frame["y_top"]), int(frame["y_bot"])
        pic_peak = float(np.nanmax(m.v_of_y(read_curve(rew_im, x0 + 1, x1 - 1, y0, y1, hue="dominant", stat="top"))))
        res.detail[f"{g}.rew_picture"] = {"peak_pct": pic_peak, "export_peak_pct": peak, "normalised": "REW draws the impulse normalised to its peak"}
    else:
        flags += [f"--db-lo={limits['bottom'] - peak_db}", f"--db-hi={limits['top'] - peak_db}"]
        with np.errstate(divide="ignore"):
            v_rew = 20.0 * np.log10(np.abs(ir.h) / 100.0)
            v_ac2 = 20.0 * np.log10(np.abs(ir.h) / peak)
        # REW's dBFS impulse picture is not 20·log10 of its percent export: 3 dB above it at
        # the peak and on a floor near −65 dBFS where the export runs on down to −80 dB; what
        # it draws is not known, so REW's side is a note and ac2 is held to the numbers.
        q = Quantity("ir", "ir", ir.t, v_rew, (limits["top"], limits["bottom"]), "dB", v_ac2=v_ac2, ac2_offset=peak_db, hue_rew="dominant",
                     stat="top", rew_verdict=False,
                     xlog=False, x_factor=1e-3)
    meta = _plotcheck(plotcheck, "ir", csv, ac2_png, w_px, h_px, *flags)
    ac2_im = load_rgb(ac2_png)
    cmp = _compare(q, rew_im, frame, limits, ac2_im, meta)
    if mode == "linear":
        # ac2's picture is in FS: its residuals in percent for the record
        cmp["ac2_units"] = "FS (ac2's axis); REW's percent = 100 × FS / peak"
    res.detail[f"{g}.ir"] = {"peak_pct": peak, "peak_dbfs": peak_db, "ac2_meta": meta.get("ir")}
    _quantity_checks(res, pname, g, q, cmp, tol)
    return ac2_png


def _graph_spectrum(res, pname, g, entry, d, out, plotcheck, tol, rew_im, frame, prefix):
    """`rta`: REW's captured RTA (dBFS, spectrum mode's bins) in ac2's spectrum pane."""
    limits = entry["limits"]
    sp = formats.read_rew_curve(d / entry["export"])
    csv = out / f"{prefix}_spectrum.csv"
    ok = np.isfinite(sp.mag) & (sp.f > 0)
    write_csv(csv, "freq_hz,level_db", sp.f[ok], sp.mag[ok])
    h_px, w_px = rew_im.shape[:2]
    ac2_png = out / f"ac2_{prefix}_{g}.png"
    meta = _plotcheck(plotcheck, "spectrum", csv, ac2_png, w_px, h_px, f"--freq-lo={limits['left']}", f"--freq-hi={limits['right']}",
                      f"--db-lo={limits['bottom']}", f"--db-hi={limits['top']}", "--style", "line")
    ac2_im = load_rgb(ac2_png)
    # Spectrum mode has far more bins than columns: REW draws every bin, a band from the
    # column's lowest to its highest; ac2 draws the column's highest. The band's top is what
    # the two share, so dense data is read at the top of the line.
    dense = len(sp.f) > 4 * (w_px - 2)
    q = Quantity("level", "spectrum", sp.f[ok], sp.mag[ok], (limits["top"], limits["bottom"]), "dB", hue_rew="dark",
                 stat="top" if dense else "centroid", curve_key="dense_px" if dense else "curve_px")
    peak_file = d / (entry.get("peak") or f"{prefix}_peak.json")
    rm = rew_mapping(frame, limits, limits["top"], limits["bottom"])
    fx0, fx1, fy0, fy1 = int(frame["x0"]), int(frame["x1"]), int(frame["y_top"]), int(frame["y_bot"])
    xs = np.arange(fx0 + 1, fx1, dtype=float) + 0.5
    if dense and peak_file.exists():
        # REW draws its peak-hold trace in red over the black spectrum. The peak is never
        # below the spectrum, and where the two coincide (the spikes' tops, in a frame that
        # set the peak) the red paints over the black's top edge: there the red edge is the
        # spectrum's. The peak export says where that is.
        pk = formats.read_rew_curve(peak_file)
        pko = np.isfinite(pk.mag) & (pk.f > 0)
        yb = read_curve(rew_im, fx0 + 1, fx1 - 1, fy0, fy1, hue="dark", stat="top")
        yr = read_curve(rew_im, fx0 + 1, fx1 - 1, fy0, fy1, hue="red", stat="top")
        _, hi_s, _ = data_span(sp.f[ok], sp.mag[ok], rm, xs)
        _, hi_p, _ = data_span(pk.f[pko], pk.mag[pko], rm, xs)
        # ... and only a red edge within a pixel or so of that peak counts (REW's picture has
        # other red: a line along the frame's top)
        same = np.isfinite(yr) & ((hi_p - hi_s) * rm.px_per_unit < 0.75) & (np.abs(yr - rm.y_of_v(hi_p)) < 1.5)
        q.rew_y = np.where(same, np.fmin(yb, yr), yb)
        res.detail[f"{g}.rew_peak_trace"] = {"columns_on_the_spectrum": int(same.sum()),
                                              "red_vs_peak": compare_curve(yr, xs, rm, pk.f[pko], pk.mag[pko]).stats()}
    cmp = _compare(q, rew_im, frame, limits, ac2_im, meta)
    if cmp["rew"]["empty_columns"] > cmp["rew"]["columns"]:
        # not a black trace in this mode: the solid coloured one
        q.hue_rew, q.rew_y = "dominant", None
        cmp = _compare(q, rew_im, frame, limits, ac2_im, meta)
    _quantity_checks(res, pname, g, q, cmp, tol)
    _axis_checks(res, pname, g, frame, limits, ac2_im, meta, "spectrum", tol)
    return ac2_png


GRAPH_KINDS = {"tf": _graph_tf, "gd": _graph_gd, "ir": _graph_ir, "spectrum": _graph_spectrum}


def read_spl_series(path: Path) -> dict:
    """ac2's `spl watch` JSONL: the meter's running Leq and its slow level at each publish,
    with the time since the watch began (`duration`)."""
    t, leq, level, weighting = [], [], [], None
    for ln in path.read_text().splitlines():
        try:
            j = json.loads(ln)
        except ValueError:
            continue
        spl = j.get("spl")
        if not isinstance(spl, dict) or spl.get("duration") is None or spl.get("leq") is None:
            continue
        t.append(float(spl["duration"]))
        leq.append(float(spl["leq"]))
        level.append(float(spl["level"]) if spl.get("level") is not None else np.nan)
        weighting = spl.get("weighting", weighting)
    return {"t": np.array(t), "leq": np.array(leq), "level": np.array(level), "weighting": weighting}


def analyse_spl(pname: str, series: Path, out: Path, plotcheck: str, tol: dict | None) -> PathResult:
    """`spl_<w>`: ac2's Leq history strip on an `spl watch` series, both strips (`live`: time
    before the newest point, as the meter view shows it; `span`: the series' own seconds),
    against the numbers. ac2 only: REW's SPL logger has no picture command."""
    g = series.stem[len("ac2_"):] if series.stem.startswith("ac2_") else series.stem
    res = PathResult(name=pname, prefix=g, detail={"unit": "dB SPL", "series": series.name})
    out.mkdir(parents=True, exist_ok=True)
    s = read_spl_series(series)
    if len(s["t"]) < 3:
        res.detail[f"{g}.skipped"] = "too few points in the series"
        return res
    # one point a second, as the daemon publishes a window: the last publish at or before each second
    secs = np.arange(1.0, np.floor(s["t"][-1]) + 1.0)
    idx = np.searchsorted(s["t"], secs, side="right") - 1
    t_s, v = secs[idx >= 0], s["leq"][idx[idx >= 0]]
    csv = out / f"{g}.csv"
    write_csv(csv, "t_s,level_db", t_s, v)
    window = float(np.ceil(s["t"][-1]))  # the running Leq's window is the watch so far
    res.detail[g] = {"weighting": s["weighting"], "points": int(len(t_s)), "window_s": window,
                     "level_range_db": [float(np.nanmin(v)), float(np.nanmax(v))]}
    for strip in ("live", "span"):
        png = out / f"ac2_{g}_{strip}.png"
        meta = _plotcheck(plotcheck, "leq", csv, png, 1200, 620, f"--window-s={window}", "--strip", strip, "--chrome", "bare")
        im = load_rgb(png)
        pane, lm = meta["panes"][0], meta["leq"]
        am = ac2_mapping(meta, pane)
        px0, py0, px1, py1 = _pane_px(meta, pane)
        xs = np.arange(px0 + 1, px1, dtype=float) + 0.5
        y = read_curve(im, px0 + 1, px1 - 1, py0, py1, hue="any")
        r = compare_curve(y, xs, am, (t_s - lm["t_ref_s"]) / lm["x_unit_s"], v)
        st = r.stats()
        x_unit = meta["x_axis"].get("unit", "s")
        res.detail[f"{g}.{strip}"] = {"ac2": st, "leq": lm, "y_range": pane["y_axis"]["range"], "x_range": meta["x_axis"]["range"]}
        res.checks.append(_check(f"visual.{pname}.{g}.{strip}.ac2_vs_data_px", pname, f"{g} {strip} strip: ac2 drawn vs data", st,
                                 _tol(tol, "curve_px"), f"ac2's Leq history ({strip} strip) against the series it drew, worst column, pixels",
                                 "dB", x_unit))
        res.images[f"{g}_{strip}"] = {"ac2": png.name}
    return res


def analyse_path(run: Path, pname: str, side_path: Path, out: Path, plotcheck: str, tol: dict | None) -> PathResult:
    d = side_path.parent
    prefix = side_path.name[: -len("_graph.json")]
    side = json.loads(side_path.read_text())
    res = PathResult(name=pname, prefix=prefix, detail={"limits": side.get("limits"), "unit": side.get("unit"),
                                                        "other_measurements": side.get("other_measurements")})
    out.mkdir(parents=True, exist_ok=True)
    for g, entry in side["graphs"].items():
        gfile = d / entry["file"]
        if not gfile.exists():
            continue
        entry = {"kind": "tf", "export": side.get("export"), "limits": side.get("limits"), **entry}
        fn = GRAPH_KINDS.get(entry["kind"])
        if fn is None:
            res.detail[f"{g}.skipped"] = f"unknown graph kind {entry['kind']!r}"
            continue
        rew_im = load_rgb(gfile)
        frame = find_frame_light(rew_im)
        res.images[g] = {"rew": os.path.relpath(gfile, out)}
        ac2_png = fn(res, pname, g, entry, d, out, plotcheck, tol, rew_im, frame, prefix)
        if ac2_png is None:
            continue
        res.images[g]["ac2"] = ac2_png.name
        _composite(gfile, ac2_png, out / f"{prefix}_{g}_side_by_side.png")
        res.images[g]["side_by_side"] = f"{prefix}_{g}_side_by_side.png"
    return res


def _composite(a: Path, b: Path, out: Path) -> None:
    from PIL import Image
    ia, ib = Image.open(a).convert("RGB"), Image.open(b).convert("RGB")
    h = max(ia.height, ib.height)
    im = Image.new("RGB", (ia.width + ib.width + 8, h), (255, 255, 255))
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
    for series in sorted(run.glob("*/ac2_spl_*.jsonl")):
        results.append(analyse_spl(series.parent.name, series, out / series.parent.name, pc, tol))
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
    if isinstance(o, Path):
        return str(o)
    return str(o)


def _fmt_x(x, unit: str) -> str:
    if x is None:
        return ""
    if unit == "ms":
        return f"{x * 1e3:.3g} ms"
    if unit == "Hz":
        return f"{x:.0f} Hz"
    return f"{x:.3g} {unit}"


def report(doc: dict) -> str:
    lines = ["# Graphs: ac2 against REW on the same numbers", "",
             "Each picture's curve read back by colour and held against the numbers it was drawn from. Residuals in "
             "pixels of the picture they were read from (a column spans a band of the abscissa; the residual is "
             "the distance to the data's span in that band, 0 when the line passes through it). Graphs: `allspl` / "
             "`splphase` the transfer pane (magnitude, phase), `groupdelay` the phase pane in group-delay view (ac2's "
             "own estimate against its own numbers; against REW's drawing with the estimators' difference in it), "
             "`impulse` / `impulse_db` the IR pane (linear, log re peak), `rta` the spectrum pane on REW's captured "
             "RTA (every FFT bin), `spl_<w>` the Leq history strip on ac2's own SPL series (no REW picture).", ""]
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
            notes = []
            if d.get("max_units") is not None:
                notes.append(f"{d['max_units']:.3g} {d.get('unit', '')}".strip())
            if d.get("p95_px") is not None:
                notes.append(f"p95 {d['p95_px']:.2f} px")
            if d.get("mean_signed_px") is not None:
                notes.append(f"mean {d['mean_signed_px']:+.2f} px")
            if d.get("empty_columns"):
                notes.append(f"{d['empty_columns']} empty columns")
            if d.get("skipped_columns"):
                notes.append(f"{d['skipped_columns']} columns off the axis")
            if d.get("band_columns"):
                notes.append(f"{d['band_columns']} band columns aside (up to {d['band_max_px']:.1f} px)")
            if d.get("seam_columns"):
                notes.append(f"{d['seam_columns']} wrap columns aside (up to {d['seam_max_px']:.0f} px)")
            if d.get("between_columns"):
                notes.append(f"{d['between_columns']} columns between samples aside (up to {d['between_max_px']:.0f} px)")
            v = "—" if c["value"] is None else f"{c['value']:.2f} {c.get('unit', '')}"
            worst = _fmt_x(d.get("worst_x", d.get("worst_hz")), d.get("x_unit", "Hz"))
            lines.append(f"| `{c['id']}` | {v} | {c['status']} | {worst} | {'; '.join(notes)} |")
        lines.append("")
        for k, v in p["detail"].items():
            if k.endswith(".skipped"):
                lines += [f"`{k[:-8]}` skipped: {v}", ""]
            if isinstance(v, dict) and "estimators" in v:
                e = v["estimators"]
                lines += [f"`{k}`: ac2's group delay against REW's, numbers alone: max {e['max_ms']:.3g} ms, p95 {e['p95_ms']:.3g} ms.", ""]
        for g, im in p["images"].items():
            if "side_by_side" in im:
                lines += [f"### {g}: REW (left) and ac2 (right)", "", f"![{name} {g}]({name}/{im['side_by_side']})", ""]
            elif "ac2" in im:
                lines += [f"### {g}: ac2", "", f"![{name} {g}]({name}/{im['ac2']})", ""]
    lines += ["REW's phase axis is read from the picture's right-axis labels (60° apart, the one-glyph label is 0°); "
              "the API neither sets nor reports it. A signed mean is the drawn line's constant offset from the value, "
              "in pixels, positive downwards; a residual is what remains outside the data's span in the column. A column "
              "whose data leaves the axis range is skipped: the line is clipped there. A band column holds more samples than "
              "a line (a dense comb): its drawn centre is nobody's value, so it is counted and set aside; so is a wrap column, "
              "where REW joins ±180° with a vertical dotted line and ac2 leaves a gap; and a column between export samples where the "
              "segment drops more than four pixels: REW draws from finer data than it exports.", ""]
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
