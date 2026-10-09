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


def test_a_sweep_divided_by_the_reference_reads_the_meas_harmonic_less_the_reference_s():
    import numpy as np
    from crosscheck.analyse import dual_channel_harmonic_dbr
    tones = [{"f": f, "ratio_db": -17.4, "ratio_deg": 0.0} for f in (20.0, 100.0)]
    d = 10 ** (-90 / 20)
    t = 10 ** (-17.4 / 20)
    clean = dict(tones[0], h_vec={"3": {"meas": [d, 0.0], "ref": [0.0, 0.0]}})
    assert abs(dual_channel_harmonic_dbr(tones, clean, 3) - -90.0) < 1e-9
    # the reference's harmonic, carried through the path, in phase with the meas one: half of it comes off
    half = dict(tones[0], h_vec={"3": {"meas": [d, 0.0], "ref": [d / 2 / t, 0.0]}})
    assert abs(dual_channel_harmonic_dbr(tones, half, 3) - (-90 + 20 * np.log10(0.5))) < 1e-9
    # k·f outside the tones: no truth to form, unless a measured transfer covers it
    top = dict(tones[1], h_vec=half["h_vec"])
    assert dual_channel_harmonic_dbr(tones, top, 3) is None
    assert dual_channel_harmonic_dbr(tones, top, 3, lambda fk: None) is None
    got = dual_channel_harmonic_dbr(tones, top, 3, lambda fk: complex(t, 0.0) if abs(fk - 300.0) < 1e-9 else None)
    assert abs(got - (-90 + 20 * np.log10(0.5))) < 1e-9


def test_settle_lag_from_full_level_to_the_column():
    from types import SimpleNamespace
    from crosscheck.analyse import _settle_lag_s
    s = SimpleNamespace(start_hz=20.0, end_hz=40000.0, duration_s=5.5)
    lag = _settle_lag_s(s, 22.0)
    assert abs(lag - 5.5 / np.log(2000.0) * np.log(1.1)) < 1e-12 and lag < 0.1
    assert _settle_lag_s(SimpleNamespace(start_hz=10.0, end_hz=40000.0, duration_s=5.5), 22.0) > 0.5
    assert _settle_lag_s(SimpleNamespace(start_hz=30.0, end_hz=40000.0, duration_s=5.5), 22.0) is None


def test_mic_curve_in_columns_reads_ac2s_export():
    from crosscheck.model import mic_curve_in_columns
    assert mic_curve_in_columns({"mic": 'MM1 34804 (curve: 90°, in the columns; file "x.txt", hash 0c)'})
    assert not mic_curve_in_columns({"mic": "MM1 34804 (curve: none)"})
    # a sweep: the input's curve is applied after capture, its exported columns are raw
    assert not mic_curve_in_columns({"mic": 'MM1 34804 (curve: 90°, applied after capture as a display '
                                            'edit, not in the columns; 0 dB at 1000 Hz; file "x.txt", hash 0c)'})
    assert not mic_curve_in_columns({})
