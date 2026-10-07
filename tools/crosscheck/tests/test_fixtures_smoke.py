"""The analysis over the 2026-10-07 hand-run fixtures reproduces the documented numbers
(docs/rigs/pupu.md, docs/design/subsample-arrival-group-delay.md). Skips without them."""
import os
from pathlib import Path

import pytest

FIX = Path(os.environ.get("CROSSCHECK_FIXTURES", "/work/ac2-scratch/crosscheck-fixtures"))
pytestmark = pytest.mark.skipif(not (FIX / "rew_fr1.json").exists(), reason="fixtures absent")


@pytest.fixture(scope="module")
def res(tmp_path_factory):
    from crosscheck import analyse, report
    r, a = analyse.analyse(FIX)
    report.write(r, a, tmp_path_factory.mktemp("rep"), plots=False)
    return {c["id"]: c for c in r["checks"]}, r


def test_group_delay_fit_matches_sine(res):
    c, _ = res
    for f in ("16", "31.5", "50", "100"):
        assert abs(c[f"xone.gd.ac2 ±1/12-oct fit.{f}"]["value"]) < 0.10


def test_rew_live_delay_matches_direct(res):
    c, _ = res
    k = next(k for k in c if k.startswith("xone.delay.rew.REW live") and k.endswith("IR peak"))
    assert abs(c[k]["value"]) < 0.3  # µs


def test_ac2_whole_sample_arrival_and_total(res):
    c, _ = res
    assert c["xone.delay.ac2_total.10Hz-5.5s -30 (ac2_sweep30)"]["status"] == "PASS"
    assert c["xone.delay.ac2_arrival.10Hz-5.5s -30 (ac2_sweep30)"]["status"] == "FAIL"


def test_lf_h2_overstated_on_10hz_sweeps(res):
    c, _ = res
    assert c["xone.lf_h2.10Hz-5.5s -30 (ac2_sweep30)"]["value"] > 15
    assert c["xone.lf_h2.E0 10Hz-5.5s -30"]["value"] > 6
    assert c["xone.lf_h2.E2 10Hz-11s -30"]["status"] == "INCONCLUSIVE"


def test_report_counts(res):
    _, r = res
    assert sum(r["summary"].values()) > 150
