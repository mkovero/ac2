"""Break down a mc.py result file: two-path outcomes by separation and direct level.

    python3 inspect_rows.py results.json [class-prefix]
"""

import json
import sys

import numpy as np

rows = json.load(open(sys.argv[1]))["rows"]
pref = sys.argv[2] if len(sys.argv) > 2 else ""
rows = [r for r in rows if r["cls"].startswith(pref) and "sep_w" in r]

SEP = [(0, 1), (1, 2), (2, 4), (4, 1e9)]
LD = [(-99, -14), (-14, -10), (-10, 0), (0, 99)]
print(f"{'sep/w':>8s} {'L_d':>10s} {'n':>4s} {'ok':>5s} {'wrong':>5s} {'amb':>5s} {'ambX':>5s} {'ref':>5s}")
for s0, s1 in SEP:
    for l0, l1 in LD:
        g = [r for r in rows if s0 <= r["sep_w"] < s1 and l0 <= r["l_d"] < l1]
        if not g:
            continue
        c = lambda *o: sum(1 for r in g if r["outcome"] in o)  # noqa: E731
        print(f"{s0:>3}-{s1:<4} {l0:>4}..{l1:<4} {len(g):4d} {c('correct'):5d} "
              f"{c('wrong_arrival', 'imprecise'):5d} {c('amb_listed', 'amb_missing'):5d} "
              f"{c('amb_missing'):5d} {c('refused'):5d}")
print("\nwrong:")
for r in rows:
    if r["outcome"] in ("wrong_arrival", "imprecise", "amb_missing"):
        print(r["cls"], r["i"], r["outcome"], "err %.2f" % (r["err"] or np.nan),
              "sep %.1f sep_w %.2f l_d %.1f pol %d psr %.1f" % (r["sep"], r["sep_w"], r["l_d"],
                                                              r["pol"], r["psr"]), r["reasons"])
