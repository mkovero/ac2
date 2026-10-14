"""The "REW ÷ ref" source (REW's own Arithmetic A / B): loaded from a run's `div_*` exports,
its lost path delay put back, and judged against the same truths as REW's offline import."""
import base64
import json

import numpy as np

from crosscheck import dsp, formats, model
from crosscheck.analyse import Analysis
from crosscheck.model import Raw, RewSet

FS = 48000.0
D = 24  # the path's delay, samples
GAIN_DB = 20 * np.log10(0.5)


def _b64(x):
    return base64.b64encode(np.asarray(x, ">f4").tobytes()).decode()


def _curve_json(f0, df, mag, ph):
    return {"unit": "dBFS", "smoothing": "None", "startFreq": f0, "freqStep": df,
            "magnitude": _b64(mag), "phase": _b64(ph)}


def test_loader_reads_div_and_takes_the_mic_curve_out(tmp_path):
    run = tmp_path / "run"
    rd = run / "genelec" / "rew"
    rd.mkdir(parents=True)
    f = 20 + 0.5 * np.arange(4000)
    flat = np.full(len(f), -1.0)
    for p in ("meas", "div"):
        (rd / f"{p}_fr.json").write_text(json.dumps(_curve_json(20.0, 0.5, flat, np.zeros(len(f)))))
    (rd / "div_summary.json").write_text(json.dumps({"title": "xc genelec meas / ref", "uuid": "u"}))
    (rd / "play.json").write_text(json.dumps({"level_dbfs": -30.0, "cal_file": "/x/mic_curve_rew.txt"}))
    (run / "cal").mkdir()
    (run / "cal" / "store_excerpt.json").write_text(json.dumps({"curve_points": [[20.0, 2.0], [2000.0, 2.0]]}))
    (run / "manifest.json").write_text(json.dumps({"paths": {"genelec": {"kind": "speaker"}}}))
    (run / "rew_reimport.json").write_text(json.dumps([{"when": "t", "suite_commit": "abc", "stages": {"genelec": {}}}]))
    r = model.load(run)
    p = r.paths["genelec"]
    assert p.rew_div is not None and p.rew_div.summary["uuid"] == "u" and p.rew_div.level_dbfs == -30.0
    # the import carried the curve (applyCal on A, so on A / B): taken out like the meas import's
    want = flat - dsp.mic_correction_db([[20.0, 2.0], [2000.0, 2.0]], f)
    assert np.allclose(p.rew_div.meas_fr.mag, want, atol=1e-6)
    assert np.allclose(p.rew_div.meas_fr.mag, p.rew.meas_fr.mag)
    assert r.manifest["rew_reimport"][0]["suite_commit"] == "abc"


def test_loader_without_div_leaves_it_out(tmp_path):
    run = tmp_path / "run"
    rd = run / "xone" / "rew"
    rd.mkdir(parents=True)
    f = np.zeros(10)
    (rd / "meas_fr.json").write_text(json.dumps(_curve_json(20.0, 0.5, f, f)))
    (run / "manifest.json").write_text(json.dumps({"paths": {"xone": {"kind": "electrical"}}}))
    assert model.load(run).paths["xone"].rew_div is None


def _path():
    """An electrical path of gain 0.5 and a D-sample delay: an ac2 sweep referred to its
    arrival, REW's recording, and REW's A / B referred to its timing markers (delay gone)."""
    rng = np.random.default_rng(1)
    ref = rng.standard_normal(int(4 * FS))
    meas = 0.5 * np.concatenate([np.zeros(D), ref[:-D]])
    f = dsp.log_centres(20, 20000, 48)
    tr = formats.Ac2Trace(meta={"sweep_info": {"arrival": D / FS}},
                          blocks={"freq_hz": {"freq_hz": f, "mag_db": np.full(len(f), GAIN_DB),
                                              "phase_deg": np.zeros(len(f))}})
    p = model.PathData(name="xone", kind="electrical")
    p.sweeps["s"] = model.Sweep(name="s", trace=tr, level_dbfs=-10.0)
    p.primary = "s"
    p.rec = Raw(fs=FS, meas=meas, ref=ref)
    fl = np.arange(1, 48000) * 0.5
    p.rew_div = RewSet(label="REW ÷ ref (REW's A / B)", level_dbfs=-10.0,
                       meas_fr=formats.RewCurve(f=fl, mag=np.full(len(fl), GAIN_DB), phase=np.zeros(len(fl)), unit="dBFS"))
    p.truth = {"tones": [{"f": fc, "ratio_db": GAIN_DB, "ratio_deg": float(dsp.wrap_deg(-360 * fc * D / FS))}
                         for fc in (100.0, 1000.0, 5000.0)]}
    return p


def test_rew_div_gets_its_delay_back_and_passes_against_the_truths():
    p = _path()
    a = Analysis(model.RunData(root=None, paths={"xone": p}, manifest={}))
    a.sources(p)
    assert "REW ÷ ref" in a.src
    assert abs(a.rew_div_put_back - D / FS) < 0.05 / FS
    a.bands(p)
    by = {c.id: c for c in a.checks}
    for lo, hi in ((20, 100), (100, 1000), (1000, 20000)):
        for g in ("mag", "phase"):
            assert by[f"xone.{g}.REW ÷ ref|direct (REW recording).{lo}-{hi}"].status == "PASS"
            assert by[f"xone.{g}.ac2 sweep s|REW ÷ ref.{lo}-{hi}"].status == "PASS"
    for fc in ("100", "1000", "5000"):
        m, ph = by[f"xone.sine_mag.REW ÷ ref.{fc}"], by[f"xone.sine_phase.REW ÷ ref.{fc}"]
        assert m.status == "PASS" and abs(m.value) < 0.01
        assert ph.status == "PASS" and abs(ph.value) < 0.5
        # an ac2 − REW ÷ ref pair forms from the twin keys, as for REW offline
        assert m.detail["twin"]["analyser"] == "REW"
    assert "µs put back" in by["xone.sine_phase.REW ÷ ref.1000"].meaning


def test_comparison_columns_pick_rew_div_and_its_difference():
    from crosscheck import comparison as C

    def e(v, tol=(0.05, 0.2)):
        return {"status": "PASS", "value": v, "unit": "dB", "tol": list(tol)}

    b = {"stage": "xone", "checks": {
        "xone.sine_mag.ac2 sweep 10Hz-5.5s@1000Hz": e(0.04), "xone.sine_mag.REW offline@1000Hz": e(0.01),
        "xone.sine_mag.REW ÷ ref@1000Hz": e(-0.02),
        "xone.mag.REW ÷ ref|direct (REW recording).1000-20000": e(0.002, (0.03, 0.1)),
        "xone.mag.ac2 sweep 10Hz-5.5s|REW ÷ ref.1000-20000": e(0.009, (0.03, 0.1))}}
    cols = dict(zip(C.COLUMNS, C.row_hits([b], C.MAG_TONES)))
    assert [h.key for h in cols["REW ÷ ref vs truth"]] == ["xone.sine_mag.REW ÷ ref@1000Hz"]
    assert [round(h.value, 3) for h in cols["ac2 − REW ÷ ref"]] == [0.06]
    assert [round(h.value, 3) for h in cols["ac2 − REW"]] == [0.03]
    hf = dict(zip(C.COLUMNS, C.row_hits([b], C.MAG_BANDS[2])))
    assert len(hf["REW ÷ ref vs truth"]) == 1 and len(hf["ac2 − REW ÷ ref"]) == 1 and not hf["REW vs truth"]


def test_soundcard_corrected_rew_import_is_compared_alone(tmp_path):
    """With REW's soundcard cal the meas import is the path's response by itself, the
    interface's 1 kHz gain (−3 dB here, what the reference input reads) aside: "REW offline" is
    that import over the one number, not meas ÷ ref (the ref's slope here would show), and its
    lost delay is put back like the quotient's."""
    p = _path()
    fl = np.arange(1, 48000) * 0.5
    curve = lambda db: formats.RewCurve(f=fl, mag=np.asarray(db) + np.zeros(len(fl)), phase=np.zeros(len(fl)), unit="dBFS")
    slope = 0.5 * np.log2(fl / 1000.0)  # a ref that is no longer flat: dividing by it would show
    p.rew = RewSet(label="REW offline import", level_dbfs=-10.0, meas_fr=curve(GAIN_DB - 3.0),
                   ref_fr=curve(-3.0 + slope), meas_alone=True)
    a = Analysis(model.RunData(root=None, paths={"xone": p}, manifest={}))
    a.sources(p)
    assert abs(a.rew_put_back - D / FS) < 0.05 / FS
    a.bands(p)
    by = {c.id: c for c in a.checks}
    for lo, hi in ((20, 100), (100, 1000), (1000, 20000)):
        for g in ("mag", "phase"):
            assert by[f"xone.{g}.ac2 sweep s|REW offline.{lo}-{hi}"].status == "PASS"
            assert by[f"xone.{g}.REW offline|direct (REW recording).{lo}-{hi}"].status == "PASS"
    assert "soundcard cal" in by["xone.mag.ac2 sweep s|REW offline.100-1000"].meaning


def test_loader_marks_a_soundcard_corrected_import(tmp_path):
    run = tmp_path / "run"
    rd = run / "xone" / "rew"
    rd.mkdir(parents=True)
    z = np.zeros(10)
    for q in ("meas", "ref"):
        (rd / f"{q}_fr.json").write_text(json.dumps(_curve_json(20.0, 0.5, z, z)))
    (run / "manifest.json").write_text(json.dumps({"paths": {"xone": {"kind": "electrical"}}}))
    assert not model.load(run).paths["xone"].rew.meas_alone  # an older run: meas ÷ ref as before
    (rd / "play.json").write_text(json.dumps({"level_dbfs": -10.0, "soundcard_cal": "/sc.cal"}))
    assert model.load(run).paths["xone"].rew.meas_alone
