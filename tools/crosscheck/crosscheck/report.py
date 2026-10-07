"""report.md, results.json and PNG plots from an Analysis. Plots need matplotlib; without
it the report says so and the numbers are unaffected."""
from __future__ import annotations

import json
from pathlib import Path

import numpy as np

STATUS_ORDER = ["FAIL", "INCONCLUSIVE", "WARN", "PASS", "INFO"]

HOW_TO_READ = """\
## How to read this

- **PASS / WARN / FAIL**: `|value|` against the two limits in `tolerances.toml` (pass, warn).
  Each row says what the number is and what it is compared with.
- **INCONCLUSIVE**: the comparison rests on an upper bound — a harmonic reading below its
  floor + margin (`[distortion] margin_db`) — or a value is missing. The row gives the
  shortfall: how many dB more SNR would have made it a value. Never read it as a pass.
- **INFO**: printed for context, no tolerance.
- Levels are dBFS in the full-scale-sine convention ac2 and REW share (a full-scale sine is
  0 dBFS, rms = 10^(L/20)/√2). Harmonics are dBr re the fundamental at the measurement input.
- *Steady sine* rows are the ground truth: one tone at a time, Blackman window, phase of
  meas÷ref, floors from a silent recording analysed in the same bins.
- *direct* rows are numpy cross-spectra of the raw recordings themselves (the same samples
  an app analysed), so a difference there is the app's analysis, not the take.
"""


def _jsonable(x):
    if isinstance(x, dict):
        return {str(k): _jsonable(v) for k, v in x.items()}
    if isinstance(x, (list, tuple)):
        return [_jsonable(v) for v in x]
    if isinstance(x, np.ndarray):
        return _jsonable(x.tolist())
    if isinstance(x, (np.floating, float)):
        v = float(x)
        return v if np.isfinite(v) else None
    if isinstance(x, (np.integer,)):
        return int(x)
    if isinstance(x, complex):
        return [x.real, x.imag]
    if isinstance(x, Path):
        return str(x)
    return x


def _md_table(cols, rows):
    out = ["| " + " | ".join(str(c) for c in cols) + " |", "|" + "---|" * len(cols)]
    for r in rows:
        out.append("| " + " | ".join(str(c).replace("|", "\\|") for c in r) + " |")
    return "\n".join(out)


def _val(c):
    if c["value"] is None:
        return "—"
    v = c["value"]
    if c["unit"] == "rel":
        return f"{v * 100:+.1f} %"
    return f"{v:+.3g} {c['unit']}" if abs(v) < 1000 else f"{v:+.0f} {c['unit']}"


def _tol(c):
    t = c["tol"]
    if not t:
        return ""
    if c["unit"] == "rel":
        return f"{t[0] * 100:g} / {t[1] * 100:g} %"
    return f"{t[0]:g} / {t[1]:g}"


def write(results: dict, analysis, out: Path, plots: bool = True) -> Path:
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "results.json").write_text(json.dumps(_jsonable(results), indent=1, ensure_ascii=False))
    pngs, why = ([], "plots off") if not plots else _plots(analysis, out)
    md = ["# ac2 cross-check report", "", f"Source: `{results['source']}`"
          + (" (2026-10-07 fixtures)" if results.get("fixture") else ""), ""]
    man = results.get("manifest") or {}
    for k in ("started", "rig", "ac2_version", "rew_version", "flags"):
        if k in man:
            md.append(f"- {k}: `{json.dumps(man[k], ensure_ascii=False) if not isinstance(man[k], str) else man[k]}`")
    md += ["", "## Summary", "",
           _md_table(["status", "count"], [[s, results["summary"].get(s, 0)] for s in STATUS_ORDER]), ""]
    bad = [c for c in results["checks"] if c["status"] in ("FAIL", "WARN")]
    if bad:
        md += ["Failures and warnings first:", "",
               _md_table(["status", "check", "value", "pass / warn"],
                         [[c["status"], c["title"] + f" ({c['path']})", _val(c), _tol(c)]
                          for c in sorted(bad, key=lambda c: STATUS_ORDER.index(c["status"]))]), ""]
    md += [HOW_TO_READ]
    stages = man.get("stages")
    if stages:
        md += ["## Stages", "", _md_table(["stage", "outcome", "detail"],
                                          [[k, v.get("outcome", ""), v.get("detail", "")] for k, v in stages.items()]), ""]
    cm = man.get("cal_mapping")
    if cm:
        md += ["## Calibration mapping (ac2 store → REW)", "", _md_table(["quantity", "ac2", "REW", "note"], cm), ""]
    groups: dict[str, list] = {}
    for c in results["checks"]:
        groups.setdefault(c["group"], []).append(c)
    md.append("## Checks")
    for g, cs in groups.items():
        md += ["", f"### {g}", "", _md_table(["status", "path", "check", "value", "pass / warn", "what it means"],
                                              [[c["status"], c["path"], c["title"], _val(c), _tol(c), c["meaning"]]
                                               for c in cs])]
    md += ["", "## Tables"]
    for name, t in results["tables"].items():
        md += ["", f"### {name}", ""]
        if t.get("note"):
            md += [t["note"], ""]
        md.append(_md_table(t["columns"], t["rows"]))
    if results.get("notes"):
        md += ["", "## Notes", ""] + [f"- {n}" for n in results["notes"]]
    md += ["", "## Plots", ""]
    md += [f"![{p.stem}]({p.name})" for p in pngs] if pngs else [f"none ({why})"]
    path = out / "report.md"
    path.write_text("\n".join(md) + "\n")
    return path


def _plots(a, out: Path):
    try:
        import matplotlib
        matplotlib.use("Agg")
        import matplotlib.pyplot as plt
    except ImportError:
        return [], "matplotlib not installed"
    pngs = []

    def save(fig, name):
        p = out / f"{name}.png"
        fig.tight_layout()
        fig.savefig(p, dpi=110)
        plt.close(fig)
        pngs.append(p)

    for pname, s in a.series.items():
        if not isinstance(s, dict) or "src" not in s:
            continue
        f, src = s["f"], s["src"]
        keys = list(src)
        base = src[keys[0]]
        fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(10, 7), sharex=True)
        from . import dsp
        coh = getattr(a, "coh_by_path", {}).get(pname, {})
        for k in keys[1:]:
            r = src[k] / base
            ok = np.isfinite(r) & (np.abs(r) > 0)
            if k in coh:
                ok &= np.nan_to_num(coh[k]) >= 0.99
            hi = ok & (f >= 1000) & (f <= 20000)
            if hi.sum() > 4:
                # a delay difference (timing reference, IR window start) is not a response difference
                tau = dsp.delay_from_phase(f[hi], r[hi], 1000, 20000)
                r = r * np.exp(2j * np.pi * f * tau)
            if ok.sum() < 2:
                continue
            r = np.where(ok, r, np.nan)  # gaps stay gaps
            m = 20 * np.log10(np.abs(r))
            ax1.semilogx(f, m - np.nanmedian(m), label=k, lw=0.8)
            ax2.semilogx(f, np.rad2deg(np.angle(r)), lw=0.8)
        ax1.set_ylabel(f"dB re {keys[0]} (median removed)")
        ax1.set_ylim(-0.5, 0.5)
        ax1.legend(fontsize=7)
        ax2.set_ylabel("phase difference °")
        ax2.set_ylim(-5, 5)
        ax2.set_xlabel("Hz")
        for ax in (ax1, ax2):
            ax.grid(True, which="both", alpha=0.3)
        fig.suptitle(f"{pname}: sources vs {keys[0]} (delay difference removed, TF at γ² ≥ 0.99)")
        save(fig, f"{pname}_response_diff")
        if "gd" in s:
            g = s["gd"]
            fig, ax = plt.subplots(figsize=(9, 5))
            ax.loglog(g["fc"], np.asarray(g["truth"]) * 1e6, "ko", label="steady sine")
            for k, v in g["ests"].items():
                v = np.asarray(v) * 1e6
                ok = v > 0
                ax.loglog(np.asarray(g["fc"])[ok], v[ok], ".-", lw=0.8, label=k)
            ax.set_xlabel("Hz")
            ax.set_ylabel("group delay µs")
            ax.grid(True, which="both", alpha=0.3)
            ax.legend(fontsize=7)
            ax.set_title(f"{pname}: group delay")
            save(fig, f"{pname}_group_delay")
        if "etc" in s:
            e = s["etc"]
            fig, ax = plt.subplots(figsize=(9, 5))
            ax.plot(np.asarray(e["t"]) * 1e3, e["ac2"], lw=0.8, label="ac2")
            ax.plot(np.asarray(e["t"]) * 1e3, e["rew"], lw=0.8, label=e["rew_label"])
            ax.set_xlim(-5, 100)
            ax.set_ylim(-100, 5)
            ax.set_xlabel("ms re arrival")
            ax.set_ylabel("dB re peak")
            ax.grid(True, alpha=0.3)
            ax.legend(fontsize=7)
            ax.set_title(f"{pname}: ETC")
            save(fig, f"{pname}_etc")
    for pname, p in a.run.paths.items():
        sw = {n: x for n, x in p.sweeps.items() if "h2_db" in x.trace.freq}
        if not sw:
            continue
        fig, ax = plt.subplots(figsize=(10, 5))
        for n, x in sw.items():
            b = x.trace.freq
            l, = ax.semilogx(b["freq_hz"], b["h2_db"], lw=0.8, label=f"{n} H2")
            ax.semilogx(b["freq_hz"], b["h2_floor_db"], ":", lw=0.6, color=l.get_color())
        t = [(x["f"], x["h_dbr"]["2"]) for x in (p.truth or {}).get("tones", []) if x.get("h_dbr") and "2" in x["h_dbr"]]
        if t:
            ax.semilogx(*zip(*t), "ko", label="steady sine H2")
        for rs in (p.rew, p.rew_live):
            if rs is not None and rs.meas_dist is not None and "H2" in rs.meas_dist.cols:
                ax.semilogx(rs.meas_dist.f, rs.meas_dist.cols["H2"], lw=0.8, label=f"{rs.label} H2")
        ax.set_xlim(10, 20000)
        ax.set_ylim(-130, -40)
        ax.set_xlabel("Hz")
        ax.set_ylabel("dBr (dotted: ac2 floor)")
        ax.grid(True, which="both", alpha=0.3)
        ax.legend(fontsize=7)
        ax.set_title(f"{pname}: H2 vs steady sine")
        save(fig, f"{pname}_h2")
    th = a.series.get("ambient.thirds")
    if th:
        fig, ax = plt.subplots(figsize=(9, 5))
        for k, v in th.items():
            if isinstance(v, list) and k != "x":
                ax.semilogx(th["x"], v, ".-", lw=0.8, label=k)
        ax.set_xlabel("Hz")
        ax.set_ylabel("dB SPL")
        ax.grid(True, which="both", alpha=0.3)
        ax.legend(fontsize=7)
        ax.set_title("ambient: third octaves")
        save(fig, "ambient_thirds")
    return pngs, ""
