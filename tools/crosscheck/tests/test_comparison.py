import json

import pytest

from crosscheck import baseline as B
from crosscheck import comparison as C
from crosscheck import comparison_quotes as Q


def test_document_is_current():
    """comparison.md's table and quoted numbers are those of the committed baselines, and every
    warn or FAIL cell has its section: after a baseline update, run `python -m crosscheck
    comparison`, write the sections it asks for, and commit both."""
    current, problems = C.update(C.DOC, C.BASELINES, check=True)
    assert current, f"comparison.md is stale: run `python -m crosscheck comparison` ({problems})"


def e(value, status="PASS", unit="dB", tol=(0.05, 0.2), **kw):
    return {"status": status, "value": value, "unit": unit, "tol": list(tol) if tol else None, **kw}


def _write(root, sub, name, b):
    (root / sub).mkdir(exist_ok=True)
    (root / sub / name).write_text(B.dumps(b))


def _baselines(tmp_path):
    pv = {"run": "20261013T100000Z", "started": "2026-10-13T10:00:00Z", "ac2_build": "0.0.0+26388938b456"}
    xone = {"schema": 1, "rig": "r", "stage": "xone", "level_dbfs": -30.0, "provenance": pv, "summary": {},
            "checks": {
                "xone.sine_mag.ac2 sweep 10Hz-5.5s@1000Hz": e(-0.06, "WARN"),
                "xone.sine_mag.ac2 sweep 10Hz-5.5s@100Hz": e(0.04),
                "xone.sine_mag.REW offline@1000Hz": e(-0.03),
                "xone.sine_mag.REW offline@100Hz": e(None, "INCONCLUSIVE"),
                # a band's max(|mean|, spread): no direction, shown unsigned
                "xone.mag.ac2 sweep 10Hz-5.5s|REW offline.20-100": e(-0.02, tol=(0.03, 0.1), spread=0.02),
                "xone.h2.ac2 sweep 10Hz-5.5s@1000Hz": e(None, "INCONCLUSIVE", tol=(3, 6)),
            }}
    osm = {"schema": 1, "rig": "none (offline)", "stage": "osm", "level_dbfs": None,
           "provenance": {**pv, "run": "osm-x", "ac2_build": "ac2d (build 0.0.0+26388938b456)"},
           "summary": {}, "checks": {"osm.identity.ac2_vs_osm_h": e(0.0),
                                     "osm.genelec-rig.ac2_vs_osm_h": e(0.2, tol=(0.3, 1.0))}}
    # an older build: not shown, its stage named as not run
    ambient = {"schema": 1, "rig": "r", "stage": "ambient", "level_dbfs": None,
               "provenance": {**pv, "started": "2026-10-01T00:00:00Z", "ac2_build": "0.0.0+0123456789ab"},
               "summary": {}, "checks": {"ambient.leq.A.ac2": e(5.0, "FAIL")}}
    _write(tmp_path, "r", "xone-30dbfs.json", xone)
    _write(tmp_path, "r", "ambient.json", ambient)
    _write(tmp_path, "host", "osm.json", osm)
    return tmp_path


TONES = "Xone: Magnitude at the tones: ac2 vs truth"


def _doc(body=f"## Disparities\n\n### {TONES}\n\nopen\n"):
    return "intro\n" + "".join(f"{C.begin(n)}\nold\n{C.end(n)}\n" for n in C.BLOCKS) + body


def _path(text, title):
    """One path's caption and table."""
    return next(p for p in text.split("\n\n**") if p.lstrip("*").startswith(title))


def _row(text, label):
    return next(line for line in text.splitlines() if line.startswith(f"| {label} |"))


def test_tables_per_path_worst_value_and_verdict(tmp_path):
    root = _baselines(tmp_path)
    text = C.blocks(root)["table"]
    xone = _path(text, "Xone (electrical), −30 dBFS.")
    # only the columns some row has a value in, the row's one limit
    assert "| quantity | ac2 vs truth | REW vs truth | ac2 − REW | limit (pass / warn) |" in xone
    # worst verdict first; ac2 − REW at a tone is the difference of the two readings vs the sine
    assert _row(xone, "Magnitude at the tones") == ("| Magnitude at the tones | −0.060 dB **warn** | −0.030 dB pass "
                                                    "| −0.030 dB pass | 0.05 / 0.2 dB |")
    # a band is its own row, held to its own limit; a band's max(|mean|, spread) is unsigned
    assert _row(xone, "Magnitude 20–100 Hz, sweeps") == "| Magnitude 20–100 Hz, sweeps | — | — | 0.020 dB pass | 0.03 / 0.1 dB |"
    # only bounds: not judged
    assert _row(xone, "Harmonics H2–H5") == "| Harmonics H2–H5 | not judged | — | — | — |"
    # OSM's synthetic fixtures and its rig recordings are separate tables
    assert _row(_path(text, "OSM fixtures."), "Magnitude") == "| Magnitude | 0.000 dB pass | 0.05 / 0.2 dB |"
    assert _row(_path(text, "OSM on the rig recordings."), "Genelec: magnitude") == "| Genelec: magnitude | 0.200 dB pass | 0.3 / 1 dB |"
    # a path not on the build: not run, no table
    assert _path(text, "Ambient (silent room).").endswith("Not run on this build.")
    assert "2638893" not in text


def test_row_mixing_limits_is_refused(tmp_path):
    root = _baselines(tmp_path)
    p = root / "r" / "xone-30dbfs.json"
    b = json.loads(p.read_text())
    b["checks"]["xone.sine_mag.ac2 TF@1000Hz"] = e(0.01, tol=(0.1, 0.3))
    p.write_text(B.dumps(b))
    problems = C.mixed_limits([x for _, x in C.current(C.load(root))])
    assert problems == ["Xone: 'Magnitude at the tones' mixes limits (0.05 / 0.2 dB; 0.1 / 0.3 dB): split the row"]


def test_pair_is_the_difference_at_one_tone():
    b = {"stage": "genelec", "checks": {
        "g.spl.ac2 sweep@1000Hz": e(-0.4, tol=(0.5, 1.0)), "g.spl.REW (SPL, cal from ac2)@1000Hz": e(0.3, tol=(0.5, 1.0)),
        "g.spl.ac2 sweep@50Hz": e(0.1, tol=(0.5, 1.0)), "g.spl.REW (SPL, cal from ac2)@50Hz": e(None, "INCONCLUSIVE")}}
    hs = C.hits(b, C.SPL.ac2_rew[0])
    assert [(round(h.value, 3), h.status) for h in hs] == [(-0.7, "WARN")]
    assert C.cell_text(hs) == "−0.700 dB **warn**"


def test_value_format():
    assert C.fmt_value(-1e-9, "dB") == "0.000 dB" and C.fmt_value(0.023, "rel") == "+2.3 %"
    assert C.fmt_value(-2.1, "°", signed=False) == "2.10°" and C.fmt_value(-0.5, "samples") == "−0.500 samples"


def test_build_line_names_one_build(tmp_path):
    text = C.blocks(_baselines(tmp_path))["build"]
    assert text == ("ac2 build **2638893**, runs of 2026-10-13: Xone (electrical) at −30 dBFS; "
                    "OSM fixtures and rig recordings. Not run on this build: ambient.")


def test_rebuild_check_and_disparity_sections(tmp_path, capsys):
    root = _baselines(tmp_path)
    doc = tmp_path / "doc.md"
    doc.write_text(_doc())
    assert not C.update(doc, root, check=True)[0] and "old" in doc.read_text()
    assert C.update(doc, root) == (False, [])
    assert C.update(doc, root, check=True) == (True, [])
    # a hand edit inside a generated part is stale again
    doc.write_text(doc.read_text().replace("−0.060 dB **warn**", "−0.060 dB pass"))
    assert not C.update(doc, root, check=True)[0]
    # a warn cell without its section, and a section without a warn cell
    # (keyed by path: the same quantity on another path is another cell)
    doc.write_text(_doc("## Disparities\n\n### Genelec: Magnitude at the tones: ac2 vs truth\n\n## Reproducing\n"))
    current, problems = C.update(doc, root, check=True)
    assert not current and len(problems) == 2
    assert f"'{TONES}' cell is warn or FAIL" in problems[0]
    assert "`### Genelec: Magnitude at the tones: ac2 vs truth`" in problems[1]


def test_quoted_numbers_follow_their_source(tmp_path, capsys):
    root = _baselines(tmp_path)
    ref = "<!--= b r/xone-30dbfs // xone.sine_mag.ac2 sweep 10Hz-5.5s@1000Hz -->"
    doc = tmp_path / "doc.md"
    doc.write_text(_doc() + f"\nIt reads +0.31 dB{ref}, or 0.3{ref} unsigned.\n")
    C.update(doc, root)
    # rewritten at the precision and sign style written
    assert f"reads −0.06 dB{ref}, or 0.1{ref} unsigned" in doc.read_text()
    assert C.update(doc, root, check=True)[0]
    # a source that does not resolve is reported, not guessed
    doc.write_text(_doc() + "\nGone: +0.20 dB<!--= b r/xone-30dbfs // xone.no_such_check -->.\n")
    assert "no check 'xone.no_such_check'" in " ".join(C.update(doc, root, check=True)[1])
    # a quote from another build would mix builds: refused
    doc.write_text(_doc() + "\nOld: +5.0 dB<!--= b r/ambient // ambient.leq.A.ac2 -->.\n")
    assert "r/ambient is on ac2 build 0123456, not the document's 2638893" in " ".join(C.update(doc, root, check=True)[1])


def test_quote_sources():
    root = None
    assert Q.resolve("t harmonics.vs_truth_db.pass", root) == 3.0
    assert Q.written_as(-0.4979, "−0.50") == "−0.50" and Q.written_as(-0.4979, "0.5") == "0.5"
    assert Q.written_as(30, "30") == "30"
    with pytest.raises(LookupError):
        Q.resolve("x nothing", root)


def test_unsourced_numbers_and_build_hashes_refused():
    bad = Q.unquoted("ac2 reads +0.32 dB here,\n30 of 159 columns,\non build 2c07efe,\nwithin 2 dB.\n")
    assert [b.split(":")[0] for b in bad] == ["line 1", "line 2", "line 3", "line 4"]
    ok = ("REW 5.40 and OSM v1.5.2, IEC 61672-1, γ² ≥ 0.99, the 20 Hz / 5.5 s sweep at −30 dBFS,\n"
          "H2–H5, 100 Hz–1 kHz, `0.25 / 0.5 dB` in code, [a link](reports/x-1.5.md), "
          "+0.20 dB<!--= t level.offset_db.pass -->\n")
    assert Q.unquoted(ok) == []


def test_missing_markers_refused(tmp_path):
    doc = tmp_path / "doc.md"
    doc.write_text("no markers\n")
    with pytest.raises(ValueError, match="marker"):
        C.update(doc, tmp_path)
