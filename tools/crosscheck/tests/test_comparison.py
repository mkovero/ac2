import json

import pytest

from crosscheck import baseline as B
from crosscheck import comparison as C
from crosscheck import comparison_quotes as Q


def test_generated_tables_are_current():
    """comparison.md's generated parts and quoted numbers are those of the committed baselines
    and reports, and every miss has its open item: after a baseline update, run
    `python -m crosscheck comparison` and commit both."""
    current, _ = C.update(C.DOC, C.BASELINES, check=True)
    assert current, "comparison.md is stale: run `python -m crosscheck comparison`"


def e(value, status="PASS", unit="dB", tol=(0.03, 0.1)):
    return {"status": status, "value": value, "unit": unit, "tol": list(tol) if tol else None}


def test_cell_worst_judged_value_with_location_and_counts():
    m = C.matches_for({"checks": {
        "xone.mag.ac2 sweep 10Hz-5.5s|REW offline.20-100": e(0.02),
        "xone.mag.ac2 sweep 10Hz-5.5s|REW offline.100-1000": e(-0.05, "WARN"),
        "xone.mag.ac2 sweep 10Hz-5.5s|REW offline.1000-20000": e(None, "INCONCLUSIVE"),
        "xone.mag.ac2 sweep dist|REW offline.20-100": e(9.0),  # another sweep: not this row
    }}, C.RIG_ROWS[0][1])
    assert C.cell(m) == "−0.050 dB @ 100–1000 Hz (WARN 1, PASS 1, INCONCLUSIVE 1)"
    # a harmonic row shows the order; an INFO-only row shows its value; bounds only show counts
    h = C.matches_for({"checks": {"x.h3.REW offline import@60.5Hz": e(0.6, tol=(3, 6)),
                                  "x.h2.REW offline import@50Hz": e(-0.1, tol=(3, 6))}},
                      r"(?P<o>h[2-5])\.REW offline import@")
    assert C.cell(h) == "+0.600 dB @ H3 60.5 Hz (PASS 2)"
    assert C.cell([("", None, e(-3.4, "INFO", "µs", None))]) == "−3.40 µs (INFO 1)"
    assert C.cell([("", None, e(None, "INCONCLUSIVE"))]) == "INCONCLUSIVE 1"
    assert C.fmt_value(-1e-9, "dB") == "0.000 dB" and C.fmt_value(0.023, "rel") == "+2.3 %"


def test_statistic_sets_the_sign():
    """A magnitude without a direction (max |Δ|, a band's max(|mean|, spread)) is shown unsigned so
    it cannot read as a bias; a signed residual keeps its sign; a share reads as a percentage."""
    assert C.fmt_value(0.031, "dB", C.ABS) == "0.031 dB" and C.fmt_value(-2.1, "°", C.ABS) == "2.100°"
    assert C.fmt_value(-0.306, "dB") == "−0.306 dB" and C.fmt_value(0.355, "", C.SHARE) == "35.5 %"
    m = [("100-1000", None, e(0.133, "WARN")), ("20-100", None, e(None, "INCONCLUSIVE"))]
    assert C.cell(m, C.ABS) == "0.133 dB @ 100–1000 Hz (WARN 1, INCONCLUSIVE 1)"
    # every row and OSM column names its statistic: unsigned rows carry |…| or "spread" in the label
    for label, _, *stat in C.RIG_ROWS + C.AMBIENT_ROWS:
        if stat == [C.ABS]:
            assert "|" in label or "spread" in label, label
    for head, _, *stat in C.OSM_TF_COLS:
        assert (stat == [C.ABS]) == head.startswith("\\|"), head


def _baselines(tmp_path):
    rig = {"schema": 1, "rig": "r", "stage": "xone", "level_dbfs": -30.0,
           "provenance": {"run": "20261007T114727Z", "ac2_build": "0.0.0+26388938b456", "rew_version": "5.40"},
           "summary": {"PASS": 1}, "checks": {"xone.level.ac2_vs_rew_meas_minus_ref": e(-0.0004, tol=(0.1, 0.3)),
                      "xone.h2.ac2 sweep 10Hz-5.5s@1000Hz": e(-1.0, tol=(3, 6)),
                      "xone.h4.ac2 sweep 10Hz-5.5s@1000Hz": e(None, "INCONCLUSIVE", tol=(3, 6))}}
    osm = {"schema": 1, "rig": "none (offline)", "stage": "osm", "level_dbfs": None,
           "provenance": {"run": "osm-x", "ac2_build": "ac2d (build 0.0.0+26388938b456)", "osm_version": "v1.5.2",
                          "flags": {"cases": ["identity", "delay10_5"]}},
           "summary": {"PASS": 2, "INFO": 1},
           "checks": {"osm.delay10_5.osm_delay_finder_vs_analytic": e(-0.5, unit="samples", tol=(0.5, 1)),
                      "osm.identity.ac2_tf_h_vs_analytic": e(0.0),
                      "osm.xone-rig.ac2_vs_osm_h": e(0.2),
                      "osm.xone-rig.ac2_delay_finder_vs_osm_s": e(0.5, unit="samples", tol=(0.6, 1)),
                      "osm.delay48.uncompensated_ac2_h_vs_analytic": e(0.417, "INFO", tol=None)}}
    # takes on an older build: not run on the document's build, so not shown
    older = {**rig, "level_dbfs": -50.0, "provenance": {**rig["provenance"], "run": "20261006T000000Z",
                                                        "ac2_build": "0.0.0+0123456789ab"},
             "checks": {"xone.level.ac2_vs_rew_meas_minus_ref": e(0.5, "WARN", tol=(0.1, 0.3))}}
    ambient = {"schema": 1, "rig": "r", "stage": "ambient", "level_dbfs": None,
               "provenance": {"run": "20261005T000000Z", "ac2_build": "0.0.0+0123456789ab"},
               "summary": {"PASS": 1}, "checks": {"ambient.leq.A.ac2": e(0.01, tol=(0.1, 0.2))}}
    # another level on the newest rig take's build: shown beside it
    same = {**rig, "level_dbfs": -20.0, "provenance": {**rig["provenance"], "run": "20261007T100000Z"},
            "checks": {"xone.level.ac2_vs_rew_meas_minus_ref": e(0.2, "WARN", tol=(0.1, 0.3))}}
    for sub, name, b in (("r", "xone-30dbfs.json", rig), ("r", "xone-50dbfs.json", older),
                         ("r", "xone-20dbfs.json", same), ("r", "ambient.json", ambient),
                         ("host", "osm.json", osm)):
        (tmp_path / sub).mkdir(exist_ok=True)
        (tmp_path / sub / name).write_text(B.dumps(b))
    return tmp_path


ITEMS = [("Xone gain at −20", "open", r"xone\.level\.")]


def _doc(body=""):
    return ("intro\n" + "".join(f"{C.begin(n)}\nold\n{C.end(n)}\n" for n in C.BLOCKS) + body
            + "\n### Xone gain at −20\n\nopen\n")


def test_render_from_a_baseline_directory(tmp_path):
    root = _baselines(tmp_path)
    out = C.render(root)
    # every baseline on the newest rig take's build, louder level first; no other build
    assert "| ac2 meas÷ref vs REW (meas − ref), absolute level | +0.200 dB (WARN 1) | 0.000 dB (PASS 1) |" in out
    assert "xone −30 dBFS (20261007T114727Z)" in out and "xone −20 dBFS" in out
    assert "−50 dBFS" not in out and "ambient" not in out and "2638893" not in out
    # OSM cases in the planned order, then the rest; INFO-only checks not tabled
    assert out.index("| identity |") < out.index("| delay10_5 |")
    assert "| delay10_5 |  | −0.500 |" in out
    assert "+0.417" not in out
    # a rig take (not a planned case) is a median in the transfer table only
    assert "| xone-rig (median) |  |  |  |  | 0.200 |" in out and "| xone-rig |  |  |  |  | +0.500 |" in out
    # distortion coverage per order, counts even where nothing was judged
    assert "| H2 ac2 sweep | — | −1.000 dB (PASS 1) |" in out and "| H4 ac2 sweep | — | INCONCLUSIVE 1 |" in out
    doc = tmp_path / "doc.md"
    doc.write_text(_doc())
    current, _ = C.update(doc, root, check=True, items=ITEMS)
    assert not current and "old" in doc.read_text()
    C.update(doc, root, items=ITEMS)
    text = doc.read_text()
    assert "old" not in text and out.strip() in text
    assert C.update(doc, root, check=True, items=ITEMS)[0]
    # a hand edit inside a generated part is stale again
    doc.write_text(text.replace("0.000 dB (PASS 1)", "0.001 dB (PASS 1)"))
    assert not C.update(doc, root, check=True, items=ITEMS)[0]


def test_summary_row_names_limit_verdict_worst_and_run(tmp_path):
    root = _baselines(tmp_path)
    doc = tmp_path / "doc.md"
    doc.write_text(_doc())
    C.update(doc, root, items=ITEMS)
    text = doc.read_text()
    # both xone levels on the build, the limit they carry, the miss, and the run it is from
    assert ("| Level: electrical gain | REW (meas − ref) | xone −20/−30 dBFS | 0.1 / 0.3 dB | "
            "**WARN** (WARN 1 of 2 judged) | +0.200 dB @ xone −20 dBFS | 20261007T100000Z |") in text
    # the build named once, and a stage with no baseline on it named, not shown with old numbers
    assert "**ac2 build 2638893**" in text and text.count("2638893") == 1
    assert ("Not run on this build (their baselines are from earlier builds and not shown): "
            "r: ambient, xone −50 dBFS.") in text
    assert ("| Leq Z, A, C | truth: numpy on the same recording | ambient | — | "
            "not run on this build | — | — |") in text
    assert ("| Harmonics H2–H5 | truth: steady sine | xone −30 dBFS | 3 / 6 dB | "
            "PASS (1 judged, 1 inconclusive) |") in text
    # OSM rows split the planned fixtures from the rig recordings
    assert ("| Transfer: ac2 vs OSM | OSM on the same rig recording (median) | osm | 0.03 / 0.1 dB | "
            "PASS (1 judged) | 0.200 dB @ xone-rig |") in text
    # the miss list links the item's section
    assert ("| [Xone gain at −20](#xone-gain-at-20) | open | xone −20 dBFS | WARN 1 | +0.200 dB | "
            "0.1 / 0.3 dB |") in text
    # a stale summary cell fails the check
    doc.write_text(text.replace("**WARN** (WARN 1 of 2 judged)", "PASS (2 judged)"))
    assert not C.update(doc, root, check=True, items=ITEMS)[0]


def test_every_miss_belongs_to_a_discussed_item(tmp_path, capsys):
    root = _baselines(tmp_path)
    doc = tmp_path / "doc.md"
    # a miss no item names
    doc.write_text(_doc())
    assert not C.update(doc, root, check=True, items=[])[0]
    assert "belongs to no item" in capsys.readouterr().err
    # an item with no miss left, and an item without its section
    gone = ITEMS + [("Closed", "open", r"nothing$")]
    assert not C.update(doc, root, check=True, items=gone)[0]
    err = capsys.readouterr().err
    assert "'Closed' has no WARN or FAIL left" in err and "no `### Closed` section" in err
    with pytest.raises(ValueError, match="problem"):
        C.update(doc, root, items=gone)


def test_quoted_numbers_follow_their_source(tmp_path, capsys):
    root = _baselines(tmp_path)
    ref = "<!--= b r/xone-20dbfs // xone.level.ac2_vs_rew_meas_minus_ref -->"
    doc = tmp_path / "doc.md"
    doc.write_text(_doc(f"\nThe gain at −20 reads +0.31 dB{ref}, or 0.3{ref} unsigned.\n"))
    current, _ = C.update(doc, root, check=True, items=ITEMS)
    assert not current
    C.update(doc, root, items=ITEMS)
    # rewritten at the precision and sign style written
    assert f"reads +0.20 dB{ref}, or 0.2{ref} unsigned" in doc.read_text()
    assert C.update(doc, root, check=True, items=ITEMS)[0]
    # a source that does not resolve is reported, not guessed
    doc.write_text(_doc("\nA gone check: +0.20 dB<!--= b r/xone-20dbfs // xone.no_such_check -->.\n"))
    assert not C.update(doc, root, check=True, items=ITEMS)[0]
    assert "no check 'xone.no_such_check'" in capsys.readouterr().err
    # a quote from another build would combine builds: refused
    doc.write_text(_doc("\nOld: +0.50 dB<!--= b r/xone-50dbfs // xone.level.ac2_vs_rew_meas_minus_ref -->.\n"))
    assert not C.update(doc, root, check=True, items=ITEMS)[0]
    assert "r/xone-50dbfs is on ac2 build 0123456, not the document's 2638893" in capsys.readouterr().err


def test_quote_sources_and_formatting(tmp_path):
    root = _baselines(tmp_path)
    assert Q.resolve("b r/xone-30dbfs // xone.h2.ac2 sweep 10Hz-5.5s@1000Hz // tol.1", root) == 6
    assert Q.resolve("b r/xone-20dbfs // xone.level.ac2_vs_rew_meas_minus_ref minus "
                     "xone.level.ac2_vs_rew_meas_minus_ref", root) == 0
    assert Q.resolve("t harmonics.vs_truth_db.pass", root) == 3.0
    assert Q.written_as(-0.4979, "−0.50") == "−0.50" and Q.written_as(-0.4979, "0.5") == "0.5"
    assert Q.written_as(0.00143, "+0.001") == "+0.001" and Q.written_as(30, "30") == "30"
    # a report cell: the i-th number of a table cell under the named headings
    rep = tmp_path / "reports"
    rep.mkdir()
    (rep / "x.md").write_text(
        "# X\n\n## a, 1/48 octave\n\n### Split\n\n| f Hz | m | total |\n|---|---|---|\n"
        "| 1000 | ac2 TF | -0.202 ± 0.167 |\n\n## b, 1/96 octave\n\n### Split\n\n"
        "| f Hz | m | total |\n|---|---|---|\n| 1000 | ac2 TF | +0.085 ± 0.045 |\n")
    old, Q.REPORTS = Q.REPORTS, rep
    try:
        assert Q.resolve("r x.md // 1/96 octave > Split // f Hz=1000; m=ac2 TF // total#1", root) == 0.045
        assert Q.resolve("r x.md // 1/48 octave > Split // f Hz=1000; m=ac2 TF // total", root) == -0.202
        with pytest.raises(LookupError):
            Q.resolve("r x.md // 1/48 octave > Split // f Hz=2000 // total", root)
    finally:
        Q.REPORTS = old


def test_unsourced_numbers_and_build_hashes_refused():
    bad = Q.unquoted("ac2 reads +0.32 dB here,\n30 of 159 columns,\non build 2c07efe,\nwithin 2 dB.\n")
    assert [b.split(":")[0] for b in bad] == ["line 1", "line 2", "line 3", "line 4"]
    # definitions, settings, frequencies, levels, run IDs and quoted numbers need no source
    ok = ("REW 5.40 and OSM v1.5.2, IEC 61672-1, γ² ≥ 0.99, the 20 Hz / 5.5 s sweep at −30 dBFS,\n"
          "1/48 octave, H2–H5, 100 Hz–1 kHz, run 20261011T191033Z, a sine reads 3.01 dB lower,\n"
          "`0.25 / 0.5 dB` in code, [a link](reports/x-1.5.md), +0.20 dB<!--= t level.offset_db.pass -->\n")
    assert Q.unquoted(ok) == []


def test_the_documents_hand_written_numbers_all_have_sources():
    """No number in comparison.md's prose can drift: each is quoted from a source (held to it by
    `test_generated_tables_are_current`) or is a listed definition."""
    doc = C.DOC.read_text()
    assert Q.unquoted(C.hand_written(doc)) == []
    quotes = list(Q.QUOTE.finditer(doc))
    assert len(quotes) > 40
    for m in quotes:
        assert Q.written_as(Q.resolve(m["ref"], C.BASELINES), m["num"]) == m["num"].replace("-", "−"), m[0]


def _parity(a_val, a_st, r_val, r_st, shared=False):
    sh = {"shared": {"ac2": a_val, "REW": r_val, "diff": a_val - r_val, "agree": 3.0}} if shared else {}
    b = {"checks": {"g.h3.ac2 sweep 20Hz-5.5s@500Hz": {**e(a_val, a_st, tol=(3, 6)), **sh},
                    "g.h3.REW offline import@500Hz": {**e(r_val, r_st, tol=(3, 6)), **sh}}}
    return C.parity_cell(b, *C.PARITY_ROWS[4][1:3], None)


def test_parity_both_miss_alike_only_when_annotated():
    # opposite signs: each analyser's own miss, never "alike"
    out = _parity(7.0, "FAIL", -7.0, "FAIL")
    assert out.startswith("ac2 FAIL, REW FAIL, not alike") and "alike at" not in out.replace("not alike", "")
    # one sign, unequal size (no annotation from the analysis): not alike either
    out = _parity(20.0, "FAIL", 4.0, "WARN")
    assert out.startswith("ac2 FAIL, REW WARN, not alike")
    # annotated: both miss alike, and the wording claims no cause
    out = _parity(4.0, "WARN", 4.2, "WARN", shared=True)
    assert out.startswith("as REW, both miss alike")
    assert "ac2 and REW miss the truth alike (ac2 − REW within tolerance) at H3 500 Hz" in out
    assert "stimulus" not in out and "device" not in out


def test_annotated_misses_stay_in_the_worst_cell():
    m = C.matches_for({"checks": {
        "g.sine_mag.ac2 sweep 20Hz-5.5s@1000Hz": {**e(-0.32, "WARN", tol=(0.3, 1.0)), "shared": {"diff": 0.01}},
        "g.sine_mag.ac2 sweep 20Hz-5.5s@100Hz": e(-0.019, tol=(0.3, 1.0))}}, C.RIG_ROWS[8][1])
    assert C.cell(m) == "−0.320 dB @ 1000 Hz (WARN 1, PASS 1)"


def test_missing_markers_refused(tmp_path):
    doc = tmp_path / "doc.md"
    doc.write_text("no markers\n")
    with pytest.raises(ValueError, match="marker"):
        C.update(doc, tmp_path)
