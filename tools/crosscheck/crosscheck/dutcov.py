"""Coverage of the digital DUT's harmonic checks: per harmonic order, how many of the cells
the run could judge were judged, and the worst signed error among them.

A cell is one comparison the plan calls for: a source (the steady sine, an ac2 sweep, REW)
reading Hk of one case's tone. Its outcome is a verdict (PASS/WARN/FAIL), INCONCLUSIVE (the
reading is a bound: the harmonic is not clear of the source's floor), or MISSING (the source
gave no reading where it should have). Cells the physics rules out are not counted at all:
k·f at or above fs/2 (the DUT aliases it, no truth at k·f) and, for a sweep, k·f beyond its
end (the sweep never played that harmonic's fundamental's k-th multiple).

Sweep cells where the DUT's pre-filter is not flat (|gain| > 0.1 dB at the tone) are
"model-limited": a sweep reaches the polynomial through the pre-filter's transient response
(dut.py: Wiener model), so a deviation there can be the model's. They are judged and shown
(†), but kept out of the order's verdict and worst error, which rest on quasi-static cells.

Pure: entries in, rows out; the analysis feeds one run, `python -m crosscheck dutcov` several
runs (one per level)."""
from __future__ import annotations

import math

JUDGED = ("PASS", "WARN", "FAIL")
RANK = {"PASS": 0, "INCONCLUSIVE": 1, "MISSING": 1, "WARN": 2, "FAIL": 3}
# the pre-filter's gain beyond which a sweep reads the truth only approximately
MODEL_DB = 0.1


def source_class(name: str) -> str:
    """Sweep variants pool into one class per analyser: the order's coverage is the
    analyser's, whichever sweep rate read it."""
    return "ac2 sweep" if name.startswith("ac2 sweep") else name


def entry(path: str, case: str | None, source: str, k: int, f: float, status: str, delta: float | None,
          truth: float, pre_db: float, sweep: bool, level_dbfs: float | None) -> dict:
    return {"path": path, "case": case, "source": source, "class": source_class(source), "k": int(k),
            "f": float(f), "status": status, "delta": None if delta is None else float(delta),
            "truth": float(truth), "pre_db": float(pre_db), "model": bool(sweep and abs(pre_db) > MODEL_DB),
            "level": level_dbfs}


def summarise(entries: list[dict]) -> dict:
    """{judged, total, inconclusive, missing, model_judged, status, worst, worst_at, model_worst}
    over a set of cells. `worst` is the signed error of largest magnitude among quasi-static
    judged cells; `status` the worst of their verdicts (INCONCLUSIVE when there are none)."""
    qs = [e for e in entries if not e["model"]]
    jq = [e for e in qs if e["status"] in JUDGED]
    jm = [e for e in entries if e["model"] and e["status"] in JUDGED]
    w = max(jq, key=lambda e: abs(e["delta"]) if e["delta"] is not None else -1.0, default=None)
    wm = max(jm, key=lambda e: abs(e["delta"]) if e["delta"] is not None else -1.0, default=None)
    st = max((e["status"] for e in jq), key=RANK.get, default="INCONCLUSIVE")
    return {"judged": len(jq) + len(jm), "total": len(entries),
            "inconclusive": sum(e["status"] == "INCONCLUSIVE" for e in entries),
            "missing": sum(e["status"] == "MISSING" for e in entries),
            "model_judged": len(jm), "status": st,
            "worst": None if w is None else w["delta"],
            "worst_at": None if w is None else {k: w[k] for k in ("case", "source", "f", "level")},
            "model_worst": None if wm is None else wm["delta"]}


def _signed(x: float | None) -> str:
    return "—" if x is None or not math.isfinite(x) else f"{x:+.2f}"


def cell(entries: list[dict]) -> str:
    """`judged/total worst` with F/W when a verdict is worse than PASS and † when a
    model-limited cell was judged; blank when the cell has no entries."""
    if not entries:
        return ""
    s = summarise(entries)
    mark = {"FAIL": " F", "WARN": " W"}.get(s["status"], "")
    if s["worst"] is None and s["model_worst"] is not None:
        txt = f"({_signed(s['model_worst'])})"
    else:
        txt = _signed(s["worst"])
    return f"{s['judged']}/{s['total']} {txt}{mark}" + (" †" if s["model_judged"] else "")


def _lvl(x) -> str:
    return "—" if x is None else f"{x:g} dBFS"


def grid(entries: list[dict], by_level: bool = False) -> tuple[list[str], list[list[str]]]:
    """Rows per harmonic order × source class (× level), columns per tone, then the row's
    totals: judged/total, inconclusive, missing, worst signed error, status."""
    tones = sorted({e["f"] for e in entries})
    levels = sorted({e["level"] for e in entries if e["level"] is not None}, reverse=True) if by_level else [None]
    cols = ["H", "source"] + (["level"] if by_level else []) + [f"{f:g} Hz" for f in tones] + \
        ["judged", "incon.", "missing", "worst Δ dB", "model-limited", "status"]
    rows = []
    for k in sorted({e["k"] for e in entries}):
        for src in sorted({e["class"] for e in entries if e["k"] == k}, key=lambda s: (s != "steady sine", s)):
            for lv in levels:
                sel = [e for e in entries if e["k"] == k and e["class"] == src and (lv is None or e["level"] == lv)]
                if not sel:
                    continue
                s = summarise(sel)
                row = [f"H{k}", src] + ([_lvl(lv)] if by_level else [])
                row += [cell([e for e in sel if e["f"] == f]) for f in tones]
                row += [f"{s['judged']}/{s['total']}", str(s["inconclusive"]), str(s["missing"]), _signed(s["worst"]),
                        (f"{s['model_judged']} judged, worst {_signed(s['model_worst'])}" if s["model_judged"] else "—"),
                        s["status"]]
                rows.append(row)
    return cols, rows


NOTE = ("Cell: judged/cells over the cases (and sweep rates), then the quasi-static worst signed "
        "error reading − truth (dB; in parentheses when only model-limited cells were judged). "
        "F/W: a FAIL/WARN in the cell. †: a judged cell where the pre-filter is > 0.1 dB from flat "
        "(Wiener model: a sweep sees it only approximately); such cells are kept out of the status "
        "and the worst Δ. Not counted: k·f ≥ fs/2 and, for a sweep, k·f beyond its end. "
        "incon.: the reading is a bound (not clear of the floor); missing: no reading where one was due.")


def markdown(cols: list[str], rows: list[list[str]]) -> str:
    out = ["| " + " | ".join(cols) + " |", "|" + "---|" * len(cols)]
    out += ["| " + " | ".join(r) + " |" for r in rows]
    return "\n".join(out)


def entries_of(results: dict) -> list[dict]:
    """The cells a results.json holds (the coverage checks carry them in their detail)."""
    out = []
    for c in results.get("checks", []):
        if c.get("group") == "dut coverage":
            out.extend((c.get("detail") or {}).get("cells", []))
    return out
