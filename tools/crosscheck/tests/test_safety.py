"""Every refusal of the emission policy, without a sound card."""
import tomllib
from dataclasses import replace
from pathlib import Path

import numpy as np
import pytest

from crosscheck import jackio, levels
from crosscheck.levels import PolicyError
from crosscheck.run import build_policy, path_policy

RIG = tomllib.loads((Path(__file__).resolve().parent.parent / "rigs" / "pupu.toml").read_text())


def pol(emit=None, speaker=None, allow=None, path="xone"):
    return path_policy(RIG, build_policy(RIG, emit, speaker, allow), path)


def test_levels_need_their_unit():
    for bad in ("-50", "-50 db", "50dbfs", "+3dbfs", "loud"):
        with pytest.raises(PolicyError):
            levels.parse_dbfs(bad)
    assert levels.parse_dbfs("-50dbfs") == -50.0
    assert levels.parse_dbfs(" -30.5 dBFS ") == -30.5


def test_full_scale_sine_convention():
    assert levels.peak_amplitude(0.0) == 1.0
    assert abs(levels.dbfs_of_rms(levels.rms_of_sine(-50.0)) + 50.0) < 1e-12


def test_nothing_without_emit():
    with pytest.raises(PolicyError, match="--emit"):
        pol().check([3, 2], -50, speaker_stage=False)
    with pytest.raises(PolicyError, match="--emit-speaker"):
        pol(emit="-50dbfs", path="genelec").check([1, 2], -50, speaker_stage=True)


def test_xone_never_touches_out_1():
    p = pol(emit="-50dbfs", speaker="-50dbfs")
    with pytest.raises(PolicyError):
        p.check([1, 2], -50, speaker_stage=False)
    with pytest.raises(PolicyError):
        p.check([3, 1], -60, speaker_stage=False)
    assert p.check([3, 2], -50, speaker_stage=False) == -50


def test_unknown_output_refused():
    with pytest.raises(PolicyError, match="not described"):
        pol(emit="-50dbfs").check([4, 2], -60, speaker_stage=False)


def test_electrical_above_rig_bound_needs_allowance():
    with pytest.raises(PolicyError, match="allow-electrical-level"):
        pol(emit="-30dbfs").check([3, 2], -30, speaker_stage=False)
    assert pol(emit="-30dbfs", allow="-30dbfs").check([3, 2], -30, speaker_stage=False) == -30
    with pytest.raises(PolicyError, match="electrical cap"):
        pol(emit="-8dbfs", allow="-8dbfs").check([3, 2], -8, speaker_stage=False)  # above pupu.toml's -10
    with pytest.raises(PolicyError, match="above --emit"):
        pol(emit="-40dbfs", allow="-30dbfs").check([3, 2], -35, speaker_stage=False)


def test_speaker_hard_ceiling_whatever_the_config():
    loose = replace(pol(emit="-30dbfs", speaker="-40dbfs", allow="-30dbfs", path="genelec"), speaker_max_dbfs=-10.0)
    with pytest.raises(PolicyError, match="speaker ceiling"):
        loose.check([1, 2], -40, speaker_stage=True)
    rig = {**RIG, "rig": {**RIG["rig"], "speaker_max_dbfs": -10.0}}
    assert build_policy(rig, None, "-50dbfs").speaker_max_dbfs == -50.0


def test_electrical_allowance_never_reaches_the_speaker():
    p = pol(emit="-30dbfs", speaker="-50dbfs", allow="-30dbfs", path="genelec")
    with pytest.raises(PolicyError):
        p.check([1, 2], -30, speaker_stage=True)
    with pytest.raises(PolicyError, match="speaker"):
        p.check([1, 2], -30, speaker_stage=False)
    assert p.check([1, 2], -50, speaker_stage=True) == -50


def test_speaker_stage_drives_one_speaker_beside_electrical_outputs():
    p = pol(speaker="-50dbfs", path="genelec")
    with pytest.raises(PolicyError, match="exactly one"):
        p.check([2, 3], -50, speaker_stage=True)


def test_stimulus_peak():
    with pytest.raises(PolicyError):
        levels.check_stimulus_peak(-45.0, -40.0, speaker=True)
    with pytest.raises(PolicyError):
        levels.check_stimulus_peak(-49.0, -50.0, speaker=False)
    levels.check_stimulus_peak(-50.0, -50.0, speaker=True)


def test_signal_above_its_ceiling_is_refused_before_jack():
    a = levels.peak_amplitude(-50)
    s = jackio.sine(1000, a * 1.01, 0.5, 96000, 0.05)
    with pytest.raises(PolicyError):
        jackio.check_plays([jackio.Play("system:playback_3", s, a)])
    jackio.check_plays([jackio.Play("system:playback_3", jackio.sine(1000, a, 0.5, 96000, 0.05), a)])


def test_fades_start_and_end_at_zero():
    s = jackio.sine(50, 1.0, 1.0, 48000, 0.1)
    assert abs(s[0]) < 1e-12 and abs(s[-1]) < 1e-3
    assert np.max(np.abs(s[:100])) < 0.01


def test_electrical_hard_backstop_whatever_the_config():
    loose = replace(pol(emit="-3dbfs", allow="-3dbfs"), electrical_max_dbfs=0.0)
    with pytest.raises(PolicyError, match="electrical cap"):
        loose.check([3, 2], -3, speaker_stage=False)
    ok = replace(pol(emit="-10dbfs", allow="-10dbfs"), electrical_max_dbfs=-10.0)
    assert ok.check([3, 2], -10, speaker_stage=False) == -10
