"""The Xone steady-sine tones keep their harmonics clear of the mains lines a sweep would read
as distortion; a tone that loses them turns its harmonic comparisons INCONCLUSIVE."""
import tomllib
from pathlib import Path

from crosscheck import dsp
from crosscheck.analyse import MAINS_GUARD_HZ

RIG = tomllib.loads((Path(__file__).resolve().parent.parent / "rigs" / "pupu.toml").read_text())
SC = RIG["stages"]["sine"]
MAINS = float(RIG["rig"]["mains_hz"])
# pupu's measurement input shows mains lines at every multiple up to 2 kHz
LINES = [MAINS * n for n in range(1, 41)]


def _clear(f: float) -> list[int]:
    return [k for k in range(2, 6)
            if all(abs(k * f - h) > dsp.sweep_harmonic_half_band(k * f, MAINS_GUARD_HZ) for h in LINES)]


def _played(f0: float) -> float:
    return dsp.avoid_mains(f0, max(float(SC["probe_seconds"]), SC["min_periods"] / f0), MAINS,
                           pair=SC.get("gd_pairs", False))


def test_each_xone_tone_keeps_two_harmonics_clear_of_the_sweep_mains_bands():
    for f0 in SC["xone_hz"]:
        assert len(_clear(_played(float(f0)))) >= 2, f0


def test_every_harmonic_below_100_hz_is_judged_by_some_xone_tone():
    low = [_played(float(f)) for f in SC["xone_hz"] if f < 100]
    assert set().union(*(_clear(f) for f in low)) == {2, 3, 4, 5}
