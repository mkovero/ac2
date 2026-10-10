"""The coherence study's port of ac2's MTW effective-average model, against figures ac2-core
states, and the expectation it yields against a Monte Carlo of the estimator."""
import numpy as np
import pytest

from crosscheck import coherence_study as cs
from crosscheck import osm


def test_overlap_model_matches_ac2_core():
    # ac2-core MIN_FAST_LF_BLOCKS: four blocks are about 1.5 averages at 87.5 % overlap, 2.4 at 75 %
    assert cs.OverlapModel(4096, 512).neff_fifo(4, 1) == pytest.approx(1.47, abs=0.01)
    assert cs.OverlapModel(4096, 1024).neff_fifo(4, 1) == pytest.approx(2.38, abs=0.01)
    # blocks that do not overlap are independent, and so are bins one Hann main lobe apart only partly
    assert cs.OverlapModel(4096, 4096).neff_fifo(10, 1) == pytest.approx(10)
    assert 1 < cs.OverlapModel(4096, 4096).neff_fifo(1, 2) < 2


def test_ladder_at_96k():
    st = cs.ac2_ladder(96000.0, 8)
    assert [s["held"] for s in st][0] == 8
    # equal confidence: every deeper stage reaches the full-rate stage's single-bin count
    n0 = st[0]["model"].neff_fifo(8, 1)
    for s in st[1:]:
        assert s["model"].neff_fifo(s["held"], 1) >= n0 * (1 - 1e-9)
        assert s["model"].neff_fifo(s["held"] - 1, 1) < n0
    # the 12 kHz stage hands over at κ(48)·Δf of the full-rate stage
    assert st[1]["xo"][0] == pytest.approx(1623.02, abs=0.01)


def test_columns_follow_bins_and_blend():
    fc = np.array([1000.0, 1800.0, 2500.0, 16000.0])
    parts = cs.ac2_column_model(96000.0, 8, fc)
    assert len(parts[0]) == 1 and len(parts[1]) == 2 and len(parts[2]) == 1
    assert sum(w for w, _ in parts[1]) == pytest.approx(1.0)
    # a wide column sums more bins, so it carries more averages
    assert parts[3][0][1] > 3 * parts[2][0][1]
    e = cs.ac2_expected_g2(0.5, parts)
    assert np.all(e > 0.5) and e[3] < e[2]


def test_expected_g2_against_monte_carlo():
    # Carter's expectation at n independent averages, as both models use it
    rng = np.random.default_rng(3)
    g2, n, trials = 0.5, 8, 40000
    x = rng.standard_normal((trials, n)) + 1j * rng.standard_normal((trials, n))
    e = rng.standard_normal((trials, n)) + 1j * rng.standard_normal((trials, n))
    y = x + e  # SNR 0 dB: γ² = 1/2
    gxy = np.sum(np.conj(x) * y, axis=1)
    est = np.abs(gxy) ** 2 / (np.sum(np.abs(x) ** 2, axis=1) * np.sum(np.abs(y) ** 2, axis=1))
    assert np.mean(est) == pytest.approx(osm.expected_g2(g2, n), abs=0.003)


def test_table_reads_residuals():
    runs = [{"setting": "A", "blocks": 8, "fft": 16, "snr": 0.0, "seed": k, "ac2": 0.52 + 0.001 * k,
             "ac2_sub": [0.53, 0.51], "osm": 0.55, "fc": [1000.0, 5000.0, 15000.0], "fs": 96000.0} for k in range(3)]
    rows = cs.summarise(runs, 3)
    assert rows[0]["seeds"] == 3 and rows[0]["g2"] == pytest.approx(0.5)
    t = cs.table(rows)
    assert t.count("\n") == 3 and "| +0 | A (ac2 --blocks 8, OSM FFT16) | 0.5000 |" in t
