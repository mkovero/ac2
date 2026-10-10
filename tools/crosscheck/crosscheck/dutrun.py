"""The digital DUT on the rig: the `ac2-jack-dut` process for a digital path's stages, and
the patch that puts ac2's inputs and outputs onto it for the ac2 stages.

Runs on the rig with numpy + JACK-Client only. JACK clients here are short-lived and
process no audio: they only read and change connections."""
from __future__ import annotations

import json
import os
import queue
import signal
import subprocess
import threading
import time
from pathlib import Path

import numpy as np

from . import dsp, dut, levels


def _jack_client(name: str = "crosscheck-patch"):
    import jack
    return jack.Client(name, no_start_server=True)


def _names(ports) -> set[str]:
    return {p if isinstance(p, str) else p.name for p in ports}


class DutProcess:
    """`ac2-jack-dut` started for a digital path; `stop()` in the caller's finally."""

    def __init__(self, cmd: list[str], client: str, logdir: Path, open_client=_jack_client):
        self.cmd, self.client, self.open_client = cmd, client, open_client
        self.lines: list[str] = []
        self._q: queue.Queue = queue.Queue()
        logdir.mkdir(parents=True, exist_ok=True)
        self._err = open(logdir / "dut.stderr.log", "w")
        self.p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self._err, text=True,
                                  bufsize=1)
        threading.Thread(target=self._read, daemon=True).start()
        self.ready: dict | None = None

    def _read(self):
        for line in self.p.stdout:
            line = line.strip()
            self.lines.append(line)
            self._q.put(line)
        self._q.put(None)

    @property
    def ports(self) -> dict[str, str]:
        return {r: f"{self.client}:{r}" for r in ("ref_in", "ref_out", "dut_in", "dut_out")}

    def wait_ready(self, expect_fs: float, timeout: float = 15.0) -> dict:
        t0 = time.monotonic()
        while True:
            left = timeout - (time.monotonic() - t0)
            if left <= 0:
                raise RuntimeError(f"{self.cmd[0]}: no 'ready' line within {timeout:g} s")
            try:
                line = self._q.get(timeout=left)
            except queue.Empty:
                continue
            if line is None:
                raise RuntimeError(f"{self.cmd[0]} exited (rc {self.p.wait()}) before 'ready'; see dut.stderr.log")
            w = line.split()
            if len(w) == 4 and w[0] == "ready":
                self.ready = {"name": w[1], "fs": float(w[2]), "buffer": int(w[3])}
                break
        if self.ready["name"] != self.client:
            raise RuntimeError(f"the DUT came up as {self.ready['name']!r}, not {self.client!r}")
        if abs(self.ready["fs"] - expect_fs) > 0.5:
            raise RuntimeError(f"the DUT runs at {self.ready['fs']:g} Hz, JACK at {expect_fs:g}")
        c = self.open_client("crosscheck-dut-check")
        try:
            have = _names(c.get_ports(f"{self.client}:"))
            missing = [p for p in self.ports.values() if p not in have]
            if missing:
                raise RuntimeError(f"DUT ports missing: {', '.join(missing)}")
        finally:
            c.close()
        self.check_free()
        return self.ready

    def check_free(self):
        """The DUT's inputs carry nothing yet: whatever feeds them is in the measurement."""
        c = self.open_client("crosscheck-dut-check")
        try:
            for r in ("dut_in", "ref_in"):
                src = _names(c.get_all_connections(self.ports[r]))
                if src:
                    raise RuntimeError(f"{self.ports[r]} is already fed by {', '.join(sorted(src))}")
        finally:
            c.close()

    def stop(self, wait_s: float = 5.0) -> dict:
        """Close stdin (the binary's stop), then SIGTERM, then kill; its last word is `xruns <n>`."""
        how = "stdin closed"
        try:
            if self.p.stdin and not self.p.stdin.closed:
                self.p.stdin.close()
        except OSError:
            pass
        try:
            self.p.wait(wait_s)
        except subprocess.TimeoutExpired:
            how = "SIGTERM"
            self.p.send_signal(signal.SIGTERM)
            try:
                self.p.wait(wait_s)
            except subprocess.TimeoutExpired:
                how = "killed"
                self.p.kill()
                self.p.wait(wait_s)
        t0 = time.monotonic()
        while time.monotonic() - t0 < 2.0 and not any(x.startswith("xruns") for x in self.lines):
            time.sleep(0.05)
        self._err.close()
        xr = next((x for x in reversed(self.lines) if x.startswith("xruns")), None)
        n = None
        if xr is not None:
            try:
                n = int(xr.split()[1])
            except (IndexError, ValueError):
                pass
        return {"rc": self.p.returncode, "stopped_by": how, "xruns_line": xr, "xruns": n}


def truth_freqs(fs: float) -> np.ndarray:
    """The dense grid the truth table is written on: 1/12 octave, 5 Hz up to where H2 still
    lies below fs/2."""
    return dsp.log_centres(5.0, fs / 4, 12)


def binary(rig: dict) -> str:
    """The DUT binary: AC2_JACK_DUT, else the rig's `[dut].binary`."""
    return os.path.expanduser(os.environ.get("AC2_JACK_DUT") or rig["dut"]["binary"])


POLY_KEYS = ("poly", "harmonics_dbr", "harmonics_dbfs")


def expand_cases(rig: dict) -> list[str]:
    """The digital paths a `--stages dut` run measures. Without `[[dut.cases]]` that is the
    one path `dut`. With cases, each becomes a path `dut-<name>`: a copy of `[paths.dut]`
    tagged with its case and with `stage = "dut"` (one baseline holds them all), and every
    `[stages.*]` setting keyed by `dut` (a table entry, or `dut_hz`) copied to it. Modifies
    `rig` in place."""
    cases = rig.get("dut", {}).get("cases") or []
    if not cases:
        return ["dut"]
    names = []
    for c in cases:
        pname = f"dut-{c['name']}"
        if pname in rig["paths"] or pname in names:
            raise ValueError(f"DUT case {c['name']!r}: path {pname} already exists")
        names.append(pname)
        rig["paths"][pname] = dict(rig["paths"]["dut"], stage="dut", case=c["name"])
        for table in rig.get("stages", {}).values():
            for key, v in list(table.items()):
                if key == "dut":
                    table[pname] = v
                elif key == "dut_hz":
                    table[f"{pname}_hz"] = v
                elif isinstance(v, dict) and "dut" in v:
                    v[pname] = v["dut"]
    return names


def dut_config(rig: dict, pname: str) -> dict:
    """`[dut]` for one path: the shared table, with the path's case laid over it (a case
    that gives its own polynomial replaces the shared one whichever form either uses)."""
    base = {k: v for k, v in rig["dut"].items() if k != "cases"}
    name = rig["paths"][pname].get("case")
    if name is None:
        return base
    case = next(c for c in rig["dut"]["cases"] if c["name"] == name)
    if any(k in case for k in POLY_KEYS):
        base = {k: v for k, v in base.items() if k not in POLY_KEYS}
    return {**base, **case}


def start(ctx, pname: str) -> DutProcess:
    """Designs the coefficients from the rig's [dut] table, starts the binary, waits for it,
    and writes `<path>/dut/dut.json`: the exact command, the coefficients and the analytic
    truth for the planned tones and a dense grid."""
    dc = dut_config(ctx.rig, pname)
    coef = dut.coefficients(dc, ctx.fs, ctx.policy.electrical_level())
    cmd = dut.command(dc, coef, binary(ctx.rig))
    d = ctx.out / pname / "dut"
    amp = levels.peak_amplitude(ctx.policy.electrical_level())
    pk = dut.peak_out(amp, coef, ctx.fs)
    if pk >= 1.0:
        raise RuntimeError(f"the DUT's output would peak at {pk:.3f} at the emit level: it clips, no exact truth")
    proc = DutProcess(cmd, dc.get("client", "ac2-dut"), d)
    try:
        ready = proc.wait_ready(ctx.fs)
    except BaseException:
        proc.stop(1.0)
        raise
    tones = [float(x) for x in ctx.rig["stages"]["sine"].get(f"{pname}_hz", [])]
    rec = {"command": cmd, "config": {k: v for k, v in dc.items()}, "coefficients": coef, "fs": ctx.fs,
           "ready": ready, "amp": amp, "level_dbfs": ctx.policy.electrical_level(), "peak_out": pk,
           "tones": dut.truth_table(tones, amp, coef, ctx.fs),
           "grid": dut.truth_table(truth_freqs(ctx.fs), amp, coef, ctx.fs)}
    (d / "dut.json").write_text(json.dumps(rec, indent=1))
    ctx.manifest["paths"][pname]["dut"] = {"command": cmd, "coefficients": coef, "ready": ready, "peak_out": pk}
    ctx.save()
    proc.record = rec
    proc.record_path = d / "dut.json"
    return proc


def finish(ctx, pname: str, proc: DutProcess) -> dict:
    r = proc.stop()
    ctx.manifest["paths"][pname].setdefault("dut", {})["stop"] = r
    rec = getattr(proc, "record", None)
    if rec is not None:
        rec["stop"] = r
        proc.record_path.write_text(json.dumps(rec, indent=1))
    ctx.save()
    return r


class Patch:
    """ac2's meas and ref inputs and its stimulus outputs onto the DUT for the ac2 stages,
    and back afterwards. ac2 connects its inputs only at session open, so what was on them
    is recorded here and put back exactly; its outputs keep their hardware connections (ac2d
    makes those itself whenever its generator routes change)."""

    def __init__(self, ac2_client: str, dut_client: str, ch: dict, open_client=_jack_client):
        self.open_client = open_client
        self.meas_in, self.ref_in = f"{ac2_client}:in_{int(ch['meas_in'])}", f"{ac2_client}:in_{int(ch['ref_in'])}"
        self.links = [(f"{dut_client}:dut_out", self.meas_in), (f"{dut_client}:ref_out", self.ref_in),
                      (f"{ac2_client}:out_{int(ch['out'])}", f"{dut_client}:dut_in"),
                      (f"{ac2_client}:out_{int(ch['ref_out'])}", f"{dut_client}:ref_in")]
        self.saved: dict[str, list[str]] | None = None
        self.made: list[tuple[str, str]] = []
        self.undone: list[tuple[str, str]] = []

    def record(self) -> dict:
        return {"saved_inputs": self.saved, "links": [list(x) for x in self.links], "made": [list(x) for x in self.made],
                "removed": [list(x) for x in self.undone]}

    def apply(self):
        c = self.open_client()
        try:
            have = _names(c.get_ports())
            missing = sorted({p for link in self.links for p in link} - have)
            if missing:
                raise RuntimeError(f"JACK ports missing for the DUT patch: {', '.join(missing)}")
            for dst in (self.links[2][1], self.links[3][1]):
                src = _names(c.get_all_connections(dst))
                if src:
                    raise RuntimeError(f"{dst} is already fed by {', '.join(sorted(src))}")
            self.saved = {p: sorted(_names(c.get_all_connections(p))) for p in (self.meas_in, self.ref_in)}
            for p, srcs in self.saved.items():
                for s in srcs:
                    c.disconnect(s, p)
                    self.undone.append((s, p))
            for a, b in self.links:
                c.connect(a, b)
                self.made.append((a, b))
        finally:
            c.close()
        self.verify_sources()

    def verify_sources(self):
        """Each patched ac2 input is fed by its DUT output alone."""
        c = self.open_client()
        try:
            for src, dst in self.links[:2]:
                got = _names(c.get_all_connections(dst))
                if got != {src}:
                    raise RuntimeError(f"{dst} is fed by {', '.join(sorted(got)) or 'nothing'}, not {src} alone")
        finally:
            c.close()

    def restore(self) -> list[str]:
        """Remove exactly the links made, put back the recorded input connections, verify.
        Returns what went wrong (empty: restored)."""
        errs = []
        try:
            c = self.open_client()
        except Exception as e:
            return [f"no JACK client to restore with: {e}"]
        try:
            for a, b in reversed(self.made):
                try:
                    c.disconnect(a, b)
                except Exception as e:
                    errs.append(f"disconnect {a} → {b}: {e}")
            for p, srcs in (self.saved or {}).items():
                for s in srcs:
                    try:
                        if s not in _names(c.get_all_connections(p)):
                            c.connect(s, p)
                    except Exception as e:
                        errs.append(f"connect {s} → {p}: {e}")
            for p, srcs in (self.saved or {}).items():
                got = _names(c.get_all_connections(p))
                if got != set(srcs):
                    errs.append(f"{p} is fed by {sorted(got)}, was {srcs}")
            for a, b in self.made:
                if a in _names(c.get_all_connections(b)):
                    errs.append(f"{a} → {b} is still connected")
        finally:
            c.close()
        return errs
