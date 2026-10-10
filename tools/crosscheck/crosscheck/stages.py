"""The stages of a run. Each writes into its own directory under the run (layout in
model.py) and records its outcome in the manifest; the analysis never needs the rig.

Runs on the rig with numpy + JACK-Client only."""
from __future__ import annotations

import json
import math
import os
import shutil
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from . import dsp, jackio, levels, wav
from .ac2 import Ac2
from .levels import Policy, PolicyError
from .rew import Rew, RewError


def _resolution_args(ctx) -> list[str]:
    """`--resolution` for ac2's sweep and TF; left out at the default, which ac2 uses anyway."""
    from .baseline import DEFAULT_PPO, resolution_of
    ppo = resolution_of(ctx.manifest)
    return [] if ppo == DEFAULT_PPO else ["--resolution", f"1/{ppo}"]


@dataclass
class Ctx:
    rig: dict
    policy: Policy
    out: Path
    ac2: Ac2
    rew: Rew | None
    manifest: dict
    yes: bool = False
    skip: set = field(default_factory=set)
    fs: float = 0.0
    dut: object = None    # dutrun.DutProcess while a digital path's DUT runs
    patch: object = None  # dutrun.Patch while ac2 is patched onto the DUT

    @property
    def max_xruns(self) -> int:
        # 0 on a rig: a take with a dropout is discarded (a JACK dummy server for tests xruns)
        return int(self.rig["rig"].get("max_xruns", 0))

    def out_port(self, ch: int) -> str:
        return self.rig["outputs"][str(ch)]["port"]

    def in_port(self, ch: int) -> str:
        return self.rig["inputs"][str(ch)]["port"]

    def path_ports(self, pname: str) -> dict[str, str]:
        """The JACK ports the suite's own player and recorder use for a path's four roles:
        the path's `ports` table where it has one (a software DUT), else the rig's channel
        tables by the path's channel numbers."""
        pc = self.rig["paths"][pname]
        own = pc.get("ports") or {}
        out = {}
        for role, table in (("out", self.out_port), ("ref_out", self.out_port), ("meas_in", self.in_port),
                            ("ref_in", self.in_port)):
            out[role] = own[role] if role in own else table(int(pc[role]))
        return out

    def path_mains_hz(self, pname: str) -> float | None:
        """Mains frequency on a path; None for a path without mains (a digital DUT: `mains_hz = 0`)."""
        return path_mains_hz(self.rig, pname)

    def stage(self, name: str, outcome: str, detail: str = ""):
        self.manifest.setdefault("stages", {})[name] = {"outcome": outcome, "detail": detail}
        self.save()
        print(f"[{name}] {outcome}{': ' + detail if detail else ''}", flush=True)

    def save(self):
        (self.out / "manifest.json").write_text(json.dumps(self.manifest, indent=1, default=str))


def path_mains_hz(rig: dict, pname: str) -> float | None:
    v = float(rig["paths"][pname].get("mains_hz", rig["rig"].get("mains_hz", 50.0)))
    return v if v > 0 else None


def _j(p: Path, obj):
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps(obj, indent=1, default=float))


# ---------------------------------------------------------------- calibration


def cal_excerpt(rig: dict) -> dict:
    """The mic input's sensitivity and active curve from ac2's store, and the REW settings
    derived from them. Asserts the store's full-scale-sine convention: 94 dB SPL at the
    stated mic sensitivity must land at 94 − sensitivity dBFS with 0 dBFS = full_scale V."""
    store = json.loads(Path(os.path.expanduser(rig["ac2"]["cal_store"])).read_text())
    if store.get("format") != "ac2-calibrations":
        raise RuntimeError("calibration store: unknown format")
    ch = int(rig["ac2"]["mic_input"]) - 1
    inp = next((x for x in store.get("inputs", []) if x.get("channel") == ch), None)
    if not inp or not inp.get("mic"):
        raise RuntimeError(f"calibration store: no mic on input {ch + 1}")
    mic = inp["mic"]
    sens = next((s for s in store.get("sensitivities", []) if s["key"].get("channel") == ch
                 and s["key"].get("mic") == mic), None)
    if not sens:
        raise RuntimeError(f"calibration store: input {ch + 1} ({mic}) has no sensitivity")
    S = float(sens["spl"]["sensitivity"])
    method = sens["spl"].get("method") or {}
    label = (inp.get("curve") or {}).get("label") if (inp.get("curve") or {}).get("type") == "curve" else None
    points = None
    for m in store.get("mics", []):
        if m.get("name") == mic:
            for c in m.get("curves", []):
                if c["reference"].get("label") == label:
                    points = c["points"]
    ex = {"input": ch + 1, "mic": mic, "sensitivity_db": S, "curve_label": label, "curve_points": points,
          "method": method, "rew_dbfs_at_94": 94.0 - S}
    fsv = method.get("full_scale")
    if fsv:
        ex["full_scale_vrms"] = float(fsv)
        ms = method.get("mic_sensitivity")
        if ms:
            implied = 20 * math.log10(float(ms) * 1e-3 / float(fsv))
            ex["convention_check_db"] = (94.0 - S) - implied
            if abs(ex["convention_check_db"]) > 0.5:
                raise RuntimeError(f"calibration store: 94 - sensitivity = {94 - S:.2f} dBFS but the mic's "
                                   f"{ms} mV/Pa at 0 dBFS = {fsv:.4f} V gives {implied:.2f} dBFS: not the "
                                   "full-scale-sine convention the suite assumes")
    return ex


def write_rew_cal_file(points, path: Path) -> None:
    """REW applies a cal file as written; ac2 normalises its curve to 0 dB at 1 kHz
    (sensitivity is defined there), so the file REW gets is the same curve shifted."""
    c = np.asarray(points, float)
    at1k = float(np.interp(np.log(1000.0), np.log(c[:, 0]), c[:, 1]))
    path.write_text("".join(f"{f:.3f}\t{v - at1k:.3f}\n" for f, v in c))


def cal_mapping(ex: dict, cal_file: str | None) -> list[list[str]]:
    return [
        ["sensitivity", f"{ex['sensitivity_db']:.3f} dB SPL at 0 dBFS", f"dBFSAt94dBSPL {ex['rew_dbfs_at_94']:.3f}",
         "94 − sensitivity"],
        ["0 dBFS", f"{ex.get('full_scale_vrms', float('nan')) * 1e3:.2f} mV (full-scale sine)",
         f"fullScaleSineVrms {ex.get('full_scale_vrms', float('nan')):.5f}",
         f"convention check {ex.get('convention_check_db', float('nan')):+.3f} dB"],
        ["mic curve", f"{ex.get('curve_label')} ({len(ex.get('curve_points') or [])} points), 0 dB at 1 kHz",
         f"calFilePath {cal_file or '—'}", "same points shifted to 0 dB at 1 kHz"],
        ["applied to", f"input {ex['input']} ({ex['mic']})", "applyCal on the mic channel's import only",
         "REW's loopback/Xone imports are uncalibrated ratios"],
    ]


# ---------------------------------------------------------------- daemon bound


def ceiling(ctx: Ctx) -> dict:
    return ctx.ac2.ceiling() or {}


def verify_bound(ctx: Ctx, want: float) -> dict:
    c = ceiling(ctx)
    if abs(float(c.get("bound", 1e9)) - want) > 1e-6:
        raise RuntimeError(f"ac2d's bound reads {c.get('bound')} dBFS, expected {want:g}")
    if float(c.get("ceiling", 1e9)) > want + 1e-6:
        raise RuntimeError(f"ac2d's ceiling reads {c.get('ceiling')} dBFS, above {want:g}")
    return c


def ensure_session(ctx: Ctx, deadline: float = 30.0):
    t0 = time.monotonic()
    while True:
        try:
            st = ctx.ac2.status() or {}
            break
        except Exception:
            if time.monotonic() - t0 > deadline:
                raise
            time.sleep(1.0)
    if not (st.get("session") or {}).get("open"):
        ctx.ac2.run(*ctx.rig["ac2"]["session_open"], json_out=False)
        st = ctx.ac2.status() or {}
    return st


def _systemctl(*a):
    subprocess.run(["systemctl", "--user", *a], check=True, timeout=90)


def dropin_path(rig: dict) -> Path:
    return Path(rig["ac2"]["dropin"].format(uid=os.getuid()))


def install_dropin(ctx: Ctx, level: float):
    """Restarts ac2d with --max-level `level` through a runtime drop-in (gone at reboot)."""
    p = dropin_path(ctx.rig)
    have = ctx.manifest.get("dropin") or {}
    if p.exists() and have.get("installed") == str(p) and have.get("level") == level and "removed" not in have:
        # an earlier electrical path installed it: no second restart (which reopens the session)
        if abs(float(ceiling(ctx).get("bound", 0)) - level) <= 1e-6:
            return
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text("[Service]\nExecStart=\nExecStart=" + ctx.rig["ac2"]["exec_start"].format(level=f"{level:g}") + "\n")
    _systemctl("daemon-reload")
    _systemctl("restart", ctx.rig["ac2"]["systemd_unit"])
    ensure_session(ctx)
    c = ceiling(ctx)
    if abs(float(c.get("bound", 0)) - level) > 1e-6:
        raise RuntimeError(f"after the drop-in ac2d's bound reads {c.get('bound')}, not {level:g}")
    if float(c.get("ceiling", -999)) < level - 1e-6:
        ctx.ac2.run("gen", "ceiling", f"{level:g}dbfs", "--yes")
    ctx.manifest["dropin"] = {"installed": str(p), "level": level, "ceiling": ceiling(ctx)}
    ctx.save()


def remove_dropin(ctx: Ctx) -> dict:
    """Back to the unit's own ExecStart (the rig bound), and prove it from `gen ceiling`."""
    p = dropin_path(ctx.rig)
    existed = p.exists()
    if existed:
        p.unlink()
        _systemctl("daemon-reload")
        _systemctl("restart", ctx.rig["ac2"]["systemd_unit"])
    ensure_session(ctx)
    c = verify_bound(ctx, float(ctx.rig["rig"]["system_max_dbfs"]))
    r = {"removed": existed, "ceiling_after": c}
    rec = ctx.manifest.setdefault("dropin", {})
    # the run's final check after a stage already removed it must not erase that record
    rec.update(r if existed or not rec.get("removed") else {"ceiling_after": c})
    ctx.save()
    return {"removed": bool(rec.get("removed")), "ceiling_after": c}


# ---------------------------------------------------------------- steady sines


def _steady(x: np.ndarray, fs: float, pre_s: float, sig_s: float, fade_s: float, settle_s: float) -> np.ndarray:
    a = int((pre_s + fade_s + settle_s) * fs)
    b = int((pre_s + sig_s - fade_s) * fs)
    return x[a:b]


def _floor_at(noise: np.ndarray, f: float, fs: float, n: int, p1: float) -> dict:
    return dsp.harmonic_floor(noise, f, fs, n, p1)


def sine_stage(ctx: Ctx, pname: str):
    """Steady sines on meas and reference outputs at the emit level; per tone the meas÷ref
    phasor, level and H2..H5 with floors from a silent recording analysed in the same bins.
    Durations are planned from a probe so the floor reaches the target within the budget."""
    rig, pc = ctx.rig, ctx.rig["paths"][pname]
    sc = rig["stages"]["sine"]
    speaker = pc["kind"] == "speaker"
    outs = [int(pc["out"]), int(pc["ref_out"])]
    level = ctx.policy.emit_speaker_dbfs if speaker else ctx.policy.electrical_level()
    level = ctx.policy.check(outs, level, speaker_stage=speaker)
    amp = levels.peak_amplitude(level)
    ports = ctx.path_ports(pname)
    out_ports = [ports["out"], ports["ref_out"]]
    ins = [ports["meas_in"], ports["ref_in"]]
    if ctx.dut is not None:
        ctx.dut.check_free()
    d = ctx.out / pname / "sine"
    d.mkdir(parents=True, exist_ok=True)
    fade_s, settle = float(sc["fade_seconds"]), float(sc["settle_seconds"])
    target = float(sc["target_floor_dbr"][pname])
    budget = float(sc["budget_seconds"][pname])
    mains = ctx.path_mains_hz(pname)
    probe_s = float(sc["probe_seconds"])

    def take(f, seconds):
        sig = jackio.sine(f, amp, seconds + 2 * fade_s + settle, ctx.fs, fade_s)
        plays = [jackio.Play(port, sig, amp) for port in out_ports]
        fs, x = jackio.play_record(plays, ins, pre_s=0.2, post_s=0.3, fade_s=fade_s, expect_fs=ctx.fs, max_xruns=ctx.max_xruns)
        return fs, x

    # a short noise take for the probe floors
    fs, nz = jackio.play_record([], ins, pre_s=max(3 * probe_s, 4.0), post_s=0.0, expect_fs=ctx.fs, max_xruns=ctx.max_xruns)
    tones = []
    pairs = sc.get("gd_pairs", False) and not speaker
    for f0 in [float(x) for x in sc[f"{pname}_hz"]]:
        seconds0 = max(probe_s, sc["min_periods"] / f0)
        f = dsp.avoid_mains(f0, seconds0, mains, pair=pairs) if mains else f0
        fs, x = take(f, seconds0)
        seg = _steady(x[:, 0], fs, 0.2, seconds0 + 2 * fade_s + settle, fade_s, settle)
        if not np.any(seg):
            raise RuntimeError(f"{f:g} Hz probe: the measurement input is silent (wiring, gain?)")
        h = dsp.sine_harmonics(seg, f, fs)
        fl = _floor_at(nz[:, 0], f, fs, len(seg), h["p1"])
        worst = max(v for v in fl.values() if np.isfinite(v)) if any(np.isfinite(v) for v in fl.values()) else np.nan
        tones.append({"f_req": f0, "f": f, "probe_s": seconds0, "probe_floor_dbr": worst})
    # floor in a lobe falls 10·log10(T) with tone length T (bandwidth ∝ 1/T)
    want = []
    for t in tones:
        g = t["probe_floor_dbr"] - target if np.isfinite(t["probe_floor_dbr"]) else 0.0
        want.append(min(max(t["probe_s"] * 10 ** (max(g, 0.0) / 10), t["probe_s"]), float(sc["max_seconds"])))
    per_tone_extra = 2 * max(probe_s, 1.0) if pairs else 0.0
    total = sum(want) + len(want) * (per_tone_extra + 1.0)
    scale = min(1.0, max(budget - len(want) * (per_tone_extra + 1.0), 1.0) / max(sum(want), 1e-9))
    for t, w in zip(tones, want):
        t["seconds"] = max(t["probe_s"], w * scale)
        t["predicted_floor_dbr"] = t["probe_floor_dbr"] - 10 * math.log10(t["seconds"] / t["probe_s"])
        t["reaches_target"] = bool(t["predicted_floor_dbr"] <= target)
    _j(d / "plan.json", {"target_floor_dbr": target, "budget_s": budget, "planned_total_s": total, "scale": scale,
                         "tones": tones})
    res = {"emit_dbfs": level, "path": pname, "fs": fs, "tones": []}
    takes = {}
    for t in tones:
        f = t["f"]
        fs, x = take(f, t["seconds"])
        sig_s = t["seconds"] + 2 * fade_s + settle
        m = _steady(x[:, 0], fs, 0.2, sig_s, fade_s, settle)
        r = _steady(x[:, 1], fs, 0.2, sig_s, fade_s, settle)
        takes[f] = (m, r)
        hm, hr = dsp.sine_harmonics(m, f, fs), dsp.sine_harmonics(r, f, fs)
        pm, pr = dsp.sine_phasor(m, f, fs), dsp.sine_phasor(r, f, fs)
        if abs(pm) == 0 or abs(pr) == 0:
            raise RuntimeError(f"{f:g} Hz: no signal on the {'reference' if abs(pr) == 0 else 'measurement'} input")
        ratio = pm / pr
        tone = {"f": f, "f_requested": t["f_req"], "seconds": t["seconds"], "ratio_db": float(20 * np.log10(abs(ratio))),
                "ratio_deg": float(np.degrees(np.angle(ratio))), "meas_level_dbfs": hm["level_dbfs"],
                "ref_level_dbfs": hr["level_dbfs"],
                "h_dbr": {str(k): float(v) for k, v in hm["h_dbr"].items() if np.isfinite(v)},
                "h_ref_dbr": {str(k): float(v) for k, v in hr["h_dbr"].items() if np.isfinite(v)},
                # each input's harmonic phasors over the meas fundamental's, one take: a sweep
                # divided by the measured reference reads the meas harmonic less the
                # reference's carried through the path, which needs both with their phases
                "h_vec": _h_vec(m, r, f, fs)}
        if pairs:
            ph = []
            for s in (-1, 1):
                fp = f * 2 ** (s / 48)
                sp = max(probe_s, sc["min_periods"] / fp)
                fs, y = take(fp, sp)
                ys = 2 * fade_s + settle + sp
                a_ = dsp.sine_phasor(_steady(y[:, 0], fs, 0.2, ys, fade_s, settle), fp, fs)
                b_ = dsp.sine_phasor(_steady(y[:, 1], fs, 0.2, ys, fade_s, settle), fp, fs)
                ph.append((fp, np.angle(a_ / b_)))
            dphi = np.angle(np.exp(1j * (ph[1][1] - ph[0][1])))
            tone["gd_s"] = float(-dphi / (2 * np.pi * (ph[1][0] - ph[0][0])))
        tone["_p1"] = hm["p1"]
        res["tones"].append(tone)
    # the noise for the floors: long enough for the longest tone, twice over
    nsec = max(float(sc["noise_seconds"]), 2 * max(t["seconds"] for t in tones) + 1.0)
    fs, nz = jackio.play_record([], ins, pre_s=nsec, post_s=0.0, expect_fs=ctx.fs, max_xruns=ctx.max_xruns)
    wav.write(d / "noise.wav", fs, nz)
    for tone in res["tones"]:
        n = len(takes[tone["f"]][0])
        fl = _floor_at(nz[:, 0], tone["f"], fs, n, tone.pop("_p1"))
        tone["floor_dbr"] = {str(k): float(v) for k, v in fl.items() if np.isfinite(v)}
    res["floor_note"] = ("floors: silent recording after the tones, cut into segments of each tone's length, same "
                         "Blackman window and ±3-bin lobe, powers averaged over segments")
    _j(d / "results.json", res)
    return res


# ---------------------------------------------------------------- REW offline


def _c(z: complex) -> list[float]:
    return [float(z.real), float(z.imag)]


def _h_vec(m: np.ndarray, r: np.ndarray, f: float, fs: float) -> dict:
    hm, hr = dsp.sine_harmonic_phasors(m, f, fs), dsp.sine_harmonic_phasors(r, f, fs)
    return {str(k): {"meas": _c(hm[k] / hm[1]), "ref": _c(hr[k] / hm[1])} for k in hm if k > 1}


def sweep_start_hz(x: np.ndarray, fs: float) -> float:
    """Instantaneous frequency at the start of a sweep from its first zero crossings after
    the onset (−40 dB re its peak)."""
    a = np.abs(x)
    on = int(np.argmax(a > a.max() * 0.01))
    seg = x[on:on + int(0.5 * fs)]
    s = np.signbit(seg)
    zc = np.nonzero(s[1:] != s[:-1])[0]
    if len(zc) < 4:
        return float("nan")
    return float(fs / (2 * np.median(np.diff(zc[:6]))))


def rew_stage(ctx: Ctx, pname: str, cal_file: str | None):
    """Plays REW's stimulus file through JACK on meas and reference outputs, records both
    inputs, averages repeats sample-synchronously and imports each channel into REW against
    the stimulus (offline import: no REW Pro needed)."""
    if ctx.rew is None:
        ctx.stage(f"{pname}.rew", "skipped", "REW not reachable")
        return "skipped"
    rig, pc = ctx.rig, ctx.rig["paths"][pname]
    speaker = pc["kind"] == "speaker"
    stim_path = rig["rew"]["stimulus_speaker" if speaker else "stimulus"]
    fs_s, st = wav.read(stim_path)
    if abs(fs_s - ctx.fs) > 0.5:
        raise PolicyError(f"REW stimulus {stim_path} is {fs_s:g} Hz, JACK runs at {ctx.fs:g}")
    sig = st[:, int(rig["rew"].get("stimulus_channel", 1)) - 1]
    peak = 20 * np.log10(np.max(np.abs(sig)))
    outs = [int(pc["out"]), int(pc["ref_out"])]
    if speaker:
        start = sweep_start_hz(sig, fs_s)
        if not np.isfinite(start) or start < float(pc["speaker_min_request_hz"]) * 0.9:
            raise PolicyError(f"REW speaker stimulus starts at {start:.1f} Hz, below the speaker's "
                              f"{pc['speaker_min_request_hz']} Hz (README: make a 20 Hz – 20 kHz one)")
        level = ctx.policy.check(outs, ctx.policy.emit_speaker_dbfs, speaker_stage=True)
    else:
        level = ctx.policy.check(outs, ctx.policy.electrical_level(), speaker_stage=False)
    # scaled to the stage's level so REW's sweep and ac2's see the same drive; the policy
    # has checked that level, and check_stimulus_peak bounds the played peak again below
    gain_db = level - peak
    played_peak = peak + gain_db
    levels.check_stimulus_peak(played_peak, level, speaker_ceiling_dbfs=ctx.policy.speaker_ceiling() if speaker else None)
    g = 10 ** (gain_db / 20)
    reps = int(rig["stages"]["rew"].get("repeats", 1))
    gap = int(3.0 * ctx.fs)
    one = np.concatenate([sig * g, np.zeros(gap)])
    train = np.tile(one, reps)
    amp = levels.peak_amplitude(played_peak) * (1 + 1e-9)
    ports = ctx.path_ports(pname)
    ins = [ports["meas_in"], ports["ref_in"]]
    if ctx.dut is not None:
        ctx.dut.check_free()
    pre = 1.0
    fs, x = jackio.play_record([jackio.Play(ports[r], train, amp) for r in ("out", "ref_out")], ins, pre_s=pre, post_s=0.5,
                               fade_s=0.0, expect_fs=ctx.fs, max_xruns=ctx.max_xruns)
    p0, L = int(pre * fs), len(one)
    segs = [x[p0 - int(0.5 * fs) + i * L: p0 - int(0.5 * fs) + (i + 1) * L] for i in range(reps)]
    n = min(len(s) for s in segs)
    avg = np.mean([s[:n] for s in segs], axis=0)  # same clock, same stimulus: coherent average
    d = ctx.out / pname / "rew"
    d.mkdir(parents=True, exist_ok=True)
    wav.write(d / "rec.wav", fs, avg)
    wav.write(d / "rec_meas.wav", fs, avg[:, :1])
    wav.write(d / "rec_ref.wav", fs, avg[:, 1:2])
    _j(d / "play.json", {"level_dbfs": played_peak, "gain_db": gain_db, "stimulus": stim_path, "file_peak_dbfs": peak,
                         "repeats": reps, "speaker": speaker, "cal_file": cal_file if speaker else None})
    ids = []
    try:
        mid = ctx.rew.import_response(stim_path, str((d / "rec_meas.wav").resolve()), 1, apply_cal=speaker)
        ids.append(mid)
        ctx.rew.put(f"/measurements/{mid}", {"title": f"xc {pname} meas"})
        ctx.rew.fetch(mid, d, "meas", spl=speaker)
        rid = ctx.rew.import_response(stim_path, str((d / "rec_ref.wav").resolve()), 1, apply_cal=False)
        ids.append(rid)
        ctx.rew.fetch(rid, d, "ref")
    finally:
        if not rig["rew"].get("keep_measurements", False):
            for i in ids:
                try:
                    ctx.rew.delete(f"/measurements/{i}")
                except RewError:
                    pass
    return d


# ---------------------------------------------------------------- ac2 sweeps and TF


def _sweep_config(ctx: Ctx, name: str) -> dict:
    for m in ctx.ac2.meas_list():
        if m["config"]["name"] == name:
            return m["config"]["kind"]["config"]
    raise RuntimeError(f"ac2: measurement {name} not found after creating it")


def ac2_sweep_once(ctx: Ctx, pname: str, vname: str, frm: str, to: str, duration: float, repeats: int,
                   lf_harmonics: str = "standard") -> dict:
    rig, pc = ctx.rig, ctx.rig["paths"][pname]
    speaker = pc["kind"] == "speaker"
    outs = [int(pc["out"]), int(pc["ref_out"])]
    level = ctx.policy.emit_speaker_dbfs if speaker else ctx.policy.electrical_level()
    level = ctx.policy.check(outs, level, speaker_stage=speaker)
    d = ctx.out / pname / "ac2_sweep" / vname
    d.mkdir(parents=True, exist_ok=True)
    name = f"xc-{pname}-{vname}"
    ctx.ac2.meas_rm(name)
    ctx.ac2.run("meas", "new", "sweep", "--name", name, "--ref", pc["ref_in"], "--meas", pc["meas_in"],
                "--out", ",".join(map(str, outs)), "--level", f"{level:g}dbfs", "--from", frm, "--to", to,
                "--duration", f"{duration:g}s", "--repeats", repeats, "--lf-harmonics", lf_harmonics,
                *_resolution_args(ctx))
    cfg = _sweep_config(ctx, name)
    start = float(cfg["sweep"]["start"])
    if abs(float(cfg["level"]) - level) > 1e-6 or sorted(cfg["outputs"]) != sorted(o - 1 for o in outs):
        ctx.ac2.meas_rm(name)
        raise PolicyError(f"ac2 stored {cfg} for {name}, not what was asked")
    if speaker and start < float(pc["speaker_min_emit_hz"]):
        ctx.ac2.meas_rm(name)
        raise PolicyError(f"ac2 would start the speaker sweep at {start:g} Hz, below speaker_min_emit_hz "
                          f"{pc['speaker_min_emit_hz']}")
    sc = rig["stages"]["ac2_sweep"]
    lead = float(sc["noise_lead_seconds"]) + duration
    tail = float(cfg.get("tail") or 1.0)
    rec_s = lead + repeats * (duration + tail + 2.0) + 15.0
    ins = [int(pc["meas_in"]), int(pc["ref_in"])]
    if ctx.patch is not None:
        # a hardware capture summed into a patched input would silently corrupt the sweep
        ctx.patch.verify_sources()
    rec = ctx.ac2.rec_start(ins, rec_s, f"{name}-{time.strftime('%Y%m%dT%H%M%S')}")
    try:
        time.sleep(lead)  # silence before the sweep: the raw file's noise windows
        ev = ctx.ac2.foreground(["sweep", "run", name, "--name", f"{vname}"], "done",
                                deadline=repeats * (duration + tail) + 120.0, events_path=d / "events.jsonl")
        time.sleep(0.5)
    finally:
        ctx.ac2.rec_stop()
    armed = next((e for e in ev if e.get("event") == "armed"), {})
    done = next(e for e in ev if e.get("event") == "done")
    _j(d / "done.json", done)
    ctx.ac2.export_trace(done["trace"], d / "trace.csv")
    src = Path(rec["path"])
    if src.exists():
        shutil.copyfile(src, d / "rec.wav")
        if src.with_suffix(".json").exists():
            shutil.copyfile(src.with_suffix(".json"), d / "rec.json")
    inputs = list(rec["inputs"])
    _j(d / "info.json", {"level_dbfs": level, "from_hz": start, "to_hz": float(cfg["sweep"]["end"]),
                         "duration_s": duration, "repeats": repeats, "fade_in_s": cfg["sweep"].get("fade_in"),
                         "lf_harmonics": cfg.get("lf_harmonics"),
                         "rec_columns": [inputs.index(int(pc["meas_in"]) - 1), inputs.index(int(pc["ref_in"]) - 1)],
                         "armed": armed, "measurement": name})
    return done


def _trace_floor(d: Path, band) -> float:
    from .formats import read_ac2_csv
    b = read_ac2_csv(d / "trace.csv").freq
    m = (b["freq_hz"] >= band[0]) & (b["freq_hz"] <= band[1]) & np.isfinite(b.get("h2_floor_db", np.array([np.nan])))
    return float(np.nanmedian(b["h2_floor_db"][m])) if m.any() else float("nan")


def ac2_sweeps(ctx: Ctx, pname: str):
    sc = ctx.rig["stages"]["ac2_sweep"]
    for v in sc[pname]:
        dur, reps = float(v["duration"]), int(v.get("repeats", 1))
        if not v.get("plan"):
            ac2_sweep_once(ctx, pname, v["name"], v["from"], v["to"], dur, reps, v.get("lf_harmonics", "standard"))
            ctx.manifest["paths"][pname].setdefault("primary_sweep", v["name"])
            continue
        # SNR plan: a probe run, then repeats (+10·log10 N) and length (+10·log10 T) to the target
        probe = f"{v['name']}-probe"
        ac2_sweep_once(ctx, pname, probe, v["from"], v["to"], dur, reps)
        band = sc["floor_band_hz"][pname]
        f0 = _trace_floor(ctx.out / pname / "ac2_sweep" / probe, band)
        target = float(sc["target_floor_dbr"][pname])
        need = (f0 - target) if np.isfinite(f0) else 0.0
        n = int(min(int(sc["max_repeats"]), max(1, math.ceil(10 ** (max(need, 0) / 10)))))
        rest = need - 10 * math.log10(n)
        T = min(float(sc["max_duration"]), dur * 10 ** (max(rest, 0) / 10))
        budget = float(sc["budget_seconds"][pname])
        while n * (T + 2.0) > budget and T > dur:
            T = max(dur, T * 0.8)
        while n * (T + 2.0) > budget and n > 1:
            n -= 1
        pred = f0 - 10 * math.log10(n) - 10 * math.log10(T / dur) if np.isfinite(f0) else float("nan")
        plan = {"probe_floor_dbr": f0, "floor_band_hz": band, "target_floor_dbr": target, "repeats": n,
                "duration_s": T, "predicted_floor_dbr": pred, "reaches_target": bool(pred <= target),
                "budget_s": budget}
        ac2_sweep_once(ctx, pname, v["name"], v["from"], v["to"], round(T, 2), n)
        plan["achieved_floor_dbr"] = _trace_floor(ctx.out / pname / "ac2_sweep" / v["name"], band)
        _j(ctx.out / pname / "ac2_sweep" / v["name"] / "plan.json", plan)


def ac2_tf(ctx: Ctx, pname: str):
    rig, pc = ctx.rig, ctx.rig["paths"][pname]
    speaker = pc["kind"] == "speaker"
    outs = [int(pc["out"]), int(pc["ref_out"])]
    level = ctx.policy.emit_speaker_dbfs if speaker else ctx.policy.electrical_level()
    level = ctx.policy.check(outs, level, speaker_stage=speaker)
    sc = rig["stages"]["ac2_tf"]
    # Pink noise peaks well above its level (ac2 refuses a level whose peaks would clip); a live
    # TF judged where γ² ≥ 0.99 needs no more than this, so hot electrical runs cap it here.
    level = min(level, float(sc.get("max_level_dbfs", -20.0)))
    d = ctx.out / pname / "ac2_tf"
    d.mkdir(parents=True, exist_ok=True)
    name = f"xc-{pname}-tf"
    ctx.ac2.meas_rm(name)
    ctx.ac2.run("meas", "new", "tf", "--name", name, "--ref", pc["ref_in"], "--meas", pc["meas_in"],
                "--blocks", sc["blocks"], *_resolution_args(ctx), "--start")
    settle = float(sc["settle_seconds"])
    ins = [int(pc["meas_in"]), int(pc["ref_in"])]
    cap = {}
    hp = ["--hp", f"{pc['speaker_min_request_hz']:g}hz"] if speaker else []
    # A path with flight time (the speaker's metres of air) is measured as an operator would:
    # the delay finder's first arrival inserted, then a full settle on the aligned windows.
    # Uncompensated, the delay decorrelates each MTW window's ends and biases |H| low.
    find_at = float(sc.get("delay_find_after_seconds", 6.0)) if speaker else 0.0
    found = {}

    def find_delay():
        r = ctx.ac2.run("delay", "find", name, "--insert", check=False)
        found.update(r if isinstance(r, dict) else {"result": r})

    rec = ctx.ac2.rec_start(ins, find_at + settle + 30, f"{name}-{time.strftime('%Y%m%dT%H%M%S')}")
    try:
        ctx.ac2.foreground(["gen", "pink", "--out", ",".join(map(str, outs)), "--level", f"{level:g}dbfs", *hp],
                           None, deadline=find_at + settle + 60, events_path=d / "events.jsonl",
                           hold_s=find_at + settle, during=(find_at, find_delay) if speaker else None,
                           before_stop=lambda: cap.update(ctx.ac2.capture(name, f"{name}-cap") or {}))
    finally:
        ctx.ac2.run("gen", "stop", json_out=False, check=False)
        ctx.ac2.rec_stop()
        ctx.ac2.run("meas", "stop", name, json_out=False, check=False)
    tid = cap.get("id") or cap.get("trace") or f"{name}-cap"
    ctx.ac2.export_trace(tid, d / "trace.csv")
    src = Path(rec["path"])
    if src.exists():
        shutil.copyfile(src, d / "rec.wav")
    inputs = list(rec["inputs"])
    _j(d / "info.json", {"level_dbfs": level, "settle_s": settle, "blocks": sc["blocks"], "hp": hp,
                         "delay_find": found or None,
                         "rec_columns": [inputs.index(int(pc["meas_in"]) - 1), inputs.index(int(pc["ref_in"]) - 1)]})


# ---------------------------------------------------------------- ambient


def _pw_active(rig) -> bool:
    return subprocess.run(rig["pipewire"]["is_active"], capture_output=True, text=True).stdout.strip() == "active"


# ac2's RTA FIFO holds at most this many frames (SpecAveraging::MAX_RTA_FIFO_FRAMES).
RTA_FIFO_MAX = 65536


def rta_average(window_s: float) -> str:
    """`--average` for an RTA that must not drop a frame over a run of `window_s` plus the
    stage's setup and teardown (a minute of margin): frames at 90 per second, above the
    daemon's 60 result intervals per second."""
    frames = int(min(RTA_FIFO_MAX, max(1, round((window_s + 60.0) * 90.0))))
    return f"fifo:{frames}"


def ambient_stage(ctx: Ctx, cal: dict, cal_file: str | None):
    """No emission: the mic input's ambient level by ac2's SPL meters (Z, A, C), REW's SPL
    meters through PipeWire, both RTAs, and a JACK recording of the same seconds."""
    import jack

    rig = ctx.rig
    secs = float(rig["stages"]["ambient"]["seconds"])
    d = ctx.out / "ambient"
    d.mkdir(parents=True, exist_ok=True)
    mic_in = int(rig["ac2"]["mic_input"])
    started_pw = False
    restore_conn = []
    procs = []
    client = None
    rew_ok = ctx.rew is not None
    try:
        if rew_ok and not _pw_active(rig):
            for c in rig["pipewire"]["start"]:
                subprocess.run(c, check=True, timeout=60)
            started_pw = True
        client = jack.Client("crosscheck-wire", no_start_server=True)
        pwc = rig["pipewire"]["client"]
        if rew_ok:
            t0 = time.monotonic()
            while not client.get_ports(f"{pwc}:capture_FL") and time.monotonic() - t0 < 20:
                time.sleep(0.5)
            for k in rig["pipewire"]["ambient_capture"]:
                dst = f"{pwc}:{k}"
                for src in client.get_all_connections(dst):
                    restore_conn.append((src.name, dst))
                    client.disconnect(src.name, dst)
            for k, src in rig["pipewire"]["ambient_capture"].items():
                client.connect(src, f"{pwc}:{k}")
            ctx.rew.set_input_cal(cal["rew_dbfs_at_94"], cal.get("full_scale_vrms", 1.0), cal_file or "")
            # One weighting per open meter, A first: the weighting a PA report quotes.
            rew_meters = list(zip(ctx.rew.spl_meters(), ("A", "C", "Z")))
            if len(rew_meters) < 3:
                ctx.manifest.setdefault("notes", []).append(
                    f"ambient: REW has {len(rew_meters)} SPL meter(s); compared weightings "
                    + ",".join(w for _, w in rew_meters))
            for m, w in rew_meters:
                ctx.rew.put(f"/spl-meter/{m}/configuration", {"splWeighting": w, "leqWeighting": w, "selWeighting": w,
                                                              "filter": "Slow", "showLeq": True, "rollingLeqActive": False})
            try:
                modes = ctx.rew.get("/rta/configuration/modes") or []
                mode = next((x for x in modes if "1/3" in x), None)
                avg = next((x for x in (ctx.rew.get("/rta/configuration/averaging-choices") or []) if "orever" in x), None)
                cfg = {**({"mode": mode} if mode else {}), **({"averaging": avg} if avg else {})}
                if cfg:
                    ctx.rew.put("/rta/configuration", cfg)
            except RewError as e:
                ctx.manifest.setdefault("notes", []).append(f"ambient: REW RTA config: {e}")
        name = "xc-ambient-rta"
        ctx.ac2.meas_rm(name)
        # The RTA must read the window's power mean, as numpy's column and an Leq do: every
        # moment counted equally. A FIFO longer than the whole run (an RTA frame is one result
        # interval, at most ~60 per second) never drops a frame, so it is the duration-weighted
        # power mean from the start to the capture: the window plus the few seconds of the
        # same room around it. An exponential with tau ~ window/3 would weight the last third
        # ~63 %, an estimate of the end of the window rather than its mean.
        rta_avg = rta_average(secs)
        ctx.ac2.run("meas", "new", "rta", "--name", name, "--input", mic_in, "--fraction", "3", "--from", "20hz",
                    "--to", "20khz", "--weight", "z", "--average", rta_avg, "--start")
        starts = {}
        for w in ("z", "a", "c"):
            argv = ctx.ac2.argv("spl", "watch", "--input", mic_in, "--weight", w, "--time", "slow", "--for",
                                f"{secs:g}s", "--json")
            f = open(d / f"ac2_spl_{w}.jsonl", "w")
            procs.append((subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=f, stderr=subprocess.PIPE, text=True), f))
            starts[f"ac2_{w}"] = time.time()
        if rew_ok:
            for m, _ in rew_meters:
                ctx.rew.command(f"/spl-meter/{m}/command", "Reset")
                ctx.rew.command(f"/spl-meter/{m}/command", "Start")
            ctx.rew.command("/rta/command", "Start")
            ctx.rew.command("/rta/command", "Reset averaging")
            starts["rew"] = time.time()
        starts["jack"] = time.time()
        fs, x = jackio.play_record([], [ctx.in_port(mic_in)], pre_s=secs, post_s=0.0, expect_fs=ctx.fs, max_xruns=ctx.max_xruns)
        wav.write(d / "in1.wav", fs, x)
        if rew_ok:
            for m, w in rew_meters:
                _j(d / f"rew_spl_{w}.json", ctx.rew.get(f"/spl-meter/{m}/levels"))
            _j(d / "rew_rta.json", ctx.rew.get("/rta/captured-data?unit=SPL"))
            for m, _ in rew_meters:
                ctx.rew.command(f"/spl-meter/{m}/command", "Stop")
            ctx.rew.command("/rta/command", "Stop")
        for p, f in procs:
            try:
                p.wait(15)
            except subprocess.TimeoutExpired:
                p.stdin.write("q\n")
                p.stdin.close()
                p.wait(5)
            f.close()
        cap = ctx.ac2.capture(name, f"{name}-cap") or {}
        ctx.ac2.export_trace(cap.get("id") or f"{name}-cap", d / "ac2_rta.csv")
        ctx.ac2.run("meas", "stop", name, json_out=False, check=False)
        _j(d / "window.json", {"seconds": secs, "starts_unix": starts,
                               "skew_s": max(starts.values()) - min(starts.values()),
                               "ac2_rta_average": rta_avg})
    finally:
        for p, f in procs:
            if p.poll() is None:
                p.terminate()
            f.close()
        if client is not None:
            pwc = rig["pipewire"]["client"]
            for k in rig["pipewire"]["ambient_capture"]:
                dst = f"{pwc}:{k}"
                try:
                    for src in client.get_all_connections(dst):
                        client.disconnect(src.name, dst)
                except jack.JackError:
                    pass
            # put back what was wired before; the config's normal wiring if nothing was
            back = restore_conn or [(src, f"{pwc}:{k}") for k, src in rig["pipewire"]["normal_capture"].items()
                                    if k in rig["pipewire"]["ambient_capture"]]
            for src, dst in back:
                try:
                    client.connect(src, dst)
                except jack.JackError:
                    pass
            client.close()
        if started_pw:
            for c in rig["pipewire"]["stop"]:
                subprocess.run(c, timeout=60)
