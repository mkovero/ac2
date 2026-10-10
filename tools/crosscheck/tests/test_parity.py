"""ac2 against REW: the arrival reference, ac2 − REW harmonics and the METHOD pass."""
import tomllib
from types import SimpleNamespace

import numpy as np

from crosscheck import dsp
from crosscheck.analyse import TOLERANCES, Analysis, direct_ir_peak, method_pass
from crosscheck.baseline import RANK, check_key
from crosscheck.model import Raw


def _analysis():
    a = Analysis.__new__(Analysis)
    a.tol = tomllib.loads(TOLERANCES.read_text())
    a.checks, a.tables, a.notes, a.mains_hz = [], {}, [], []
    return a


def _check(id_, status, value, analyser, key="p|h3|500", sign=None, claim=None):
    twin = {"key": key, "analyser": analyser}
    if sign is not None:
        twin.update(sign=sign, claim=claim)
    return {"id": id_, "title": id_, "status": status, "value": value, "unit": "dB",
            "meaning": "m", "detail": {"twin": twin}}


def test_method_both_miss_the_same_way():
    a = _check("p.h3.ac2 sweep x.500", "WARN", 5.5, "ac2")
    r = _check("p.h3.REW.500", "FAIL", 6.1, "REW")
    assert method_pass([a, r]) == 2
    assert a["status"] == r["status"] == "METHOD"
    assert a["detail"]["method_of"] == "WARN" and r["detail"]["method_of"] == "FAIL"
    assert "+6.10 dB" in a["meaning"] and "+5.50 dB" in r["meaning"]


def test_method_needs_the_same_direction_and_a_missing_twin():
    a = _check("a", "FAIL", 7.0, "ac2")
    r = _check("r", "WARN", -4.0, "REW")
    assert method_pass([a, r]) == 0 and a["status"] == "FAIL"
    # ac2 broken where REW passes: stays FAIL
    a = _check("a", "FAIL", 7.0, "ac2")
    r = _check("r", "PASS", 1.0, "REW")
    method_pass([a, r])
    assert (a["status"], r["status"]) == ("FAIL", "PASS")
    # REW off where ac2 passes: REW's row stays as it is
    a = _check("a", "PASS", 1.0, "ac2")
    r = _check("r", "FAIL", 7.0, "REW")
    method_pass([a, r])
    assert (a["status"], r["status"]) == ("PASS", "FAIL")
    # a twin at another tone is no twin
    a = _check("a", "FAIL", 7.0, "ac2")
    r = _check("r", "FAIL", 7.0, "REW", key="p|h3|1000")
    method_pass([a, r])
    assert a["status"] == r["status"] == "FAIL"


def test_method_one_sided_bound_claims_carry_their_direction():
    a = _check("a", "FAIL", None, "ac2", sign=1, claim="-40.0 dBr over the sine's bound -60.0")
    r = _check("r", "FAIL", None, "REW", sign=1, claim="-41.0 dBr over the sine's bound -60.0")
    method_pass([a, r])
    assert a["status"] == r["status"] == "METHOD"
    assert "-41.0 dBr over the sine's bound" in a["meaning"]
    # no direction known: not paired
    a = _check("a", "FAIL", None, "ac2")
    r = _check("r", "FAIL", None, "REW")
    method_pass([a, r])
    assert a["status"] == r["status"] == "FAIL"


def test_method_ranks_between_a_pass_and_a_warning():
    assert RANK["PASS"] < RANK["METHOD"] < RANK["WARN"] < RANK["FAIL"]


def test_cmp_h_rew_signed_and_inconclusive_on_a_bound():
    a = _analysis()
    p = SimpleNamespace(name="g")
    va = {"kind": "value", "value": -50.0, "floor": -80.0}
    vr = {"kind": "value", "value": -51.5, "floor": -80.0}
    a._cmp_h_rew(p, "ac2 sweep x", "REW offline import", 501.5, 3, va, vr)
    c = a.checks[-1]
    assert c.id == "g.h3.ac2 sweep x|REW offline import.501.5" and c.group == "harmonics ac2 vs REW"
    assert c.status == "PASS" and abs(c.value - 1.5) < 1e-9
    key, _, _ = check_key({"id": c.id, "group": c.group, "path": "g"}, {"g": {501.5: 500.0}})
    assert key == "g.h3.ac2 sweep x|REW offline import@500Hz"
    a._cmp_h_rew(p, "ac2 sweep x", "REW offline import", 501.5, 3, va, {"kind": "bound", "value": None, "bound": -70.0})
    assert a.checks[-1].status == "INCONCLUSIVE" and a.checks[-1].value is None
    a._cmp_h_rew(p, "ac2 sweep x", "REW offline import", 501.5, 3, va, None)
    assert a.checks[-1].status == "INCONCLUSIVE"
    n = len(a.checks)
    a._cmp_h_rew(p, "ac2 sweep x", "REW offline import", 501.5, 3, None, None)
    assert len(a.checks) == n


def _delayed(x, fs, tau):
    X = np.fft.rfft(x)
    return np.fft.irfft(X * np.exp(-2j * np.pi * np.fft.rfftfreq(len(x), 1 / fs) * tau), len(x))


def test_arrival_is_judged_against_its_own_capture():
    fs, n = 48000.0, 1 << 15
    rng = np.random.default_rng(1)
    t_rew, t_cap = 100.3e-6, 102.3e-6  # the same path, two stimuli: their IR peaks differ by 2 µs
    ref1, ref2 = rng.standard_normal(n), rng.standard_normal(n)
    rec = Raw(fs, _delayed(ref1, fs, t_rew), ref1)
    cap = Raw(fs, _delayed(ref2, fs, t_cap), ref2)
    a = _analysis()
    a.f = dsp.log_centres(100, 20000, 12)
    a.direct_delay = {"direct (ac2 capture x)": direct_ir_peak(cap)}
    a.src = {"direct (REW recording)": np.exp(-2j * np.pi * a.f * t_rew),
             "direct (ac2 capture x)": np.exp(-2j * np.pi * a.f * t_cap),
             "ac2 sweep x": np.exp(-2j * np.pi * a.f * (t_cap + 0.1e-6))}
    sw = SimpleNamespace(trace=SimpleNamespace(sweep_info={"arrival": t_cap + 0.1e-6}), raw=cap)
    p = SimpleNamespace(name="x", kind="electrical", sweeps={"x": sw}, rec=rec, truth=None, rew=None, rew_live=None)
    a.delays(p)
    got = {c.id: c for c in a.checks}
    arr = got["x.delay.ac2_arrival.x"]
    assert abs(arr.value - 0.1) < 0.05 and arr.status == "PASS"
    assert abs(got["x.delay.ac2_total.x"].value - 0.1) < 0.05
    cv = got["x.delay.capture_vs_rew_recording.x"]
    assert cv.status == "INFO" and abs(cv.value - 2.0) < 0.05
