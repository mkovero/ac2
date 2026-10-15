"""`python -m crosscheck comparison`: the generated parts of `comparison.md` from the committed
baselines, and the numbers quoted in its prose rewritten from their sources (`comparison_quotes`).

The document shows one ac2 build: that of the newest rig baseline. Baselines on other builds are
not shown (a path with none on the build is named as not run). There is one table per signal
path; a row is one quantity on that path's signal, held to one limit, and a column one
comparison. Each cell is the worst judged value of that comparison in the row (worst verdict
first, then the largest share of the limit) and its verdict. Every warn or FAIL cell needs a
`### <path>: <quantity>: <column>` section under "Disparities", and every such section a warn or
FAIL cell; `--check` fails while the document is stale, either is missing, or a row mixes
limits."""
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
    """One quantity on one path's signal, held to one limit: each column's patterns pick that
    tool's checks of it."""
    label: str
    ac2: tuple = ()
    rew: tuple = ()
    rew_div: tuple = ()  # REW's own reference division (its A / B)
    osm: tuple = ()
    ac2_rew: tuple = ()
    ac2_rew_div: tuple = ()
    ac2_osm: tuple = ()
    unit: str | None = None  # only checks in this unit: a quantity judged in two units is two rows
    # INFO checks printed after the cell they belong to, as (label, pattern): the tails of a
    # median, the population it is taken over
    context: tuple = ()

    def columns(self) -> tuple[tuple, ...]:
        return self.ac2, self.rew, self.rew_div, self.osm, self.ac2_rew, self.ac2_rew_div, self.ac2_osm


@dataclass(frozen=True)
class SignalPath:
    """One signal path: the stage of its baselines, what its truth is, and its rows."""
    name: str  # the first part of a disparity's key
    stage: str
    title: str
    caption: str
    rows: tuple[Row, ...]


COLUMNS = ("ac2 vs truth", "REW vs truth", "REW ÷ ref vs truth", "OSM vs truth", "ac2 − REW", "ac2 − REW ÷ ref",
           "ac2 − OSM")

# Patterns match a check key after its first segment (the stage, a DUT case, or `osm`; an OSM
# key goes on with its case).
SWEEP = r"ac2 sweep [^|@]+"
AC2_ANY = rf"(?:{SWEEP}|ac2 TF)"
SWEEP_DIRECT = r"ac2 sweep [^|]+\|direct \(ac2 capture [^)]*\)"
TF_DIRECT = r"ac2 TF\|direct \(TF capture\)"
REW_DIRECT = r"REW offline\|direct \(REW recording\)"
# REW's Arithmetic A / B of its two imports: REW's own meas ÷ ref
REW_DIV = r"REW ÷ ref"
REW_DIV_DIRECT = rf"{REW_DIV}\|direct \(REW recording\)"
# Magnitude limits differ per band (a cable's low band is noisier than its mid band).
BANDS = (("20-100", "20–100 Hz"), ("100-1000", "100 Hz–1 kHz"), ("1000-20000", "1–20 kHz"))


def _tones(q: str, label: str, ac2: str = AC2_ANY, rew: str = r"REW offline", unit: str | None = None,
           rew_div: str | None = None) -> Row:
    """At each steady sine's frequency; ac2 − REW is the difference of the two readings."""
    div = dict(rew_div=(rf"{q}\.{rew_div}@",), ac2_rew_div=(Pair(rf"{q}\.{ac2}@", rf"{q}\.{rew_div}@"),)) if rew_div else {}
    return Row(label, ac2=(rf"{q}\.{ac2}@",), rew=(rf"{q}\.{rew}@",),
               ac2_rew=(Pair(rf"{q}\.{ac2}@", rf"{q}\.{rew}@"),), unit=unit, **div)


REF_LEVEL = Row("Reference level", ac2=(r"level\.ac2_ref_vs_sine$",), ac2_rew=(r"level\.ac2_ref_vs_rew_ref$",))
GAIN = Row("Gain, measurement ÷ reference", ac2_rew=(r"level\.ac2_vs_rew_meas_minus_ref$",))
SPL = _tones("spl", "SPL at the tones", ac2=r"ac2 (?:sweep|TF × ref level)", rew=r"REW \(SPL, cal from ac2\)")
MAG_TONES = _tones("sine_mag", "Magnitude at the tones", rew_div=REW_DIV)
MAG_BANDS = tuple(Row(f"Magnitude {name}, sweeps", ac2=(rf"mag\.{SWEEP_DIRECT}\.{key}$",),
                      rew=(rf"mag\.{REW_DIRECT}\.{key}$",), ac2_rew=(rf"mag\.{SWEEP}\|REW offline\.{key}$",),
                      rew_div=(rf"mag\.{REW_DIV_DIRECT}\.{key}$",),
                      ac2_rew_div=(rf"mag\.{SWEEP}\|{REW_DIV}\.{key}$",))
                  for key, name in BANDS)
MAG_TF = tuple(Row(f"Magnitude {name}, live TF", ac2=(rf"mag\.{TF_DIRECT}\.{key}$",)) for key, name in BANDS)
PHASE_TONES = _tones("sine_phase", "Phase at the tones", rew_div=REW_DIV)
PHASE = Row("Phase 20 Hz–20 kHz", ac2=(rf"phase\.(?:{SWEEP_DIRECT}|{TF_DIRECT})\.",),
            rew=(rf"phase\.{REW_DIRECT}\.",), ac2_rew=(rf"phase\.{SWEEP}\|REW offline\.",),
            rew_div=(rf"phase\.{REW_DIV_DIRECT}\.",), ac2_rew_div=(rf"phase\.{SWEEP}\|{REW_DIV}\.",))
# Group delay is judged relative below 1 kHz and in µs above (where it is a few µs).
GD = tuple(_tones("gd", label, ac2=r"ac2 ±1/12-oct fit", rew=r"REW offline import ±1/12-oct fit", unit=unit)
           for label, unit in (("Group delay below 1 kHz", "rel"), ("Group delay above 1 kHz", "µs")))
ARRIVAL = Row("Arrival", ac2=(r"delay\.ac2_arrival\.",))
HARMONICS = Row("Harmonics H2–H5", ac2=(rf"h[2-5]\.{SWEEP}@",), rew=(r"h[2-5]\.REW offline import@",),
                ac2_rew=(rf"h[2-5]\.{SWEEP}\|REW offline import@",))

# OSM keys go `osm.<case>.<check>`; a synthetic fixture is any case but a rig recording.
FIX = r"(?![^.]*-rig\.)[^.]+"


def _tails(rig: str, q: str) -> tuple:
    return (("columns", rf"{rig}-rig\.columns_judged$"), ("95th percentile", rf"{rig}-rig\.ac2_vs_osm_{q}_95th_percentile_difference$"),
            ("max", rf"{rig}-rig\.ac2_vs_osm_{q}_max_difference$"), ("at", rf"{rig}-rig\.ac2_vs_osm_{q}_max_difference_frequency$"))


def _rig_osm(rig: str, label: str) -> tuple[Row, ...]:
    return (Row(f"{label}: magnitude, median", ac2_osm=(rf"{rig}-rig\.ac2_vs_osm_h$",), context=_tails(rig, "h")),
            Row(f"{label}: phase, median", ac2_osm=(rf"{rig}-rig\.ac2_vs_osm_phase$",), context=_tails(rig, "phase")),
            Row(f"{label}: delay finder", ac2_osm=(rf"{rig}-rig\.ac2_delay_finder_vs_osm_s$",)))


RIG_TRUTH = ("Truth: the steady sine at each tone, analysed in numpy, for the level, tone, group delay "
             "and harmonic rows; numpy's direct cross-spectrum of the same capture for the band rows and "
             "the arrival, for the live TF read through the TF's own windows and columns.")
PATHS: tuple[SignalPath, ...] = (
    SignalPath("Xone", "xone", "Xone (electrical)",
               "Out of the interface, through the Xone:62 and back in. " + RIG_TRUTH,
               (REF_LEVEL, GAIN, MAG_TONES, *MAG_BANDS, *MAG_TF, PHASE_TONES, PHASE, *GD, ARRIVAL, HARMONICS)),
    SignalPath("Genelec", "genelec", "Genelec (speaker at 56 cm)",
               "The Genelec 1083 in the room, the MM1 on its tweeter axis. " + RIG_TRUTH
               + " SPL: the steady sine through the stored calibration.",
               (REF_LEVEL, GAIN, SPL, MAG_TONES, *MAG_BANDS, *MAG_TF, PHASE_TONES, PHASE, ARRIVAL, HARMONICS)),
    SignalPath("ambient", "ambient", "Ambient (silent room)",
               "The room's own noise at the MM1, nothing played. Truth: numpy's Leq of the same recording "
               "with analytic weightings; for the RTA, ac2's own band filters applied to the recording.",
               (Row("Leq, A-weighted", ac2=(r"leq\.A\.ac2$",), rew=(r"leq\.A\.REW$",), ac2_rew=(r"leq\.A\.ac2_rew$",)),
                Row("Leq, Z- and C-weighted", ac2=(r"leq\.[ZC]\.ac2$",)),
                Row("1/3-octave RTA", ac2=(r"bands\.ac2 RTA\|own filters$",)))),
    SignalPath("digital DUT", "dut", "Digital DUT",
               "A software device with known distortion on a dummy JACK server, in four configurations; "
               "a cell is the worst over them and the levels. Truth: the device's exact harmonics, the "
               "steady sine at each tone, numpy's direct cross-spectrum of the same capture (the sweep's "
               "harmonic products cut) for the bands "
               "and the arrival.",
               (REF_LEVEL, MAG_TONES, *MAG_BANDS, PHASE_TONES, PHASE, ARRIVAL,
                Row("Harmonics H2–H5 vs exact", ac2=(r"dut\.(?:grid\.)?h[2-5]\.ac2 sweep ",)),
                Row("Harmonics H2–H5 vs the steady sine", ac2=HARMONICS.ac2))),
    SignalPath("OSM fixtures", "osm", "OSM fixtures",
               "Synthetic WAV pairs (identity, a peaking biquad, two delays, a polarity flip, uncorrelated "
               "noise at three SNRs, a 1 kHz sine), analysed by ac2 and by OSM's DSP. Truth: each "
               "fixture's closed form; in noise, each estimator's expected value.",
               (Row("Magnitude", ac2=(rf"{FIX}\.ac2_tf_h_vs_analytic$",), osm=(rf"{FIX}\.osm_m_r_vs_analytic$",),
                    ac2_osm=(rf"{FIX}\.ac2_vs_osm_h$",)),
                Row("Phase", ac2=(rf"{FIX}\.ac2_tf_phase_vs_analytic$",), osm=(rf"{FIX}\.osm_phase_vs_analytic$",),
                    ac2_osm=(rf"{FIX}\.ac2_vs_osm_phase$",)),
                Row("Magnitude in noise, H1 vs 0 dB", ac2=(r"[^.]+\.ac2_h1_h_vs_analytic_0_db$",)),
                Row("Magnitude in noise, mean vs its bias", osm=(r"[^.]+\.osm_mean_m_r_vs_its_expected_bias$",)),
                Row("Coherence γ²", ac2=(r"[^.]+\.ac2_γ²_vs_(?:true_γ²|its_expected_value)$",),
                    osm=(r"[^.]+\.osm_γ²_vs_its_expected_value$",)),
                Row("Delay, phase slope", ac2=(r"[^.]+\.ac2_phase_slope_vs_analytic$",),
                    osm=(r"[^.]+\.osm_phase_slope_vs_analytic$",)),
                Row("Delay, ac2's sub-sample finder", ac2=(r"[^.]+\.ac2_delay_finder_vs_analytic$",)),
                Row("Delay, OSM's whole-sample finder", osm=(r"[^.]+\.osm_delay_finder_vs_analytic$",)),
                Row("Delay finder, ac2 against OSM", ac2_osm=(rf"{FIX}\.ac2_delay_finder_vs_osm_s$",)),
                Row("Spectrum level, 1 kHz sine", ac2=(r"sine1k\.ac2_spectrum$",),
                    osm=(r"sine1k\.osm_module_3_01_db_vs_analytic$",)),
                Row("Spectrum level, white noise", ac2=(r"identity\.ac2_spectrum$",),
                    osm=(r"identity\.osm_module_3_01$",)))),
    SignalPath("OSM on rig recordings", "osm", "OSM on the rig recordings",
               "ac2 and OSM's DSP on the Xone and Genelec recordings above. No truth: only ac2 − OSM, "
               "for magnitude and phase a median difference (their windows see a room differently).",
               (*_rig_osm("xone", "Xone"), *_rig_osm("genelec", "Genelec"))),
)

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


def levels_text(bs: list[dict]) -> str:
    """`−10, −30 dBFS` of the baselines, loudest first; empty when none has a level."""
    lv = [level(b) for b in sorted(bs, key=lambda b: -(b.get("level_dbfs") or 0)) if level(b)]
    return f"{', '.join(lv)} dBFS" if lv else ""


def build_text(items: list[tuple[Path, dict]]) -> str:
    shown = current(items)
    days = sorted({((b.get("provenance") or {}).get("started") or "")[:10] for _, b in shown} - {""})
    when = days[0] if len(days) == 1 else f"{days[0]} to {days[-1]}" if days else "—"
    takes: dict[str, list[dict]] = {}
    for _, b in sorted(shown, key=lambda pb: list(STAGE_NAMES).index(pb[1]["stage"])):
        takes.setdefault(b["stage"], []).append(b)
    parts = [STAGE_NAMES[st] + (f" at {levels_text(bs)}" if levels_text(bs) else "") for st, bs in takes.items()]
    out = f"ac2 build **{doc_build(items)}**, runs of {when}: {'; '.join(parts)}."
    cals = [c for _, b in shown if (c := (b.get("provenance") or {}).get("calibration"))]
    if cals:
        c = cals[0]
        out += (f" SPL calibration: {c['mic']} on input {c['input']}, 0 dBFS = {c['sensitivity_db']:.2f} dB SPL")
        if "loading_db" in c:
            out += (f", including its loading ({c['mic_impedance']:g} Ω into {c['input_impedance']:g} Ω,"
                    f" {c['loading_db']:+.2f} dB)".replace("-", "−"))
        out += f"; REW given {c['rew_dbfs_at_94']:.2f} dBFS at 94 dB SPL.".replace("-", "−")
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


def population(hs: list[Hit], w: Hit) -> str:
    """How much of the cell the verdict stands on: judged checks of all the cell's checks, and
    the worst check's surviving columns of its band, wherever either is short of all."""
    k = sum(h.status in JUDGED and h.value is not None for h in hs)
    e = w.b["checks"].get(w.key.partition(" minus ")[0], {})
    parts = [f"{k} of {len(hs)} judged"] if k < len(hs) else []
    if isinstance(e.get("n"), int) and isinstance(e.get("of"), int) and e["n"] < e["of"]:
        parts.append(f"{e['n']} of {e['of']} columns")
    return f" ({'; '.join(parts)})" if parts else ""


def context_text(bs: list[dict], row: Row) -> str:
    vals = []
    for label, pat in row.context:
        for b in bs:
            for _, _, e in _matches(b, pat):
                if e["value"] is not None:
                    vals.append((label, e["value"], e.get("unit") or ""))
    if not vals:
        return ""
    out = []
    for label, v, u in vals:
        if label == "columns":
            out.append(f"{v:.0f} columns")
        elif label == "at":
            out[-1] += f" at {v:.1f} Hz"
        else:
            out.append(f"{label} {fmt_value(v, u, False)}")
    return "; " + ", ".join(out)


def cell_text(hs: list[Hit]) -> str:
    if not hs:
        return "—"
    w = worst(hs)
    return "not judged" if w is None else f"{fmt_value(w.value, w.unit, w.signed)} {WORD[w.status]}{population(hs, w)}"


def limits(hs: list[Hit]) -> list[tuple[str, tuple[float, float]]]:
    """The distinct (unit, (pass, warn)) of the judged values: one in a well-formed row."""
    return sorted({(h.unit, h.tol) for h in hs if h.tol and h.status in JUDGED})


def limit_text(lims: list[tuple[str, tuple[float, float]]]) -> str:
    def one(unit, tol):
        scale, u = (100, " %") if unit == "rel" else (1, {"°": "°", "": ""}.get(unit, f" {unit}"))
        return f"{tol[0] * scale:g} / {tol[1] * scale:g}{u}"
    return "; ".join(one(*lim) for lim in lims) or "—"


def path_bs(bs: list[dict], path: SignalPath) -> list[dict]:
    return [b for b in bs if b["stage"] == path.stage]


def row_hits(bs: list[dict], row: Row) -> list[list[Hit]]:
    """Per column, every value of the row in the path's baselines."""
    return [[h for b in bs for s in sources for h in hits(b, s) if row.unit is None or h.unit == row.unit]
            for sources in row.columns()]


def path_table(bs: list[dict], path: SignalPath) -> str:
    """The path's caption and its table: only the columns some row has a value in."""
    pbs = path_bs(bs, path)
    lv = levels_text(pbs)
    head = f"**{path.title}{', ' + lv if lv else ''}.** {path.caption}"
    if not pbs:
        return head + " Not run on this build."
    grid = [(row, row_hits(pbs, row)) for row in path.rows]
    used = [i for i in range(len(COLUMNS)) if any(cols[i] for _, cols in grid)]
    out = ["| quantity | " + " | ".join(COLUMNS[i] for i in used) + " | limit (pass / warn) |",
           "|---|" + "---|" * (len(used) + 1)]
    for row, cols in grid:
        ctx = context_text(pbs, row)
        out.append(f"| {row.label} | " + " | ".join(cell_text(cols[i]) + (ctx if cols[i] else "") for i in used)
                   + f" | {limit_text(limits([h for c in cols for h in c]))} |")
    return head + "\n\n" + "\n".join(out)


def table(bs: list[dict]) -> str:
    return "\n\n".join(path_table(bs, p) for p in PATHS)


def disparities(bs: list[dict]) -> list[str]:
    """`<path>: <quantity>: <column>` of every warn or FAIL cell, then `<path>: <check>` of every
    warn or FAIL check that no row shows: a judged exception outside the tables still needs its
    section, or a reader of the tables would not know it is there."""
    out, shown = [], set()
    for p in PATHS:
        for row in p.rows:
            for col, hs in zip(COLUMNS, row_hits(path_bs(bs, p), row)):
                shown.update((id(h.b), k) for h in hs for k in h.key.split(" minus "))
                w = worst(hs)
                if w is not None and w.status in ("WARN", "FAIL"):
                    out.append(f"{p.name}: {row.label}: {col}")
    for b in bs:
        p = next((p for p in PATHS if p.stage == b["stage"]), None)
        for k, e in b["checks"].items():
            if p is not None and e["status"] in ("WARN", "FAIL") and (id(b), k) not in shown:
                out.append(f"{p.name}: {k.partition('.')[2]}")
    return out


def mixed_limits(bs: list[dict]) -> list[str]:
    """Rows whose values are held to more than one limit: their cells would not compare."""
    out = []
    for p in PATHS:
        for row in p.rows:
            lims = limits([h for c in row_hits(path_bs(bs, p), row) for h in c])
            if len(lims) > 1:
                out.append(f"{p.name}: {row.label!r} mixes limits ({limit_text(lims)}): split the row")
    return out


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
    without a source, rows that mix limits, disparities without a section and sections without
    a disparity."""
    for name, text in blocks(root).items():
        doc = splice(doc, name, text)
    doc, problems = Q.refresh(doc, Path(root))
    problems += Q.unquoted(hand_written(doc))
    bs = [b for _, b in current(load(Path(root)))]
    problems += mixed_limits(bs)
    want, have = disparities(bs), sections(doc)
    problems += [f"the {d!r} cell is warn or FAIL: it needs a `### {d}` section under {DISPARITIES!r}"
                 for d in want if d not in have]
    problems += [f"`### {s}` under {DISPARITIES!r} has no warn or FAIL cell: remove it"
                 for s in have if s not in want]
    return doc, problems


def update(doc_path: Path = DOC, root: Path = BASELINES, check: bool = False) -> tuple[bool, list[str]]:
    """Rewrites the generated parts and the quoted numbers (unless `check`); returns (the document
    was current and has no problem, the problems). Problems are printed; the rewrite still
    happens, so the tables follow the baselines while the prose is being brought up to date."""
    doc = Path(doc_path).read_text()
    new, problems = build(doc, root)
    for p in problems:
        print(f"{Path(doc_path).name}: {p}", file=sys.stderr)
    if not check and new != doc:
        Path(doc_path).write_text(new)
    return new == doc and not problems, problems
