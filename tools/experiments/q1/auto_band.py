"""Auto band (§9.1): full -> mid -> sub, first band that is not NoEstimate.

    python3 auto_band.py

Excitations: pink (all bands excited), bandlimited 100 Hz-8 kHz, 'narrow' (to 1.2 kHz) and
'subonly' (25-150 Hz feed). Prints the band chosen and the reasons the higher bands refused.
"""

import os

os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")

import mc  # noqa: E402,F401  (sys.path)
import delay_finder as df  # noqa: E402
import finder as F  # noqa: E402

P = df.Path
for exc in ("pink", "bandlimited", "narrow", "subonly"):
    for seed in (1, 2, 3):
        sc = df.Scenario("a", "auto", "sub", [P(-1234.4, -3.0), P(-1234.4 + 2400, 0.0)],
                         excitation=exc, snr_db=30.0, lm=192000, search=(-9600, 9600), seed=seed)
        ref, rs, m, ms = df.synth(sc)
        trail = []
        chosen = None
        for band in ("full", "mid", "sub"):
            r = F.find_delay(ref, rs, m, ms, 48000.0, band, sc.search)
            if r.status != "no_estimate":
                chosen = (band, r.status, round(r.delay, 2))
                break
            trail.append(f"{band}:{','.join(r.reasons)}")
        print(f"{exc:12s} seed {seed}: chosen {chosen}  refused {trail}")
