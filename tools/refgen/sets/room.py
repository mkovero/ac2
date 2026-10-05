"""Golden vectors for ISO 3382-1 room parameters: an impulse response through the IEC
61260-1 octave-band filters run backwards in time, its Schroeder curve, the decay times
fitted to it, and C50 / C80 by window-before-filtering (docs/design/room-metrics.md).

Noise truncation (Lundeby) is not part of this set: the curve is truncated at a fixed point
without correction, so the comparison covers the filter, the integral and the fits; the
truncation is tested against analytic rooms in crates/ac2-core/src/room/tests.rs.
"""

import pathlib
import sys

import numpy as np
import scipy.signal as sig

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import generate as g  # noqa: E402

G10 = 10.0 ** 0.3  # IEC 61260-1 base-10 octave ratio


def octave_centres(f_lo: float, f_hi: float) -> np.ndarray:
    x = np.arange(-20, 20)
    f = 1000.0 * G10 ** x
    return f[(f >= f_lo) & (f <= f_hi)]


def onset(e: np.ndarray, trigger_db: float = -20.0) -> int:
    """First index whose energy is within trigger_db of the peak (ISO 3382-1)."""
    return int(np.argmax(e >= np.max(e) * 10.0 ** (trigger_db / 10.0)))


def schroeder_db(e: np.ndarray) -> np.ndarray:
    edc = np.cumsum(e[::-1])[::-1]
    return 10.0 * np.log10(edc / edc[0])


def decay_time(edc_db: np.ndarray, fs: float, top: float, bottom: float) -> float:
    i0 = int(np.argmax(edc_db <= top))
    i1 = int(np.argmax(edc_db <= bottom))
    assert edc_db[i1] <= bottom and i1 > i0 + 1
    t = np.arange(i0, i1 + 1) / fs
    slope, _ = np.polyfit(t, edc_db[i0:i1 + 1], 1)
    return -60.0 / slope


def gen_room_schroeder_octaves() -> g.VectorSet:
    fs = 48000.0
    n = 48000
    end = 43200  # truncation index: 0.9 s
    pre = 960  # 20 ms of background before the direct sound
    seed = 0x0AC2_3382
    rng = np.random.default_rng(seed)
    t = np.maximum(np.arange(n) - pre, 0) / fs
    live = (np.arange(n) >= pre).astype(float)
    # Two decays: a low-passed one of 1.0 s and a broadband one of 0.4 s, so the bands
    # differ; a direct sound; background noise 90 dB down.
    low = sig.sosfilt(sig.butter(2, 500.0, fs=fs, output="sos"),
                      rng.standard_normal(n)) * 10.0 ** (-3.0 * t / 1.0) * live
    broad = rng.standard_normal(n) * 10.0 ** (-3.0 * t / 0.4) * live * 0.5
    ir = low + broad + 10.0 ** (-90.0 / 20.0) * rng.standard_normal(n)
    ir[pre] += 8.0

    start = onset(ir ** 2)
    centres = octave_centres(63.0, 8000.0)
    edc_points, edt, t20, t30, c50, c80 = [], [], [], [], [], []
    for fm in centres:
        lo, hi = fm / G10 ** 0.5, fm * G10 ** 0.5
        sos = sig.butter(3, [lo, hi], btype="bandpass", fs=fs, output="sos")
        y = sig.sosfilt(sos, ir[:end][::-1])[::-1]
        e = y ** 2
        edc = schroeder_db(e[onset(e):])
        edc_points.append(edc[::48])
        edt.append(decay_time(edc, fs, 0.0, -10.0))
        t20.append(decay_time(edc, fs, -5.0, -25.0))
        t30.append(decay_time(edc, fs, -5.0, -35.0))
        pad = int(np.ceil(20.0 / (hi - lo) * fs))

        def energy(x):
            return float(np.sum(sig.sosfilt(sos, np.concatenate([x, np.zeros(pad)])) ** 2))

        for ms, out in ((50.0, c50), (80.0, c80)):
            k = min(start + int(round(ms * 1e-3 * fs)), end)
            out.append(10.0 * np.log10(energy(ir[start:k]) / energy(ir[k:end])))

    vs = g.VectorSet(
        name="room_schroeder_octaves",
        description=(
            "An impulse response (two exponential decays, a direct sound, background noise) "
            "through IEC 61260-1 octave-band Butterworth filters (order 3) run backwards in "
            "time; Schroeder integral from each band's -20 dB trigger to a fixed truncation "
            "point without correction; EDT/T20/T30 by numpy.polyfit over 0..-10, -5..-25, "
            "-5..-35 dB; C50/C80 by window-before-filtering from the broadband trigger."
        ),
        function="gen_room_schroeder_octaves",
        parameters={
            "fs": fs,
            "n": n,
            "seed": seed,
            "truncation_index": end,
            "band_filter": ("scipy.signal.butter(3, [fm/G^0.5, fm*G^0.5], btype='bandpass', "
                            "fs=fs, output='sos'), sosfilt on the reversed IR[:end], reversed"),
            "onset": "first index with energy >= max * 10^(-20/10)",
            "ratios": ("energy of sosfilt(ir[start:k] + pad zeros) / energy of "
                       "sosfilt(ir[k:end] + pad zeros), pad = ceil(20/B*fs)"),
            "edc_sampling": "every 48th sample (1 ms) of each band, bands concatenated",
        },
        references=[
            "ISO 3382-1:2009 (decay ranges, onset trigger, C50/C80/D50)",
            "M. R. Schroeder, J. Acoust. Soc. Am. 37, 409-412 (1965)",
            "F. Jacobsen, J. H. Rindel, J. Sound Vib. 117(1), 187-190 (1987)",
            "scipy.signal.butter / sosfilt, numpy.polyfit",
        ],
        scalars={
            "fs": fs,
            "truncation_index": float(end),
            "broadband_onset_index": float(start),
        },
    )
    vs.add("ir", ir, unit="FS", description="impulse response (input)")
    vs.add("centres_hz", centres, unit="Hz", description="octave mid-band frequencies")
    vs.add("edc_db_every_ms", np.concatenate(edc_points), unit="dB",
           description="Schroeder curve of each band from its onset, every 1 ms",
           tolerance=g.lin_tol(1e-6, 0.0))
    for name, v, unit in (("edt_s", edt, "s"), ("t20_s", t20, "s"), ("t30_s", t30, "s")):
        vs.add(name, np.array(v), unit=unit, description=f"{name} per band",
               tolerance=g.lin_tol(0.0, 1e-7), axis="centres_hz")
    for name, v in (("c50_db", c50), ("c80_db", c80)):
        vs.add(name, np.array(v), unit="dB", description=f"{name} per band",
               tolerance=g.lin_tol(1e-6, 0.0), axis="centres_hz")
    return vs


GENERATORS = [gen_room_schroeder_octaves]
