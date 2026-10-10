"""Preflight and the run: ambient (silent) → speaker path at −50 dBFS → electrical path →
digital DUT path, the electrical drop-in last and always removed (with `gen ceiling` read
back) in a finally."""
from __future__ import annotations

import fcntl
import json
import os
import shutil
import signal
import sys
import time
import tomllib
from pathlib import Path

from . import dutrun, levels, stages
from .ac2 import Ac2
from .levels import Output, Policy, PolicyError
from .rew import Rew, RewError

ORDER = ("ambient", "genelec", "xone", "dut")


def build_policy(rig: dict, emit=None, emit_speaker=None, allow=None, forbidden=(), allow_speaker=None) -> Policy:
    outs = {int(k): Output(int(k), v["port"], v["role"], v.get("note", "")) for k, v in rig["outputs"].items()}
    r = rig["rig"]
    return Policy(outputs=outs, forbidden=frozenset(int(x) for x in forbidden),
                  system_max_dbfs=float(r["system_max_dbfs"]), electrical_max_dbfs=float(r["electrical_max_dbfs"]),
                  speaker_max_dbfs=min(float(r["speaker_max_dbfs"]), levels.SPEAKER_HARD_MAX_DBFS),
                  emit_dbfs=levels.parse_dbfs(emit) if emit else None,
                  emit_speaker_dbfs=levels.parse_dbfs(emit_speaker) if emit_speaker else None,
                  allow_electrical_dbfs=levels.parse_dbfs(allow) if allow else None,
                  allow_speaker_dbfs=levels.parse_dbfs(allow_speaker) if allow_speaker else None)


def path_policy(rig: dict, base: Policy, pname: str) -> Policy:
    pc = rig["paths"][pname]
    from dataclasses import replace
    return replace(base, forbidden=frozenset(int(x) for x in pc.get("forbidden_outputs", [])))


def preflight(rig: dict, ac2: Ac2, rew: Rew | None, want: set[str], strict: bool = True) -> dict:
    """Read-only. Raises on what makes a run unsafe or pointless; returns what it found."""
    import jack

    rep: dict = {"ok": [], "warn": []}
    c = jack.Client("crosscheck-preflight", no_start_server=True)
    try:
        fs = float(c.samplerate)
        if abs(fs - float(rig["rig"]["sample_rate"])) > 0.5:
            raise RuntimeError(f"JACK runs at {fs:g} Hz, the rig config says {rig['rig']['sample_rate']}")
        names = {p.name for p in c.get_ports()}
        for kind in ("outputs", "inputs"):
            for ch, v in rig[kind].items():
                if v["port"] not in names:
                    raise RuntimeError(f"{kind} {ch}: JACK port {v['port']} not found")
        rep["jack_fs"] = fs
    finally:
        c.close()
    rep["ok"].append(f"JACK {fs:g} Hz, configured ports present")
    if "dut" in want:
        b = Path(os.path.expanduser(rig["dut"]["binary"]))
        if not (b.is_file() and os.access(b, os.X_OK)):
            if strict:
                raise RuntimeError(f"the DUT binary {b} is missing or not executable (README: Digital DUT path)")
            rep["warn"].append(f"the DUT binary {b} is missing or not executable: --stages dut would be refused")
        client = rig["dut"].get("client", "ac2-dut")
        for role, port in (rig["paths"]["dut"].get("ports") or {}).items():
            if not port.startswith(client + ":"):
                raise RuntimeError(f"paths.dut.ports.{role} = {port} is not a port of the DUT client {client}")
        rep["ok"].append(f"DUT binary {b}")
    st = ac2.status() or {}
    rep["ac2_build"] = st.get("build_id")
    if not (st.get("session") or {}).get("open"):
        rep["warn"].append("ac2: no session open (the run opens one with [ac2].session_open)")
    ceil = ac2.ceiling() or {}
    rep["ac2_ceiling"] = ceil
    bound = float(rig["rig"]["system_max_dbfs"])
    drop = stages.dropin_path(rig)
    if drop.exists():
        rep["warn"].append(f"a crosscheck drop-in is left over ({drop}): the run removes it first")
    elif abs(float(ceil.get("bound", 1e9)) - bound) > 1e-6:
        raise RuntimeError(f"ac2d's bound is {ceil.get('bound')} dBFS, the rig's is {bound:g}: not running as configured")
    rep["ok"].append(f"ac2 {st.get('build_id')}, bound {ceil.get('bound')} dBFS, ceiling {ceil.get('ceiling')}")
    if want & {"ambient", "genelec"}:
        cal = stages.cal_excerpt(rig)
        rep["cal"] = {k: v for k, v in cal.items() if k != "curve_points"}
        mic = next((i for i in st.get("inputs", []) if i.get("input") == cal["input"]), None)
        if mic is not None and not mic.get("sensitivity"):
            rep["warn"].append(f"ac2 status shows input {cal['input']} without a sensitivity")
        rep["ok"].append(f"calibration: {cal['mic']} {cal['sensitivity_db']:.2f} dB SPL @ 0 dBFS, curve {cal['curve_label']}")
    if rew is not None:
        try:
            rep["rew_version"] = rew.version()
            rep["ok"].append(f"REW {rep['rew_version']}")
        except RewError as e:
            rep["warn"].append(f"REW not reachable ({e}): its stages are skipped")
            rep["rew_version"] = None
        for key in ("stimulus", "stimulus_speaker"):
            p = rig["rew"].get(key)
            if p and not Path(p).exists():
                rep["warn"].append(f"REW {key} {p} missing")
    free = shutil.disk_usage(Path.home()).free / 1e9
    if free < 2:
        raise RuntimeError(f"{free:.1f} GB free in $HOME")
    return rep


def _confirm(ctx, text: str):
    print(text, flush=True)
    if ctx.yes:
        return
    if not sys.stdin.isatty():
        raise PolicyError("an audible stage needs the operator: run from a terminal or pass --yes")
    if input("Enter to continue, anything else to skip: ").strip():
        raise PolicyError("operator skipped")


def _single_run_lock():
    """Two runs at once share JACK ports, the generator and ac2's one recording slot, so each
    would corrupt the other's captures: refuse the second."""
    path = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "crosscheck-run.lock"
    f = open(path, "w")
    try:
        fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        raise SystemExit(f"another crosscheck run holds {path}; stop it first (pgrep -af 'crosscheck run')")
    return f


def main(a) -> int:
    rig = tomllib.loads(a.rig.read_text())
    lock = _single_run_lock() if a.cmd == "run" else None  # noqa: F841  held until exit
    want = [s for s in ORDER if s in set(x.strip() for x in getattr(a, "stages", ",".join(ORDER)).split(","))]
    out = a.out or (Path(__file__).resolve().parent.parent / "runs" / time.strftime("%Y%m%dT%H%M%SZ", time.gmtime()))
    out = Path(os.path.expanduser(str(out)))
    out.mkdir(parents=True, exist_ok=True)
    ac2 = Ac2(rig["ac2"]["cmd"], rig["ac2"].get("timeout", "5s"), log=out / "ac2.log")
    rew = Rew(rig["rew"]["api"]) if rig.get("rew") else None
    if a.cmd == "run":
        base = build_policy(rig, a.emit, a.emit_speaker, a.allow_electrical_level,
                            allow_speaker=a.allow_speaker_level)
        # refuse impossible combinations before anything is touched
        if "genelec" in want and base.emit_speaker_dbfs is None:
            raise PolicyError("the genelec stage needs --emit-speaker <level>dbfs (at most -50dbfs, or up to -30dbfs with --allow-speaker-level); or --stages without it")
        if "genelec" in want:
            pc = rig["paths"]["genelec"]
            path_policy(rig, base, "genelec").check([int(pc["out"]), int(pc["ref_out"])], base.emit_speaker_dbfs,
                                                    speaker_stage=True)
        for s in ("xone", "dut"):
            if s not in want:
                continue
            if base.emit_dbfs is None:
                raise PolicyError(f"the {s} stage needs --emit <level>dbfs; or --stages without it")
            pc = rig["paths"][s]
            # the dut path's channel numbers are ac2's: its stimulus also reaches those hardware outputs
            path_policy(rig, base, s).check([int(pc["out"]), int(pc["ref_out"])], base.emit_dbfs,
                                            speaker_stage=False)
    rep = preflight(rig, ac2, rew, set(want), strict=a.cmd == "run")
    for x in rep["ok"]:
        print("ok   ", x)
    for x in rep["warn"]:
        print("warn ", x)
    if a.cmd == "preflight":
        return 0
    if rew is not None and not rep.get("rew_version"):
        rew = None
    man = {"started": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "rig": rig["rig"]["name"],
           "ac2_version": rep.get("ac2_build"), "rew_version": rep.get("rew_version"),
           "flags": {"emit": a.emit, "emit_speaker": a.emit_speaker, "allow_electrical_level": a.allow_electrical_level,
                     "allow_speaker_level": a.allow_speaker_level,
                     "stages": want, "skip": a.skip, "resolution": getattr(a, "resolution", "1/48")},
           "mains_hz": rig["rig"].get("mains_hz", 50.0), "paths": {}, "preflight": rep,
           # the rig gets a copy without .git; rig-run.sh passes the commit it copied
           "suite_commit": os.environ.get("CROSSCHECK_SUITE_COMMIT") or None}
    ctx = stages.Ctx(rig=rig, policy=base, out=out, ac2=ac2, rew=rew, manifest=man, yes=a.yes,
                     skip=set(x for x in a.skip.split(",") if x), fs=rep["jack_fs"])
    ctx.save()
    (out / "rig.toml").write_text(a.rig.read_text())
    # SIGTERM / SIGHUP take the same path as Ctrl-C: outputs fade, the finally restores
    def _interrupt(*_):
        raise KeyboardInterrupt

    for sig in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(sig, _interrupt)
    snap = None
    cal = None
    cal_file = None
    rc = 0
    try:
        if stages.dropin_path(rig).exists():
            stages.remove_dropin(ctx)
        stages.ensure_session(ctx)
        if rew is not None:
            snap = rew.snapshot()
            (out / "cal").mkdir(exist_ok=True)
            (out / "cal" / "rew_input_cal.before.json").write_text(json.dumps(snap["input_cal"], indent=1))
        if set(want) & {"ambient", "genelec"}:
            cal = stages.cal_excerpt(rig)
            (out / "cal").mkdir(exist_ok=True)
            (out / "cal" / "store_excerpt.json").write_text(json.dumps(cal, indent=1))
            (out / "cal" / "ac2_cal.json").write_text(json.dumps(ac2.cal_list(), indent=1))
            if cal.get("curve_points"):
                cf = out / "cal" / "mic_curve_rew.txt"
                stages.write_rew_cal_file(cal["curve_points"], cf)
                cal_file = str(cf.resolve())
            man["cal_mapping"] = stages.cal_mapping(cal, cal_file)
            if rew is not None:
                applied = rew.set_input_cal(cal["rew_dbfs_at_94"], cal.get("full_scale_vrms", 1.0), cal_file or "")
                (out / "cal" / "rew_input_cal.applied.json").write_text(json.dumps(applied, indent=1))
            ctx.save()
        for s in want:
            try:
                if s == "ambient":
                    stages.ambient_stage(ctx, cal, cal_file)
                    ctx.stage("ambient", "done")
                    continue
                pc = rig["paths"][s]
                man["paths"][s] = {"kind": pc["kind"], "mains_hz": stages.path_mains_hz(rig, s)}
                ctx.policy = path_policy(rig, base, s)
                if pc["kind"] == "speaker":
                    spl = 63.0 + base.emit_speaker_dbfs - levels.SPEAKER_HARD_MAX_DBFS
                    _confirm(ctx, f"\n*** {s}: AUDIBLE. Out {pc['out']} drives the speaker at {base.emit_speaker_dbfs:g} dBFS "
                                  f"(≈{spl:.0f} dB SPL at the mic for a 1 kHz sine). Operator present, hearing "
                                  f"protected, nothing loose near the driver?")
                    if base.needs_raised_daemon_for_speaker():
                        # ac2d's bound only as far as this stage's level, and back before the next stage
                        stages.install_dropin(ctx, base.emit_speaker_dbfs)
                    else:
                        stages.remove_dropin(ctx)
                        stages.verify_bound(ctx, levels.SPEAKER_HARD_MAX_DBFS)
                elif base.needs_raised_daemon():
                    stages.install_dropin(ctx, base.allow_electrical_dbfs)
                rc = run_path(ctx, s, cal_file) or rc
                if pc["kind"] == "speaker" and base.needs_raised_daemon_for_speaker():
                    stages.remove_dropin(ctx)
                ctx.stage(s, "done")
            except PolicyError as e:
                ctx.stage(s, "refused", str(e))
                rc = 2
            except KeyboardInterrupt:
                ctx.stage(s, "aborted", "Ctrl-C: outputs faded, restoring")
                raise
            except Exception as e:  # one path failing doesn't stop the others; the finally still restores
                ctx.stage(s, "failed", f"{type(e).__name__}: {e}")
                rc = 1
    except KeyboardInterrupt:
        rc = 130
    finally:
        try:
            ac2.run("gen", "stop", json_out=False, check=False)
        except Exception:
            pass
        try:
            r = stages.remove_dropin(ctx)
            print(f"ac2d bound {r['ceiling_after'].get('bound')} dBFS, ceiling {r['ceiling_after'].get('ceiling')} "
                  f"(drop-in {'removed' if r['removed'] else 'never installed'})", flush=True)
        except Exception as e:
            print(f"!!! could not verify ac2d's bound after the run: {e}. Check `ac2 gen ceiling` NOW "
                  f"and remove {stages.dropin_path(rig)} by hand.", flush=True)
            rc = rc or 3
        if rew is not None and snap is not None:
            errs = rew.restore(snap)
            man["rew_restored"] = not errs
            if errs:
                print("REW restore:", "; ".join(errs))
        man["finished"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        ctx.save()
    print(f"run directory: {out}")
    return rc


AC2_STAGES = ("ac2_sweep", "ac2_tf")


def run_path(ctx: stages.Ctx, pname: str, cal_file: str | None) -> int:
    """The sub-stages of one path. A digital path runs its DUT around them and patches ac2
    onto it for the ac2 stages only (the suite's own player feeds the DUT in the others, and
    the two must never overlap). Returns nonzero when ac2's inputs could not be restored."""
    digital = ctx.rig["paths"][pname]["kind"] == "digital"
    subs = (("sine", lambda: stages.sine_stage(ctx, pname)),
            ("rew", lambda: stages.rew_stage(ctx, pname, cal_file)),
            ("ac2_sweep", lambda: stages.ac2_sweeps(ctx, pname)),
            ("ac2_tf", lambda: stages.ac2_tf(ctx, pname)))
    rc = 0
    patch_error = None
    if digital:
        ctx.dut = dutrun.start(ctx, pname)
    try:
        for sub, fn in subs:
            if sub in ctx.skip:
                ctx.stage(f"{pname}.{sub}", "skipped", "--skip")
                continue
            try:
                if digital and sub in AC2_STAGES:
                    if patch_error:
                        raise RuntimeError(f"ac2 not patched onto the DUT: {patch_error}")
                    if ctx.patch is None:
                        try:
                            patch_ac2(ctx, pname)
                        except Exception as e:
                            patch_error = f"{type(e).__name__}: {e}"
                            raise
                if fn() != "skipped":
                    ctx.stage(f"{pname}.{sub}", "done")
            except (PolicyError, KeyboardInterrupt):
                raise
            except Exception as e:
                ctx.stage(f"{pname}.{sub}", "failed", f"{type(e).__name__}: {e}")
    finally:
        if ctx.patch is not None:
            rc = unpatch_ac2(ctx, pname)
        if ctx.dut is not None:
            r = dutrun.finish(ctx, pname, ctx.dut)
            ctx.dut = None
            print(f"[{pname}] DUT stopped ({r['stopped_by']}, rc {r['rc']}): {r['xruns_line'] or 'no xruns line'}",
                  flush=True)
    return rc


def patch_ac2(ctx: stages.Ctx, pname: str):
    pc = ctx.rig["paths"][pname]
    dc = ctx.rig["dut"]
    ctx.patch = dutrun.Patch(dc.get("ac2_client", "ac2"), dc.get("client", "ac2-dut"), pc)
    try:
        ctx.patch.apply()
    finally:
        ctx.manifest["paths"][pname]["patch"] = ctx.patch.record()
        ctx.save()


def unpatch_ac2(ctx: stages.Ctx, pname: str) -> int:
    errs = ctx.patch.restore()
    rec = ctx.patch.record()
    rec["restore_errors"] = errs
    ctx.manifest["paths"][pname]["patch"] = rec
    ctx.patch = None
    ctx.save()
    if not errs:
        print(f"[{pname}] ac2's inputs restored: {rec['saved_inputs']}", flush=True)
        return 0
    cmd = " ".join(["ac2", *map(str, ctx.rig["ac2"]["session_open"])])
    print(f"!!! ac2's inputs were NOT restored after the DUT patch: {'; '.join(errs)}.\n"
          f"!!! Its captures may still come from the DUT. Run `{cmd}` NOW to reconnect them.", flush=True)
    return 4
