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
