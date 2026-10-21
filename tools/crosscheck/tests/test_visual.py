"""The graph comparison's reader: REW's pictures (fixtures from the xone path), a drawn line
of known values, the axes it infers, and the rig half against a fake REW."""
import copy
import json
from pathlib import Path

import numpy as np
import pytest

pytest.importorskip("PIL")
from PIL import Image, ImageDraw  # noqa: E402

from crosscheck import formats, visual, visual_rew  # noqa: E402
from crosscheck.rew import Rew  # noqa: E402

FX = Path(__file__).resolve().parent / "fixtures" / "visual"


def _fixture():
    side = json.loads((FX / "meas_graph.json").read_text())
    curve = formats.read_rew_curve(FX / "meas_fr.json")
    return side, curve


def test_frame_from_grid_and_decade_lines():
    im = visual.load_rgb(FX / "meas_graph_allspl.png")
    fr = visual.find_frame_light(im)
    # the grid fills the plot: 20 Hz at the left edge, 20 kHz at the right, 0 and -60 dB top and bottom
    assert fr["x0"] == 50.5 and fr["x1"] == 1175.5
    assert fr["y_top"] == 17.5 and fr["y_bot"] == 669.5
    side, _ = _fixture()
    m = visual.rew_mapping(fr, side["limits"], side["limits"]["top"], side["limits"]["bottom"])
    dc = visual.decade_check(m, fr["decade_lines"])
    assert dc["max_error_px"] < 0.5, dc


def test_rew_magnitude_reads_back_within_a_pixel():
    side, curve = _fixture()
    im = visual.load_rgb(FX / "meas_graph_allspl.png")
    fr = visual.find_frame_light(im)
    m = visual.rew_mapping(fr, side["limits"], side["limits"]["top"], side["limits"]["bottom"])
    fx0, fx1 = int(fr["x0"]), int(fr["x1"])
    xs = np.arange(fx0 + 1, fx1, dtype=float) + 0.5
    y = visual.read_curve(im, fx0 + 1, fx1 - 1, int(fr["y_top"]), int(fr["y_bot"]))
    r = visual.compare_curve(y, xs, m, curve.f, curve.mag)
    st = r.stats()
    assert st["empty_columns"] == 0
    assert st["max_px"] < 1.0, st
    # REW's stroke sits half a pixel below its grid, the same everywhere
    assert 0.3 < st["mean_signed_px"] < 0.7, st


def test_rew_phase_axis_from_labels_and_phase_read_back():
    side, curve = _fixture()
    im = visual.load_rgb(FX / "meas_graph_splphase.png")
    fr = visual.find_frame_light(im)
    pa = visual.rew_phase_axis(im, fr, side["limits"])
    assert pa["zero_label_width"] < 8  # "0" is one glyph
    assert pa["labels"] >= 10
    assert abs(pa["px_per_deg"] - 46 / 60) < 0.01
    assert abs(pa["y_zero"] - 481.5) < 1.0
    # the axis does not follow the magnitude axis: more than 720° over the frame
    assert pa["v_top"] - pa["v_bot"] > 780
    m = visual.rew_mapping(fr, side["limits"], pa["v_top"], pa["v_bot"])
    fx0, fx1 = int(fr["x0"]), int(fr["x1"])
    xs = np.arange(fx0 + 1, fx1, dtype=float) + 0.5
    y = visual.read_curve(im, fx0 + 1, fx1 - 1, int(fr["y_top"]), int(fr["y_bot"]), hue="red")
    r = visual.compare_curve(y, xs, m, curve.f, curve.phase, wrap=True)
    st = r.stats()
    assert st["columns"] > 400  # a dotted line: about half the columns carry a dot
    assert st["max_px"] < 2.0, st
    # the blue magnitude of the same picture, apart from the red phase
    yb = visual.read_curve(im, fx0 + 1, fx1 - 1, int(fr["y_top"]), int(fr["y_bot"]), hue="blue")
    mb = visual.rew_mapping(fr, side["limits"], side["limits"]["top"], side["limits"]["bottom"])
    rb = visual.compare_curve(yb, xs, mb, curve.f, curve.mag)
    assert rb.stats()["max_px"] < 1.0


def _synthetic(width=900, height=500, x0=60.5, x1=860.5, y_top=20.5, y_bot=460.5, v_top=10.0, v_bot=-50.0):
    """A page like a renderer's: white, grey grid at the decades and every 5 dB, one blue
    antialiased line of a known curve. Returns the picture, the mapping and the data."""
    im = Image.new("RGB", (width, height), (255, 255, 255))
    dr = ImageDraw.Draw(im)
    m = visual.Mapping(x0=x0, x1=x1, f0=20.0, f1=20000.0, y_top=y_top, y_bot=y_bot, v_top=v_top, v_bot=v_bot)
    # grid lines fill whole pixel columns / rows (as a renderer without antialiasing paints
    # them): the one nearest the value, so a found centre is within half a pixel of it
    for hz in (20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000):
        c = int(round(float(m.x_of_f(hz)) - 0.5))
        dr.line([(c, int(y_top)), (c, int(y_bot))], fill=(128, 128, 128) if hz in (100, 1000, 10000) else (223, 223, 223))
    for v in np.arange(v_bot, v_top + 0.1, 5.0):
        r = int(round(float(m.y_top + (m.v_top - v) / (m.v_top - m.v_bot) * (m.y_bot - m.y_top)) - 0.5))
        dr.line([(int(x0), r), (int(x1), r)], fill=(223, 223, 223))
    f = 10.0 * 2 ** (np.arange(0, 96 * 12) / 96)
    v = -20.0 + 8.0 * np.sin(np.log2(f / 20.0) * 1.3) - 12.0 / (1 + (f / 60.0) ** 2)
    # supersampled drawing: PIL's lines are not antialiased, so draw at 4x and shrink
    big = Image.new("RGB", (width * 4, height * 4), (255, 255, 255))
    db = ImageDraw.Draw(big)
    # a pixel-centre coordinate c lands on supersampled pixel 4c - 0.5 (PIL's pixel k spans k..k+1)
    pts = [(float(m.x_of_f(fi)) * 4 - 0.5, float(m.y_top + (m.v_top - vi) / (m.v_top - m.v_bot) * (m.y_bot - m.y_top)) * 4 - 0.5)
           for fi, vi in zip(f, v) if 20 <= fi <= 20000]
    db.line(pts, fill=(0, 116, 240), width=4)
    small = big.resize((width, height), Image.BOX)
    a = np.asarray(im).astype(int)
    b = np.asarray(small).astype(int)
    blend = np.where((b < 250).any(axis=2, keepdims=True), b, a)
    return blend, m, f, v


def test_synthetic_line_reads_back_at_its_value():
    im, m, f, v = _synthetic()
    fr = visual.find_frame_light(im)
    assert abs(fr["x0"] - m.x0) < 0.01 and abs(fr["x1"] - m.x1) < 0.01
    assert abs(fr["y_top"] - m.y_top) < 0.01 and abs(fr["y_bot"] - m.y_bot) < 0.01
    dc = visual.decade_check(m, fr["decade_lines"])
    assert dc["max_error_px"] <= 0.5  # a whole-pixel line can sit half a pixel from its value
    fx0, fx1 = int(fr["x0"]), int(fr["x1"])
    xs = np.arange(fx0 + 1, fx1, dtype=float) + 0.5
    y = visual.read_curve(im, fx0 + 1, fx1 - 1, int(fr["y_top"]), int(fr["y_bot"]))
    r = visual.compare_curve(y, xs, m, f, v)
    st = r.stats()
    assert st["empty_columns"] == 0
    assert st["max_px"] < 0.6, st
    assert abs(st["mean_signed_px"]) < 0.2, st


def test_data_span_covers_the_column_and_the_wrap_seam():
    m = visual.Mapping(x0=0.5, x1=1000.5, f0=20.0, f1=20000.0, y_top=0.5, y_bot=100.5, v_top=180.0, v_bot=-180.0)
    f = np.linspace(20, 20000, 20001)
    v = (-0.03 * f + 180) % 360 - 180  # a steep wrapped phase
    xs = np.arange(1, 1000, dtype=float) + 0.5
    lo, hi, shape = visual.data_span(f, v, m, xs, wrap=True)
    seam = (hi - lo) > 350
    assert seam.any() and (~seam).any()
    assert (shape["seam"] == seam).all() and not shape["zigzag"].any()
    # away from the seam the span is tight and holds the sample values of its column
    k = np.where(~seam)[0][len(xs) // 3]
    inside = (f >= m.f_of_x(xs[k] - 0.5)) & (f <= m.f_of_x(xs[k] + 0.5))
    assert lo[k] <= v[inside].min() and hi[k] >= v[inside].max()


def test_graph_limits_put_the_curve_under_the_top():
    c = formats.RewCurve(f=np.array([10.0, 20.0, 1000.0, 20000.0, 40000.0]), mag=np.array([9.0, -8.4, -8.9, -9.0, -30.0]),
                         phase=None, unit="dBFS")
    lim = visual_rew.graph_limits(c)
    assert lim == {"left": 20.0, "right": 20000.0, "top": 0.0, "bottom": -60.0}  # the 9 dB at 10 Hz is outside


class FakeRew(Rew):
    def __init__(self):
        super().__init__("http://fake")
        self.calls = []
        self.state = {"/application/graph-main": "allspl", "/application/y-axis-main": "SPL",
                      "/application/graph-limits-main": {"left": 20.0, "right": 20000.05, "top": 105.0, "bottom": 45.0},
                      "/application/image-capture-settings-main": {"imageWidth": 800, "asPNG": False},
                      "/rta/configuration": {"mode": "RTA 1/3 octave", "fftLength": "32k", "averaging": "Exponential 0.94",
                                             "smoothing": "1/48", "calcDistortionEnabled": True},
                      "/rta/y-axis": "SPL", "/rta/graph-limits": {"left": 10.0, "right": 24000.0, "top": 100.0, "bottom": 0.0},
                      "/rta/image-capture-settings": {"imageWidth": 800, "asPNG": False},
                      "/rta/appearance-configuration": {"useBarsOnRTA": True}}
        self.saved = []
        self.rta_commands = []

    def _req(self, method, path, body=None):
        self.calls.append((method, path, body))
        if path == "/measurements" and method == "GET":
            return {"1": {"uuid": "u1"}, "2": {"uuid": "u2"}}
        if path == "/version":
            return {"message": "fake 0.9.8"}
        if path in self.state:
            if method == "GET":
                return json.loads(json.dumps(self.state[path]))
            self.state[path] = json.loads(json.dumps(body))
            return {}
        if path == "/application/command":
            assert body["command"] == "Save main graph image"
            self.saved.append(body["parameters"][0])
            return {"message": "Graph image save processed"}
        if path == "/rta/command":
            self.rta_commands.append(body["command"])
            if body["command"] == "Save graph image":
                self.saved.append(body["parameters"][0])
            return {"message": "ok"}
        if path.startswith("/rta/captured-"):
            assert self.rta_commands[-1] in ("Stop", "Save graph image"), "the frame is read stopped"
            return json.loads((FX / "meas_fr.json").read_text())
        if path.endswith("/command"):
            assert body == {"command": "Smooth", "parameters": [["smoothing", "None"]]}
            return {"message": "Smooth completed"}
        if path == "/measurements/selected-uuid":
            assert body == "u1"
            return {}
        return {}


def test_save_graphs_sets_axes_saves_and_restores(tmp_path):
    side_src, _ = _fixture()
    d = tmp_path / "rew"
    d.mkdir()
    (d / "meas_fr.json").write_text((FX / "meas_fr.json").read_text())
    rew = FakeRew()
    before = json.loads(json.dumps(rew.state))
    side = visual_rew.save_graphs(rew, "u1", d, "meas", "dBFS")
    assert side["limits"] == {"left": 20.0, "right": 20000.0, "top": 0.0, "bottom": -60.0}
    assert side["other_measurements"] == 1
    assert [Path(p).name for p in rew.saved] == ["meas_graph_allspl.png", "meas_graph_splphase.png"]
    assert all(Path(p).parent == d.resolve() for p in rew.saved)
    assert set(side["graphs"]) == {"allspl", "splphase"}
    assert side["graphs"]["splphase"]["y_axis"] == "dBFS"
    # the graph settings are put back
    assert rew.state == before
    # the capture used the suite's settings while it ran
    puts = [b for m, p, b in rew.calls if p == "/application/image-capture-settings-main" and m == "PUT"]
    assert puts[0]["asPNG"] is True and puts[0]["includeLegend"] is False and puts[0]["imageWidth"] == 1200
    assert json.loads((d / "meas_graph.json").read_text())["export"] == "meas_fr.json"


def test_report_lists_every_check(tmp_path):
    doc = {"paths": {"xone": {"prefix": "meas", "detail": {"unit": "dBFS"}, "images": {},
                              "checks": [{"id": "visual.xone.allspl.mag.ac2_vs_data_px", "value": 0.2, "unit": "px", "status": "PASS",
                                          "detail": {"p95_px": 0.1, "mean_signed_px": 0.0, "max_units": 0.02, "unit": "dB", "worst_x": 100.0,
                                                     "band_columns": 3, "band_max_px": 4.5, "seam_columns": 2, "seam_max_px": 114.0}},
                                         {"id": "visual.xone.impulse.ir.ac2_vs_data_px", "value": 0.4, "unit": "px", "status": "PASS",
                                          "detail": {"max_units": 0.3, "unit": "%", "worst_x": 1.5e-3, "x_unit": "ms"}}]}}}
    md = visual.report(doc)
    assert "ac2_vs_data_px" in md and "PASS 2" in md and "mean +0.00 px" in md and "100 Hz" in md
    assert "3 band columns aside (up to 4.5 px)" in md and "2 wrap columns aside (up to 114 px)" in md
    assert "1.5 ms" in md


def test_save_rta_captures_a_stopped_spectrum_frame_and_restores(tmp_path):
    rew = FakeRew()
    before = copy.deepcopy(rew.state)
    d = tmp_path / "rew"
    side = visual_rew.save_rta(rew, d, seconds=0.0)
    assert set(side["graphs"]) == {"rta"}
    cfg = side["configuration"]["rta"]
    assert cfg["mode"] == "Spectrum" and cfg["fftLength"] == "64k" and cfg["averaging"] == "None" and cfg["smoothing"] == "None"
    assert rew.rta_commands == ["Start", "Stop", "Save graph image"]
    assert [Path(p).name for p in rew.saved] == ["rta_graph_rta.png"]
    g = side["graphs"]["rta"]
    assert g["export"] == "rta_rta_data.json" and g["peak"] == "rta_rta_peak.json" and g["kind"] == "spectrum"
    assert (d / g["export"]).exists() and (d / g["peak"]).exists() and (d / "rta_graph.json").exists()
    curve = formats.read_rew_curve(d / g["export"])
    assert g["limits"]["top"] >= curve.mag.max() + 10 and g["limits"]["left"] == 20.0
    assert g["data_unchanged_after_image"] is True
    # the RTA's settings are put back, the bars left as they were
    assert rew.state == before


def test_ir_limits_span_the_normalised_picture_and_the_tail():
    fs = 48000.0
    t = np.arange(-0.01, 0.2, 1 / fs)
    h = np.zeros_like(t)
    k = int(np.argmin(np.abs(t)))
    h[k] = 0.8  # percent of full scale: tiny, as an SPL-calibrated acoustic measurement's
    h[k + int(0.05 * fs)] = 0.002  # a late reflection above 0.1 % of the peak
    lim = visual_rew.ir_limits(formats.RewIR(t=t, h=h, fs=fs, delay=None, meta={}))
    assert lim["linear"] == {"left": -5e-3, "right": pytest.approx(0.075), "top": 110.0, "bottom": -110.0}
    assert lim["log"]["top"] - lim["log"]["bottom"] == pytest.approx(63.0)
    assert lim["log"]["top"] == pytest.approx(20 * np.log10(0.008) + 3.0)


def test_polyline_distance_is_to_the_nearest_segment():
    X = np.array([0.0, 10.0, 20.0])
    Y = np.array([0.0, 0.0, 10.0])
    d = visual.polyline_distance(np.array([5.0, 15.0, 15.0]), np.array([3.0, 5.0, 9.0]), X, Y)
    assert d[0] == pytest.approx(3.0)
    assert d[1] == pytest.approx(0.0)  # on the sloped segment y = x - 10
    assert d[2] == pytest.approx(4.0 / np.sqrt(2))


def test_data_span_marks_a_dense_comb_as_a_band():
    m = visual.Mapping(x0=0.5, x1=1000.5, f0=20.0, f1=20000.0, y_top=0.5, y_bot=600.5, v_top=0.0, v_bot=-60.0)
    f = np.linspace(1000.0, 1100.0, 4000)
    comb = -30.0 + 2.0 * np.where(np.arange(len(f)) % 2 == 0, 1.0, -1.0)  # ±2 dB every sample: a band
    smooth = -30.0 + np.linspace(0.0, 1.0, len(f))  # many samples, one line
    xs = np.arange(1, 1000, dtype=float) + 0.5
    inside = (m.f_of_x(xs) > 1001) & (m.f_of_x(xs) < 1099)
    _, _, s1 = visual.data_span(f, comb, m, xs)
    _, _, s2 = visual.data_span(f, smooth, m, xs)
    assert s1["zigzag"][inside].all() and not s1["zigzag"][~inside].any()
    assert not s2["zigzag"].any() and not s1["between"].any()
    # a sparse export through a steep stretch: the columns between its samples
    fs = np.array([1000.0, 1010.0, 1020.0, 1030.0, 1040.0])
    vs = np.array([-10.0, -40.0, -40.1, -40.2, -40.3])
    _, _, s3 = visual.data_span(fs, vs, m, xs)
    steep = (m.f_of_x(xs) > 1000.5) & (m.f_of_x(xs) < 1009.5)
    assert s3["between"][steep].all() and not s3["between"][m.f_of_x(xs) > 1021].any()


def test_stats_keep_band_and_seam_columns_out_of_the_verdict():
    n = 6
    r = visual.CurveRead(x=np.arange(n, dtype=float), f=np.arange(n, dtype=float), drawn=np.zeros(n), lo=np.zeros(n) - 1, hi=np.zeros(n) + 1,
                         px_per_unit=10.0, dist_px=np.array([1.0, 2.0, 30.0, 0.5, 90.0, 1.5]),
                         band=np.array([False, False, True, False, False, False]), seam=np.array([False, False, False, False, True, False]),
                         between=np.array([False, True, False, False, False, False]))
    st = r.stats()
    assert st["max_px"] == pytest.approx(1.5) and st["columns"] == 3
    assert st["band_columns"] == 1 and st["band_max_px"] == pytest.approx(30.0)
    assert st["seam_columns"] == 1 and st["seam_max_px"] == pytest.approx(90.0)
    assert st["between_columns"] == 1 and st["between_max_px"] == pytest.approx(2.0)


def test_compare_curve_sets_the_wrap_seam_and_its_neighbours_aside():
    m = visual.Mapping(x0=0.5, x1=1000.5, f0=20.0, f1=20000.0, y_top=0.5, y_bot=360.5, v_top=180.0, v_bot=-180.0)
    f = np.linspace(20, 20000, 20001)
    v = (-0.03 * f + 180) % 360 - 180
    xs = np.arange(1, 1000, dtype=float) + 0.5
    # a drawn line that joins each wrap with a vertical stroke: its centre reads 0° in the
    # seam column and spills into the neighbours
    drawn_v = np.interp(m.u(m.f_of_x(xs)), m.u(f), v)
    _, _, shape = visual.data_span(f, v, m, xs, wrap=True)
    k = np.where(shape["seam"])[0]
    drawn_v[k] = 0.0
    drawn_v[k - 1] = drawn_v[k - 1] / 2
    drawn_v[k + 1] = drawn_v[k + 1] / 2
    r = visual.compare_curve(m.y_of_v(drawn_v), xs, m, f, v, wrap=True)
    st = r.stats()
    assert st["seam_columns"] == 3 * len(k)
    assert st["seam_max_px"] > 100 and st["max_px"] < 1.0, st


def _two_traces(width=400, height=200):
    """White page, a solid yellow line (hue 50) and a dotted magenta one (hue 300): REW's
    measurement colour for magnitude, the same colour's dotted phase in another hue."""
    im = np.full((height, width, 3), 255, np.uint8)
    im[80, :, :] = (255, 212, 0)
    for x in range(0, width, 3):
        im[140, x, :] = (255, 0, 255)
    return im


def test_trace_hues_rank_the_solid_line_first_and_read_each():
    im = _two_traces()
    hues = visual.trace_hues(im)
    assert len(hues) == 2
    assert abs(hues[0]["hue"] - 50) <= 10 and abs(hues[1]["hue"] - 300) <= 10
    y1 = visual.read_curve(im, 0, 399, 0, 199, hue="dominant")
    y2 = visual.read_curve(im, 0, 399, 0, 199, hue="second")
    assert np.allclose(y1, 80.5)
    assert np.allclose(y2[::3], 140.5) and np.isnan(y2[1::3]).all()


def test_dark_trace_reads_through_grid_lines_and_up_faint_spikes():
    im = np.full((300, 400, 3), 255, np.uint8)
    im[:, 50, :] = 128  # a decade line, mid grey (REW's RTA)
    im[100, :, :] = 192  # a minor grid row
    xs = np.arange(400)
    rows = 140 + xs // 20  # the trace: one pixel, antialiased to a grey, sloping gently
    im[rows, xs, :] = 90
    im[142, 50, :] = 45  # over the decade line it is darker by its own coverage
    im[60:146, 120, :] = 140  # a steep spike up from the trace: faint, still the trace
    im[100, 120, :] = 105  # the spike across the minor grid row
    y = visual.read_curve(im, 0, 399, 0, 299, hue="dark", stat="top")
    assert y[50] == pytest.approx(143.0)  # through the decade line
    assert y[120] == pytest.approx(61.0)  # the spike's top
    assert y[200] == pytest.approx(151.0)
    cen = visual.read_curve(im, 0, 399, 0, 299, hue="dark")
    assert cen[50] == pytest.approx(142.5) and cen[200] == pytest.approx(150.5)


def test_frame_stops_above_an_overview_strip():
    blend, m, f, v = _synthetic(height=500)
    page = np.full((700, blend.shape[1], 3), 255, np.uint8)
    page[:500] = blend
    # REW's impulse picture: a second, shorter frame under the plot with its own grid
    for x in range(int(m.x0), int(m.x1) + 1, 100):
        page[540:680, x, :] = 223
    page[540, int(m.x0):int(m.x1) + 1, :] = 223
    page[680, int(m.x0):int(m.x1) + 1, :] = 223
    fr = visual.find_frame_light(page)
    assert abs(fr["y_bot"] - m.y_bot) < 0.01 and abs(fr["y_top"] - m.y_top) < 0.01


def test_spl_series_is_one_point_a_second(tmp_path):
    rows = []
    for i in range(1, 61):
        t = i * 0.1
        rows.append({"spl": {"duration": t, "leq": 60.0 + t, "level": 62.0, "weighting": "A"}})
    rows.insert(3, {"other": 1})
    p = tmp_path / "ac2_spl_a.jsonl"
    p.write_text("\n".join(json.dumps(r) for r in rows) + "\nnot json\n")
    s = visual.read_spl_series(p)
    assert len(s["t"]) == 60 and s["weighting"] == "A" and s["leq"][-1] == pytest.approx(66.0)
    pc = visual.find_plotcheck()
    if not pc:
        pytest.skip("no plotcheck binary (PLOTCHECK or on PATH)")
    res = visual.analyse_spl("ambient", p, tmp_path / "out", pc, {"curve_px": [1.0, 2.0]})
    assert [c.id.split(".")[-2] for c in res.checks] == ["live", "span"]
    assert all(c.status == "PASS" for c in res.checks), [(c.id, c.value) for c in res.checks]
    assert res.detail["spl_a"]["points"] == 6
