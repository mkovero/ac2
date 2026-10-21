# tools/experiments

Prototypes and parameter studies behind design decisions; not built or run by CI.

| path | what it is |
|---|---|
| `q1/` | numpy prototype of the delay finder and the evidence for the defaults in `docs/design/q1-delay-finder.md` |
| `q1/finder.py` | reference prototype of the finder (the design note is normative) |
| `q1/mc.py` | Monte-Carlo evaluation of the finder over randomised scenario classes |
| `q1/sweep.sh` | parameter sweeps over `mc.py`, one result file per setting |
| `q1/report.py` | Markdown tables from a `sweep.sh` output directory |
| `q1/inspect_rows.py` | breaks down an `mc.py` result: two-path outcomes by separation and direct level |
| `q1/auto_band.py` | auto band choice (full → mid → sub) under different excitations |
| `q1/tracking.py` | tracking rule over consecutive windows through the finder |
| `q1/check_fixtures.py` | runs the prototype over the committed `delay_finder_*` golden sets, as the Rust acceptance test does |
| `q1/results/` | committed outputs of the runs above |
