"""Repeated takes of one stage at one level: each check's signed value across takes.

A single take's residual mixes a bias with that take's own variance (the room's noise, its
drift between captures); only the spread over takes with unchanged geometry tells them apart.
Each check is summarised by the count, mean, sample standard deviation, range, whether the
takes' range and the mean's 2-standard-error interval contain zero, and in how many takes
|value| exceeded the pass limit.

The value taken is the signed one: a per-band pair check's `mean` (its stored value is
max(|mean|, spread), which has no sign; its spread is shown beside it), otherwise the check's
value, which for some checks is an absolute by construction (a largest |difference|).

The column-to-tone split: a measurement X read at a sine's frequency differs from the steady
sine by
    (X − C column) + (C column − C narrow) + (C narrow − sine)
where C is the direct estimate of the capture X itself was computed from, read on the
1/48-octave columns and narrow at the sine's own frequency. The first term is the
measurement's processing, the second the room's structure inside a column (support, not
error), the third what is left between the capture and the sine taken at another time.
"""
from __future__ import annotations

import math
import re
from pathlib import Path

from . import baseline as bl

NARROW = ", narrow at the sine"
# a measurement and the raw capture it was computed from
_CAPTURE_OF = {"ac2 TF": "direct (TF capture)", "REW offline": "direct (REW recording)"}
_SINE = re.compile(r"^(?P<stage>[^.]+)\.(?P<q>sine_mag|sine_phase)\.(?P<src>.+)@(?P<f>[\d.]+)Hz$")


def capture_of(src: str) -> str | None:
    if src in _CAPTURE_OF:
        return _CAPTURE_OF[src]
    if src.startswith("ac2 sweep "):
        return f"direct (ac2 capture {src[len('ac2 sweep '):]})"
    return None


# ------------------------------------------------------------------ loading
def baselines_from(path: Path) -> list[dict]:
    """A run directory (every stage in its results) or a stored baseline file, as baseline dicts."""
    path = Path(path)
    if path.is_file():
        import json
        return [json.loads(path.read_text())]
    res = bl.load_results(path)
    return [bl.make_baseline(res, st, path.name) for st in bl.stages_of(res)]


def resolution(b: dict) -> int | None:
    """A take's column resolution (per octave); None for a stage without ac2 columns."""
    if b["stage"] == "ambient" or b["stage"] in bl.HOST_STAGES:
        return None
    return b.get("resolution_ppo", bl.DEFAULT_PPO)


def group(baselines: list[dict]) -> dict[tuple[str, float | None, int | None], list[dict]]:
    """Takes by (stage, level, resolution): a residual depends on each (a column averages the
    room's structure over its width), so takes are only pooled within one.
    One run counts once (a run directory and its stored baseline are the same take); the copy
    with more checks is kept."""
    out: dict[tuple[str, float | None, int | None], dict] = {}
    for i, b in enumerate(baselines):
        run = (b.get("provenance") or {}).get("run") or f"#{i}"
        g = out.setdefault((b["stage"], b.get("level_dbfs"), resolution(b)), {})
        if run not in g or len(b["checks"]) > len(g[run]["checks"]):
            g[run] = b
    return {k: list(v.values()) for k, v in out.items()}


# ------------------------------------------------------------------ statistics
def signed(e: dict) -> float | None:
    v = e.get("mean", e.get("value"))
    return v if isinstance(v, (int, float)) and math.isfinite(v) else None


def stats(xs: list[float], tol: list[float] | None = None) -> dict:
    n = len(xs)
    if not n:
        return {"n": 0}
    m = sum(xs) / n
    sd = math.sqrt(sum((x - m) ** 2 for x in xs) / (n - 1)) if n > 1 else None
    s = {"n": n, "mean": m, "sd": sd, "min": min(xs), "max": max(xs),
         "range_has_zero": min(xs) <= 0 <= max(xs)}
    # mean ± 2 standard errors: whether the takes so far tell a bias from zero
    s["mean_2se_has_zero"] = None if sd is None else abs(m) <= 2 * sd / math.sqrt(n)
    if n < 2:
        s["range_has_zero"] = None
    if tol:
        s["over_pass"] = sum(abs(x) > tol[0] for x in xs)
    return s


def per_check(takes: list[dict]) -> dict[str, dict]:
    """key → stats of its signed values over the takes, with unit, limit and statuses."""
    keys = sorted({k for b in takes for k in b["checks"]})
    out = {}
    for k in keys:
        es = [b["checks"][k] for b in takes if k in b["checks"]]
        tol = next((e["tol"] for e in es if e.get("tol")), None)
        xs = [x for x in (signed(e) for e in es) if x is not None]
        s = stats(xs, tol)
        s.update(unit=es[0].get("unit") or "", tol=tol, quantity="mean" if any("mean" in e for e in es) else "value",
                 statuses=[e["status"] for e in es], spread=[e["spread"] for e in es if "spread" in e])
        out[k] = s
    return out


def split(takes: list[dict]) -> list[dict]:
    """Per (stage, quantity, tone, measurement): the three terms of the column-to-tone split,
    each with its stats over the takes."""
    rows = []
    found: dict[tuple, dict] = {}
    for b in takes:
        for k, e in b["checks"].items():
            m = _SINE.match(k)
            if m:
                found.setdefault((m["stage"], m["q"], m["f"]), {}).setdefault(m["src"], []).append((b, e))
    for (stage, q, f), by in sorted(found.items(), key=lambda t: (t[0][0], t[0][1], float(t[0][2]))):
        for src in sorted(by):
            if src.endswith(NARROW) or src.startswith("direct ("):
                continue
            cap = capture_of(src)
            if cap is None or cap not in by or cap + NARROW not in by:
                continue
            terms = {"total": [], "processing": [], "column_to_tone": [], "capture_vs_sine": []}
            for b in takes:
                ck = lambda s: b["checks"].get(f"{stage}.{q}.{s}@{f}Hz")
                x, c, nw = (signed(ck(s)) if ck(s) else None for s in (src, cap, cap + NARROW))
                if None in (x, c, nw):
                    continue
                terms["total"].append(x)
                terms["processing"].append(x - c)
                terms["column_to_tone"].append(c - nw)
                terms["capture_vs_sine"].append(nw)
            if terms["total"]:
                rows.append({"stage": stage, "quantity": q, "f": float(f), "source": src, "capture": cap,
                             **{t: stats(v) for t, v in terms.items()}})
    return rows


# ------------------------------------------------------------------ markdown
def _n(x, d=3):
    return "" if x is None else f"{x:+.{d}f}"


def _yn(x):
    return "" if x is None else ("yes" if x else "no")


def _check_table(pc: dict[str, dict], keys: list[str]) -> list[str]:
    lines = ["| check | unit | takes | mean | sd | min | max | 0 in range | 0 in mean±2se | >pass | spread | statuses |",
             "|---|---|---|---|---|---|---|---|---|---|---|---|"]
    for k in keys:
        s = pc[k]
        q = " (signed mean)" if s["quantity"] == "mean" else ""
        over = "" if s.get("over_pass") is None else f"{s['over_pass']}/{s['n']} (>{s['tol'][0]:g})"
        sts = ", ".join(f"{v}×{t}" for t, v in sorted({t: s["statuses"].count(t) for t in s["statuses"]}.items()))
        if not s["n"]:
            lines.append(f"| {k}{q} | {s['unit']} | 0 | | | | | | | | | {sts} |")
            continue
        sd = "" if s["sd"] is None else f"{s['sd']:.3f}"
        # a pair check's largest deviation from its own mean, the worst over the takes
        spread = f"≤ {max(s['spread']):.3f}" if s["spread"] else ""
        lines.append(f"| {k}{q} | {s['unit']} | {s['n']} | {_n(s['mean'])} | {sd} "
                     f"| {_n(s['min'])} | {_n(s['max'])} | {_yn(s['range_has_zero'])} | {_yn(s['mean_2se_has_zero'])} "
                     f"| {over} | {spread} | {sts} |")
    return lines


FOCUS = (("Steady sines: magnitude", r"\.sine_mag\."), ("Steady sines: phase", r"\.sine_phase\."),
         ("Live TF vs its direct estimate, per band", r"\.(mag|phase)\.ac2 TF\|"),
         ("Arrival and delay", r"\.delay\."))


def render(takes: list[dict], match: str | None = None) -> str:
    stage, level, ppo = takes[0]["stage"], takes[0].get("level_dbfs"), resolution(takes[0])
    runs = [((b.get("provenance") or {}).get("run") or "?") for b in takes]
    builds = sorted({str((b.get("provenance") or {}).get("ac2_build")) for b in takes})
    md = [f"## {stage}" + ("" if level is None else f" at {level:g} dBFS")
          + ("" if ppo is None else f", 1/{ppo} octave") + f": {len(takes)} takes", "",
          f"Runs: {', '.join(runs)}. ac2 builds: {', '.join(builds)}.", "",
          "Signed values across takes: sd is the sample standard deviation (n − 1); `0 in mean±2se` "
          "is whether the mean lies within two standard errors of zero (no bias shown by these takes); "
          "`>pass` counts takes with |value| above the pass limit.", ""]
    pc = per_check(takes)
    sections = ((f"Checks matching `{match}`", match),) if match else FOCUS
    for title, rx in sections:
        keys = [k for k in pc if re.search(rx, k)]
        if keys:
            md += [f"### {title}", ""] + _check_table(pc, keys) + [""]
    if not match:
        rows = split(takes)
        if rows:
            md += ["### Column-to-tone split at the sines", "",
                   f"measurement − sine = processing (measurement − its capture's 1/{ppo}-oct column) + "
                   "column-to-tone (column − the same capture narrow at the sine) + capture vs sine "
                   "(narrow − sine). Mean ± sd over the takes.", "",
                   "| resolution | quantity | f Hz | measurement | capture | takes | total | processing | column-to-tone "
                   "| capture vs sine |",
                   "|---|---|---|---|---|---|---|---|---|---|"]
            for r in rows:
                cell = lambda s: _n(s["mean"]) + ("" if s.get("sd") is None else f" ± {s['sd']:.3f}")
                md.append(f"| 1/{ppo} | {r['quantity']} | {r['f']:g} | {r['source']} | {r['capture']} | {r['total']['n']} | "
                          f"{cell(r['total'])} | {cell(r['processing'])} | {cell(r['column_to_tone'])} | "
                          f"{cell(r['capture_vs_sine'])} |")
            md.append("")
    return "\n".join(md)


def report(sources: list[Path], match: str | None = None) -> str:
    bs = [b for s in sources for b in baselines_from(s)]
    groups = group(bs)
    md = ["# Repeated takes", ""]
    for _, takes in sorted(groups.items(), key=lambda t: (t[0][0], t[0][1] or 0.0, t[0][2] or 0)):
        md.append(render(takes, match))
    return "\n".join(md) + "\n"
