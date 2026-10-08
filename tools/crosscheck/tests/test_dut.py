"""The digital DUT: filter designs, the analytic truth against a simulation of the binary's
DSP chain, its command line, port resolution, the ac2 patch and the process lifecycle."""
import json
import math
import sys
import textwrap
import tomllib
from pathlib import Path
from types import SimpleNamespace

import numpy as np
import pytest

from crosscheck import dsp, dut, dutrun, levels

ROOT = Path(__file__).resolve().parent.parent
RIG = tomllib.loads((ROOT / "rigs" / "pupu.toml").read_text())
FS = 96000.0


@pytest.mark.parametrize("kind,fc", [("hp1", 20.0), ("lp1", 1000.0), ("hp2", 50.0), ("lp2", 30000.0)])
def test_corner_gain_is_minus_3_01_db(kind, fc):
    bq = dut.design({"type": kind, "hz": fc, "q": 1 / math.sqrt(2)}, FS)
    g = 20 * np.log10(abs(dut.response([bq], [fc], FS)[0]))
    assert abs(g - 20 * np.log10(1 / math.sqrt(2))) < 1e-9
    # and the pass band is unity
    far = fc / 100 if kind.startswith("lp") else min(fc * 100, FS / 2.2)
    assert abs(abs(dut.response([bq], [far], FS)[0]) - 1) < 1e-3


def test_design_refuses_nonsense():
    with pytest.raises(ValueError):
        dut.design({"type": "bp2", "hz": 100.0}, FS)
    with pytest.raises(ValueError):
        dut.design({"type": "lp2", "hz": 50000.0}, FS)


def test_response_matches_the_df2t_loop():
    chain = dut.design_chain([{"type": "hp1", "hz": 20.0}, {"type": "lp2", "hz": 3000.0, "q": 0.9}], FS)
    n = 1 << 17  # the 20 Hz pole needs a long tail
    imp = np.zeros(n)
    imp[0] = 1.0
    h = dut.biquad(chain, imp)
    f = np.fft.rfftfreq(n, 1 / FS)[1:200]
    H = np.fft.rfft(h)[1:200]
    assert np.max(np.abs(H - dut.response(chain, f, FS))) < 1e-6


def test_poly_harmonics_closed_form():
    a = 0.3
    # cos^5 = (10 cos + 5 cos 3θ + cos 5θ) / 16
    h = dut.poly_harmonics([0, 1, 0, 0, 0, 1], a)
    assert abs(h[1] - (a + 10 * a ** 5 / 16)) < 1e-12
    assert abs(h[3] - 5 * a ** 5 / 16) < 1e-12 and abs(h[5] - a ** 5 / 16) < 1e-12
    assert abs(h[2]) < 1e-12 and abs(h[4]) < 1e-12


def _rig_coef():
    return dut.coefficients(RIG["dut"], FS)


@pytest.mark.parametrize("f", [40.5, 5000.0])
def test_analytic_matches_a_simulation_of_the_chain(f):
    """Same chain as the binary (DF2T per sample, polynomial), harmonics read with the suite's
    own steady-sine analysis: within 0.01 dB of the truth."""
    coef = _rig_coef()
    amp = levels.peak_amplitude(-10.0)
    settle, steady = 0.5, max(2.0, 80 / f)
    n = int((settle + steady) * FS)
    x = amp * np.cos(2 * np.pi * f * np.arange(n) / FS)
    y = dut.biquad(coef["post"], dut.poly(coef["poly"], dut.biquad(coef["pre"], x)))
    h = dsp.sine_harmonics(y[int(settle * FS):], f, FS)
    truth = dut.analytic(f, amp, coef, FS)
    assert set(truth) == {2, 3, 4, 5}
    for k, v in truth.items():
        assert abs(h["h_dbr"][k] - v) < 0.01, (k, h["h_dbr"][k], v)


def test_default_poly_levels_and_headroom():
    coef = _rig_coef()
    amp = levels.peak_amplitude(-10.0)
    t = dut.analytic(1040.0, amp, coef, FS)
    for k, want in {2: -50.0, 3: -55.1, 4: -63.0, 5: -70.1}.items():
        assert abs(t[k] - want) < 0.1, (k, t[k])
    assert dut.peak_out(amp, coef, FS) < 0.5
    # the pre-filter lowers the polynomial's input at LF: every harmonic falls
    lf = dut.analytic(15.0, amp, coef, FS)
    assert all(lf[k] < t[k] - 5 for k in t)
    # H5 of a 10 kHz tone is beyond fs/2: left out, not aliased into the truth
    assert 5 not in dut.analytic(10000.0, amp, coef, FS)


def test_every_dut_tone_has_h5_below_40_khz():
    for f in RIG["stages"]["sine"]["dut_hz"]:
        assert 5 * float(f) < 40000.0


def test_command_round_trips_the_coefficients():
    coef = _rig_coef()
    cmd = dut.command(RIG["dut"], coef, "/bin/ac2-jack-dut")
    assert cmd[1] == "--name=ac2-dut"
    opts = {}
    for a in cmd[1:]:
        k, _, v = a.partition("=")
        opts.setdefault(k, []).append(v)
    assert [float(x) for x in opts["--poly"][0].split(",")] == coef["poly"]
    assert [[float(x) for x in v.split(",")] for v in opts["--pre"]] == coef["pre"]
    assert [[float(x) for x in v.split(",")] for v in opts["--post"]] == coef["post"]
    assert float(opts["--noise-dbfs"][0]) == -120.0


# ---------------------------------------------------------------- ports


def _ctx(**kw):
    from crosscheck.stages import Ctx
    return Ctx(rig=RIG, policy=None, out=Path("."), ac2=None, rew=None, manifest={}, **kw)


def test_path_ports_resolve_per_path():
    c = _ctx()
    assert c.path_ports("xone") == {"out": "system:playback_5", "ref_out": "system:playback_2",
                                    "meas_in": "system:capture_5", "ref_in": "system:capture_2"}
    assert c.path_ports("genelec")["out"] == "system:playback_1"
    assert c.path_ports("dut") == {"out": "ac2-dut:dut_in", "ref_out": "ac2-dut:ref_in",
                                   "meas_in": "ac2-dut:dut_out", "ref_in": "ac2-dut:ref_out"}
    assert c.path_mains_hz("dut") is None and c.path_mains_hz("xone") == 50.0


def test_dut_path_policy_checks_ac2_s_hardware_outputs():
    from crosscheck.levels import PolicyError
    from crosscheck.run import build_policy, path_policy
    pol = path_policy(RIG, build_policy(RIG, "-10dbfs", None, "-10dbfs"), "dut")
    pc = RIG["paths"]["dut"]
    assert pol.check([int(pc["out"]), int(pc["ref_out"])], -10.0, speaker_stage=False) == -10.0
    with pytest.raises(PolicyError):
        pol.check([1, 2], -50.0, speaker_stage=False)


# ---------------------------------------------------------------- patch


class FakeJack:
    """Connections as a set of (source, destination); ports by name."""

    def __init__(self, ports, edges):
        self.ports, self.edges, self.closed = set(ports), set(edges), 0

    def get_ports(self, pattern=""):
        return [SimpleNamespace(name=p) for p in sorted(self.ports) if p.startswith(pattern)]

    def get_all_connections(self, port):
        return [SimpleNamespace(name=a if b == port else b) for a, b in self.edges if port in (a, b)]

    def connect(self, a, b):
        if (a, b) in self.edges:
            raise RuntimeError("already connected")
        self.edges.add((a, b))

    def disconnect(self, a, b):
        self.edges.remove((a, b))

    def close(self):
        self.closed += 1


AC2 = [f"ac2:in_{i}" for i in range(1, 9)] + [f"ac2:out_{i}" for i in range(1, 7)]
DUTP = ["ac2-dut:dut_in", "ac2-dut:ref_in", "ac2-dut:dut_out", "ac2-dut:ref_out"]
SYS = [f"system:capture_{i}" for i in range(1, 9)] + [f"system:playback_{i}" for i in range(1, 9)]
NORMAL = {(f"system:capture_{i}", f"ac2:in_{i}") for i in range(1, 9)} | {
    ("ac2:out_5", "system:playback_5"), ("ac2:out_2", "system:playback_2")}


def _patch(j):
    return dutrun.Patch("ac2", "ac2-dut", RIG["paths"]["dut"], open_client=lambda *a: j)


def test_patch_and_restore_exactly():
    j = FakeJack(AC2 + DUTP + SYS, NORMAL | {("pw_rew:x", "ac2:in_2")})
    p = _patch(j)
    p.apply()
    assert {x.name for x in j.get_all_connections("ac2:in_3")} == {"ac2-dut:dut_out"}
    assert {x.name for x in j.get_all_connections("ac2:in_2")} == {"ac2-dut:ref_out"}
    assert ("ac2:out_5", "ac2-dut:dut_in") in j.edges and ("ac2:out_2", "ac2-dut:ref_in") in j.edges
    assert ("ac2:out_5", "system:playback_5") in j.edges  # ac2's hardware outputs are left alone
    assert p.saved == {"ac2:in_3": ["system:capture_3"], "ac2:in_2": ["pw_rew:x", "system:capture_2"]}
    p.verify_sources()
    # a hardware capture summed back in (ac2 reopening its session) fails the check
    j.connect("system:capture_3", "ac2:in_3")
    with pytest.raises(RuntimeError, match="ac2:in_3"):
        p.verify_sources()
    j.disconnect("system:capture_3", "ac2:in_3")
    assert p.restore() == []
    assert j.edges == NORMAL | {("pw_rew:x", "ac2:in_2")}


def test_patch_refuses_missing_ports_and_fed_dut_inputs():
    j = FakeJack(AC2 + SYS, NORMAL)
    with pytest.raises(RuntimeError, match="missing"):
        _patch(j).apply()
    assert j.edges == NORMAL
    j = FakeJack(AC2 + DUTP + SYS, NORMAL | {("crosscheck:out_0", "ac2-dut:dut_in")})
    with pytest.raises(RuntimeError, match="already fed"):
        _patch(j).apply()


def test_restore_reports_what_it_could_not_put_back():
    j = FakeJack(AC2 + DUTP + SYS, NORMAL)
    p = _patch(j)
    p.apply()
    j.ports.discard("system:capture_3")

    def bad_connect(a, b):
        if a == "system:capture_3":
            raise RuntimeError("no such port")
        j.edges.add((a, b))
    j.connect = bad_connect
    errs = p.restore()
    assert any("system:capture_3" in e for e in errs)


# ---------------------------------------------------------------- process


def test_dut_process_ready_check_and_xruns(tmp_path):
    fake = tmp_path / "fake-dut"
    fake.write_text(textwrap.dedent(f"""\
        #!{sys.executable}
        import sys
        print("ready ac2-dut 96000 256", flush=True)
        sys.stdin.read()
        print("xruns 2", flush=True)
        """))
    fake.chmod(0o755)
    j = FakeJack(DUTP, set())
    p = dutrun.DutProcess([str(fake)], "ac2-dut", tmp_path, open_client=lambda *a: j)
    assert p.wait_ready(96000.0) == {"name": "ac2-dut", "fs": 96000.0, "buffer": 256}
    r = p.stop()
    assert r == {"rc": 0, "stopped_by": "stdin closed", "xruns_line": "xruns 2", "xruns": 2}


def test_dut_process_refuses_a_wrong_rate_and_fed_inputs(tmp_path):
    fake = tmp_path / "fake-dut"
    fake.write_text(f"#!{sys.executable}\nimport sys\nprint('ready ac2-dut 48000 256', flush=True)\nsys.stdin.read()\n")
    fake.chmod(0o755)
    p = dutrun.DutProcess([str(fake)], "ac2-dut", tmp_path, open_client=lambda *a: FakeJack(DUTP, set()))
    with pytest.raises(RuntimeError, match="48000"):
        p.wait_ready(96000.0)
    p.stop()
    fake.write_text(f"#!{sys.executable}\nimport sys\nprint('ready ac2-dut 96000 256', flush=True)\nsys.stdin.read()\n")
    j = FakeJack(DUTP + ["ac2:out_5"], {("ac2:out_5", "ac2-dut:dut_in")})
    p = dutrun.DutProcess([str(fake)], "ac2-dut", tmp_path, open_client=lambda *a: j)
    with pytest.raises(RuntimeError, match="already fed"):
        p.wait_ready(96000.0)
    assert p.stop()["xruns"] is None


def test_dut_process_that_dies_early(tmp_path):
    fake = tmp_path / "fake-dut"
    fake.write_text(f"#!{sys.executable}\nimport sys\nprint('cannot connect to a JACK server', file=sys.stderr)\nsys.exit(1)\n")
    fake.chmod(0o755)
    p = dutrun.DutProcess([str(fake)], "ac2-dut", tmp_path, open_client=lambda *a: FakeJack(DUTP, set()))
    with pytest.raises(RuntimeError, match="before 'ready'"):
        p.wait_ready(96000.0)
    p.stop()
    assert "cannot connect" in (tmp_path / "dut.stderr.log").read_text()


def test_truth_table_is_json():
    coef = _rig_coef()
    t = dut.truth_table(dutrun.truth_freqs(FS), 0.316, coef, FS)
    json.dumps(t)
    assert t[0]["f"] >= 5.0 and set(t[-1]["h_dbr"]) == {"2"}


def test_run_path_patches_only_the_ac2_stages_and_flags_a_failed_restore(monkeypatch, tmp_path, capsys):
    from crosscheck import run, stages

    order = []
    ctx = _ctx()
    ctx.out = tmp_path
    ctx.manifest = {"paths": {"dut": {"kind": "digital"}}}

    class P:
        def __init__(self, *a, **k):
            pass

        def apply(self):
            order.append("patch")

        def record(self):
            return {"saved_inputs": {"ac2:in_3": ["system:capture_3"]}}

        def restore(self):
            order.append("restore")
            return ["ac2:in_3 is fed by [], was ['system:capture_3']"]

    monkeypatch.setattr(dutrun, "Patch", P)
    monkeypatch.setattr(dutrun, "start", lambda c, p: order.append("start") or object())
    monkeypatch.setattr(dutrun, "finish", lambda c, p, d: order.append("stop") or
                        {"stopped_by": "stdin closed", "rc": 0, "xruns_line": "xruns 0"})
    for name in ("sine_stage", "rew_stage", "ac2_sweeps", "ac2_tf"):
        monkeypatch.setattr(stages, name, lambda *a, n=name: order.append(n))
    rc = run.run_path(ctx, "dut", None)
    assert order == ["start", "sine_stage", "rew_stage", "patch", "ac2_sweeps", "ac2_tf", "restore", "stop"]
    assert rc == 4 and ctx.patch is None and ctx.dut is None
    out = capsys.readouterr().out
    assert "!!!" in out and "ac2 session open" in out


def test_a_failed_patch_runs_no_ac2_stage(monkeypatch, tmp_path):
    from crosscheck import run, stages

    order = []
    ctx = _ctx()
    ctx.out = tmp_path
    ctx.manifest = {"paths": {"dut": {"kind": "digital"}}}

    class P:
        def __init__(self, *a, **k):
            pass

        def apply(self):
            raise RuntimeError("JACK ports missing for the DUT patch: ac2:in_3")

        def record(self):
            return {"saved_inputs": None}

        def restore(self):
            order.append("restore")
            return []

    monkeypatch.setattr(dutrun, "Patch", P)
    monkeypatch.setattr(dutrun, "start", lambda c, p: object())
    monkeypatch.setattr(dutrun, "finish", lambda c, p, d: {"stopped_by": "x", "rc": 0, "xruns_line": None})
    for name in ("sine_stage", "rew_stage", "ac2_sweeps", "ac2_tf"):
        monkeypatch.setattr(stages, name, lambda *a, n=name: order.append(n))
    assert run.run_path(ctx, "dut", None) == 0
    assert order == ["sine_stage", "rew_stage", "restore"]
    st = ctx.manifest["stages"]
    assert st["dut.ac2_sweep"]["outcome"] == "failed" and "missing" in st["dut.ac2_tf"]["detail"]
