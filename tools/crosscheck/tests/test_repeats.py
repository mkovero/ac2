import math

from crosscheck import repeats as R


def take(run, checks, stage="genelec", level=-30.0):
    return {"schema": 1, "rig": "pupu", "stage": stage, "level_dbfs": level,
            "provenance": {"run": run, "ac2_build": "0.0.0+abc"}, "checks": checks}


def e(v, status="PASS", unit="dB", tol=(0.3, 1.0), **kw):
    return {"status": status, "value": v, "unit": unit, "tol": list(tol) if tol else None, **kw}


def sines(tf, col, narrow, f="1000"):
    return {f"genelec.sine_mag.ac2 TF@{f}Hz": e(tf),
            f"genelec.sine_mag.direct (TF capture)@{f}Hz": e(col, "INFO", tol=None),
            f"genelec.sine_mag.direct (TF capture), narrow at the sine@{f}Hz": e(narrow, "INFO", tol=None)}


def test_stats_signed_mean_sd_and_zero_cover():
    s = R.stats([-0.4, -0.5, -0.3], (0.3, 1.0))
    assert math.isclose(s["mean"], -0.4) and math.isclose(s["sd"], 0.1)
    assert s["min"] == -0.5 and s["max"] == -0.3
    assert s["range_has_zero"] is False and s["mean_2se_has_zero"] is False
    assert s["over_pass"] == 2  # |−0.3| is at the limit, not over it
    s = R.stats([-0.2, 0.25, 0.05])
    assert s["range_has_zero"] and s["mean_2se_has_zero"]
    one = R.stats([0.1])
    assert one["sd"] is None and one["range_has_zero"] is None and one["mean_2se_has_zero"] is None


def test_pair_checks_use_their_signed_mean():
    takes = [take(f"r{i}", {"genelec.mag.ac2 TF|direct (TF capture).100-1000":
                            e(0.13, "WARN", tol=(0.1, 0.3), mean=m, spread=0.13, n=40)})
             for i, m in enumerate((0.01, -0.02, 0.015))]
    s = R.per_check(takes)["genelec.mag.ac2 TF|direct (TF capture).100-1000"]
    assert s["quantity"] == "mean" and math.isclose(s["mean"], 0.005 / 3)
    assert s["mean_2se_has_zero"] and s["statuses"] == ["WARN"] * 3 and s["spread"] == [0.13] * 3
    md = R.render(takes)
    assert "(signed mean)" in md and "≤ 0.130" in md and "3×WARN" in md


def test_column_to_tone_split():
    takes = [take("a", sines(-0.45, -0.29, -0.03)), take("b", sines(-0.43, -0.27, -0.05))]
    (row,) = R.split(takes)
    assert row["source"] == "ac2 TF" and row["capture"] == "direct (TF capture)" and row["f"] == 1000
    assert math.isclose(row["total"]["mean"], -0.44)
    assert math.isclose(row["processing"]["mean"], -0.16)
    assert math.isclose(row["column_to_tone"]["mean"], -0.24)
    assert math.isclose(row["capture_vs_sine"]["mean"], -0.04)
    # the three terms add up to the total in every take
    t = row
    assert math.isclose(t["processing"]["mean"] + t["column_to_tone"]["mean"] + t["capture_vs_sine"]["mean"],
                        t["total"]["mean"])
    assert "Column-to-tone split" in R.render(takes)


def test_sweep_and_rew_find_their_captures():
    assert R.capture_of("ac2 sweep 20Hz-5.5s") == "direct (ac2 capture 20Hz-5.5s)"
    assert R.capture_of("REW offline") == "direct (REW recording)"
    assert R.capture_of("REW live") is None


def test_takes_group_by_stage_and_level_and_count_a_run_once():
    a = take("r1", {"x": e(0.1)})
    a_more = take("r1", {"x": e(0.1), "y": e(0.2)})
    b = take("r2", {"x": e(0.2)}, level=-50.0)
    g = R.group([a, b, a_more])
    assert sorted(g) == [("genelec", -50.0), ("genelec", -30.0)]
    assert g[("genelec", -30.0)] == [a_more]


def test_missing_values_are_left_out_not_zero():
    takes = [take("a", {"k": e(None, "INCONCLUSIVE")}), take("b", {"k": e(-0.2)})]
    s = R.per_check(takes)["k"]
    assert s["n"] == 1 and s["mean"] == -0.2 and s["statuses"] == ["INCONCLUSIVE", "PASS"]
    md = R.render(takes, match="^k$")
    assert "| k | dB | 1 |" in md
