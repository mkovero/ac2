"""The recordings behind the baselines and reports, as archives others can fetch (README,
"Recordings"): `data pack` writes one deterministic .tar.gz per run plus manifest.json (SHA-256
and size of each archive and of each file in it, and which baseline or report cites which run);
`data fetch` downloads from the manifest's URL template, refuses a hash mismatch and unpacks
within the manifest's size and entry counts; `data verify` re-hashes what is on disk.

Stdlib only, so a fetch needs nothing beyond Python."""
from __future__ import annotations

import gzip
import hashlib
import fnmatch
import json
import os
import re
import shutil
import sys
import tarfile
import urllib.parse
import urllib.request
from pathlib import Path, PurePosixPath

SUITE = Path(__file__).resolve().parent.parent
FORMAT = "ac2-crosscheck-recordings"
VERSION = 1
# `.invalid` never resolves (RFC 2606): an unpublished manifest cannot fetch from anywhere.
PLACEHOLDER_URL = "https://recordings.invalid/ac2-crosscheck/{archive}"
RUN_ID = re.compile(r"\b(\d{8}T\d{6}Z|osm-[0-9a-f]{8,})\b")
# Hard ceilings over whatever a manifest claims: a hostile manifest cannot ask for more.
MAX_ENTRIES = 20_000
MAX_UNPACKED = 32 << 30
CHUNK = 1 << 20

# What an analysis-only re-run does not read: the report the analysis rewrites (results.json
# stays, `repeats`, `dutcov`, `compare` and `baseline` read it), its plots, the daemon log, and
# REW's rec_meas/rec_ref.wav (the two columns of rec.wav, split only for REW's import).
# Everything else in a run is either an input of `analyse` or provenance (manifest.json, rig.toml,
# cal/) that the report quotes.
ANALYSIS_DROP = (re.compile(r"^report/(?!results\.json$)"), re.compile(r"(^|/)ac2\.log$"),
                 re.compile(r"\.png$"), re.compile(r"(^|/)rew/rec_(meas|ref)\.wav$"))


class Refused(Exception):
    """A download or archive that does not match the manifest, or an unsafe archive."""


def default_data_dir() -> Path:
    """Where fetched runs go: $CROSSCHECK_RUNS, else runs/ in the suite (where osm.toml's
    relative recording paths and rig-run.sh look too)."""
    env = os.environ.get("CROSSCHECK_RUNS")
    return Path(env) if env else SUITE / "runs"


def _sha256_file(p: Path) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        while b := f.read(CHUNK):
            h.update(b)
    return h.hexdigest()


def select(run_dir: Path, profile: str = "analysis", exclude: tuple[str, ...] = ()) -> tuple[list[str], list[str]]:
    """Run-relative POSIX paths of the regular files to pack, sorted, and those an `exclude`
    pattern (fnmatch on `<run>/<path>`) left out."""
    out, dropped = [], []
    for p in sorted(run_dir.rglob("*")):
        if p.is_symlink() or not p.is_file():
            continue
        rel = p.relative_to(run_dir).as_posix()
        if profile == "analysis" and any(r.search(rel) for r in ANALYSIS_DROP):
            continue
        if any(fnmatch.fnmatchcase(f"{run_dir.name}/{rel}", x) for x in exclude):
            dropped.append(rel)
            continue
        out.append(rel)
    return out, dropped


def suite_cites(suite: Path = SUITE) -> dict[str, list[str]]:
    """Suite document (relative path) -> run IDs it cites: each baseline's provenance.run, the
    run IDs named in reports/*.md and comparison.md, and the runs osm.toml's recordings live in."""
    cites: dict[str, list[str]] = {}
    for b in sorted((suite / "baselines").glob("*/*.json")):
        run = (json.loads(b.read_text()).get("provenance") or {}).get("run")
        if run:
            cites[b.relative_to(suite).as_posix()] = [run]
    for d in [*sorted((suite / "reports").glob("*.md")), suite / "comparison.md"]:
        if d.exists():
            ids = sorted(set(RUN_ID.findall(d.read_text())))
            if ids:
                cites[d.relative_to(suite).as_posix()] = ids
    osm = suite / "osm.toml"
    if osm.exists():
        import tomllib
        ids = sorted({PurePosixPath(r["wav"]).parts[0] for r in tomllib.loads(osm.read_text()).get("recordings", [])
                      if RUN_ID.fullmatch(PurePosixPath(r["wav"]).parts[0])})
        if ids:
            cites["osm.toml"] = ids
    return cites


def _describe(run_dir: Path) -> dict:
    m = run_dir / "manifest.json"
    if not m.exists():
        return {}
    j = json.loads(m.read_text())
    flags = j.get("flags") or {}
    return {k: v for k, v in (("rig", j.get("rig")), ("started", j.get("started")),
                               ("stages", flags.get("stages")), ("ac2_build", j.get("ac2_version")))
            if v is not None}


def _write_tar(run_dir: Path, files: list[str], dest: Path) -> None:
    """A tar.gz whose bytes depend only on the files' contents and names: fixed mtime, owner
    and modes, sorted entries, no gzip name or time, so a re-pack hashes the same."""
    with open(dest, "wb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0,
                                                 compresslevel=6) as gz, \
            tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT) as tar:
        for rel in files:
            p = run_dir / rel
            ti = tarfile.TarInfo(f"{run_dir.name}/{rel}")
            ti.size = p.stat().st_size
            ti.mtime, ti.mode, ti.uid, ti.gid, ti.uname, ti.gname = 0, 0o644, 0, 0, "", ""
            with open(p, "rb") as f:
                tar.addfile(ti, f)


def pack(run_dirs: list[Path], out: Path, profile: str = "analysis", suite: Path = SUITE,
         url_template: str = PLACEHOLDER_URL, exclude: tuple[str, ...] = (), log=print) -> dict:
    out.mkdir(parents=True, exist_ok=True)
    names = [r.name for r in run_dirs]
    if len(set(names)) != len(names):
        raise ValueError("two runs with the same name")
    runs = {}
    for r in run_dirs:
        files, dropped = select(r, profile, exclude)
        if not files:
            raise ValueError(f"{r}: nothing to pack")
        arc = out / f"{r.name}.tar.gz"
        _write_tar(r, files, arc)
        listing = {rel: {"size": (r / rel).stat().st_size, "sha256": _sha256_file(r / rel)} for rel in files}
        runs[r.name] = {"archive": arc.name, "size": arc.stat().st_size, "sha256": _sha256_file(arc),
                        "unpacked_bytes": sum(v["size"] for v in listing.values()), "entries": len(listing),
                        **_describe(r), "excluded": dropped, "files": listing}
        log(f"{arc}: {runs[r.name]['entries']} files, {runs[r.name]['unpacked_bytes'] / 1e6:.1f} MB "
            f"-> {runs[r.name]['size'] / 1e6:.1f} MB")
    cites = suite_cites(suite)
    cited = {x for ids in cites.values() for x in ids}
    man = {"format": FORMAT, "version": VERSION, "profile": profile, "url_template": url_template,
           "runs": runs, "cites": cites,
           "cited_not_packed": sorted(cited - set(runs)), "packed_not_cited": sorted(set(runs) - cited)}
    (out / "manifest.json").write_text(json.dumps(man, indent=1, ensure_ascii=False) + "\n")
    return man


def _open(src: str):
    """A manifest or archive source: a local path, or a URL (file://, https://)."""
    if re.match(r"^[a-z][a-z0-9+.-]*://", src):
        if urllib.parse.urlparse(src).scheme not in ("file", "https", "http"):
            raise Refused(f"{src}: unsupported URL scheme")
        return urllib.request.urlopen(src, timeout=60)  # noqa: S310 (scheme checked above)
    return open(src, "rb")


def load_manifest(src: str) -> dict:
    with _open(src) as f:
        man = json.loads(f.read(64 << 20))
    if man.get("format") != FORMAT or man.get("version") != VERSION:
        raise Refused(f"{src}: not a {FORMAT} v{VERSION} manifest")
    for name, e in man["runs"].items():
        if not RUN_ID.fullmatch(name) or e.get("archive") != f"{name}.tar.gz":
            raise Refused(f"{src}: run {name!r}: bad name or archive")
    return man


def _check_name(member: tarfile.TarInfo, run: str) -> str:
    """The run-relative path of a safe regular-file entry under <run>/, or Refused."""
    if not member.isfile():
        raise Refused(f"{member.name}: not a regular file")
    p = PurePosixPath(member.name)
    if p.is_absolute() or "\\" in member.name or any(x in ("..", "", ".") for x in member.name.split("/")) \
            or p.parts[0] != run or len(p.parts) < 2:
        raise Refused(f"{member.name}: outside {run}/")
    return PurePosixPath(*p.parts[1:]).as_posix()


def unpack(archive: Path, run: str, entry: dict, dest: Path) -> Path:
    """Unpack one verified archive to dest/<run>: every member a regular file under <run>/ that
    the manifest lists, at its listed size and hash, no more entries or bytes than listed (and
    never past the hard ceilings). Written to a staging directory first, renamed when whole."""
    files = entry["files"]
    max_entries = min(int(entry["entries"]), MAX_ENTRIES)
    max_bytes = min(int(entry["unpacked_bytes"]), MAX_UNPACKED)
    stage = dest / f".unpack-{run}"
    if stage.exists():
        shutil.rmtree(stage)
    stage.mkdir(parents=True)
    seen, total = set(), 0
    try:
        with tarfile.open(archive, "r:gz") as tar:
            for m in tar:
                rel = _check_name(m, run)
                if rel in seen or rel not in files:
                    raise Refused(f"{m.name}: duplicate or not in the manifest")
                seen.add(rel)
                if len(seen) > max_entries:
                    raise Refused(f"{archive.name}: more than {max_entries} entries")
                want = files[rel]
                if m.size != want["size"] or total + m.size > max_bytes:
                    raise Refused(f"{m.name}: size {m.size} (manifest {want['size']}, budget {max_bytes - total})")
                total += m.size
                out = stage / rel
                out.parent.mkdir(parents=True, exist_ok=True)
                h, n = hashlib.sha256(), 0
                src = tar.extractfile(m)
                with open(out, "xb") as f:
                    while b := src.read(CHUNK):
                        n += len(b)
                        if n > m.size:
                            raise Refused(f"{m.name}: longer than its header")
                        h.update(b)
                        f.write(b)
                if h.hexdigest() != want["sha256"]:
                    raise Refused(f"{m.name}: SHA-256 differs from the manifest")
        if seen != set(files):
            raise Refused(f"{archive.name}: missing {sorted(set(files) - seen)[:5]}")
        final = dest / run
        if final.exists():
            shutil.rmtree(final)
        stage.rename(final)
        return final
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def _download(url: str, part: Path, size: int, sha: str) -> None:
    h, n = hashlib.sha256(), 0
    try:
        with _open(url) as src, open(part, "wb") as f:
            while b := src.read(CHUNK):
                n += len(b)
                if n > size:
                    raise Refused(f"{url}: larger than the manifest's {size} bytes")
                h.update(b)
                f.write(b)
        if n != size or h.hexdigest() != sha:
            raise Refused(f"{url}: {n} bytes, SHA-256 {h.hexdigest()[:16]}…; the manifest says {size}, {sha[:16]}…")
    except BaseException:
        part.unlink(missing_ok=True)
        raise


def check_tree(run_dir: Path, entry: dict) -> list[str]:
    """Differences between an unpacked run and its manifest listing (empty: matches)."""
    if not run_dir.is_dir():
        return [f"{run_dir}: missing"]
    probs = []
    for rel, want in entry["files"].items():
        p = run_dir / rel
        if not p.is_file():
            probs.append(f"{run_dir.name}/{rel}: missing")
        elif p.stat().st_size != want["size"] or _sha256_file(p) != want["sha256"]:
            probs.append(f"{run_dir.name}/{rel}: differs from the manifest")
    return probs


def _pick(man: dict, runs: list[str] | None) -> list[str]:
    if not runs:
        return sorted(man["runs"])
    unknown = [r for r in runs if r not in man["runs"]]
    if unknown:
        raise Refused(f"not in the manifest: {', '.join(unknown)}")
    return runs


def fetch(manifest: str, dest: Path, runs: list[str] | None = None, url_template: str | None = None,
          keep: bool = False, log=print) -> None:
    man = load_manifest(manifest)
    tmpl = url_template or man["url_template"]
    if "{archive}" not in tmpl:
        raise Refused(f"URL template {tmpl!r} has no {{archive}}")
    if urllib.parse.urlparse(tmpl).hostname == "recordings.invalid":
        raise Refused("the manifest's URL is the unpublished placeholder; pass --url-template")
    dest.mkdir(parents=True, exist_ok=True)
    for run in _pick(man, runs):
        e = man["runs"][run]
        if not check_tree(dest / run, e):
            log(f"{dest / run}: already present and matching")
            continue
        url = tmpl.format(archive=e["archive"])
        part = dest / f".{e['archive']}.part"
        log(f"{run}: fetching {url} ({e['size'] / 1e6:.1f} MB)")
        _download(url, part, int(e["size"]), e["sha256"])
        try:
            unpack(part, run, e, dest)
            if keep:
                part.rename(dest / e["archive"])
        finally:
            part.unlink(missing_ok=True)
        log(f"{dest / run}: {e['entries']} files verified")


def verify(manifest: str, dest: Path, runs: list[str] | None = None, archives: Path | None = None,
           log=print) -> list[str]:
    """Problems with the unpacked runs under dest (and, with `archives`, the archives there)."""
    man = load_manifest(manifest)
    probs = []
    for run in _pick(man, runs):
        e = man["runs"][run]
        if archives is not None:
            a = archives / e["archive"]
            if not a.is_file():
                probs.append(f"{a}: missing")
            elif a.stat().st_size != e["size"] or _sha256_file(a) != e["sha256"]:
                probs.append(f"{a}: differs from the manifest")
        else:
            probs += check_tree(dest / run, e)
    for p in probs:
        log(p)
    return probs


def _resolve_run(s: str, runs_dir: Path) -> Path:
    p = Path(s)
    if p.is_dir():
        return p.resolve()
    if (runs_dir / s).is_dir():
        return (runs_dir / s).resolve()
    raise FileNotFoundError(f"{s}: no such run (looked in . and {runs_dir})")


def register(sub) -> None:
    d = sub.add_parser("data", help="the recordings behind the baselines: pack, fetch, verify (README, Recordings)")
    ds = d.add_subparsers(dest="data_cmd", required=True)
    p = ds.add_parser("pack", help="one .tar.gz per run plus manifest.json (SHA-256, sizes, what cites what)")
    p.add_argument("runs", nargs="+", help="run directories, or run IDs under --runs-dir")
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--runs-dir", type=Path, help="where run IDs resolve (default: $CROSSCHECK_RUNS, else "
                                                 "/work/ac2-crosscheck/runs, else runs/)")
    p.add_argument("--all-files", action="store_true", help="pack the whole run, not only what analysis reads")
    p.add_argument("--url-template", default=PLACEHOLDER_URL, help="where the archives will be, with {archive}")
    p.add_argument("--exclude", action="append", default=[], metavar="GLOB",
                   help="leave out files matching <run>/<path> (repeatable), e.g. '*/ambient/in1.wav'; "
                        "the manifest lists what was left out")
    for name, hlp in (("fetch", "download the runs, refuse a hash mismatch, unpack under the data directory"),
                      ("verify", "re-hash the unpacked runs (or, with --archives, the archives) against the manifest")):
        q = ds.add_parser(name, help=hlp)
        q.add_argument("--manifest", required=True, help="manifest.json: a path or a URL")
        q.add_argument("runs", nargs="*", help="only these run IDs (default: all in the manifest)")
        q.add_argument("--dir", type=Path, help="data directory (default: $CROSSCHECK_RUNS, else the suite's runs/)")
        if name == "fetch":
            q.add_argument("--url-template", help="override the manifest's, e.g. file:///mnt/usb/{archive}")
            q.add_argument("--keep-archives", action="store_true", help="keep the .tar.gz beside the runs")
        else:
            q.add_argument("--archives", type=Path, help="check the archives in this directory instead")


def main(a) -> int:
    try:
        if a.data_cmd == "pack":
            runs_dir = a.runs_dir or (Path(os.environ["CROSSCHECK_RUNS"]) if os.environ.get("CROSSCHECK_RUNS")
                                      else Path("/work/ac2-crosscheck/runs") if Path("/work/ac2-crosscheck/runs").is_dir()
                                      else Path("runs"))
            dirs = [_resolve_run(r, runs_dir) for r in a.runs]
            man = pack(dirs, a.out, "all" if a.all_files else "analysis", url_template=a.url_template,
                       exclude=tuple(a.exclude))
            tot = sum(e["size"] for e in man["runs"].values())
            print(f"{a.out / 'manifest.json'}: {len(man['runs'])} runs, {tot / 1e6:.1f} MB")
            if man["cited_not_packed"]:
                print("cited but not packed: " + ", ".join(man["cited_not_packed"]))
            return 0
        dest = a.dir or default_data_dir()
        if a.data_cmd == "fetch":
            fetch(a.manifest, dest, a.runs or None, a.url_template, a.keep_archives)
            return 0
        probs = verify(a.manifest, dest, a.runs or None, a.archives)
        print(f"{len(probs)} problem(s)" if probs else "all match the manifest")
        return 1 if probs else 0
    except Refused as e:
        print(f"refused: {e}", file=sys.stderr)
        return 1
    except (FileNotFoundError, ValueError, OSError) as e:
        print(f"stopped: {e}", file=sys.stderr)
        return 2
