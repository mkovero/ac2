"""The calibration store's convention check: the stored sensitivity against the mic's
data-sheet sensitivity and the input's full-scale voltage, with the loading divider."""
import json
import math

import pytest

from crosscheck.stages import cal_excerpt


def _store(tmp_path, S, loading):
    method = {"type": "electrical", "full_scale": 0.2717618390666213, "mic_sensitivity": 15.0}
    if loading:
        method["loading"] = {"mic_impedance": 330.0, "input_impedance": 2000.0}
    store = {"format": "ac2-calibrations",
             "inputs": [{"channel": 0, "mic": "m"}],
             "sensitivities": [{"key": {"channel": 0, "mic": "m"},
                                "spl": {"sensitivity": S, "method": method}}],
             "mics": []}
    p = tmp_path / "cal.json"
    p.write_text(json.dumps(store))
    return {"ac2": {"cal_store": str(p), "mic_input": 1}}


def test_loaded_mic_passes_the_convention_check(tmp_path):
    # pupu's input 1 after the loaded electrical cal: 0 dBFS = 120.47 dB SPL.
    ex = cal_excerpt(_store(tmp_path, 120.46786286943512, loading=True))
    assert abs(ex["convention_check_db"]) < 0.05


def test_loading_left_out_is_refused(tmp_path):
    # The same store without its loading: the divider's −1.33 dB is unexplained.
    assert 20 * math.log10(2000 / 2330) == pytest.approx(-1.327, abs=1e-3)
    with pytest.raises(RuntimeError, match="convention"):
        cal_excerpt(_store(tmp_path, 120.46786286943512, loading=False))
