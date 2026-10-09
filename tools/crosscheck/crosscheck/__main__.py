"""python -m crosscheck {preflight,run,analyse,baseline,compare} — see README.md."""
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
