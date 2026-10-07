import numpy as np
import pytest

from crosscheck import dsp

FS = 96000.0


def frac_delayed_impulse(n, delay, fs=FS, f_hi=None):
    """An exact fractional delay: a frequency-domain phase ramp (band-limited to f_hi)."""
    f = np.fft.rfftfreq(n, 1 / fs)
    H = np.exp(-2j * np.pi * f * delay / fs)
    if f_hi:
        H *= (f <= f_hi)
    return np.fft.irfft(H, n)


@pytest.mark.parametrize("phi", [0.0, 0.1, 0.25, 0.5, 0.73])
def test_fractional_peak_exact_delay(phi):
    h = frac_delayed_impulse(4096, 1000 + phi)
    pos, val = dsp.fractional_peak(h)
    assert abs(pos - (1000 + phi)) < 0.005


def test_fractional_peak_band_limited():
    h = frac_delayed_impulse(8192, 2000.37, f_hi=40000)
    pos, _ = dsp.fractional_peak(h)
    assert abs(pos - 2000.37) < 0.01


def test_delay_from_phase_and_cross_spectrum():
    rng = np.random.default_rng(1)
    n = 1 << 18
    ref = rng.standard_normal(n)
    tau = 3.7e-6
    f = np.fft.rfftfreq(n, 1 / FS)
    meas = np.fft.irfft(np.fft.rfft(ref) * np.exp(-2j * np.pi * f * tau) * 0.18, n)
    c = dsp.log_centres(100, 20000)
    H, coh = dsp.cross_spectrum_bands(meas, ref, FS, c)
    assert np.allclose(dsp.db(H), 20 * np.log10(0.18), atol=1e-6)
    assert np.nanmin(coh) > 0.9999  # the delay turns the phase across a band
    assert abs(dsp.delay_from_phase(c, H, 1000, 20000) - tau) < 1e-9


def hp2(f, fc):
    s = 2j * np.pi * f
    w = 2 * np.pi * fc
    return s * s / (s * s + np.sqrt(2) * w * s + w * w)


def test_group_delay_of_analytic_high_pass():
    f = dsp.log_centres(10, 20000)
    H = hp2(f, 3.0) * np.exp(-2j * np.pi * f * 3.7e-6)
    d = 1e-4
    ph = np.unwrap(np.angle(hp2(np.stack([f * (1 - d), f * (1 + d)]), 3.0)), axis=0)
    truth = -(ph[1] - ph[0]) / (2 * np.pi * f * 2 * d) + 3.7e-6
    gc = dsp.gd_central(f, np.angle(H))
    gs = dsp.gd_slope(f, np.angle(H), f, 1 / 12)
    m = (f > 16) & (f < 10000)
    assert np.nanmax(np.abs(gc[m] / truth[m] - 1)) < 0.01
    # the slope fit over ±1/12 oct is exact for a smooth phase to second order
    assert np.nanmax(np.abs(gs[m] / truth[m] - 1)) < 0.03


def test_sine_harmonics_known_levels():
    n = int(2 * FS)
    t = np.arange(n) / FS
    a = 10 ** (-30 / 20)
    f0 = 101.3
    x = a * np.sin(2 * np.pi * f0 * t) + a * 10 ** (-75 / 20) * np.sin(2 * np.pi * 2 * f0 * t + 0.3) \
        + a * 10 ** (-80 / 20) * np.sin(2 * np.pi * 3 * f0 * t)
    r = dsp.sine_harmonics(x, f0, FS)
    assert abs(r["level_dbfs"] - (-30)) < 0.01
    assert abs(r["h_dbr"][2] + 75) < 0.05
    assert abs(r["h_dbr"][3] + 80) < 0.05
    ph = dsp.sine_phasor(x, f0, FS)
    assert abs(np.degrees(np.angle(ph)) - (-90)) < 0.01  # sin = cos − 90°


def test_harmonic_floor_and_classify():
    rng = np.random.default_rng(2)
    n = int(1 * FS)
    t = np.arange(n) / FS
    f0 = 1003.0
    sig = 10 ** (-30 / 20) * np.sin(2 * np.pi * f0 * t)
    noise = 10 ** (-100 / 20) * rng.standard_normal(int(8 * FS))
    r = dsp.sine_harmonics(sig + noise[:n], f0, FS)
    fl = dsp.harmonic_floor(noise, f0, FS, n, r["p1"])
    # white noise σ² over the 7-bin lobe: 7·σ²·Σw² against the tone's (A·Σw/2)²·(lobe share)
    w = np.blackman(n)
    # the tone's lobe holds its whole power: peak bin × the window's ENBW in bins
    enbw = n * np.sum(w ** 2) / np.sum(w) ** 2
    expect = 10 * np.log10(7 * 1e-10 * np.sum(w ** 2) / ((10 ** -1.5) ** 2 * np.sum(w) ** 2 / 4 * enbw))
    assert abs(fl[2] - expect) < 1.5
    c = dsp.classify(r["h_dbr"][2], fl[2], 10)
    assert c["kind"] == "bound" and c["shortfall"] > 0
    assert dsp.classify(-70, -95, 10)["kind"] == "value"


def test_avoid_mains():
    g = dsp.avoid_mains(100.0, 2.0)
    assert g != 100.0 and abs(g / 100 - 1) < 0.06
    clear = (dsp.MAINLOBE_BINS + 2) / 2.0
    for k in range(2, 6):
        assert abs(k * g - 50 * round(k * g / 50)) > clear


def test_weighting_reference_points():
    w = dsp.weighting_db(np.array([1000.0, 100.0, 31.5]), "A")
    assert abs(w[0]) < 0.01 and abs(w[1] + 19.1) < 0.1 and abs(w[2] + 39.5) < 0.1
    assert abs(dsp.weighting_db(np.array([1000.0]), "C")[0]) < 0.01


def test_leq_of_calibrated_sine():
    n = int(FS)
    t = np.arange(n) / FS
    x = 10 ** (-25.13 / 20) * np.sin(2 * np.pi * 1000 * t)  # 94 dB SPL at 119.13 dB / 0 dBFS
    assert abs(dsp.leq_spectrum(x, FS, 119.13, "Z") - 94.0) < 0.01
    assert abs(dsp.leq_spectrum(x, FS, 119.13, "A") - 94.0) < 0.02
    fc, lv = dsp.third_octave_levels(x, FS, 119.13)
    assert abs(lv[np.argmin(abs(fc - 1000))] - 94.0) < 0.01


def test_mic_correction_normalised_at_1k():
    curve = [[50, 0.6], [1000, 0.2], [10000, -1.0]]
    c = dsp.mic_correction_db(curve, np.array([20.0, 1000.0, 10000.0, 30000.0]))
    assert np.allclose(c, [-0.4, 0.0, 1.2, 1.2])


def test_room_parameters_exponential_decay():
    rng = np.random.default_rng(3)
    t60 = 0.5
    n = int(1.5 * FS)
    t = np.arange(n) / FS
    h = rng.standard_normal(n) * 10 ** (-3 * t / t60) + 1e-6 * rng.standard_normal(n)
    r = dsp.room_parameters(h, FS, 0)
    assert abs(r["t20"] / t60 - 1) < 0.05 and abs(r["t30"] / t60 - 1) < 0.05


def test_sweep_harmonics_replica():
    # an exponential sweep through y = x + c·x²: H2 appears L·ln2 before the linear IR
    fs = 48000.0
    f1, f2, L = 20.0, 20000.0, 0.4
    m = int(L * np.log(f2 / f1) * fs)
    t = np.arange(m) / fs
    s = 10 ** (-20 / 20) * np.sin(2 * np.pi * f1 * L * np.expm1(t / L))
    pad = np.zeros(int(0.5 * fs))
    ref = np.concatenate([pad, s, pad, pad])
    a = 10 ** (-20 / 20)
    c2 = 2 * 10 ** (-60 / 20) / a
    meas = ref + c2 * ref ** 2
    h = dsp.deconvolve(ref, meas)
    d = int(np.argmax(np.abs(h[:1000])))
    r = dsp.sweep_harmonics(h, d, fs, L, np.array([200.0, 1000.0]))
    assert np.all(np.abs(r["h"][2] + 60) < 1.0)
