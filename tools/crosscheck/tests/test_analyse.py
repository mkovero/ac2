from types import SimpleNamespace

import numpy as np

from crosscheck.ac2 import event_errors
from crosscheck.analyse import Analysis, judge_u


def test_judge_u():
    tol = (0.1, 0.3)
    assert judge_u(0.05, tol, 0.02) == "PASS"
    assert judge_u(0.2, tol, 0.05) == "WARN"
    # beyond the warn limit, but not by more than the uncertainty
    assert judge_u(0.35, tol, 0.2) == "INCONCLUSIVE"
    assert judge_u(0.6, tol, 0.2) == "FAIL"
    # an uncertainty wider than the pass limit cannot tell a pass from a fail
    assert judge_u(0.0, tol, 0.15) == "INCONCLUSIVE"
    assert judge_u(0.2, tol, float("nan")) == "WARN"


def test_event_errors_reads_the_daemon_refusal():
    ev = [{"event": "started"},
          {"error": {"code": "refused", "msg": "refused: the signal's peak would exceed the output limit"}},
          {"event": "refused", "why": "x"}]
    e = event_errors(ev)
    assert len(e) == 2
    assert e[0] == "refused: refused: the signal's peak would exceed the output limit"
    assert event_errors([{"event": "done"}]) == []


def test_sine_near_mains():
    a = SimpleNamespace(mains_hz=[50.0, 100.0])
    t = {"plan": {"tones": [{"f": 51.275, "probe_s": 2.0}, {"f": 101.3, "probe_s": 8.0}]}}
    # 51.275·2^(−1/48) = 50.54 Hz, inside the 2 s probe's lobe of ±1.5 Hz around 50 Hz
    d = Analysis._sine_near_mains(a, t, {"f": 51.275})
    assert d is not None and abs(d - 0.54) < 0.01
    # 101.3·2^(−1/48) = 99.85 Hz is 0.15 Hz off 100 Hz: inside an 8 s probe's ±0.375 Hz lobe
    assert Analysis._sine_near_mains(a, t, {"f": 101.3}) is not None
    assert Analysis._sine_near_mains(SimpleNamespace(mains_hz=[]), t, {"f": 51.275}) is None
    assert Analysis._sine_near_mains(a, {"plan": {"tones": []}}, {"f": 1001.5}) is None
    assert np.isfinite(Analysis._sine_near_mains(a, t, {"f": 51.275}))


def test_at_follows_a_long_delay_between_columns():
    from crosscheck import dsp
    f = dsp.log_centres(1000, 20000, 48)
    tau = 3.6e-3
    H = 0.5 * np.exp(-2j * np.pi * f * tau)
    a = SimpleNamespace(bulk_delay=tau)
    ft = np.array([5002.5, 10005.0])
    h = Analysis.at(a, f, H, ft)
    want = 0.5 * np.exp(-2j * np.pi * ft * tau)
    assert np.all(np.abs(np.angle(h / want)) < 1e-3)
    # without the delay taken out the unwrap cannot follow half a turn per column
    h0 = Analysis.at(a, f, H, ft, tau=0.0)
    assert np.abs(np.angle(h0[1] / want[1])) > 0.5
