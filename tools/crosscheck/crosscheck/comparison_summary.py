"""The summary and the list of misses at the top of `comparison.md`, from the committed baselines.

The summary has one row per quantity and reference: what ac2 is held to, the limit its checks
carry (from `tolerances.toml`, as stored in the baseline), the verdict over every check in the
current baselines, the worst value relative to its pass limit and the run it comes from.

The miss list groups every WARN and FAIL of the current baselines under a named item of
`MISS_ITEMS`. A miss that no item names, or an item with no miss left, is a stale document:
`problems()` reports both, so a change that adds or removes a miss also updates what the
document says about it."""
from __future__ import annotations

import re

from . import comparison as C
from .baseline import JUDGED

STATUSES = ("FAIL", "WARN", "PASS", "INCONCLUSIVE", "INFO")
RIG = ("xone", "genelec")
OSM_FIXTURES, OSM_RIG = "osm:fixtures", "osm:rig"

# (quantity, measured against, stages, pattern on the key after its first segment).
# A named group `o` is a harmonic order, `w` where in the stage the check sits (an OSM case).
SWEEP = r"ac2 sweep [^|@]+"
SUMMARY_ROWS: list[tuple[str, str, tuple[str, ...], str]] = [
    ("Level: electrical gain", "REW (meas − ref)", RIG, r"level\.ac2_vs_rew_meas_minus_ref$"),
    ("Level: absolute SPL", "truth: steady sine", ("genelec",), r"spl\.ac2 (?:sweep|TF × ref level)@"),
    ("Magnitude", "truth: steady sine", RIG, rf"sine_mag\.(?:{SWEEP}|ac2 TF)@"),
    ("Magnitude", "truth: direct estimate of the same capture", RIG + ("dut",),
     r"mag\.(?:ac2 sweep [^|]+\|direct \(ac2 capture [^)]*\)|ac2 TF\|direct \(TF capture\))\."),
    ("Magnitude", "REW (offline import)", RIG, r"mag\.ac2 sweep [^|]+\|REW offline\."),
    ("Magnitude", "truth: closed form", (OSM_FIXTURES,), r"(?P<w>[^.]+)\.ac2_(?:tf_h_vs_analytic|h1_h_vs_analytic_0_db)$"),
    ("Phase", "truth: steady sine", RIG, rf"sine_phase\.(?:{SWEEP}|ac2 TF)@"),
    ("Phase", "truth: direct estimate of the same capture", RIG + ("dut",),
     r"phase\.(?:ac2 sweep [^|]+\|direct \(ac2 capture [^)]*\)|ac2 TF\|direct \(TF capture\))\."),
    ("Phase", "REW (offline import)", RIG, r"phase\.ac2 sweep [^|]+\|REW offline\."),
    ("Phase", "truth: closed form", (OSM_FIXTURES,), r"(?P<w>[^.]+)\.ac2_tf_phase_vs_analytic$"),
    ("Magnitude and phase: repeatability", "another ac2 sweep of the same path", RIG,
     r"(?:mag|phase)\.ac2 sweep [^|]+\|ac2 sweep \S+?\.(?=[\d.]+-[\d.]+$)"),
    ("Group delay (±1/12-oct fit)", "truth: steady-sine pairs", RIG, r"gd\.ac2 ±1/12-oct fit@"),
    ("Arrival", "truth: IR peak of its own capture", RIG + ("dut",), r"delay\.ac2_arrival\."),
    ("Delay finder and phase slope", "truth: closed form", (OSM_FIXTURES,),
     r"(?P<w>[^.]+)\.ac2_(?:delay_finder|phase_slope)_vs_analytic$"),
    ("Coherence γ²", "truth: closed form, in noise its estimator's expected value", (OSM_FIXTURES,),
     r"(?P<w>[^.]+)\.ac2_γ²_vs_(?:true_γ²|its_expected_value)$"),
    ("Harmonics H2–H5", "truth: steady sine", RIG, rf"(?P<o>h[2-5])\.{SWEEP}@"),
    ("Harmonics H2–H5", "REW (offline import)", RIG, rf"(?P<o>h[2-5])\.{SWEEP}\|REW offline import@"),
    ("Harmonics H2–H5", "truth: exact values of a digital device", ("dut",), r"dut\.(?P<o>h[2-5])\.(?P<src>steady sine|ac2 sweep \S+?)\.(?=\d+$)"),
    ("Leq Z, A, C", "truth: numpy on the same recording", ("ambient",), r"leq\.[ZAC]\.ac2$"),
    ("LAeq", "REW", ("ambient",), r"leq\.A\.ac2_rew$"),
    ("1/3-octave RTA", "its own band filters on the same recording", ("ambient",), r"bands\.ac2 RTA\|own filters$"),
    ("Spectrum (tone, noise)", "truth: closed form", (OSM_FIXTURES,), r"(?P<w>[^.]+)\.ac2_spectrum$"),
    ("Transfer: ac2 vs OSM", "OSM on the same fixtures (max)", (OSM_FIXTURES,),
     r"(?P<w>[^.]+)\.ac2_vs_osm_(?:h|phase)$"),
    ("Transfer: ac2 vs OSM", "OSM on the same rig recording (median)", (OSM_RIG,),
     r"(?P<w>[^.]+)\.ac2_vs_osm_(?:h|phase)$"),
    ("Delay finder: ac2 vs OSM", "OSM's whole-sample finder", (OSM_FIXTURES, OSM_RIG),
     r"(?P<w>[^.]+)\.ac2_delay_finder_vs_osm_s$"),
]

# Every WARN and FAIL belongs to one item: (title, status, pattern on the full check key).
# The title is also the heading under "Open items" that discusses it. A status other than
# "open" must name its evidence in that section; an explanation stays open until a report,
# plot or controlled test shows it accounts for the residual.
# A band's max(|mean|, spread) carries `spread`; these OSM statistics are a maximum or median
# |difference|: none has a direction, so the worst value is shown unsigned.
UNSIGNED = re.compile(r"\.ac2_(?:tf_h|tf_phase)_vs_analytic$|\.ac2_vs_osm_(?:h|phase)$")

OPEN, SHARED = "open", "open; ac2 and REW alike"
MISS_ITEMS: list[tuple[str, str, str]] = [
    ("Genelec H2 at 5 kHz", SHARED, r"genelec\.h2\.(?:ac2 sweep [^|@]+|REW offline import)@5000Hz$"),
    ("Genelec H4 at 500 Hz on the dist sweep", OPEN, r"genelec\.h4\.ac2 sweep dist@500Hz$"),
    ("Genelec live TF at 1 kHz", OPEN, r"genelec\.(?:sine_mag\.ac2 TF|spl\.ac2 TF × ref level)@1000Hz$"),
    ("Genelec live TF SPL at 51 Hz", OPEN, r"genelec\.spl\.ac2 TF × ref level@50Hz$"),
    ("Genelec live TF against its direct estimate", OPEN, r"genelec\.(?:mag|phase)\.ac2 TF\|direct \(TF capture\)\."),
    ("REW's deconvolution against numpy", OPEN, r"genelec\.(?:mag|phase)\.REW offline\|direct"),
    ("The truth chain at the 100 Hz sine", OPEN, r"genelec\.sine_phase\.direct \(REW recording\), narrow"),
    ("Genelec at −50 dBFS: ac2 sweep repeatability", OPEN,
     r"genelec\.(?:mag|phase)\.ac2 sweep [^|]+\|(?:ac2 sweep|direct)"),
    ("Genelec at −50 dBFS: REW's SPL at 51 Hz", OPEN, r"genelec\.spl\.REW \(SPL, cal from ac2\)@50Hz$"),
    ("Digital DUT: dut-loud sweep against its capture", OPEN, r"dut-loud\.mag\.ac2 sweep [^|]+\|direct"),
]


# ------------------------------------------------------------------ helpers
def label(b: dict) -> str:
    return f"{b['stage']} {C._level(b)}".replace(" silent", "")


def stages_text(bs: list[dict]) -> str:
    """The baselines a row stands on, levels per stage: `genelec −30/−50 dBFS`."""
    levels: dict[str, list[str]] = {}
    for b in bs:
        lv = C._level(b)
        levels.setdefault(b["stage"], [])
        if lv != "silent" and lv not in levels[b["stage"]]:
            levels[b["stage"]].append(lv)
    return ", ".join(st + (" " + "/".join(v.replace(" dBFS", "") for v in lv) + " dBFS" if lv else "")
                     for st, lv in sorted(levels.items(), key=lambda kv: (kv[0] not in RIG, kv[0] != "xone", kv[0])))


def provenance(b: dict) -> str:
    pv = b.get("provenance") or {}
    return str(pv.get("run"))


def _osm_split(b: dict) -> tuple[set[str], set[str]]:
    planned = set(((b.get("provenance") or {}).get("flags") or {}).get("cases") or [])
    seen = {k.split(".")[1] for k in b["checks"]}
    return seen & planned, seen - planned


def _selected(bs: list[dict], stages: tuple[str, ...]) -> list[tuple[dict, set[str] | None]]:
    out = []
    for b in bs:
        for s in stages:
            if s.startswith("osm:") and b["stage"] == "osm":
                fixtures, rigs = _osm_split(b)
                out.append((b, fixtures if s == OSM_FIXTURES else rigs))
            elif s == b["stage"]:
                out.append((b, None))
    return out


def entries(b: dict, pattern: str, cases: set[str] | None = None) -> list[tuple[str, dict, str]]:
    """(where, entry, key) of each check matching `pattern` after the key's first segment."""
    rx = re.compile(pattern)
    out = []
    for k, e in b["checks"].items():
        head, _, k2 = k.partition(".")
        m = rx.match(k2)
        if not m:
            continue
        g = m.groupdict()
        if cases is not None and g.get("w") not in cases:
            continue
        rest = k2[m.end():]
        rest = f"{rest}Hz" if re.fullmatch(r"\d+(?:\.\d+)?", rest) else rest
        where = g.get("w") or C._loc(rest, g.get("o"))
        if g.get("src"):
            where = f"{where}, {g['src']}"
        if b["stage"] == "dut":
            where = f"{head}, {where}" if where else head
        out.append((where, e, k))
    return out


def _g(v: float) -> str:
    return f"{v:g}"


def limit_text(es: list[dict]) -> str:
    """The (pass / warn) limits the judged checks carry, per unit; a range where they differ."""
    parts = []
    for unit in dict.fromkeys(e.get("unit") or "" for e in es):
        tols = [e["tol"] for e in es if (e.get("unit") or "") == unit and e.get("tol")]
        if not tols:
            continue
        scale = 100 if unit == "rel" else 1
        u = {"rel": " %", "°": "°", "": ""}.get(unit, f" {unit}")

        def rng(vals):
            lo, hi = min(vals) * scale, max(vals) * scale
            return _g(lo) if lo == hi else f"{_g(lo)}–{_g(hi)}"
        parts.append(f"{rng([t[0] for t in tols])} / {rng([t[1] for t in tols])}{u}")
    return "; ".join(parts) or "—"


def verdict_text(es: list[dict]) -> str:
    c = {s: sum(e["status"] == s for e in es) for s in STATUSES}
    judged = c["PASS"] + c["WARN"] + c["FAIL"]
    if not judged:
        return "INFO only" if c["INFO"] and not c["INCONCLUSIVE"] else "not judged"
    worst = next(s for s in ("FAIL", "WARN", "PASS") if c[s])
    misses = ", ".join(f"{s} {c[s]}" for s in ("FAIL", "WARN") if c[s])
    head = f"**{worst}**" if worst != "PASS" else "PASS"
    tail = f"{misses} of {judged} judged" if misses else f"{judged} judged"
    if c["INCONCLUSIVE"]:
        tail += f", {c['INCONCLUSIVE']} inconclusive"
    return f"{head} ({tail})"


def _ratio(e: dict) -> float:
    tol = e.get("tol")
    return abs(e["value"]) / tol[0] if tol and tol[0] else 0.0


def _stage(selector: str) -> str:
    return "osm" if selector.startswith("osm:") else selector


def summary_row(bs: list[dict], quantity: str, against: str, stages, pattern: str,
                not_run: set[str] = frozenset()) -> str | None:
    """One row; `not_run`: the stages with no baseline on the document's build, named in the row
    instead of an older build's numbers."""
    found = [(b, w, e, k) for b, cases in _selected(bs, stages) for w, e, k in entries(b, pattern, cases)]
    absent = list(dict.fromkeys(_stage(s) for s in stages if _stage(s) in not_run))
    if not found:
        if not absent:
            return None
        return f"| {quantity} | {against} | {', '.join(absent)} | — | not run on this build | — | — |"
    es = [t[2] for t in found]
    judged = [t for t in found if t[2]["status"] in JUDGED and t[2]["value"] is not None]
    several = len({id(t[0]) for t in found}) > 1
    if judged:
        b, w, e, k = max(judged, key=lambda t: _ratio(t[2]))
        stat = C.ABS if e.get("spread") is not None or UNSIGNED.search(k) else C.SIGNED
        where = ", ".join(x for x in (w, label(b) if several else "") if x)
        worst = C.fmt_value(e["value"], e.get("unit") or "", stat) + (f" @ {where}" if where else "")
        src = provenance(b)
    else:
        worst, src = "—", ", ".join(dict.fromkeys(provenance(t[0]) for t in found))
    on = stages_text([t[0] for t in found]) + "".join(f"; {a} not run on this build" for a in absent)
    return (f"| {quantity} | {against} | {on} | {limit_text([t[2] for t in judged] or es)} | "
            f"{verdict_text(es)} | {worst} | {src} |")


def summary_table(bs: list[dict], not_run: set[str] = frozenset()) -> list[str]:
    out = ["| quantity | measured against | on | limit (pass / warn) | verdict | worst (largest share of its limit) | from run |",
           "|---|---|---|---|---|---|---|"]
    out += [r for r in (summary_row(bs, *row, not_run=not_run) for row in SUMMARY_ROWS) if r]
    return out


# ------------------------------------------------------------------ misses
Item = tuple[str, str, str]


def misses(bs: list[dict]) -> list[tuple[dict, str, dict]]:
    return [(b, k, e) for b in bs for k, e in b["checks"].items() if e["status"] in ("WARN", "FAIL")]


def item_of(key: str, items: list[Item]) -> Item | None:
    return next((it for it in items if re.match(it[2], key)), None)


def _range(es: list[dict]) -> str:
    by_unit: dict[str, list[dict]] = {}
    for e in es:
        by_unit.setdefault(e.get("unit") or "", []).append(e)
    parts = []
    for unit, mine in by_unit.items():
        stat = C.ABS if all(e.get("spread") is not None for e in mine) else C.SIGNED
        lo, hi = min(e["value"] for e in mine), max(e["value"] for e in mine)
        parts.append(C.fmt_value(lo, unit, stat) if lo == hi else
                     f"{C.fmt_value(lo, unit, stat)} … {C.fmt_value(hi, unit, stat)}")
    return "; ".join(parts)


def anchor(title: str) -> str:
    """The heading's link target as GitHub forms it: lower case, punctuation dropped."""
    return re.sub(r"[^\w\- ]", "", title.lower()).replace(" ", "-")


def miss_table(bs: list[dict], items: list[Item] = MISS_ITEMS) -> list[str]:
    out = ["| item | status | on | checks | values | limit (pass / warn) | from run |",
           "|---|---|---|---|---|---|---|"]
    for title, status, _ in items:
        for b in bs:
            es = [e for _, k, e in misses([b]) if (item_of(k, items) or ("",))[0] == title]
            if not es:
                continue
            c = ", ".join(f"{s} {sum(e['status'] == s for e in es)}" for s in ("FAIL", "WARN")
                          if any(e["status"] == s for e in es))
            out.append(f"| [{title}](#{anchor(title)}) | {status} | {label(b)} | {c} | {_range(es)} | "
                       f"{limit_text(es)} | {provenance(b)} |")
    return out


def problems(bs: list[dict], doc: str, items: list[Item] = MISS_ITEMS) -> list[str]:
    """Misses no item names, items with no miss left, items the document does not discuss."""
    out = [f"{label(b)}: {e['status']} {k!r} belongs to no item of comparison_summary.MISS_ITEMS"
           for b, k, e in misses(bs) if item_of(k, items) is None]
    used = {item_of(k, items)[0] for _, k, _ in misses(bs) if item_of(k, items)}
    out += [f"item {t!r} has no WARN or FAIL left: close it (MISS_ITEMS and its section)"
            for t, _, _ in items if t not in used]
    out += [f"item {t!r} has no `### {t}` section in the document"
            for t, _, _ in items if f"\n### {t}\n" not in doc]
    return out
