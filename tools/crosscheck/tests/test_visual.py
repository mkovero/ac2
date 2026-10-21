"""The graph comparison's reader: REW's pictures (fixtures from the xone path), a drawn line
of known values, the axes it infers, and the rig half against a fake REW."""
import json
from pathlib import Path

import numpy as np
import pytest

pytest.importorskip("PIL")
from PIL import Image, ImageDraw  # noqa: E402

from crosscheck import formats, visual  # noqa: E402
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
    lo, hi = visual.data_span(f, v, m, xs, wrap=True)
    seam = (hi - lo) > 350
    assert seam.any() and (~seam).any()
    # away from the seam the span is tight and holds the sample values of its column
    k = np.where(~seam)[0][len(xs) // 3]
    inside = (f >= m.f_of_x(xs[k] - 0.5)) & (f <= m.f_of_x(xs[k] + 0.5))
    assert lo[k] <= v[inside].min() and hi[k] >= v[inside].max()


def test_graph_limits_put_the_curve_under_the_top():
    c = formats.RewCurve(f=np.array([10.0, 20.0, 1000.0, 20000.0, 40000.0]), mag=np.array([9.0, -8.4, -8.9, -9.0, -30.0]),
                         phase=None, unit="dBFS")
    lim = visual.graph_limits(c)
    assert lim == {"left": 20.0, "right": 20000.0, "top": 0.0, "bottom": -60.0}  # the 9 dB at 10 Hz is outside


class FakeRew(Rew):
    def __init__(self):
        super().__init__("http://fake")
        self.calls = []
        self.state = {"/application/graph-main": "allspl", "/application/y-axis-main": "SPL",
                      "/application/graph-limits-main": {"left": 20.0, "right": 20000.05, "top": 105.0, "bottom": 45.0},
                      "/application/image-capture-settings-main": {"imageWidth": 800, "asPNG": False}}
        self.saved = []

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
    side = visual.save_graphs(rew, "u1", d, "meas", "dBFS")
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
                                          "detail": {"p95_px": 0.1, "mean_signed_px": 0.0, "max_units": 0.02, "unit": "dB", "worst_hz": 100.0}}]}}}
    md = visual.report(doc)
    assert "ac2_vs_data_px" in md and "PASS 1" in md and "mean +0.00 px" in md
