"""Stored baselines and `compare`: a reviewed run's verdicts and values per (rig, stage, level),
and the differences of a later run against them.

A check's `key` is its id with the tone frequency replaced by the frequency the sine plan
asked for: the planner moves each tone off the mains family by up to a few per cent (50 Hz
may play at 47.8 Hz in one run and 51.275 Hz in the next), so the played frequency is not an
identity. Results written before keys existed get them here, from the run's sine results."""
from __future__ import annotations

import json
import math
import re
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASELINES = ROOT / "baselines"
SCHEMA = 1

# groups whose ids end in a tone frequency (`<...>.<f:g>`)
FREQ_GROUPS = {"phase vs sine", "magnitude vs sine", "group delay", "harmonics", "absolute SPL"}
_FREQ_TAIL = re.compile(r"\.(\d+(?:\.\d+)?)$")
# `avoid_mains` moves a tone by at most 6 %; two nominal tones are never that close
NEAREST_REL = 0.06
# a verdict lost is worse than a pass; INFO is not judged, so it ranks with PASS
RANK = {"PASS": 0, "INFO": 0, "INCONCLUSIVE": 1, "WARN": 2, "FAIL": 3}
# a value is held to its baseline only where both runs judged it: an INFO value is context and
# an INCONCLUSIVE one is a bound or rests on noise wider than its pass limit
JUDGED = {"PASS", "WARN", "FAIL"}
# A host stage needs no rig (it runs the analysers offline on files), so a run of it is one
# stage whatever paths its checks carry (the OSM stage's paths are its cases), it plays nothing
# (no level), and its baseline lives under `baselines/host/`.
HOST_STAGES = {"osm"}
HOST_DIR = "host"


# ------------------------------------------------------------------ keys
def nominal_maps_from_truth(truths: dict[str, dict | None]) -> dict[str, dict[float, float]]:
    """{path: {played f: requested f}} from each path's steady-sine results."""
    out = {}
    for path, t in truths.items():
        m = {}
        for x in (t or {}).get("tones", []):
            req = x.get("f_requested", x.get("f_req"))
            if x.get("f") is not None and req is not None:
                m[float(x["f"])] = float(req)
        out[path] = m
    return out


def nominal_maps_from_run(run_dir: Path) -> dict[str, dict[float, float]]:
    truths = {}
    for p in Path(run_dir).glob("*/sine/results.json"):
        try:
            truths[p.parent.parent.name] = json.loads(p.read_text())
        except (OSError, ValueError):
            pass
    return nominal_maps_from_truth(truths)


def check_key(c: dict, nominal: dict[str, dict[float, float]]) -> tuple[str, float | None, float | None]:
    """(key, played f, nominal f) for a results.json check."""
    if c.get("group") in FREQ_GROUPS:
        m = _FREQ_TAIL.search(c["id"])
        if m:
            f = float(m.group(1))
            fmap = nominal.get(c.get("path"), {})
            fn = next((v for k, v in fmap.items() if math.isclose(k, f, rel_tol=1e-6)), f)
            return f"{c['id'][:m.start()]}@{fn:g}Hz", f, fn
    return c["id"], None, None


def annotate(results: dict, nominal: dict[str, dict[float, float]]) -> dict:
    """Adds key / f_hz / f_nominal_hz to checks that lack them (results written before keys)."""
    for c in results.get("checks", []):
        if "key" not in c:
            c["key"], c["f_hz"], c["f_nominal_hz"] = check_key(c, nominal)
    return results


def load_results(run_dir: Path) -> dict:
    run_dir = Path(run_dir)
    p = run_dir / "report" / "results.json"
    if not p.exists():
        raise FileNotFoundError(f"{p}: no results; run `python -m crosscheck analyse {run_dir}` first")
    return annotate(json.loads(p.read_text()), nominal_maps_from_run(run_dir))


# ------------------------------------------------------------------ stages and levels
def host_stage(results: dict) -> str | None:
    st = (results.get("manifest") or {}).get("stage")
    return st if st in HOST_STAGES else None


def stage_of(results: dict, c: dict) -> str:
    return host_stage(results) or c["path"]


def stages_of(results: dict) -> list[str]:
    seen = []
    for c in results.get("checks", []):
        st = stage_of(results, c)
        if st not in seen:
            seen.append(st)
    return seen


def baseline_rig(results: dict) -> str | None:
    """The baseline directory's name: the rig, or `host` for a stage that needs none."""
    return HOST_DIR if host_stage(results) else (results.get("manifest") or {}).get("rig")


def stage_level(results: dict, stage: str) -> float | None:
    """The digital level a stage played at; None for a silent stage (ambient) or unknown."""
    if stage == "ambient" or stage in HOST_STAGES:
        return None
    man = results.get("manifest") or {}
    flags = man.get("flags") or {}
    kind = ((man.get("paths") or {}).get(stage) or {}).get("kind")
    text = flags.get("emit_speaker") if kind == "speaker" else flags.get("emit") if kind in ("electrical", "digital") else None
    if not text:
        return None
    m = re.match(r"\s*([-+]?\d+(?:\.\d+)?)\s*dbfs\s*$", str(text), re.I)
    return float(m.group(1)) if m else None


def baseline_name(stage: str, level: float | None) -> str:
    if level is None:
        return f"{stage}.json"
    return f"{stage}-{abs(level):g}dbfs.json"


def baseline_path(root: Path, rig: str, stage: str, level: float | None) -> Path:
    return Path(root) / rig / baseline_name(stage, level)


# ------------------------------------------------------------------ baselines
def _num(x):
    if x is None:
        return None
    return float(f"{x:.6g}")


def make_baseline(results: dict, stage: str, run_id: str) -> dict:
    man = results.get("manifest") or {}
    checks = {}
    for c in results["checks"]:
        if stage_of(results, c) != stage:
            continue
        e = {"status": c["status"], "value": _num(c["value"]), "unit": c["unit"],
             "tol": [_num(t) for t in c["tol"]] if c.get("tol") else None}
        if c.get("f_nominal_hz") is not None:
            e["f_hz"] = _num(c["f_hz"])
        checks[c["key"]] = e
    summary = {}
    for e in checks.values():
        summary[e["status"]] = summary.get(e["status"], 0) + 1
    return {"schema": SCHEMA, "rig": man.get("rig"), "stage": stage, "level_dbfs": stage_level(results, stage),
            "provenance": {"run": run_id, "started": man.get("started"), "ac2_build": man.get("ac2_version"),
                           "rew_version": man.get("rew_version"), "osm_version": man.get("osm_version"),
                           "flags": man.get("flags"),
                           "suite_commit": man.get("suite_commit")},
            "summary": summary, "checks": dict(sorted(checks.items()))}


def dumps(b: dict) -> str:
    """One check per line: compact, and a baseline update reads as a line diff."""
    head = {k: v for k, v in b.items() if k != "checks"}
    lines = ["{"]
    for k, v in head.items():
        lines.append(f" {json.dumps(k)}: {json.dumps(v, ensure_ascii=False)},")
    lines.append(' "checks": {')
    items = list(b["checks"].items())
    for i, (k, v) in enumerate(items):
        lines.append(f"  {json.dumps(k, ensure_ascii=False)}: {json.dumps(v, ensure_ascii=False)}"
                     + ("," if i < len(items) - 1 else ""))
    lines += [" }", "}"]
    return "\n".join(lines) + "\n"


def stage_view(results: dict, stage: str) -> dict:
    """A run's stage in the baseline's shape, for diffing."""
    return make_baseline(results, stage, "")


# ------------------------------------------------------------------ diff
def compare_settings(tol: dict) -> tuple[dict[str, float], float]:
    c = tol.get("compare", {})
    return {str(k): float(v) for k, v in c.get("step", {}).items()}, float(c.get("pass_fraction", 0.5))


def step_for(entry: dict, steps: dict[str, float], frac: float) -> float:
    """A value has moved when it changed by more than the unit's step or `frac` of the check's
    pass limit, whichever is larger: a reading whose limit is wide (a harmonic vs the sine,
    within 3 dB) is that much noisier take to take than a band mean held to 0.03 dB."""
    s = steps.get(entry.get("unit") or "", steps.get("", 0.0))
    if entry.get("tol"):
        s = max(s, frac * abs(entry["tol"][0]))
    return s


def _base(key: str):
    i = key.rfind("@")
    return (key[:i], float(key[i + 1:-2])) if i >= 0 and key.endswith("Hz") else (key, None)


def match(base: dict, cur: dict) -> tuple[list[tuple[str, str]], list[str], list[str]]:
    """Pairs (baseline key, current key); then unmatched keys on each side. A tone key that
    has no exact partner pairs with the nearest tone of the same check within NEAREST_REL:
    an older baseline or a fixture may carry played rather than requested frequencies."""
    pairs = [(k, k) for k in base if k in cur]
    lb = [k for k in base if k not in cur]
    lc = [k for k in cur if k not in base]
    for kb in list(lb):
        stem, fb = _base(kb)
        if fb is None:
            continue
        cands = [(abs(fc / fb - 1), kc) for kc in lc for s, fc in [_base(kc)] if s == stem and fc is not None
                 and abs(fc / fb - 1) <= NEAREST_REL]
        if cands:
            kc = min(cands)[1]
            pairs.append((kb, kc))
            lb.remove(kb)
            lc.remove(kc)
    return pairs, lb, lc


def diff(base: dict, cur: dict, tol: dict) -> dict:
    steps, frac = compare_settings(tol)
    pairs, missing, new = match(base["checks"], cur["checks"])
    worse, better, moved, drift = [], [], [], []
    for kb, kc in pairs:
        b, c = base["checks"][kb], cur["checks"][kc]
        row = {"key": kc, "base_key": kb, "base": b, "cur": c}
        if b["status"] != c["status"]:
            (worse if RANK.get(c["status"], 0) > RANK.get(b["status"], 0) else better).append(row)
        if b["value"] is not None and c["value"] is not None and b.get("unit") == c.get("unit"):
            st = step_for(c if c.get("tol") else b, steps, frac)
            d = c["value"] - b["value"]
            if abs(d) > st:
                judged = b["status"] in JUDGED and c["status"] in JUDGED
                (moved if judged else drift).append({**row, "delta": d, "step": st})
    return {"worse": worse, "better": better, "moved": moved, "drift": drift, "missing": missing, "new": new,
            "matched": len(pairs)}


def gates(d: dict) -> bool:
    return bool(d["worse"] or d["moved"])


# ------------------------------------------------------------------ report
def _v(e: dict) -> str:
    if e.get("value") is None:
        return "—"
    v, u = e["value"], e.get("unit") or ""
    if u == "rel":
        return f"{v * 100:+.2f} %"
    return f"{v:+.4g} {u}".rstrip()


def _d(r: dict) -> str:
    u = r["cur"].get("unit") or ""
    if u == "rel":
        return f"{r['delta'] * 100:+.2f} % (step {r['step'] * 100:g} %)"
    return f"{r['delta']:+.4g} {u} (step {r['step']:g})"


def _tools(p: dict) -> str:
    """The reference tool(s) a run used, from its provenance."""
    t = [f"{name} {p[k]}" for name, k in (("REW", "rew_version"), ("OSM", "osm_version")) if p.get(k)]
    return ", ".join(t) or "no reference tool recorded"


def _lvl(x):
    return "silent" if x is None else f"{x:g} dBFS"


def render(blocks: list[dict]) -> str:
    """Markdown for one compare: a block per stage (or the reason it was not compared)."""
    out = ["# ac2 cross-check compare", ""]
    for b in blocks:
        out += [f"## {b['stage']} ({_lvl(b['level'])})", ""]
        if b.get("skipped"):
            out += [b["skipped"], ""]
            continue
        bp, cp = b["base"]["provenance"], b["cur"]["provenance"]
        d = b["diff"]
        out += [f"- baseline: `{b['file']}` — run {bp.get('run')}, ac2 {bp.get('ac2_build')}, "
                f"{_tools(bp)}",
                f"- this run: {cp.get('run')}, ac2 {cp.get('ac2_build')}, {_tools(cp)}",
                f"- builds: {bp.get('ac2_build')} → {cp.get('ac2_build')}",
                f"- matched {d['matched']}; worse {len(d['worse'])}, better {len(d['better'])}, "
                f"moved {len(d['moved'])} (+{len(d['drift'])} unjudged), new {len(d['new'])}, "
                f"missing {len(d['missing'])}",
                ""]
        for name, rows in (("Status worse", d["worse"]), ("Status better", d["better"])):
            if rows:
                out += [f"### {name}", "", "| check | baseline | this run |", "|---|---|---|"]
                out += [f"| `{r['key']}` | {r['base']['status']} {_v(r['base'])} | {r['cur']['status']} {_v(r['cur'])} |"
                        for r in rows]
                out.append("")
        if d["moved"]:
            out += ["### Values moved beyond their step", "", "| check | baseline | this run | change |", "|---|---|---|---|"]
            out += [f"| `{r['key']}` | {_v(r['base'])} | {_v(r['cur'])} | {_d(r)} |"
                    for r in sorted(d["moved"], key=lambda r: -abs(r["delta"]) / max(r["step"], 1e-12))]
            out.append("")
        if d["drift"]:
            out += ["### Unjudged values moved (INFO / INCONCLUSIVE; context, not gating)", "",
                    "| check | baseline | this run | change |", "|---|---|---|---|"]
            out += [f"| `{r['key']}` | {r['base']['status']} {_v(r['base'])} | {r['cur']['status']} {_v(r['cur'])} | {_d(r)} |"
                    for r in d["drift"]]
            out.append("")
        for name, keys in (("New checks", d["new"]), ("Missing checks", d["missing"])):
            if keys:
                out += [f"### {name}", ""] + [f"- `{k}`" for k in keys] + [""]
    return "\n".join(out)


def summary_lines(blocks: list[dict]) -> list[str]:
    lines = []
    for b in blocks:
        if b.get("skipped"):
            lines.append(f"{b['stage']} ({_lvl(b['level'])}): {b['skipped']}")
            continue
        d = b["diff"]
        bp, cp = b["base"]["provenance"], b["cur"]["provenance"]
        lines.append(f"{b['stage']} ({_lvl(b['level'])}) ac2 {bp.get('ac2_build')} → {cp.get('ac2_build')}: "
                     f"worse {len(d['worse'])}, better {len(d['better'])}, moved {len(d['moved'])} "
                     f"(+{len(d['drift'])} unjudged), "
                     f"new {len(d['new'])}, missing {len(d['missing'])}")
        for r in d["worse"] + d["better"]:
            lines.append(f"  {r['base']['status']:>12} → {r['cur']['status']:<12} {r['key']}  "
                         f"{_v(r['base'])} → {_v(r['cur'])}")
        for r in d["moved"]:
            lines.append(f"  {'moved':>12}   {'':<12} {r['key']}  {_v(r['base'])} → {_v(r['cur'])}  {_d(r)}")
    return lines


# ------------------------------------------------------------------ commands
def _tolerances(path: Path | None) -> dict:
    from .analyse import TOLERANCES
    return tomllib.loads((path or TOLERANCES).read_text())


def find_baseline(where: Path, rig: str | None, stage: str, level: float | None) -> Path | None:
    """`where` is a baseline file or a directory holding <rig>/<stage>-<level>.json
    (host/<stage>.json for a host stage)."""
    where = Path(where)
    if where.is_file():
        return where
    for p in ([where / rig / baseline_name(stage, level)] if rig else []) + [where / baseline_name(stage, level)]:
        if p.exists():
            return p
    return None


def compare(run_dir: Path, where: Path | None = None, tolerances: Path | None = None,
            stages: list[str] | None = None, out: Path | None = None) -> tuple[int, list[dict], Path]:
    run_dir = Path(run_dir)
    res = load_results(run_dir)
    tol = _tolerances(tolerances)
    rig = baseline_rig(res)
    where = Path(where) if where else BASELINES
    blocks = []
    for st in stages or stages_of(res):
        lvl = stage_level(res, st)
        blk = {"stage": st, "level": lvl}
        bp = find_baseline(where, rig, st, lvl)
        if bp is None:
            blk["skipped"] = f"no baseline for this stage and level under {where}"
            blocks.append(blk)
            continue
        base = json.loads(bp.read_text())
        if base.get("stage") != st or base.get("level_dbfs") != lvl:
            blk["skipped"] = (f"{bp} is {base.get('stage')} at {_lvl(base.get('level_dbfs'))}; "
                              f"a different stage or level is not comparable")
            blocks.append(blk)
            continue
        cur = make_baseline(res, st, run_dir.name)
        blk.update(file=str(bp), base=base, cur=cur, diff=diff(base, cur, tol))
        blocks.append(blk)
    rc = 1 if any(gates(b["diff"]) for b in blocks if "diff" in b) else 0
    out = Path(out) if out else run_dir / "report"
    out.mkdir(parents=True, exist_ok=True)
    path = out / "compare.md"
    path.write_text(render(blocks))
    return rc, blocks, path


def write_baselines(run_dir: Path, where: Path | None = None, stages: list[str] | None = None,
                    force: bool = False, tolerances: Path | None = None) -> tuple[list[str], list[Path]]:
    """Writes one baseline per stage of the run. Refuses (ValueError) a stage with FAILs
    unless forced: a baseline is a reviewed state, and a FAIL in it would hide a regression
    that keeps failing."""
    run_dir = Path(run_dir)
    res = load_results(run_dir)
    tol = _tolerances(tolerances)
    rig = baseline_rig(res)
    if not rig:
        raise ValueError(f"{run_dir}: no rig in the manifest (fixtures cannot be a baseline)")
    want = stages or stages_of(res)
    unknown = [s for s in want if s not in stages_of(res)]
    if unknown:
        raise ValueError(f"{run_dir}: no checks for stage(s) {', '.join(unknown)}")
    fails = [c["key"] for c in res["checks"] if stage_of(res, c) in want and c["status"] == "FAIL"]
    if fails and not force:
        raise ValueError(f"{run_dir}: {len(fails)} FAIL(s), e.g. {fails[0]}; review the run, or pass --force")
    root = Path(where) if where else BASELINES
    lines, written = [], []
    for st in want:
        lvl = stage_level(res, st)
        if st != "ambient" and st not in HOST_STAGES and lvl is None:
            raise ValueError(f"{run_dir}: stage {st} has no level in the manifest")
        b = make_baseline(res, st, run_dir.name)
        p = baseline_path(root, rig, st, lvl)
        if p.exists():
            old = json.loads(p.read_text())
            d = diff(old, b, tol)
            blk = {"stage": st, "level": lvl, "base": old, "cur": b, "diff": d}
            lines += summary_lines([blk])
        else:
            lines.append(f"{st} ({_lvl(lvl)}): new baseline, {len(b['checks'])} checks")
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(dumps(b))
        written.append(p)
    return lines, written
