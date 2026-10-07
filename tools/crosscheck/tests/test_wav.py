import numpy as np

from crosscheck import wav


def test_write_takes_jack_float_rate(tmp_path):
    x = np.linspace(-0.5, 0.5, 96, dtype=np.float64)
    wav.write(tmp_path / "x.wav", 96000.0, x)
    fs, y = wav.read(tmp_path / "x.wav")
    assert fs == 96000
    assert np.allclose(y.ravel(), x, atol=1e-7)
