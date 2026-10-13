"""Numbers quoted in the hand-written parts of `comparison.md`, held to their sources.

A quoted number carries its source in an HTML comment right after it (hidden when rendered):

    +4.81 dB<!--= b pupu/genelec-30dbfs // genelec.h2.REW offline import@5000Hz -->

`python -m crosscheck comparison` rewrites the number from the source at the precision and sign
style it is written with (written unsigned: the magnitude), and `--check` reports one that
differs. Sources, fields separated by ` // ` (a `|` would split a table cell):

- `b <rig>/<baseline> // <check key> [minus <check key>] [// <field>]`: a committed baseline.
  The field is `value` (default), `mean`, `spread`, `n`, `of`, `f_hz`, `shared.<name>` or
  `tol.0` / `tol.1`; a trailing `*100` reads a share as a percentage.
- `r <report> // <heading> [> <heading>] // <column>=<cell>[; …] // <column>[#i]`: a cell of a
  committed `reports/` table, the i-th number in it (default 0). The headings narrow down to the
  tables under the last one; the row is the first whose cells match.
- `t <dotted.path>`: a limit in `tolerances.toml`.

Every other measured-looking number in the hand-written text (a decimal, a number with a
measured unit, an `n of m` count) or a build hash is refused by `unquoted()`: it would be a
number nothing keeps current. Definitional constants (standards, versions, sweep names,
estimator settings) are listed in `ALLOWED` with why each needs no source."""
from __future__ import annotations

import json
import re
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REPORTS = ROOT / "reports"
TOLERANCES = ROOT / "tolerances.toml"

NUM = r"[+−-]?\d+(?:\.\d+)?"
QUOTE = re.compile(rf"(?P<num>{NUM})(?P<mid>[^\S\n]?(?:%|°|dBr|dB|µs|samples?|sem)?)<!--=\s*(?P<ref>.*?)\s*-->")

# Phrases whose numbers are definitions, not measurements: (pattern, why it needs no source)
ALLOWED: list[tuple[str, str]] = [
    (r"REW 5\.40", "the REW version the suite drives"),
    (r"v1\.5\.2", "the OSM version osm-harness builds"),
    (r"6167[23]-1|61260-1", "standard numbers"),
    (r"γ² ≥ 0\.9[59]", "the coherence gates the suite applies (README)"),
    (r"\d+ Hz / \d+(?:\.\d+)? s", "sweep names: start frequency and length"),
    (r"\+?3\.01 dB", "a sine's peak over its RMS, 20·log10 √2"),
    (r"\+1\.05 dB", "the Rayleigh amplitude mean under the power mean, −20·log10 √(π/4)"),
    (r"0\.68 s", "OSM's FFT16 window at 96 kHz, 65536 / 96000 s"),
    (r"7\.2 kHz", "the digital DUT's designed low-pass corner (crosscheck/dut.py)"),
    (r"\d+ dB SNR", "fixture settings of the OSM stage and the coherence study"),
    (r"(?<![\d.])0 dB\b", "the transfer of the noise fixtures, whose meas is the ref plus uncorrelated noise"),
    (r"true 0\.5", "γ² at 0 dB SNR, 1 / (1 + 1)"),
    (r"\d+-frame", "the dummy JACK server's period"),
    (r"70 dB under the reference peak", "OSM's float32 floor as the suite masks it, osm.REF_FLOOR_DB"),
]
RUN_ID = r"\b\d{8}T\d{6}Z\b"
MEASURED = re.compile(
    r"(?<![\w.])[+−-]?\d+\.\d+"                                 # a decimal
    r"|(?<![\w.])[+−-]?\d+[^\S\n]?(?:%|°|dBr\b|dB\b|µs|sem\b)"  # a number with a measured unit
    r"|\b\d+ of \d+\b"                                          # a count
    r"|\b(?=[0-9a-f]*\d)(?=[0-9a-f]*[a-f])[0-9a-f]{7,12}\b")    # a build hash


# ------------------------------------------------------------------ sources
def _field(e: dict, field: str) -> float | None:
    for part in field.split("."):
        if e is None:
            return None
        e = e[int(part)] if isinstance(e, list) else e.get(part)
    return e


def _baseline(args: list[str], root: Path) -> float:
    where, keys, *rest = args
    field = rest[0] if rest else "value"
    scale = 1.0
    if field.endswith("*100"):
        field, scale = field[:-4], 100.0
    b = json.loads((Path(root) / f"{where}.json").read_text())
    vals = []
    for key in keys.split(" minus "):
        e = b["checks"].get(key)
        if e is None:
            raise LookupError(f"{where}: no check {key!r}")
        v = _field(e, field)
        if v is None:
            raise LookupError(f"{where}: {key!r} has no {field}")
        vals.append(float(v))
    return (vals[0] - sum(vals[1:])) * scale


def _tables(text: str, headings: list[str]) -> list[list[list[str]]]:
    """The tables under the last of the headings (each found after the one before it)."""
    lines = text.splitlines()
    i = 0
    for h in headings:
        i = next((j for j in range(i, len(lines)) if lines[j].startswith("#") and h in lines[j]), None)
        if i is None:
            raise LookupError(f"no heading with {h!r}")
        i += 1
    tables, cur = [], []
    for line in lines[i:]:
        if line.startswith("#"):
            break
        if line.startswith("|"):
            cur.append([c.strip() for c in line.strip().strip("|").split(" | ")])
        elif cur:
            tables.append(cur)
            cur = []
    return tables + ([cur] if cur else [])


def _report(args: list[str]) -> float:
    name, heads, sel, col = args
    col, _, idx = col.partition("#")
    want = dict(kv.split("=", 1) for kv in sel.split("; "))
    for t in _tables((REPORTS / name).read_text(), [h.strip() for h in heads.split(">")]):
        head = t[0]
        if col not in head or any(k not in head for k in want):
            continue
        for row in t[2:]:
            cells = dict(zip(head, row))
            if all(cells.get(k) == v for k, v in want.items()):
                nums = re.findall(NUM, cells[col].replace("−", "-"))
                if int(idx or 0) >= len(nums):
                    raise LookupError(f"{name}: {col!r} of {sel!r} has no number #{idx or 0}")
                return float(nums[int(idx or 0)].replace("−", "-"))
    raise LookupError(f"{name}: no row {sel!r} with column {col!r} under {heads!r}")


def _tolerance(args: list[str]) -> float:
    v = tomllib.loads(TOLERANCES.read_text())
    for part in args[0].split("."):
        v = v[part]
    return float(v)


def resolve(ref: str, root: Path) -> float:
    kind, _, rest = ref.partition(" ")
    args = [a.strip() for a in rest.split(" // ")]
    if kind == "b":
        return _baseline(args, root)
    if kind == "r":
        return _report(args)
    if kind == "t":
        return _tolerance(args)
    raise LookupError(f"unknown source kind {kind!r}")


# ------------------------------------------------------------------ rewrite and check
def written_as(v: float, written: str) -> str:
    """`v` at the precision and sign style of `written`."""
    w = written.replace("−", "-")
    d = len(w.split(".")[1]) if "." in w else 0
    s = f"{v:+.{d}f}" if w[0] in "+-" else f"{abs(v):.{d}f}"
    return s.replace("-", "−")


def refresh(doc: str, root: Path) -> tuple[str, list[str]]:
    """The doc with every quoted number rewritten from its source, and the sources that failed."""
    problems = []

    def one(m: re.Match) -> str:
        try:
            new = written_as(resolve(m["ref"], root), m["num"])
        except (LookupError, KeyError, ValueError, OSError) as ex:
            line = doc.count("\n", 0, m.start()) + 1
            problems.append(f"line {line}: {m['num']}{m['mid']}: {ex}")
            return m[0]
        return new + m[0][len(m["num"]):]
    return QUOTE.sub(one, doc), problems


def unquoted(text: str) -> list[str]:
    """Measured-looking numbers and build hashes in hand-written text with no source."""
    out = []
    for n, line in enumerate(text.splitlines(), 1):
        s = QUOTE.sub(" ", line)
        s = re.sub(r"<!--.*?-->|`[^`]*`|\]\([^)]*\)|" + RUN_ID, " ", s)
        for pat, _why in ALLOWED:
            s = re.sub(pat, " ", s)
        out += [f"line {n}: {m[0]!r} has no source (quote it from a baseline, report or "
                f"tolerances.toml; build hashes go in the generated provenance only)"
                for m in MEASURED.finditer(s)]
    return out
