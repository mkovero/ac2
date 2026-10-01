"""Run the prototype finder over the committed delay_finder_* golden sets and check each
case's expectation exactly as the Rust acceptance test will (docs/design/q1-delay-finder.md).

    python3 check_fixtures.py [fixtures/golden]
"""

from __future__ import annotations

import json
import pathlib
import sys

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import finder as F  # noqa: E402

ROOT = HERE.parents[2]


def load(path: pathlib.Path):
    meta = json.loads(path.read_text())
    blob = (path.parent / meta["blob"]["file"]).read_bytes()
    arrs = {}
    for a in meta["arrays"]:
        arrs[a["name"]] = np.frombuffer(blob[a["offset"] : a["offset"] + a["nbytes"]], "<f8")
    return meta, arrs


def check_case(meta, arrs, case) -> tuple[bool, str]:
    prm = meta["parameters"]
    sc = meta["scalars"]
    n = case["name"]
    fs = prm["fs_hz"]
    res = F.find_delay(arrs["ref"], int(prm["ref_start"]), arrs[f"meas.{n}"],
                       int(sc[f"{n}.meas_start"]), fs, prm["band"],
                       (int(sc[f"{n}.search_min"]), int(sc[f"{n}.search_max"])))
    tol = sc[f"{n}.tol_samples"]
    acc = case["acceptable_first_delays"]
    span = case["span"]
    exp = case["expect"]

    def near(d):
        return any(abs(d - a) <= tol for a in acc)

    def accepted_ok():
        if span:
            return span[0] <= res.delay <= span[1]
        return near(res.delay)

    if exp == "accepted":
        ok = res.status == "accepted" and near(res.delay)
    elif exp == "ambiguous":
        ok = res.status == "ambiguous" and any(near(c.delay) for c in res.listed)
    elif exp == "accepted_or_ambiguous":
        ok = res.status == "ambiguous" or (res.status == "accepted" and accepted_ok())
    elif exp == "no_estimate":
        ok = res.status == "no_estimate" and any(r in res.reasons for r in case["reasons_any"])
    else:
        raise ValueError(exp)
    got = (f"{res.status} {res.reasons} first={res.delay if res.delay is None else round(res.delay, 3)} "
           f"listed={[round(c.delay, 2) for c in res.listed]} psr={res.psr_db:.1f} "
           f"snr={res.band_snr_db:.1f} unc={res.pick.uncertainty if res.pick else float('nan'):.3f}")
    return ok, got


def main():
    d = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "fixtures" / "golden"
    bad = 0
    for p in sorted(d.glob("delay_finder_*.json")):
        meta, arrs = load(p)
        for case in meta["parameters"]["cases"]:
            ok, got = check_case(meta, arrs, case)
            bad += not ok
            print(f"{'PASS' if ok else 'FAIL'} {meta['name']}/{case['name']:24s} "
                  f"expect {case['expect']:22s} got {got}")
    print("all pass" if not bad else f"{bad} FAILED")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
