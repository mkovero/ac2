"""Numbers quoted in the prose of `comparison.md`, held to their sources.

A quoted number carries its source in an HTML comment right after it (hidden when rendered):

    +1.17 dB<!--= b pupu/genelec-50dbfs // genelec.spl.REW (SPL, cal from ac2)@50Hz -->

`python -m crosscheck comparison` rewrites the number from the source at the precision and sign
style it is written with (written unsigned: the magnitude), and `--check` reports one that
differs. Sources, fields separated by ` // `:

- `b <rig>/<baseline> // <check key> [minus <check key>] [// <field>]`: a committed baseline on
  the document's build. The field is `value` (default), `mean`, `spread`, `n`, `of` or
  `tol.0` / `tol.1`.
- `t <dotted.path>`: a limit in `tolerances.toml`.

Any other measured-looking number in the prose (a decimal, a number with a measured unit, an
`n of m` count) or a build hash is refused by `unquoted()`: nothing would keep it current.
Definitional constants are listed in `ALLOWED` with why each needs no source."""
from __future__ import annotations

import json
import re
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOLERANCES = ROOT / "tolerances.toml"

NUM = r"[+−-]?\d+(?:\.\d+)?"
QUOTE = re.compile(rf"(?P<num>{NUM})(?P<mid>[^\S\n]?(?:%|°|dB|µs|samples?)?)<!--=\s*(?P<ref>.*?)\s*-->")

# Phrases whose numbers are definitions, not measurements: (pattern, why it needs no source)
ALLOWED: list[tuple[str, str]] = [
    (r"REW 5\.40", "the REW version the suite drives"),
    (r"v1\.5\.2", "the OSM version osm-harness builds"),
    (r"6167[23]-1|61260-1", "standard numbers"),
    (r"γ² ≥ 0\.9[59]", "the coherence gates the suite applies (README)"),
    (r"\d+ Hz / \d+(?:\.\d+)? s", "sweep names: start frequency and length"),
]
MEASURED = re.compile(
    r"(?<![\w.])[+−-]?\d+\.\d+"                             # a decimal
    r"|(?<![\w.])[+−-]?\d+[^\S\n]?(?:%|°|dB\b|µs)"          # a number with a measured unit
    r"|\b\d+ of \d+\b"                                      # a count
    r"|\b(?=[0-9a-f]*\d)(?=[0-9a-f]*[a-f])[0-9a-f]{7,12}\b")  # a build hash


def _field(e: dict, field: str) -> float | None:
    for part in field.split("."):
        if e is None:
            return None
        e = e[int(part)] if isinstance(e, list) else e.get(part)
    return e


def _baseline(args: list[str], root: Path) -> float:
    where, keys, *rest = args
    field = rest[0] if rest else "value"
    b = json.loads((Path(root) / f"{where}.json").read_text())
    from . import comparison as C
    build = C.doc_build(C.load(Path(root)))
    if C.build_of(b) != build:
        # a quote from another build would mix builds the table keeps apart
        raise LookupError(f"{where} is on ac2 build {C.build_of(b)}, not the document's {build}")
    vals = []
    for key in keys.split(" minus "):
        e = b["checks"].get(key)
        if e is None:
            raise LookupError(f"{where}: no check {key!r}")
        v = _field(e, field)
        if v is None:
            raise LookupError(f"{where}: {key!r} has no {field}")
        vals.append(float(v))
    return vals[0] - sum(vals[1:])


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
    if kind == "t":
        return _tolerance(args)
    raise LookupError(f"unknown source kind {kind!r}")


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
    """Measured-looking numbers and build hashes in the prose with no source."""
    out = []
    for n, line in enumerate(text.splitlines(), 1):
        s = QUOTE.sub(" ", line)
        s = re.sub(r"<!--.*?-->|`[^`]*`|\]\([^)]*\)", " ", s)
        for pat, _why in ALLOWED:
            s = re.sub(pat, " ", s)
        out += [f"line {n}: {m[0]!r} has no source (quote it from a baseline or tolerances.toml; "
                f"the build is named in the generated part only)" for m in MEASURED.finditer(s)]
    return out
