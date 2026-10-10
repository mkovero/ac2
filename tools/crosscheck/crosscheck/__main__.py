"""python -m crosscheck {preflight,run,analyse,baseline,compare,repeats,osm,comparison} — see README.md."""
from __future__ import annotations

import argparse
import sys
from pathlib import Path


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(prog="crosscheck", description="ac2 vs REW vs steady-sine cross-check")
    sub = ap.add_subparsers(dest="cmd", required=True)

    an = sub.add_parser("analyse", help="analyse a run directory or the 2026-10-07 fixtures (offline, pure)")
    an.add_argument("source", type=Path)
    an.add_argument("--out", type=Path, help="report directory (default: <source>/report)")
    an.add_argument("--tolerances", type=Path, help="tolerances TOML (default: the suite's)")
    an.add_argument("--no-plots", action="store_true")

    bl = sub.add_parser("baseline", help="write the baselines (one per stage and level) from a reviewed run")
    bl.add_argument("source", type=Path, help="run directory with report/results.json")
    bl.add_argument("--stage", action="append", help="only this stage (repeatable; default: all in the run)")
    bl.add_argument("--dir", type=Path, help="baseline directory (default: the suite's baselines/)")
    bl.add_argument("--force", action="store_true", help="accept a run with FAILs")
    bl.add_argument("--tolerances", type=Path, help="tolerances TOML (default: the suite's)")

    cp = sub.add_parser("compare", help="compare a run with the baselines of its stages and levels")
    cp.add_argument("source", type=Path, help="run directory with report/results.json")
    cp.add_argument("--baseline", type=Path, help="baseline file or directory (default: the suite's baselines/)")
    cp.add_argument("--stage", action="append", help="only this stage (repeatable)")
    cp.add_argument("--out", type=Path, help="where compare.md goes (default: <source>/report)")
    cp.add_argument("--tolerances", type=Path, help="tolerances TOML (default: the suite's)")

    rp = sub.add_parser("repeats", help="repeated takes of a stage and level: each check's signed value across "
                                        "takes (n, mean, sd, range) and the column-to-tone split at the sines")
    rp.add_argument("sources", type=Path, nargs="+", help="run directories with report/results.json, or baseline files")
    rp.add_argument("--match", help="only checks whose key matches this regular expression (default: the focus sets)")
    rp.add_argument("--out", type=Path, help="write the markdown here (default: stdout)")

    om = sub.add_parser("osm", help="the OSM stage: ac2 vs Open Sound Meter's DSP on WAV pairs (offline, no rig; "
                                    "SKIP without OSM_HARNESS)")
    om.add_argument("--out", type=Path, help="run directory (default: runs/osm-<UTC time>)")
    om.add_argument("--config", type=Path, help="stage config (default: the suite's osm.toml)")
    om.add_argument("--cases", help="comma list of synthetic cases (default: osm.toml's [cases].run)")
    om.add_argument("--no-recordings", action="store_true", help="synthetic cases only")
    om.add_argument("--tolerances", type=Path, help="tolerances TOML (default: the suite's)")
    om.add_argument("--no-plots", action="store_true")

    cm = sub.add_parser("comparison", help="rebuild the results tables of comparison.md from the baselines")
    cm.add_argument("--check", action="store_true", help="write nothing; exit 1 when the tables are stale")
    cm.add_argument("--doc", type=Path, help="document (default: the suite's comparison.md)")
    cm.add_argument("--baselines", type=Path, help="baseline directory (default: the suite's baselines/)")

    for name, hlp in (("preflight", "read-only checks of the rig, no emission"),
                      ("run", "preflight, then the stages; emitting stages need the flags below")):
        p = sub.add_parser(name, help=hlp)
        p.add_argument("--rig", type=Path, required=True, help="rig TOML (rigs/pupu.toml)")
        p.add_argument("--out", type=Path, help="run directory (default: runs/<UTC time>)")
        if name == "run":
            p.add_argument("--emit", help="level for electrical-only stages, e.g. -50dbfs (required to emit)")
            p.add_argument("--emit-speaker", help="level for the speaker stage, e.g. -50dbfs (above -50 only "
                                                   "with --allow-speaker-level)")
            p.add_argument("--allow-speaker-level",
                           help="operator present: lift the speaker ceiling and ac2d's bound for the speaker "
                                "stage only, e.g. -30dbfs (never above -30; drop-in removed after the stage)")
            p.add_argument("--allow-electrical-level",
                           help="lift ac2d's bound for the electrical stages only, e.g. -30dbfs (systemd "
                                "runtime drop-in, restored afterwards)")
            p.add_argument("--stages", default="ambient,genelec,xone",
                           help="comma list from ambient,genelec,xone,dut (order is fixed; dut: the digital "
                                "DUT, README)")
            p.add_argument("--skip", default="", help="comma list of sub-stages to skip: sine,rew,ac2_sweep,ac2_tf")
            p.add_argument("--yes", action="store_true", help="don't wait for Enter before audible stages")
            p.add_argument("--resolution", default="1/48", choices=["1/12", "1/24", "1/48", "1/96"],
                           help="ac2's sweep and TF Resolution (columns per octave); a non-default one gets "
                                "baselines of its own (<stage>-<level>dbfs-r<N>.json)")

    a = ap.parse_args(argv)
    if a.cmd == "analyse":
        import tomllib
        import warnings

        import numpy as np
        np.seterr(all="ignore")  # empty bands and silent columns are NaN by design
        warnings.simplefilter("ignore", RuntimeWarning)

        from . import analyse, report
        tol = tomllib.loads(a.tolerances.read_text()) if a.tolerances else None
        res, an_ = analyse.analyse(a.source, tol)
        out = a.out or (a.source / "report")
        path = report.write(res, an_, out, plots=not a.no_plots)
        s = res["summary"]
        print(f"{path}: " + ", ".join(f"{k} {s.get(k, 0)}" for k in report.STATUS_ORDER))
        return 1 if s.get("FAIL") else 0
    if a.cmd == "osm":
        import time
        import tomllib
        import warnings

        import numpy as np
        np.seterr(all="ignore")
        warnings.simplefilter("ignore", RuntimeWarning)
        from . import osm
        out = a.out or Path("runs") / time.strftime("osm-%Y%m%dT%H%M%SZ", time.gmtime())
        tol = tomllib.loads(a.tolerances.read_text()) if a.tolerances else None
        cases = [c.strip() for c in a.cases.split(",") if c.strip()] if a.cases else None
        rc, _, line = osm.run_stage(out, a.config, tol, cases, recordings=not a.no_recordings, plots=not a.no_plots)
        print(line)
        return rc
    if a.cmd == "repeats":
        from . import repeats
        try:
            md = repeats.report(a.sources, a.match)
        except FileNotFoundError as e:
            print(e, file=sys.stderr)
            return 2
        if a.out:
            a.out.write_text(md)
            print(a.out)
        else:
            sys.stdout.write(md)
        return 0
    if a.cmd == "baseline":
        from . import baseline
        try:
            lines, written = baseline.write_baselines(a.source, a.dir, a.stage, a.force, a.tolerances)
        except (ValueError, FileNotFoundError) as e:
            print(f"refused: {e}", file=sys.stderr)
            return 2
        print("\n".join(lines))
        for p in written:
            print(f"wrote {p}")
        return 0
    if a.cmd == "compare":
        from . import baseline
        try:
            rc, blocks, path = baseline.compare(a.source, a.baseline, a.tolerances, a.stage, a.out)
        except FileNotFoundError as e:
            print(f"stopped: {e}", file=sys.stderr)
            return 2
        print("\n".join(baseline.summary_lines(blocks)))
        print(f"{path}: " + ("differences beyond the baseline" if rc else "no worse status, no value beyond its step"))
        return rc
    if a.cmd == "comparison":
        from . import comparison
        current, path = comparison.update(a.doc or comparison.DOC, a.baselines or comparison.BASELINES, a.check)
        if a.check:
            print(f"{path}: " + ("tables current" if current else "tables stale; run `python -m crosscheck comparison`"))
            return 0 if current else 1
        print(f"{path}: " + ("unchanged" if current else "tables rebuilt"))
        return 0
    from . import run
    from .levels import PolicyError
    try:
        return run.main(a)
    except PolicyError as e:
        print(f"refused: {e}", file=sys.stderr)
        return 2
    except RuntimeError as e:  # preflight: the rig is not as configured
        print(f"stopped: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
