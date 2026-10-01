#!/bin/sh
# Parameter sweeps behind the defaults in docs/design/q1-delay-finder.md.
# Usage: sh sweep.sh OUTDIR   (each run writes OUTDIR/<tag>.txt and .json)
set -e
cd "$(dirname "$0")"
out=${1:-results}
mkdir -p "$out"
run() { tag=$1; shift; python3 mc.py "$@" --out "$out/$tag.json" > "$out/$tag.txt" 2>&1; }

run baseline --n 200
# estimator: regularised H1 vs GCC-PHAT
run phat --n 100 --only full/,mid/two,mid/room,sub/single,sub/exc --set estimator=phat
# regularisation
for e in 0.001 0.1 0.3; do
  run eps_$e --n 100 --only full/exc,full/two,full/room,sub/exc,mid/two --set eps=$e
done
# borderline band and close spacing
run border_1 --n 150 --only full/two,mid/two --set borderline_db=1.0
run border_3 --n 150 --only full/two,mid/two --set borderline_db=3.0
run close_1 --n 150 --only full/two,mid/two,full/room --set close_k=1.0
run close_3 --n 150 --only full/two,mid/two,full/room --set close_k=3.0
# PSR threshold and p_fa
run psr_12 --n 100 --only full/snr,sub/snr --set psr_min_db=12.0
run psr_18 --n 100 --only full/snr,sub/snr --set psr_min_db=18.0
# observation length
run obs_short --n 100 --only full/single,full/two,full/snr,mid/single,sub/single,sub/snr --obs full=9600 mid=16384 sub=96000
run obs_long --n 100 --only full/snr,sub/snr --obs full=24000 sub=288000
# no refinement pass (acquisition only)
run norefine --n 100 --only full/two,full/room,mid/two,sub/single,sub/snr --set refine=False
# programme material in the sub band needs a longer observation
run sub_music_8s --n 100 --only sub/exc_music --obs sub=384000
