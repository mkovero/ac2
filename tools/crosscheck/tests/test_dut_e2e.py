"""Opt-in (CROSSCHECK_JACK_E2E=1, and AC2_JACK_DUT pointing at the binary): the sine stage
on the dut path against the real ac2-jack-dut on a JACK dummy server, never a sound card.
No ac2 daemon: the sine stage does not use it."""
import copy
import json
import os
import shutil
import subprocess
import time
import tomllib
from pathlib import Path

import pytest

ON = os.environ.get("CROSSCHECK_JACK_E2E") == "1" and os.environ.get("AC2_JACK_DUT")
pytestmark = pytest.mark.skipif(not ON, reason="opt-in: CROSSCHECK_JACK_E2E=1 AC2_JACK_DUT=<binary>")

ROOT = Path(__file__).resolve().parent.parent


@pytest.fixture(scope="module")
def dummy():
    jack = pytest.importorskip("jack")
    if not shutil.which("jackd"):
        pytest.skip("jackd not installed")
    name = f"xc-dut-{os.getpid()}"
    old = {k: os.environ.get(k) for k in ("JACK_DEFAULT_SERVER", "JACK_NO_START_SERVER")}
    os.environ["JACK_DEFAULT_SERVER"] = name
    os.environ["JACK_NO_START_SERVER"] = "1"
    p = subprocess.Popen(["jackd", "-n", name, "-d", "dummy", "-r", "96000", "-p", "256", "-C", "8", "-P", "8"],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    t0 = time.monotonic()
    while True:
        try:
            jack.Client("probe", no_start_server=True, servername=name).close()
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


def test_sine_stage_reads_the_dut_s_truth(dummy, tmp_path):
    from crosscheck import dut, dutrun, stages
    from crosscheck.analyse import analyse
    from crosscheck.run import build_policy, path_policy

    rig = copy.deepcopy(tomllib.loads((ROOT / "rigs" / "pupu.toml").read_text()))
    rig["rig"]["max_xruns"] = 100000  # a dummy server without RT scheduling xruns
    rig["stages"]["sine"].update(dut_hz=[40.5, 1040], budget_seconds={"dut": 20.0}, noise_seconds=4.0)
    pol = path_policy(rig, build_policy(rig, "-10dbfs", None, "-10dbfs"), "dut")
    man = {"rig": "pupu", "flags": {"emit": "-10dbfs"}, "mains_hz": 50.0,
           "paths": {"dut": {"kind": "digital", "mains_hz": None}}}
    ctx = stages.Ctx(rig=rig, policy=pol, out=tmp_path, ac2=None, rew=None, manifest=man, fs=96000.0)
    ctx.dut = dutrun.start(ctx, "dut")
    try:
        stages.sine_stage(ctx, "dut")
    finally:
        r = dutrun.finish(ctx, "dut", ctx.dut)
    assert r["xruns_line"] is not None and r["rc"] == 0
    ctx.save()
    res, _ = analyse(tmp_path)
    rows = [c for c in res["checks"] if c["id"].startswith("dut.dut.h") and "steady sine" in c["id"]]
    assert len(rows) == 8
    # each xrun leaves a stale block in dut_out: judge strictly only on a clean run
    lim = 0.1 if r["xruns"] == 0 else 1.0
    bad = [(c["title"], c["value"]) for c in rows if c["value"] is None or abs(c["value"]) > lim]
    assert not bad, (r, bad)
    rec = json.loads((tmp_path / "dut" / "dut" / "dut.json").read_text())
    assert rec["command"][0] == os.path.expanduser(os.environ["AC2_JACK_DUT"])
    assert rec["coefficients"] == dut.coefficients(rig["dut"], 96000.0)


def test_patch_on_a_real_server(dummy, tmp_path):
    """The patch against real JACK connections: a stand-in `ac2` client with the ports ac2d
    registers, wired as a session open leaves them."""
    import jack

    from crosscheck import dut, dutrun

    rig = tomllib.loads((ROOT / "rigs" / "pupu.toml").read_text())
    coef = dut.coefficients(rig["dut"], 96000.0)
    proc = dutrun.DutProcess(dut.command(rig["dut"], coef, os.path.expanduser(os.environ["AC2_JACK_DUT"])),
                             "ac2-dut", tmp_path)
    fake = jack.Client("ac2", no_start_server=True)
    try:
        proc.wait_ready(96000.0)
        for i in range(1, 4):
            fake.inports.register(f"in_{i}")
        for i in range(1, 6):
            fake.outports.register(f"out_{i}")
        fake.activate()
        fake.connect("system:capture_2", "ac2:in_2")
        fake.connect("system:capture_3", "ac2:in_3")
        fake.connect("ac2:out_5", "system:playback_5")
        before = {p: sorted(x.name for x in fake.get_all_connections(p)) for p in ("ac2:in_2", "ac2:in_3",
                                                                                    "ac2:out_5", "ac2:out_2")}
        p = dutrun.Patch("ac2", "ac2-dut", rig["paths"]["dut"])
        p.apply()
        assert [x.name for x in fake.get_all_connections("ac2:in_3")] == ["ac2-dut:dut_out"]
        assert sorted(x.name for x in fake.get_all_connections("ac2:out_5")) == ["ac2-dut:dut_in", "system:playback_5"]
        p.verify_sources()
        assert p.restore() == []
        after = {k: sorted(x.name for x in fake.get_all_connections(k)) for k in before}
        assert after == before
    finally:
        fake.close()
        proc.stop()
