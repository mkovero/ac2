"""The OSM stage: its models and fixtures always; one small end-to-end case when the external
osm-harness (OSM_HARNESS) and the ac2 binaries (AC2_BIN_DIR or PATH) are there."""
import math
import os
import shutil

import numpy as np
import pytest

from crosscheck import dsp, osm, osm_fixtures, wav


def test_expected_g2_tends_to_the_truth_with_many_averages():
    assert osm.expected_g2(0.5, 1e6) == pytest.approx(0.5, abs=1e-5)
    # few averages read high at low coherence: E ≈ γ² + (1 − γ²)²/n
    assert osm.expected_g2(0.5, 10) == pytest.approx(0.5 + 0.25 / 10, abs=0.01)
    assert osm.expected_g2(1.0, 5) == pytest.approx(1.0, abs=1e-9)


def test_welch_neff():
    # frames that do not overlap are independent
    assert osm.welch_neff(1024, 1024, 21) == pytest.approx(21)
    # OSM's 21 ticks at FFT14 / 48 kHz carry about 10 averages (harness validation)
    assert osm.welch_neff(16384, 3840, 21) == pytest.approx(10.5, abs=0.3)


def test_expected_ratio_mean_against_monte_carlo():
    rng = np.random.default_rng(0)
    n = 400_000
    for snr in (20.0, 10.0, 0.0):
        r = rng.standard_normal(n) + 1j * rng.standard_normal(n)
        e = (rng.standard_normal(n) + 1j * rng.standard_normal(n)) * 10 ** (-snr / 20)
        mc = np.mean(np.abs(r + e) / np.abs(r))
        assert osm.expected_ratio_mean(snr) == pytest.approx(mc, rel=0.02), snr
    assert osm.expected_ratio_mean(60.0) == pytest.approx(1.0, abs=1e-5)


def test_fixture_delay_and_biquad_truth(tmp_path):
    c = osm_fixtures.CASES["delay10_5"]
    p = osm_fixtures.generate(osm_fixtures.Case("t", 1.0, "tf", "", delay_samples=10.5), tmp_path / "d.wav")
    fs, x = wav.read(p)
    M, R = np.fft.rfft(x[:, 0]), np.fft.rfft(x[:, 1])
    f = np.fft.rfftfreq(len(x), 1 / fs)
    tau = dsp.delay_from_phase(f, M / R, 1000, 20000) * fs
    assert tau == pytest.approx(10.5, abs=1e-3)
    assert c.truth(np.array([0.0]))[0] == pytest.approx(1.0)
    b = osm_fixtures.CASES["biquad"]
    assert 20 * math.log10(abs(b.truth(np.array([1000.0]))[0])) == pytest.approx(6.0, abs=1e-9)


def test_missing_harness_is_skip(tmp_path, monkeypatch):
    monkeypatch.setenv("OSM_HARNESS", str(tmp_path / "no-such-harness"))
    rc, res, line = osm.run_stage(tmp_path / "out", cases=["identity"], recordings=False, plots=False)
    assert rc == 0 and res is None
    assert line.startswith("osm: SKIP") and "missing" in line


def _tools():
    if not os.environ.get("OSM_HARNESS"):
        return None
    d = os.environ.get("AC2_BIN_DIR") or (os.path.dirname(shutil.which("ac2d") or "") or None)
    return d


@pytest.mark.skipif(_tools() is None, reason="OSM_HARNESS not set (and the ac2 binaries via AC2_BIN_DIR or PATH)")
def test_identity_case_end_to_end(tmp_path):
    rc, res, line = osm.run_stage(tmp_path / "run", cases=["identity"], recordings=False, plots=False)
    assert res is not None, line
    assert res["summary"].get("FAIL", 0) == 0, [c for c in res["checks"] if c["status"] == "FAIL"]
    ids = {c["id"]: c for c in res["checks"]}
    assert ids["osm.identity.ac2_vs_osm_h"]["status"] == "PASS"
    assert ids["osm.identity.osm_delay_finder_vs_analytic"]["value"] == 0
    assert (tmp_path / "run" / "report" / "report.md").exists()
