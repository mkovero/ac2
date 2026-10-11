import numpy as np

from crosscheck import dsp
from crosscheck.ambient import _on_centres, butterworth_band_gain, rta_filter_model


def test_butterworth_band_gain_edges_and_centre():
    fs, fc = 96000.0, 1000.0
    lo, hi = fc * 10 ** -0.05, fc * 10 ** 0.05
    g = butterworth_band_gain(np.array([lo, hi]), lo, hi, fs)
    # the pre-warped edges are the digital −3 dB points
    assert np.allclose(10 * np.log10(g), -10 * np.log10(2), atol=1e-9)
    # the peak (1, 0 dB) is at the digital image of the analog centre √(ωl·ωu)
    k = 2 * fs
    fm = fs / np.pi * np.arctan(np.sqrt(k * np.tan(np.pi * lo / fs) * k * np.tan(np.pi * hi / fs)) / k)
    assert np.isclose(butterworth_band_gain(np.array([fm]), lo, hi, fs)[0], 1.0)
    # order 3 per side: 18 dB per octave of the normalised offset far outside the band
    a1, a2 = 10 * np.log10(butterworth_band_gain(np.array([4 * hi, 8 * hi]), lo, hi, fs))
    assert -20 < a2 - a1 < -16


def test_filter_model_on_white_noise():
    # Flat spectrum: a Butterworth band's noise bandwidth is (π/2N)/sin(π/2N) of its −3 dB width
    # for a narrow band, so order 3 reads 10·lg(1.0472) ≈ +0.20 dB above ideal edges.
    fs = 48000.0
    rng = np.random.default_rng(1)
    x = rng.standard_normal(int(fs * 20)) * 0.01
    fc = np.array([250.0, 1000.0, 4000.0, 12589.0])
    filt, ideal = rta_filter_model(x, fs, 100.0, None, fc)
    _, lv = dsp.third_octave_levels(x, fs, 100.0)
    enbw = 10 * np.log10((np.pi / 6) / np.sin(np.pi / 6))
    assert np.allclose((filt - ideal)[:3], enbw, atol=0.05)
    # with no curve, ideal edges here are the numpy column exactly
    lv_at = np.interp(np.log(fc[:3]), np.log(10 ** (np.arange(13, 44) / 10)), lv)
    assert np.allclose(ideal[:3], lv_at, atol=0.2)
    # above fs/4 ac2 raises the order: not modelled
    assert np.isnan(filt[3])


def test_on_centres_matches_differently_rounded_centres():
    fc = 10 ** (np.arange(14, 17) / 10)
    src = np.array([25.118864315095813, 31.6227766016838, 39.81071705534973])
    v = _on_centres(fc, src, np.array([1.0, 2.0, 3.0]))
    assert v.tolist() == [1.0, 2.0, 3.0]
    assert np.isnan(_on_centres(np.array([60.0]), src, np.array([1.0, 2.0, 3.0]))[0])


class _A:
    """Just what `ambient.analyse` touches of an Analysis."""

    def __init__(self):
        import tomllib
        from pathlib import Path
        self.tol = tomllib.loads((Path(__file__).parent.parent / "tolerances.toml").read_text())
        self.checks, self.notes, self.series = {}, [], {}

    def add(self, **c):
        self.checks[c["id"]] = c

    def table(self, *args, **kw):
        pass


def _ambient_run(tmp_path, offset_db, average="fifo:10800"):
    import types

    from crosscheck import wav
    from crosscheck.ambient import analyse
    fs, S = 48000, 100.0
    rng = np.random.default_rng(3)
    x = rng.standard_normal(fs * 10) * 0.01
    # a strong tone near the 224 Hz edge between the 200 and 250 Hz bands: the order-3 skirts
    # put it into both, which ideal edges do not
    t = np.arange(len(x)) / fs
    x = x + 0.3 * np.sin(2 * np.pi * 223.0 * t)
    wav.write(tmp_path / "in1.wav", fs, x[:, None])
    fc, _ = dsp.third_octave_levels(x, fs, S)
    filt, _ = rta_filter_model(x, fs, S, None, fc)
    ok = np.isfinite(filt)
    rows = "\n".join(f"{f:.6g},{v + offset_db:.6f}" for f, v in zip(fc[ok], filt[ok]))
    (tmp_path / "ac2_rta.csv").write_text("freq_hz,level_db\n" + rows + "\n")
    run = types.SimpleNamespace(
        ambient={"dir": tmp_path, "window": {"ac2_rta_average": average} if average else {}},
        cal={"sensitivity_db": S}, manifest={"mains_hz": 50.0})
    a = _A()
    analyse(a, run)
    return a.checks


def test_ac2_rta_is_judged_against_its_own_filters(tmp_path):
    c = _ambient_run(tmp_path, 0.0)
    own = c["ambient.bands.ac2 RTA|own filters"]
    assert own["status"] == "PASS" and abs(own["value"]) < 1e-3
    assert own["tol"] == (0.25, 0.5)
    # ideal edges: several dB off at the tone's neighbour band, but context only
    ideal = c["ambient.bands.ac2 RTA"]
    assert ideal["status"] == "INFO" and ideal["tol"] is None and ideal["value"] > 2.0
    assert "filter definition" in ideal["meaning"]


def test_own_filter_residual_beyond_its_limits(tmp_path):
    assert _ambient_run(tmp_path, 0.3)["ambient.bands.ac2 RTA|own filters"]["status"] == "WARN"


def test_own_filter_check_needs_an_averaged_rta(tmp_path):
    c = _ambient_run(tmp_path, 0.0, average=None)
    assert c["ambient.bands.ac2 RTA|own filters"]["status"] == "INCONCLUSIVE"
