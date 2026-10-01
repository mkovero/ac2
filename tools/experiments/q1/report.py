"""Markdown tables from a sweep.sh output directory (used for the design note §11).

    python3 report.py DIR
"""

import json
import pathlib
import sys

import mc

d = pathlib.Path(sys.argv[1])


def rows(tag):
    p = d / f"{tag}.json"
    return json.loads(p.read_text())["rows"] if p.exists() else None


def pct(x):
    return "—" if x != x else f"{100 * x:.1f}"


def table(tag, title):
    r = rows(tag)
    if r is None:
        return
    print(f"\n#### {title} (`{tag}`)\n")
    print("| class | n | acc % | ok % | wrong % | amb % | ambX % | ref % | e50 | e95 | emax | span % |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|")
    for t in mc.summarise(r):
        print(f"| {t['key']} | {t['n']} | {pct(t['accepted'])} | {pct(t['correct'])} | "
              f"{pct(t['wrong'])} | {pct(t['ambiguous'])} | {pct(t['amb_missing'])} | "
              f"{pct(t['refused'])} | {t['err_p50']:.2f} | {t['err_p95']:.2f} | "
              f"{t['err_max']:.2f} | {pct(t['span_of_accepted'])} |")


def compare(tags, classes, title):
    print(f"\n#### {title}\n")
    print("| run | " + " | ".join(classes) + " |")
    print("|---|" + "---|" * len(classes))
    for tag in tags:
        r = rows(tag)
        if r is None:
            continue
        summ = {t["key"]: t for t in mc.summarise(r)}
        cells = []
        for c in classes:
            t = summ.get(c)
            cells.append("—" if t is None else
                         f"{pct(t['correct'])}/{pct(t['wrong'])}/{pct(t['ambiguous'])}/{pct(t['refused'])}")
        print(f"| {tag} | " + " | ".join(cells) + " |")
    print("\ncells: ok / wrong / ambiguous / refused, % of trials")


table("baseline", "Defaults")
compare(["baseline", "phat"], ["full/two_path", "full/room", "full/exc_music", "full/exc_music_hf",
                               "full/exc_bandlimited", "mid/two_path", "sub/single", "sub/exc_music"],
        "Estimator")
compare(["baseline", "eps_0.001", "eps_0.1", "eps_0.3"],
        ["full/exc_music", "full/exc_music_hf", "full/exc_bandlimited", "full/two_path",
         "full/room", "mid/two_path", "sub/exc_music", "sub/exc_subonly"], "Regularisation eps")
compare(["baseline", "border_1", "border_3"], ["full/two_path", "mid/two_path"], "Borderline band m_b")
compare(["baseline", "close_1", "close_3"], ["full/two_path", "mid/two_path", "full/room"],
        "Close spacing k (pulse widths)")
compare(["psr_12", "baseline", "psr_18"],
        ["full/snr_-10", "full/snr_+0", "full/snr_+10", "sub/snr_+0", "sub/snr_+10", "sub/snr_+20"],
        "PSR threshold (dB)")
compare(["obs_short", "baseline", "obs_long"],
        ["full/single", "full/two_path", "full/snr_+0", "full/snr_+10", "mid/single", "sub/single",
         "sub/snr_+10", "sub/snr_+20"], "Observation length")
compare(["norefine", "baseline"], ["full/two_path", "full/room", "mid/two_path", "sub/single",
                                   "sub/snr_+10", "sub/snr_+20", "sub/snr_+30"], "Refinement pass")
compare(["baseline", "sub_music_8s"], ["sub/exc_music"], "Sub band, programme, 4 s vs 8 s")


def separation(tag):
    r = rows(tag)
    if r is None:
        return
    print("\n#### Two-path outcome by separation (pulse widths) and direct level re the copy\n")
    print("| band | sep / w_p | direct level | n | ok % | wrong % | amb % | ref % | "
          "max \\|err\\| accepted (ms) |")
    print("|---|---|---|---|---|---|---|---|---|")
    SEP = [(0, 0.25), (0.25, 1), (1, 2), (2, 4), (4, 1e9)]
    LD = {-99: "< −14 dB", -14: "−14…−10 dB", -10: "−10…0 dB", 0: "> 0 dB"}
    LDB = [(-99, -14), (-14, -10), (-10, 0), (0, 99)]
    for band in ("full", "mid", "sub"):
        g0 = [x for x in r if x["cls"] == f"{band}/two_path"]
        for s0, s1 in SEP:
            for l0, l1 in LDB:
                g = [x for x in g0 if s0 <= x["sep_w"] < s1 and l0 <= x["l_d"] < l1]
                if not g:
                    continue
                n = len(g)
                c = lambda *o: sum(1 for x in g if x["outcome"] in o)  # noqa: E731
                errs = [abs(x["err_raw"]) / 48.0 for x in g if x["status"] == "accepted"]
                sep = f"{s0}–{s1}" if s1 < 1e9 else f"≥ {s0}"
                print(f"| {band} | {sep} | {LD[l0]} | {n} | {100 * c('correct') / n:.0f} | "
                      f"{100 * c('wrong_arrival', 'imprecise') / n:.0f} | "
                      f"{100 * c('amb_listed', 'amb_missing') / n:.0f} | "
                      f"{100 * c('refused') / n:.0f} | "
                      f"{max(errs) if errs else float('nan'):.2f} |")


separation("baseline")
