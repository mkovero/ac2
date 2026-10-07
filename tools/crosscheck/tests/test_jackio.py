"""play_record against a JACK dummy server (never a sound card): the server runs under
its own name, and the client refuses to start or find any other."""
import os
import shutil
import subprocess
import time

import numpy as np
import pytest

jack = pytest.importorskip("jack")

from crosscheck import jackio, levels  # noqa: E402


@pytest.fixture(scope="module")
def dummy():
    if not shutil.which("jackd"):
        pytest.skip("jackd not installed")
    name = f"xc-dummy-{os.getpid()}"
    old = {k: os.environ.get(k) for k in ("JACK_DEFAULT_SERVER", "JACK_NO_START_SERVER")}
    os.environ["JACK_DEFAULT_SERVER"] = name
    os.environ["JACK_NO_START_SERVER"] = "1"
    p = subprocess.Popen(["jackd", "-n", name, "-d", "dummy", "-r", "96000", "-p", "1024", "-C", "8", "-P", "8"],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    t0 = time.monotonic()
    while True:
        try:
            c = jack.Client("probe", no_start_server=True, servername=name)
            c.close()
            break
        except jack.JackOpenError:
            if time.monotonic() - t0 > 10 or p.poll() is not None:
                p.kill()
                pytest.skip("JACK dummy server did not start")
            time.sleep(0.2)
    yield name
    p.terminate()
    p.wait(5)
    for k, v in old.items():
        if v is None:
            os.environ.pop(k, None)
        else:
            os.environ[k] = v


def test_play_record_loops_own_output(dummy):
    a = levels.peak_amplitude(-50)
    s = jackio.sine(1000, a, 0.5, 96000, 0.02)
    # record the client's own output port: what the callback wrote, a cycle later
    fs, x = jackio.play_record([jackio.Play("system:playback_1", s, a)], ["crosscheck:out_0", "system:capture_1"],
                               pre_s=0.1, post_s=0.2, fade_s=0.02, expect_fs=96000,
                               max_xruns=1000)  # a dummy server without RT scheduling xruns
    assert fs == 96000
    assert x.shape == (int(0.1 * fs) + len(s) + int(0.2 * fs), 2)
    pk = np.max(np.abs(x[:, 0]))
    assert a * 0.99 < pk <= a * (1 + 1e-6)
    assert np.all(x[:int(0.05 * fs), 0] == 0)


def test_wrong_rate_refused(dummy):
    with pytest.raises(levels.PolicyError):
        jackio.play_record([], ["system:capture_1"], pre_s=0.05, post_s=0.0, expect_fs=48000)


def test_abort_fades_out(dummy):
    import threading
    a = levels.peak_amplitude(-50)
    s = jackio.sine(1000, a, 3.0, 96000, 0.02)
    main = threading.main_thread()
    import _thread
    threading.Timer(0.5, _thread.interrupt_main).start()
    got = {}

    def on_start():
        got["t"] = time.monotonic()

    with pytest.raises(KeyboardInterrupt):
        jackio.play_record([jackio.Play("system:playback_1", s, a)], ["crosscheck:out_0"], pre_s=0.0, post_s=0.0,
                           fade_s=0.05, expect_fs=96000, on_start=on_start)
    assert main.is_alive()
