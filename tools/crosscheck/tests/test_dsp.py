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


def _ess(fs, f1, f2, L, level_db=-20):
    m = int(L * np.log(f2 / f1) * fs)
    t = np.arange(m) / fs
    return 10 ** (level_db / 20) * np.sin(2 * np.pi * f1 * L * np.expm1(t / L))


def test_sweep_noise_floor_matches_the_noise_inside_the_record():
    # The floor from a separate noise-only record of the same length (windows at the harmonic
    # lags) must equal the noise the analysed record carries: read in h itself, in the silence
    # after the linear IR, where only noise lands.
    fs, L = 48000.0, 0.4
    s = _ess(fs, 20.0, 20000.0, L)
    pre, post = int(0.1 * fs), int(1.0 * fs)
    ref = np.concatenate([np.zeros(pre), s, np.zeros(post)])
    rng = np.random.default_rng(3)
    sigma = 1e-4
    meas = 0.5 * ref + sigma * rng.standard_normal(len(ref))
    noise = sigma * rng.standard_normal(len(ref))
    h = dsp.deconvolve(ref, meas)
    hn = dsp.deconvolve(ref, noise)
    freqs = np.array([300.0, 1000.0, 3000.0])
    r = dsp.sweep_harmonics(h, 0, fs, L, freqs, kmax=3, pre=0.02, post=0.06, noise_h=hn)
    # the same windows read in h's own silence, +0.3 … +0.9 s after the arrival
    start2 = -round(L * np.log(2) * fs) - int(0.02 * fs)
    h_quiet = np.roll(h, -(int(0.6 * fs) - start2))
    r_in = dsp.sweep_harmonics(h, 0, fs, L, freqs, kmax=3, pre=0.02, post=0.06, noise_h=h_quiet)
    assert np.all(np.abs(r["floor"][2] - r_in["floor"][2]) < 1.5), (r["floor"][2], r_in["floor"][2])
    r8 = dsp.sweep_harmonics(h, 0, fs, L, freqs, kmax=3, pre=0.02, post=0.06, noise_h=hn, repeats=8)
    assert np.allclose(r8["floor"][2], r["floor"][2] - 10 * np.log10(8))


def test_sweep_noise_floor_counts_the_reference_inputs_noise():
    # Unity path with equal noise on both inputs (a digital loop): dividing by the noisy
    # reference doubles the noise in h, so the floor reads 3 dB above the measurement noise.
    fs, L = 48000.0, 0.4
    s = _ess(fs, 20.0, 20000.0, L)
    pre, post = int(0.1 * fs), int(1.0 * fs)
    clean = np.concatenate([np.zeros(pre), s, np.zeros(post)])
    rng = np.random.default_rng(4)
    sigma = 1e-4
    ref = clean + sigma * rng.standard_normal(len(clean))
    meas = clean + sigma * rng.standard_normal(len(clean))
    n_ref, n_meas = (sigma * rng.standard_normal(len(clean)) for _ in range(2))
    h = dsp.deconvolve(ref, meas)
    freqs = np.array([300.0, 1000.0, 3000.0])
    kw = dict(kmax=3, pre=0.02, post=0.06)
    both = dsp.sweep_harmonics(h, 0, fs, L, freqs, noise_h=dsp.deconvolve_noise(ref, meas, n_ref, n_meas), **kw)
    meas_only = dsp.sweep_harmonics(h, 0, fs, L, freqs, noise_h=dsp.deconvolve(ref, n_meas), **kw)
    start2 = -round(L * np.log(2) * fs) - int(0.02 * fs)
    h_quiet = np.roll(h, -(int(0.6 * fs) - start2))
    inside = dsp.sweep_harmonics(h, 0, fs, L, freqs, noise_h=h_quiet, **kw)
    assert np.all(np.abs(both["floor"][2] - inside["floor"][2]) < 1.5), (both["floor"][2], inside["floor"][2])
    assert np.all(np.abs(both["floor"][2] - meas_only["floor"][2] - 3.0) < 1.0)


def test_gd_slope_err_and_sine_sigma():
    f = 1000.0 * 2 ** (np.arange(-40, 41) / 96)
    tau = 3e-6
    rng = np.random.default_rng(5)
    ph = -2 * np.pi * f * tau + 1e-4 * rng.standard_normal(len(f))
    g, se, sp = dsp.gd_slope_err(f, ph, np.array([1000.0]), 1 / 12)
    assert abs(g[0] - tau) < 4 * se[0]
    assert 0.5e-4 < sp[0] < 2e-4
    # a quadratic phase (group delay changing with f) is not noise
    g2, se2, sp2 = dsp.gd_slope_err(f, -2 * np.pi * f * tau + 1e-7 * (f - 1000.0) ** 2, np.array([1000.0]), 1 / 12)
    assert sp2[0] < 1e-9
    # floor −80 dBr at 1 kHz over ±1/48 oct: 1e-4 rad / (2π·28.9 Hz) ≈ 0.55 µs
    assert abs(dsp.sine_gd_sigma(1000.0, -80.0) - 1e-4 / (2 * np.pi * 1000 * (2 ** (1 / 48) - 2 ** (-1 / 48)))) < 1e-12


def test_mains_columns():
    c = np.array([49.0, 100.0, 148.0, 152.0, 180.0])
    assert dsp.mains_columns(c, [50.0, 150.0], 1 / 48, guard_hz=0.0).tolist() == [False] * 5
    assert dsp.mains_columns(c, [50.0, 150.0], 1 / 48, guard_hz=1.0).tolist() == [True, False, True, True, False]


def test_avoid_mains_keeps_the_gd_pair_off_the_lines():
    f = dsp.avoid_mains(50.0, 2.0, 50.0, pair=True)
    lobe = dsp.MAINLOBE_BINS / 2.0
    for s in (-1, 1):
        fp = f * 2 ** (s / 48)
        assert abs(fp - 50 * round(fp / 50)) > lobe
    # without the pair the fundamental's neighbourhood is not checked
    g = dsp.avoid_mains(50.0, 2.0, 50.0)
    assert min(abs(g * 2 ** (s / 48) - 50) for s in (-1, 1)) < lobe


def test_cross_spectrum_bands_keeps_a_delayed_path_whole():
    # a 3.6 ms delay turns the phase half a turn across a 1/48-octave band at 10 kHz: without
    # taking it out the band's complex sum cancels; with it the band reads the gain and delay
    fs, n = 96000.0, 2 ** 18
    rng = np.random.default_rng(3)
    ref = rng.standard_normal(n)
    d = 3.6e-3
    D = int(round(d * fs))
    meas = 0.5 * np.roll(ref, D)
    c = np.array([1000.0, 10000.0])
    H0, c0 = dsp.cross_spectrum_bands(meas, ref, fs, c)
    H1, c1 = dsp.cross_spectrum_bands(meas, ref, fs, c, delay_s=D / fs)
    assert np.all(np.abs(dsp.db(H1) - dsp.db(0.5)) < 0.05)
    want = np.angle(np.exp(-2j * np.pi * c * D / fs))
    assert np.all(np.abs(np.angle(H1 * np.exp(-1j * want))) < 0.02)
    # without it Σ M·R* over the 10 kHz band sums half a turn of phase: it cancels
    assert c1[1] > 0.99 and c0[1] < 0.5


def test_band_noise_rel_falls_with_snr_and_band_width():
    fs, n = 48000.0, 2 ** 17
    rng = np.random.default_rng(5)
    sig = rng.standard_normal(n)
    noise = 0.01 * rng.standard_normal(4 * n)
    meas = sig + 0.01 * rng.standard_normal(n)
    c = np.array([200.0, 2000.0])
    z = dsp.band_noise_rel(meas, noise, fs, c)
    # SNR 40 dB per bin, then √n bins of averaging: wider bands (higher c) read lower
    nb = c * (2 ** (1 / 96) - 2 ** (-1 / 96)) * n / fs
    want = 0.01 / np.sqrt(nb)
    assert np.all(np.abs(20 * np.log10(z / want)) < 1.5)
    assert z[1] < z[0]


def test_band_mean_reads_power_in_a_null():
    # two equal reflections cancelling at the band centre: the complex mean is near zero,
    # the power mean is the level a column reads (√2 / √2 = 1 for unit paths)
    f = np.linspace(990, 1010, 201)
    H = 1 + np.exp(-2j * np.pi * f * (1 / 2000) * 1.0)
    out = dsp.band_mean(f, H, np.array([1000.0]), frac=1 / 48)
    assert abs(abs(out[0]) - np.sqrt(np.mean(np.abs(H[(f >= 1000 * 2 ** (-1 / 96)) & (f < 1000 * 2 ** (1 / 96))]) ** 2))) < 1e-9


def test_harmonic_phasors_are_fitted_with_the_fundamental_so_it_does_not_leak():
    fs, f = 96000.0, 26.263
    t = np.arange(int(2.0 * fs)) / fs
    x = 0.3 * np.cos(2 * np.pi * f * t) + 0.3e-5 * np.cos(2 * np.pi * 3 * f * t + 1.0)
    h = dsp.sine_harmonic_phasors(x, f, fs)
    assert abs(20 * np.log10(abs(h[3] / h[1])) - -100.0) < 0.01
    assert abs(np.angle(h[3] / h[1]) - 1.0) < 1e-3
    assert abs(h[2] / h[1]) < 1e-9
