"""`python -m crosscheck comparison`: the generated parts of `comparison.md` from the committed
baselines, and the numbers quoted in its prose rewritten from their sources (`comparison_quotes`).

The document shows one ac2 build: that of the newest rig baseline. Baselines on other builds are
not shown (a stage with none on the build is named as not run). The table has one row per
quantity and one column per comparison; each cell is the worst judged value (worst verdict
first, then the largest share of its pass limit) and its verdict. Every warn or FAIL cell needs
a `### <quantity>: <column>` section under "Disparities", and every such section a warn or FAIL
cell; `--check` fails while the document is stale or either is missing."""
from __future__ import annotations

import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

from . import comparison_quotes as Q
from .baseline import BASELINES, HOST_DIR, JUDGED

ROOT = Path(__file__).resolve().parent.parent
DOC = ROOT / "comparison.md"
BLOCKS = ("build", "table")
DISPARITIES = "## Disparities"


def begin(name: str) -> str:
    return f"<!-- BEGIN generated {name}: `python -m crosscheck comparison` from baselines/; do not edit by hand -->"


def end(name: str) -> str:
    return f"<!-- END generated {name} -->"


@dataclass(frozen=True)
class Pair:
    """ac2 − another tool at the same tone, from their two checks against one truth (the truth
    cancels); judged against the ac2 check's limits."""
    ac2: str
    other: str


@dataclass(frozen=True)
class Row:
    label: str
    stages: tuple[str, ...]  # the stages that measure it: none on the build reads "not run"
    ac2: tuple = ()
    rew: tuple = ()
    osm: tuple = ()
    ac2_rew: tuple = ()
    ac2_osm: tuple = ()

    def columns(self) -> tuple[tuple, ...]:
        return self.ac2, self.rew, self.osm, self.ac2_rew, self.ac2_osm


COLUMNS = ("ac2 vs truth", "REW vs truth", "OSM vs truth", "ac2 − REW", "ac2 − OSM")

# Patterns match a check key after its first segment (the stage, a DUT case, or `osm`; an OSM
# key goes on with its case). Truths: the steady sine and the direct cross-spectrum of the same
# capture on the rig, the closed form of the OSM fixtures, the digital DUT's exact harmonics.
SWEEP = r"ac2 sweep [^|@]+"
AC2_ANY = rf"(?:{SWEEP}|ac2 TF)"
PATHS = ("xone", "genelec", "dut", "osm")
ROWS: list[Row] = [
    Row("Gain / level", ("xone", "genelec"),
        ac2=(r"level\.ac2_ref_vs_sine$",),
        ac2_rew=(r"level\.ac2_vs_rew_meas_minus_ref$", r"level\.ac2_ref_vs_rew_ref$")),
    Row("SPL", ("genelec",),
        ac2=(r"spl\.ac2 (?:sweep|TF × ref level)@",),
        rew=(r"spl\.REW \(SPL, cal from ac2\)@",),
        ac2_rew=(Pair(r"spl\.ac2 (?:sweep|TF × ref level)@", r"spl\.REW \(SPL, cal from ac2\)@"),)),
    Row("Magnitude", PATHS,
        ac2=(rf"sine_mag\.{AC2_ANY}@",
             r"mag\.(?:ac2 sweep [^|]+\|direct \(ac2 capture [^)]*\)|ac2 TF\|direct \(TF capture\))\.",
             r"[^.]+\.ac2_(?:tf_h_vs_analytic|h1_h_vs_analytic_0_db)$"),
        rew=(r"sine_mag\.REW offline@", r"mag\.REW offline\|direct \(REW recording\)\."),
        osm=(r"[^.]+\.osm_(?:m_r_vs_analytic|mean_m_r_vs_its_expected_bias)$",),
        ac2_rew=(rf"mag\.{SWEEP}\|REW offline\.", Pair(rf"sine_mag\.{AC2_ANY}@", r"sine_mag\.REW offline@")),
        ac2_osm=(r"[^.]+\.ac2_vs_osm_h$",)),
    Row("Phase", PATHS,
        ac2=(rf"sine_phase\.{AC2_ANY}@",
             r"phase\.(?:ac2 sweep [^|]+\|direct \(ac2 capture [^)]*\)|ac2 TF\|direct \(TF capture\))\.",
             r"[^.]+\.ac2_tf_phase_vs_analytic$"),
        rew=(r"sine_phase\.REW offline@", r"phase\.REW offline\|direct \(REW recording\)\."),
        osm=(r"[^.]+\.osm_phase_vs_analytic$",),
        ac2_rew=(rf"phase\.{SWEEP}\|REW offline\.", Pair(rf"sine_phase\.{AC2_ANY}@", r"sine_phase\.REW offline@")),
        ac2_osm=(r"[^.]+\.ac2_vs_osm_phase$",)),
    Row("Group delay", ("xone", "genelec"),
        ac2=(r"gd\.ac2 ±1/12-oct fit@",),
        rew=(r"gd\.REW offline import ±1/12-oct fit@",),
        ac2_rew=(Pair(r"gd\.ac2 ±1/12-oct fit@", r"gd\.REW offline import ±1/12-oct fit@"),)),
    Row("Arrival / delay", PATHS,
        ac2=(r"delay\.ac2_arrival\.", r"[^.]+\.ac2_(?:delay_finder|phase_slope)_vs_analytic$"),
        osm=(r"[^.]+\.osm_(?:delay_finder|phase_slope)_vs_analytic$",),
        ac2_osm=(r"[^.]+\.ac2_delay_finder_vs_osm_s$",)),
    Row("Coherence γ²", ("osm",),
        ac2=(r"[^.]+\.ac2_γ²_vs_(?:true_γ²|its_expected_value)$",),
        osm=(r"[^.]+\.osm_γ²_vs_its_expected_value$",)),
    Row("Harmonics H2–H5", ("xone", "genelec", "dut"),
        ac2=(rf"h[2-5]\.{SWEEP}@", r"dut\.(?:grid\.)?h[2-5]\.ac2 sweep "),
        rew=(r"h[2-5]\.REW offline import@",),
        ac2_rew=(rf"h[2-5]\.{SWEEP}\|REW offline import@",)),
    Row("Leq (Z, A, C)", ("ambient",),
        ac2=(r"leq\.[ZAC]\.ac2$",),
        rew=(r"leq\.A\.REW$",),
        ac2_rew=(r"leq\.A\.ac2_rew$",)),
    Row("1/3-octave RTA", ("ambient",),
        ac2=(r"bands\.ac2 RTA\|own filters$",)),
    Row("Spectrum level", ("osm",),
        ac2=(r"[^.]+\.ac2_spectrum$",),
        osm=(r"[^.]+\.osm_module_3_01(?:_db_vs_analytic)?$",)),
]

# A maximum |difference| or a band's max(|mean|, spread) has no direction: shown unsigned.
UNSIGNED = re.compile(r"\.(?:ac2_tf_h|ac2_tf_phase|osm_m_r|osm_phase)_vs_analytic$|\.ac2_vs_osm_(?:h|phase)$")
RANK = {"FAIL": 0, "WARN": 1, "PASS": 2}
WORD = {"FAIL": "**FAIL**", "WARN": "**warn**", "PASS": "pass"}


@dataclass(frozen=True)
class Hit:
    """One judged (or unjudged) value of a cell: a check, or a Pair's difference."""
    b: dict
    key: str
    status: str
    value: float | None
    unit: str
    tol: tuple[float, float] | None
    signed: bool


# ------------------------------------------------------------------ baselines and the build
def load(root: Path) -> list[tuple[Path, dict]]:
    return [(p, json.loads(p.read_text())) for p in sorted(Path(root).glob("*/*.json"))]


def build_of(b: dict) -> str:
    s = (b.get("provenance") or {}).get("ac2_build") or ""
    m = re.search(r"\+([0-9a-f]{7})", s)
    return m.group(1) if m else (s or "—")


def is_rig(p: Path, b: dict) -> bool:
    return p.parent.name != HOST_DIR and b["stage"] in ("xone", "genelec")


def doc_build(items: list[tuple[Path, dict]]) -> str | None:
    """The newest rig baseline's build (a release is tested on the rig; host stages are re-run on
    the same build); without a rig baseline, the newest one's."""
    run = lambda pb: (pb[1].get("provenance") or {}).get("started") or ""  # noqa: E731
    pool = [pb for pb in items if is_rig(*pb)] or items
    return build_of(max(pool, key=run)[1]) if pool else None


def current(items: list[tuple[Path, dict]]) -> list[tuple[Path, dict]]:
    build = doc_build(items)
    return [(p, b) for p, b in items if build_of(b) == build]


def level(b: dict) -> str:
    return "" if b.get("level_dbfs") is None else f"{b['level_dbfs']:g}".replace("-", "−")


STAGE_NAMES = {"xone": "Xone (electrical)", "genelec": "Genelec (speaker)", "ambient": "ambient",
               "dut": "digital DUT", "osm": "OSM fixtures and rig recordings"}


def build_text(items: list[tuple[Path, dict]]) -> str:
    shown = current(items)
    days = sorted({((b.get("provenance") or {}).get("started") or "")[:10] for _, b in shown} - {""})
    when = days[0] if len(days) == 1 else f"{days[0]} to {days[-1]}" if days else "—"
    takes: dict[str, list[str]] = {}
    for _, b in sorted(shown, key=lambda pb: (list(STAGE_NAMES).index(pb[1]["stage"]), -(pb[1].get("level_dbfs") or 0))):
        takes.setdefault(b["stage"], []).append(level(b))
    parts = [STAGE_NAMES[st] + (f" at {', '.join(lv)} dBFS" if any(lv) else "") for st, lv in takes.items()]
    out = f"ac2 build **{doc_build(items)}**, runs of {when}: {'; '.join(parts)}."
    gone = sorted({b["stage"] for _, b in items} - set(takes))
    if gone:
        out += f" Not run on this build: {', '.join(STAGE_NAMES[s] for s in gone)}."
    return out


# ------------------------------------------------------------------ cells
def _matches(b: dict, pattern: str):
    rx = re.compile(pattern)
    for k, e in b["checks"].items():
        k2 = k.partition(".")[2]
        if rx.match(k2):
            yield k, k2, e


def judge(v: float, tol) -> str:
    return "PASS" if abs(v) <= tol[0] else "WARN" if abs(v) <= tol[1] else "FAIL"


def hits(b: dict, source) -> list[Hit]:
    if isinstance(source, Pair):
        other = {k2.rpartition("@")[2]: (k, e) for k, k2, e in _matches(b, source.other)
                 if e["status"] in JUDGED and e["value"] is not None}
        out = []
        for k, k2, e in _matches(b, source.ac2):
            o = other.get(k2.rpartition("@")[2])
            if o is None or e["status"] not in JUDGED or e["value"] is None or not e.get("tol"):
                continue
            v = e["value"] - o[1]["value"]
            out.append(Hit(b, f"{k} minus {o[0]}", judge(v, e["tol"]), v, e.get("unit") or "", tuple(e["tol"]), True))
        return out
    return [Hit(b, k, e["status"], e["value"], e.get("unit") or "", tuple(e["tol"]) if e.get("tol") else None,
                "spread" not in e and not UNSIGNED.search(k))
            for k, _, e in _matches(b, source)]


def _ratio(h: Hit) -> float:
    return abs(h.value) / h.tol[0] if h.tol and h.tol[0] else 0.0


def worst(hs: list[Hit]) -> Hit | None:
    judged = [h for h in hs if h.status in JUDGED and h.value is not None]
    return min(judged, key=lambda h: (RANK[h.status], -_ratio(h))) if judged else None


def fmt_value(v: float, unit: str, signed: bool = True) -> str:
    if unit == "rel":
        s = f"{v * 100:+.1f} %"
    elif unit == "°":
        s = f"{v:+.2f}°"
    elif unit == "µs":
        s = f"{v:+.2f} µs"
    elif unit == "samples":
        s = f"{v:+.3f} samples"
    elif unit == "":
        s = f"{v:+.4f}"
    else:
        s = f"{v:+.3f} {unit}"
    if not signed or re.match(r"[+-]0\.0+(?:\D|$)", s):  # no direction, or rounds to zero
        s = s[1:]
    return s.replace("-", "−", 1)


def cell_text(hs: list[Hit], sourced: bool, run: bool) -> str:
    if not sourced:
        return "—"
    if not run:
        return "not run"
    if not hs:
        return "—"
    w = worst(hs)
    return "not judged" if w is None else f"{fmt_value(w.value, w.unit, w.signed)} {WORD[w.status]}"


def _g(v: float) -> str:
    return f"{v:g}"


def limits_text(hs: list[Hit]) -> str:
    """pass / warn limits of the row's judged checks, per unit; the speaker path's apart (a room
    is noisier than a cable)."""
    groups: dict[tuple[bool, str], list[tuple[float, float]]] = {}
    for h in hs:
        if h.tol and h.status in JUDGED:
            speaker = h.b["stage"] == "genelec" or ".genelec-rig." in h.key
            groups.setdefault((speaker, h.unit), []).append(h.tol)
    parts = []
    for (speaker, unit), tols in sorted(groups.items()):
        scale, u = (100, " %") if unit == "rel" else (1, {"°": "°", "": ""}.get(unit, f" {unit}"))

        def rng(vals):
            lo, hi = min(vals) * scale, max(vals) * scale
            return _g(lo) if lo == hi else f"{_g(lo)}–{_g(hi)}"
        parts.append(("speaker " if speaker else "") + f"{rng([t[0] for t in tols])} / {rng([t[1] for t in tols])}{u}")
    return "; ".join(parts) or "—"


def cells(bs: list[dict], row: Row) -> list[tuple[str, Hit | None]]:
    run = any(b["stage"] in row.stages for b in bs)
    out = []
    for sources in row.columns():
        hs = [h for b in bs for s in sources for h in hits(b, s)]
        out.append((cell_text(hs, bool(sources), run), worst(hs)))
    return out


def table(bs: list[dict]) -> str:
    out = ["| quantity | " + " | ".join(COLUMNS) + " | limits (pass / warn) |",
           "|---|" + "---|" * (len(COLUMNS) + 1)]
    for row in ROWS:
        texts = [t for t, _ in cells(bs, row)]
        every = [h for b in bs for sources in row.columns() for s in sources for h in hits(b, s)]
        out.append(f"| {row.label} | " + " | ".join(texts) + f" | {limits_text(every)} |")
    return "\n".join(out)


def disparities(bs: list[dict]) -> list[str]:
    """`<quantity>: <column>` of every warn or FAIL cell."""
    return [f"{row.label}: {col}" for row in ROWS for col, (_, w) in zip(COLUMNS, cells(bs, row))
            if w is not None and w.status in ("WARN", "FAIL")]


# ------------------------------------------------------------------ the document
def blocks(root: Path) -> dict[str, str]:
    items = load(Path(root))
    return {"build": build_text(items), "table": table([b for _, b in current(items)])}


def splice(doc: str, name: str, generated: str) -> str:
    b, e = begin(name), end(name)
    i, j = doc.find(b), doc.find(e)
    if i < 0 or j < i:
        raise ValueError(f"{DOC.name}: the marker comments of the {name} part are missing")
    return doc[:i] + b + "\n\n" + generated.strip("\n") + "\n\n" + doc[j:]


def hand_written(doc: str) -> str:
    """The document without its generated parts (their lines kept blank, so line numbers hold)."""
    for name in BLOCKS:
        i, j = doc.find(begin(name)), doc.find(end(name))
        if 0 <= i < j:
            doc = doc[:i] + "\n" * doc.count("\n", i, j) + doc[j:]
    return doc


def sections(doc: str) -> list[str]:
    """The `###` headings under "Disparities"."""
    i = doc.find("\n" + DISPARITIES + "\n")
    if i < 0:
        return []
    out = []
    for line in doc[i + 1:].splitlines()[1:]:
        if line.startswith("## "):
            break
        if line.startswith("### "):
            out.append(line[4:].strip())
    return out


def build(doc: str, root: Path = BASELINES) -> tuple[str, list[str]]:
    """The document rebuilt, and what a rebuild cannot fix: quotes whose source failed, numbers
    without a source, disparities without a section and sections without a disparity."""
    for name, text in blocks(root).items():
        doc = splice(doc, name, text)
    doc, problems = Q.refresh(doc, Path(root))
    problems += Q.unquoted(hand_written(doc))
    want, have = disparities([b for _, b in current(load(Path(root)))]), sections(doc)
    problems += [f"the {d!r} cell is warn or FAIL: it needs a `### {d}` section under {DISPARITIES!r}"
                 for d in want if d not in have]
    problems += [f"`### {s}` under {DISPARITIES!r} has no warn or FAIL cell: remove it"
                 for s in have if s not in want]
    return doc, problems


def update(doc_path: Path = DOC, root: Path = BASELINES, check: bool = False) -> tuple[bool, list[str]]:
    """Rewrites the generated parts and the quoted numbers (unless `check`); returns (the document
    was current and has no problem, the problems). Problems are printed; the rewrite still
    happens, so the table follows the baselines while the prose is being brought up to date."""
    doc = Path(doc_path).read_text()
    new, problems = build(doc, root)
    for p in problems:
        print(f"{Path(doc_path).name}: {p}", file=sys.stderr)
    if not check and new != doc:
        Path(doc_path).write_text(new)
    return new == doc and not problems, problems
